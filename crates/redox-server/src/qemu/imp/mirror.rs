//! Eşzamanlı disk yansıtma.
//!
//! **Kaynakta** konuğun her diski için `blockdev-mirror` işi açılır ve hiç tamamlanmaz: ilk kopya bittikten sonra
//! konuğun her yazması hem yerel diske hem ağdaki kopyaya gider (`write-blocking`). **Hedefte** kopyayı küçük bir
//! "tutucu" QEMU sunar: makinesi yoktur, yalnızca disk dosyalarını şifreli NBD ile yazılabilir açar.
//!
//! Karşı taraf sessizce donarsa (fiş çekilmesi, ağ kopması) konuğun disk yazmaları da bekler. Bunu çağıran
//! `mirror_probe` ile görür ve `mirror_stop` ile bağlantıyı zorla keser (`yank`); bekleyen yazmalar tamamlanır.

use super::*;
use redox_core::MirrorProbe;
use std::path::PathBuf;

const JOB_PREFIX: &str = "ayna-";
const TLS_ID: &str = "aynatls";

fn source_tls_dir(d: &QemuDriver, vmid: VmId) -> PathBuf {
    d.cfg.run_dir().join(format!("{vmid}.ayna"))
}

/// Kopya adı dosya yoluna girer: yalnızca kaynak adı, kimlik ve kuşaktan oluşan düz bir ad kabul edilir.
fn check_id(id: &str) -> Result<(), String> {
    if !id.is_empty() && id.len() <= 56 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        Ok(())
    } else {
        Err(format!("geçersiz kopya adı: {}", id.escape_default()))
    }
}

fn replica_dir(d: &QemuDriver, id: &str) -> PathBuf {
    d.cfg.data_dir.join("mirrors").join(id)
}

fn holder_socket(d: &QemuDriver, id: &str) -> PathBuf {
    d.cfg.run_dir().join(format!("k-{id}.sock"))
}

fn holder_pidfile(d: &QemuDriver, id: &str) -> PathBuf {
    d.cfg.run_dir().join(format!("k-{id}.pid"))
}

/// Tutucu süreç yaşıyor mu? Komut satırında kendi soketi aranır (PID yeniden kullanılmış olabilir).
fn holder_pid(d: &QemuDriver, id: &str) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(holder_pidfile(d, id)).ok()?.trim().parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    String::from_utf8_lossy(&cmdline).contains(&holder_socket(d, id).display().to_string()).then_some(pid)
}

pub fn mirror_disk_sizes(d: &QemuDriver, g: &Guest) -> Result<Vec<u64>, String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    let devices = disk_devices(d, g)?;
    let reply = qmp(&d.cfg.qmp_path(g.vmid), &["query-block"])?;
    let blocks = reply.first().and_then(Value::as_array).ok_or("konuğun diskleri QEMU'dan okunamadı")?;
    devices
        .iter()
        .map(|device| blocks.iter().find(|b| b["device"] == device.as_str()).and_then(|b| b["inserted"]["image"]["virtual-size"].as_u64()).ok_or(format!("diskin boyutu okunamadı: {device}")))
        .collect()
}

pub fn mirror_probe(d: &QemuDriver, g: &Guest) -> Option<MirrorProbe> {
    live_pid(d, g.vmid)?;
    let jobs = block_jobs(&d.cfg.qmp_path(g.vmid), JOB_PREFIX).ok()?;
    let disks = 1 + g.hw.disks.len();
    let sum = |key: &str| jobs.iter().filter_map(|j| j[key].as_u64()).sum();
    Some(MirrorProbe {
        disks,
        jobs: jobs.len(),
        synced: jobs.len() == disks && jobs.iter().all(|j| j["ready"] == true && j["actively-synced"] != false),
        done: sum("offset"),
        total: sum("len"),
    })
}

pub fn mirror_stop(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let finish = || {
        let _ = std::fs::remove_dir_all(source_tls_dir(d, g.vmid));
    };
    if live_pid(d, g.vmid).is_none() {
        finish();
        return Ok(());
    }
    let jobs = block_jobs(&sock, JOB_PREFIX)?;
    let names: Vec<String> = jobs.iter().filter_map(|j| j["device"].as_str().map(str::to_string)).collect();
    let gone = || block_jobs(&sock, JOB_PREFIX).is_ok_and(|jobs| jobs.is_empty()) || live_pid(d, g.vmid).is_none();
    // Karşı taraf yetişiyorsa işler olağan biçimde bırakılır: kopya tutarlı bir noktada kalır.
    let caught_up = !jobs.is_empty() && jobs.iter().all(|j| j["ready"] == true && j["offset"] == j["len"]);
    if caught_up {
        for name in &names {
            let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": name } })]);
        }
    }
    if !(caught_up && wait_until(Duration::from_secs(5), gone)) {
        // Takılan bağlantılar önce zorla kesilir; yoksa iş, yanıt gelmeyecek yazmaları bekler. **Hepsi** kesildikten
        // sonra iptal edilir: bir işin iptali, öbür diskin bağlantısında bekleyen yazmaya takılabilir.
        let mut problems = Vec::new();
        let mut run = |command: Value| {
            if let Err(e) = qmp_values(&sock, &[command]) {
                problems.push(e);
            }
        };
        for name in &names {
            run(serde_json::json!({ "execute": "yank", "arguments": { "instances": [{ "type": "block-node", "node-name": name }] } }));
        }
        for name in &names {
            run(serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": name, "force": true } }));
        }
        if !wait_until(Duration::from_secs(15), gone) {
            let left = block_jobs(&sock, JOB_PREFIX).map(|jobs| Value::Array(jobs).to_string()).unwrap_or_else(|e| e);
            return Err(format!("yansıtma işleri durdurulamadı: {}; kalan: {left}", problems.join("; ")));
        }
    }
    // İş hiç açılamadan kalmış düğümler de kaldırılır.
    for device in disk_devices(d, g).unwrap_or_default() {
        let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "blockdev-del", "arguments": { "node-name": format!("{JOB_PREFIX}{device}") } })]);
    }
    let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "object-del", "arguments": { "id": TLS_ID } })]);
    finish();
    Ok(())
}

pub fn mirror_start(d: &QemuDriver, g: &Guest, host: &str, port: u16, key: &str) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    if !redox_core::valid_migrate_host(host) {
        return Err(format!("geçersiz hedef adresi: {host}"));
    }
    // Önceki yarım denemenin işi ya da düğümü kalmış olabilir.
    mirror_stop(d, g)?;
    let devices = disk_devices(d, g)?;
    let started = (|| -> Result<(), String> {
        add_tls_object(&sock, &source_tls_dir(d, g.vmid), TLS_ID, key, "client")?;
        for (i, device) in devices.iter().enumerate() {
            let node = format!("{JOB_PREFIX}{device}");
            let target = serde_json::json!({ "execute": "blockdev-add", "arguments": {
                "driver": "nbd", "node-name": node, "export": format!("disk{i}"), "tls-creds": TLS_ID,
                "server": { "type": "inet", "host": host, "port": port.to_string() } } });
            qmp_run(&sock, &[target], Duration::from_secs(30)).map_err(|e| format!("kopyanın disk kapısına ({host}:{port}) bağlanılamadı: {e}"))?;
            let mirror = serde_json::json!({ "execute": "blockdev-mirror", "arguments": { "job-id": node, "device": device, "target": node, "sync": "full", "copy-mode": "write-blocking", "on-target-error": "report" } });
            qmp_values(&sock, &[mirror]).map_err(|e| format!("disk yansıtılamadı: {e}"))?;
        }
        Ok(())
    })();
    started.inspect_err(|_| {
        let _ = mirror_stop(d, g);
    })
}

pub fn replica_serve(d: &QemuDriver, id: &str, sizes: &[u64], bind: &str, port: u16, key: &str) -> Result<(), String> {
    check_id(id)?;
    if !redox_core::valid_migrate_host(bind) {
        return Err(format!("geçersiz dinleme adresi: {bind}"));
    }
    let dir = replica_dir(d, id);
    replica_delete(d, id)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    std::fs::create_dir_all(d.cfg.run_dir()).map_err(|e| format!("{} oluşturulamadı: {e}", d.cfg.run_dir().display()))?;
    let served = (|| -> Result<(), String> {
        let mut files = Vec::new();
        for (i, size) in sizes.iter().enumerate() {
            let file = dir.join(format!("disk{i}.qcow2"));
            tool(&d.cfg.qemu_img_bin, &["create", "-q", "-f", "qcow2", &file.display().to_string(), &size.to_string()])?;
            files.push(file);
        }
        let sock = holder_socket(d, id);
        let args = ["-machine", "none", "-nodefaults", "-display", "none", "-daemonize", "-pidfile"];
        let out = std::process::Command::new(&d.cfg.qemu_bin)
            .args(args)
            .arg(holder_pidfile(d, id))
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", crate::qemu::opt(&sock)))
            .output()
            .map_err(|e| format!("{} çalıştırılamadı: {e}", d.cfg.qemu_bin))?;
        if !out.status.success() {
            return Err(format!("kopya tutucu başlatılamadı: {}", last_line(&out.stderr)));
        }
        if !wait_until(Duration::from_secs(5), || holder_pid(d, id).is_some() && sock.exists()) {
            return Err("kopya tutucu başladı ama süreç bulunamadı".to_string());
        }
        add_tls_object(&sock, &dir.join("tls"), TLS_ID, key, "server")?;
        let listen = serde_json::json!({ "execute": "nbd-server-start", "arguments": { "addr": { "type": "inet", "data": { "host": bind, "port": port.to_string() } }, "tls-creds": TLS_ID } });
        qmp_values(&sock, &[listen]).map_err(|e| format!("disk kapısı ({port}) açılamadı: {e}"))?;
        for (i, file) in files.iter().enumerate() {
            let add = serde_json::json!({ "execute": "blockdev-add", "arguments": { "driver": "qcow2", "node-name": format!("d{i}"), "file": { "driver": "file", "filename": file.display().to_string() } } });
            let export = serde_json::json!({ "execute": "block-export-add", "arguments": { "type": "nbd", "id": format!("e{i}"), "node-name": format!("d{i}"), "name": format!("disk{i}"), "writable": true } });
            qmp_values(&sock, &[add, export]).map_err(|e| format!("kopya diski sunulamadı: {e}"))?;
        }
        Ok(())
    })();
    served.inspect_err(|_| {
        let _ = replica_delete(d, id);
    })
}

pub fn replica_release(d: &QemuDriver, id: &str) -> Result<(), String> {
    check_id(id)?;
    let alive = || holder_pid(d, id).is_some();
    if alive() {
        // `quit` açık diskleri boşaltıp kapatır.
        let _ = qmp(&holder_socket(d, id), &["quit"]);
        if !wait_until(Duration::from_secs(15), || !alive()) {
            if let Some(pid) = holder_pid(d, id) {
                let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
            }
            if !wait_until(Duration::from_secs(3), || !alive()) {
                return Err("kopya tutucu süreç kapatılamadı".to_string());
            }
        }
    }
    let _ = std::fs::remove_file(holder_pidfile(d, id));
    let _ = std::fs::remove_file(holder_socket(d, id));
    let _ = std::fs::remove_dir_all(replica_dir(d, id).join("tls"));
    Ok(())
}

pub fn replica_delete(d: &QemuDriver, id: &str) -> Result<(), String> {
    replica_release(d, id)?;
    let dir = replica_dir(d, id);
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{} silinemedi: {e}", dir.display())),
        _ => Ok(()),
    }
}

pub fn replica_adopt(d: &QemuDriver, id: &str, g: &Guest) -> Result<(), String> {
    replica_release(d, id)?;
    let dir = replica_dir(d, id);
    let mut targets = vec![d.cfg.disk_of(g)];
    targets.extend(g.hw.disks.iter().map(|disk| d.cfg.extra_disk_of(g, disk.id)));
    let moves: Vec<(PathBuf, PathBuf)> = targets.into_iter().enumerate().map(|(i, to)| (dir.join(format!("disk{i}.qcow2")), to)).collect();
    // Hiçbir şey taşınmadan hepsinin var olduğu görülür.
    if let Some((missing, _)) = moves.iter().find(|(from, _)| !from.is_file()) {
        return Err(format!("kopya diski bulunamadı: {}", missing.display()));
    }
    for (from, to) in &moves {
        // Başka bir dosya sistemine adlandırma yapılamaz; o zaman kopyalanır.
        if std::fs::rename(from, to).is_err() {
            std::fs::copy(from, to).map_err(|e| format!("{} kopyalanamadı: {e}", from.display()))?;
        }
    }
    replica_delete(d, id)
}
