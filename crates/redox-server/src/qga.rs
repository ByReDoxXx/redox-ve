//! QEMU konuk ajanı (`qemu-guest-agent`) istemcisi: VMware Tools'un karşılığı.
//!
//! Ajan konuğun içinde çalışır ve sunucuyla bir virtio seri kanalı üzerinden JSON satırlarıyla
//! konuşur. Kurulu olması şart değildir: yanıt gelmezse konuk "ajansız" sayılır ve ajana bağlı
//! özellikler (adres, işletim sistemi bilgisi, yedek öncesi dosya sistemi dondurma) atlanır.
//!
//! Yanıtların ayrıştırılması saf işlevlerdir (her platformda test edilir); soket yalnızca Unix'te.

use serde_json::Value;

/// `guest-network-get-interfaces` yanıtından konuğun gösterilecek adresi: geri döngü ve bağlantı-yerel
/// olmayan ilk IPv4, yoksa ilk IPv6.
pub fn first_ip(interfaces: &Value) -> Option<String> {
    let addresses: Vec<(&str, &str)> = interfaces
        .as_array()?
        .iter()
        .filter(|i| i["name"] != "lo")
        .flat_map(|i| i["ip-addresses"].as_array().into_iter().flatten())
        .filter_map(|a| Some((a["ip-address-type"].as_str()?, a["ip-address"].as_str()?)))
        .filter(|(_, ip)| match ip.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(v4)) => !v4.is_loopback() && !v4.is_link_local(),
            Ok(std::net::IpAddr::V6(v6)) => !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80,
            // Adres arayüzde gösterilir: ajan (konuk) ne derse desin yalnızca gerçek bir adres kabul edilir.
            Err(_) => false,
        })
        .collect();
    let pick = |kind: &str| addresses.iter().find(|(t, _)| *t == kind).map(|(_, ip)| ip.to_string());
    pick("ipv4").or_else(|| pick("ipv6"))
}

/// `guest-get-osinfo` yanıtından kısa işletim sistemi adı. Konuktan gelen metin olduğu için kısaltılır
/// ve denetim karakterlerinden arındırılır.
pub fn os_name(info: &Value) -> Option<String> {
    let text = info["pretty-name"].as_str().map(str::to_string).or_else(|| {
        let name = info["name"].as_str()?;
        Some(match info["version"].as_str() {
            Some(version) => format!("{name} {version}"),
            None => name.to_string(),
        })
    })?;
    let clean: String = text.chars().filter(|c| !c.is_control()).take(60).collect();
    Some(clean.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(unix)]
mod imp {
    use super::Value;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    /// Ajan çalışıyorsa eşitleme yanıtı hemen gelir; gelmezse konukta ajan yoktur.
    const SYNC_TIMEOUT: Duration = Duration::from_millis(400);

    /// Ajana tek bir komut gönderir ve yanıtını döndürür.
    ///
    /// Önce `guest-sync` ile hat eşitlenir: kanalda önceki bir istemciden kalmış yanıtlar olabilir,
    /// bizim gönderdiğimiz sayı geri gelene kadar okunanlar atılır.
    pub fn call(sock: &Path, command: &str, timeout: Duration) -> Result<Value, String> {
        call_with(sock, command, None, timeout)
    }

    /// Konuğun içinde bir program çalıştırır ve bitmesini bekler; çıkış kodunu ve çıktısının son satırını döndürür.
    /// Kabuk kullanılmaz: program yolu ve argümanları ayrı ayrı verilir.
    pub fn exec(sock: &Path, path: &str, args: &[String], timeout: Duration) -> Result<(i64, String), String> {
        let started = call_with(sock, "guest-exec", Some(serde_json::json!({ "path": path, "arg": args, "capture-output": true })), Duration::from_secs(10))?;
        let pid = started["pid"].as_i64().ok_or("konuk ajanı çalıştırdığı programın kimliğini döndürmedi")?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let status = call_with(sock, "guest-exec-status", Some(serde_json::json!({ "pid": pid })), Duration::from_secs(10))?;
            if status["exited"].as_bool() == Some(true) {
                let text = |key: &str| status[key].as_str().map(|data| String::from_utf8_lossy(&unbase64(data)).to_string()).unwrap_or_default();
                let output = format!("{}{}", text("out-data"), text("err-data"));
                let last = output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(200).collect();
                return Ok((status["exitcode"].as_i64().unwrap_or(-1), last));
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("konuktaki program {} saniyede bitmedi: {path}", timeout.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    fn unbase64(text: &str) -> Vec<u8> {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let values: Vec<u32> = text.bytes().filter_map(|b| TABLE.iter().position(|t| *t == b).map(|p| p as u32)).collect();
        values
            .chunks(4)
            .flat_map(|c| {
                let n = c.iter().enumerate().fold(0u32, |n, (i, v)| n | v << (18 - 6 * i));
                (0..c.len().saturating_sub(1)).map(move |i| (n >> (16 - 8 * i)) as u8).collect::<Vec<_>>()
            })
            .collect()
    }

    /// `call`'ın argüman alan hali.
    pub fn call_with(sock: &Path, command: &str, arguments: Option<Value>, timeout: Duration) -> Result<Value, String> {
        let absent = || "konuk ajanı yanıt vermiyor (konukta qemu-guest-agent kurulu ve çalışıyor mu?)".to_string();
        let stream = UnixStream::connect(sock).map_err(|_| absent())?;
        stream.set_read_timeout(Some(SYNC_TIMEOUT)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
        let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(stream);
        let mut token = [0u8; 4];
        getrandom::getrandom(&mut token).map_err(|e| e.to_string())?;
        let id = u32::from_le_bytes(token) as u64;
        writer.write_all(format!("{}\n", serde_json::json!({ "execute": "guest-sync", "arguments": { "id": id } })).as_bytes()).map_err(|_| absent())?;
        let mut line = String::new();
        // Eski yanıtlar atlanır; sınırlı sayıda satır okunur ki konuk bizi sonsuza dek oyalayamasın.
        let mut synced = false;
        for _ in 0..64 {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(n) if n > 0 => {}
                _ => return Err(absent()),
            }
            if serde_json::from_str::<Value>(line.trim()).is_ok_and(|v| v["return"] == id) {
                synced = true;
                break;
            }
        }
        if !synced {
            return Err(absent());
        }
        reader.get_ref().set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let request = match arguments {
            Some(arguments) => serde_json::json!({ "execute": command, "arguments": arguments }),
            None => serde_json::json!({ "execute": command }),
        };
        writer.write_all(format!("{request}\n").as_bytes()).map_err(|e| format!("konuk ajanına yazılamadı: {e}"))?;
        line.clear();
        match reader.read_line(&mut line) {
            Ok(n) if n > 0 => {}
            _ => return Err(format!("konuk ajanı {command} isteğine süresinde yanıt vermedi")),
        }
        let reply: Value = serde_json::from_str(line.trim()).map_err(|e| format!("konuk ajanının yanıtı okunamadı: {e}"))?;
        match reply.get("return") {
            Some(value) => Ok(value.clone()),
            None => Err(format!("konuk ajanı: {}", reply["error"]["desc"].as_str().unwrap_or("bilinmeyen hata"))),
        }
    }
}

#[cfg(unix)]
pub use imp::{call, call_with, exec};

/// `guest-get-fsinfo` yanıtından dosya sistemi dolulukları. Boyut bildirmeyen (sanal) dosya sistemleri atlanır;
/// bağlama noktası denetim karakteri içeriyorsa ya da çok uzunsa da atlanır (konuk bu metni kendisi belirler).
pub fn filesystems(info: &serde_json::Value) -> Vec<redox_core::FsUsage> {
    const MB: u64 = 1024 * 1024;
    info.as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let mount = f["mountpoint"].as_str().filter(|m| !m.is_empty() && m.len() <= 128 && !m.chars().any(char::is_control))?;
            let (used, total) = (f["used-bytes"].as_u64()?, f["total-bytes"].as_u64().filter(|t| *t > 0)?);
            Some(redox_core::FsUsage { mount: mount.to_string(), used_mb: used / MB, total_mb: (total / MB).max(1) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_report_skips_virtual_and_hostile_entries() {
        let info = serde_json::json!([
            {"name": "vda1", "mountpoint": "/", "type": "ext4", "used-bytes": 3u64 << 30, "total-bytes": 10u64 << 30},
            {"name": "proc", "mountpoint": "/proc", "type": "proc"},
            {"name": "x", "mountpoint": "/kötü\nad", "used-bytes": 1, "total-bytes": 2},
            {"name": "C", "mountpoint": "C:\\", "type": "NTFS", "used-bytes": 90u64 << 30, "total-bytes": 100u64 << 30}
        ]);
        let list = filesystems(&info);
        assert_eq!(list.iter().map(|f| (f.mount.as_str(), f.used_mb, f.total_mb, f.pct())).collect::<Vec<_>>(), [("/", 3072, 10240, 30), ("C:\\", 92160, 102400, 90)]);
        assert!(filesystems(&serde_json::json!({"hata": true})).is_empty());
    }
    use serde_json::json;

    #[test]
    fn picks_a_routable_address() {
        let interfaces = json!([
            {"name": "lo", "ip-addresses": [{"ip-address-type": "ipv4", "ip-address": "127.0.0.1"}]},
            {"name": "eth0", "ip-addresses": [
                {"ip-address-type": "ipv6", "ip-address": "fe80::5054:ff:fe00:64"},
                {"ip-address-type": "ipv6", "ip-address": "2001:db8::10"},
                {"ip-address-type": "ipv4", "ip-address": "169.254.3.4"},
                {"ip-address-type": "ipv4", "ip-address": "10.0.2.15"}]},
            {"name": "eth1"}
        ]);
        assert_eq!(first_ip(&interfaces).as_deref(), Some("10.0.2.15"), "IPv4 öncelikli; bağlantı-yerel adresler atlanır");
        let v6_only = json!([{"name": "eth0", "ip-addresses": [{"ip-address-type": "ipv6", "ip-address": "fe80::1"}, {"ip-address-type": "ipv6", "ip-address": "2001:db8::10"}]}]);
        assert_eq!(first_ip(&v6_only).as_deref(), Some("2001:db8::10"));
        // Konuk adres yerine başka bir metin gönderirse arayüze taşınmaz.
        let hostile = json!([{"name": "eth0", "ip-addresses": [{"ip-address-type": "ipv4", "ip-address": "<b>merhaba</b>"}]}]);
        assert_eq!(first_ip(&hostile), None);
        assert_eq!(first_ip(&json!([])), None);
        assert_eq!(first_ip(&json!("bozuk")), None);
    }

    #[test]
    fn os_name_is_short_and_clean() {
        assert_eq!(os_name(&json!({"pretty-name": "Debian GNU/Linux 12 (bookworm)", "name": "Debian"})).as_deref(), Some("Debian GNU/Linux 12 (bookworm)"));
        assert_eq!(os_name(&json!({"name": "Microsoft Windows", "version": "11"})).as_deref(), Some("Microsoft Windows 11"));
        assert_eq!(os_name(&json!({"name": "Alpine"})).as_deref(), Some("Alpine"));
        assert_eq!(os_name(&json!({})), None);
        assert_eq!(os_name(&json!({"pretty-name": "  \n "})), None);
        let long = os_name(&json!({"pretty-name": format!("a\u{7}b{}", "x".repeat(200))})).unwrap();
        assert!(long.starts_with("ab") && long.chars().count() == 60, "denetim karakteri atılır, 60 karakterde kesilir");
    }
}
