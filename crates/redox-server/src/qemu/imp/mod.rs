use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use redox_core::{Action, Guest, GuestStatus, Probe, SnapshotOp, VmId};
use serde_json::Value;

use super::{cpu_pct, last_line, lock, parse_qmp_line, parse_stat_ticks, parse_vmrss_kb, QemuDriver, QmpLine};

mod host;
mod hotplug;
mod limits;
mod live;
mod measure;
mod migrate;
mod mirror;
mod power;
mod storage;

pub(super) use host::*;
pub use hotplug::*;
pub use limits::*;
pub use live::*;
pub(super) use measure::*;
pub use migrate::*;
pub use mirror::*;
pub use power::*;
pub use storage::*;

const QMP_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(150);

/// PID dosyasındaki süreç yaşıyor ve gerçekten bu konuğun QEMU'su mu?
/// (PID yeniden kullanılmış olabilir; komut satırında soket yolu aranır.)
fn live_pid(d: &QemuDriver, vmid: VmId) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(d.cfg.pid_path(vmid)).ok()?.trim().parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let sock = d.cfg.qmp_path(vmid).display().to_string();
    String::from_utf8_lossy(&cmdline).contains(&sock).then_some(pid)
}

/// QMP oturumu açar, yetenek el sıkışmasını yapar ve komutları sırayla çalıştırır.
fn qmp(sock: &Path, commands: &[&str]) -> Result<Vec<Value>, String> {
    let values: Vec<Value> = commands.iter().map(|c| serde_json::json!({ "execute": c })).collect();
    qmp_values(sock, &values)
}

/// Argümanlı komutlar için: her öge eksiksiz bir QMP isteğidir.
fn qmp_values(sock: &Path, commands: &[Value]) -> Result<Vec<Value>, String> {
    qmp_run(sock, commands, QMP_TIMEOUT)
}

pub fn is_alive(d: &QemuDriver, vmid: VmId) -> bool {
    live_pid(d, vmid).is_some()
}

/// Yanıt için `timeout` kadar bekler (uzun süren izleyici komutları için).
fn qmp_run(sock: &Path, commands: &[Value], timeout: Duration) -> Result<Vec<Value>, String> {
    let err = |e: std::io::Error| format!("QMP ({}): {e}", sock.display());
    let stream = UnixStream::connect(sock).map_err(err)?;
    stream.set_read_timeout(Some(timeout)).map_err(err)?;
    stream.set_write_timeout(Some(QMP_TIMEOUT)).map_err(err)?;
    let mut writer = stream.try_clone().map_err(err)?;
    let mut reader = BufReader::new(stream);

    let read = |reader: &mut BufReader<UnixStream>| -> Result<QmpLine, String> {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => Err("QMP bağlantısı kapandı".to_string()),
            Ok(_) => Ok(parse_qmp_line(&line)),
            Err(e) => Err(err(e)),
        }
    };
    if read(&mut reader)? != QmpLine::Greeting {
        return Err("QMP karşılaması alınamadı".to_string());
    }
    let mut results = Vec::new();
    let hello = serde_json::json!({ "execute": "qmp_capabilities" });
    for (i, command) in std::iter::once(&hello).chain(commands).enumerate() {
        let name = command["execute"].as_str().unwrap_or("?");
        writer.write_all(format!("{command}\n").as_bytes()).map_err(err)?;
        loop {
            match read(&mut reader)? {
                QmpLine::Return(v) => {
                    if i > 0 {
                        results.push(v);
                    }
                    break;
                }
                QmpLine::Error(desc) => return Err(format!("QMP {name}: {desc}")),
                // Olaylar ve tanınmayan satırlar yanıt değildir.
                QmpLine::Event(_) | QmpLine::Other | QmpLine::Greeting => continue,
            }
        }
    }
    Ok(results)
}

fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

fn tool(bin: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(bin).args(args).output().map_err(|e| format!("{bin} çalıştırılamadı: {e} (iproute2 kurulu mu?)"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{bin} {}: {}", args[..args.len().min(3)].join(" "), last_line(&out.stderr)))
    }
}

fn cleanup(d: &QemuDriver, vmid: VmId) {
    drop_limits(d, vmid);
    stop_tpm(d, vmid);
    remove_vlan_taps(d, vmid);
    let _ = std::fs::remove_file(d.cfg.pid_path(vmid));
    let _ = std::fs::remove_file(d.cfg.qmp_path(vmid));
    let _ = std::fs::remove_file(d.cfg.qga_path(vmid));
    let _ = std::fs::remove_file(d.cfg.vnc_path(vmid));
    lock(&d.agents).remove(&vmid);
    lock(&d.samples).remove(&vmid);
}

/// İzleyici komutunun çıktısını döndürür.
fn hmp_text(d: &QemuDriver, g: &Guest, line: &str) -> Result<String, String> {
    let command = serde_json::json!({ "execute": "human-monitor-command", "arguments": { "command-line": line } });
    let out = qmp_values(&d.cfg.qmp_path(g.vmid), &[command])?;
    Ok(out.first().and_then(Value::as_str).unwrap_or_default().trim().to_string())
}

/// Kabuk için tek tırnakla güvenli alıntı (QEMU `exec:` adresini kabukta çalıştırır).
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// QEMU izleyici (HMP) komutu çalıştırır. Başarıda çıktı boştur; hata metin olarak döner.
fn hmp(d: &QemuDriver, g: &Guest, line: &str) -> Result<(), String> {
    let command = serde_json::json!({ "execute": "human-monitor-command", "arguments": { "command-line": line } });
    // Bellek durumu diske yazılırken yanıt gecikebilir.
    let out = qmp_run(&d.cfg.qmp_path(g.vmid), &[command], Duration::from_secs(900))?;
    let text = out.first().and_then(Value::as_str).unwrap_or_default().trim().to_string();
    if text.is_empty() {
        Ok(())
    } else {
        Err(text.lines().last().unwrap_or_default().trim_start_matches("Error: ").to_string())
    }
}
