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
        writer.write_all(format!("{}\n", serde_json::json!({ "execute": command })).as_bytes()).map_err(|e| format!("konuk ajanına yazılamadı: {e}"))?;
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
pub use imp::call;

#[cfg(test)]
mod tests {
    use super::*;
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
