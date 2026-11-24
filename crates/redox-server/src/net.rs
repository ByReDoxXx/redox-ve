//! Sunucunun ağına bakan yardımcılar: köprüleri listeleme, bir MAC adresinin IP'sini bulma
//! ve yetkisiz konteynerler için alt kimlik (subuid/subgid) aralığını okuma.

/// Sunucudaki ağ köprüleri (`/sys/class/net/<ad>/bridge` dizini olan arayüzler), sıralı.
pub fn bridges() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir("/sys/class/net")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("bridge").is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| redox_core::valid_bridge(n))
        .collect();
    names.sort();
    names
}

/// `/proc/net/arp` tablosunda `mac` adresine karşılık gelen IP.
/// Yalnızca tamamlanmış kayıtlar (bayrak 0x2) sayılır.
pub fn parse_arp(table: &str, mac: &str) -> Option<String> {
    table.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        let complete = f.get(2).and_then(|flags| u32::from_str_radix(flags.trim_start_matches("0x"), 16).ok())? & 0x2 != 0;
        (complete && f.get(3)?.eq_ignore_ascii_case(mac)).then(|| f[0].to_string())
    })
}

/// Komşu tablosundan konuğun adresi. Konuk henüz hiç paket göndermediyse bulunamaz.
pub fn ip_of_mac(mac: &str) -> Option<String> {
    parse_arp(&std::fs::read_to_string("/proc/net/arp").ok()?, mac)
}

/// `/etc/subuid` ya da `/etc/subgid` içinde kullanıcının ilk aralığı: (başlangıç, adet).
/// Kullanıcı adıyla ya da sayısal kimliğiyle yazılmış olabilir.
pub fn subid_range(text: &str, user: &str, uid: &str) -> Option<(u64, u64)> {
    text.lines().find_map(|line| {
        let mut parts = line.trim().split(':');
        let owner = parts.next()?;
        if owner != user && owner != uid {
            return None;
        }
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARP: &str = "IP address       HW type     Flags       HW address            Mask     Device\n\
        172.29.96.1      0x1         0x2         00:15:5d:82:50:c9     *        eth0\n\
        10.10.1.20       0x1         0x2         52:54:00:00:00:64     *        vmbr0\n\
        10.10.1.99       0x1         0x0         52:54:00:00:00:65     *        vmbr0\n";

    #[test]
    fn arp_lookup_by_mac() {
        assert_eq!(parse_arp(ARP, "52:54:00:00:00:64").as_deref(), Some("10.10.1.20"));
        assert_eq!(parse_arp(ARP, "52:54:00:00:00:64".to_uppercase().as_str()).as_deref(), Some("10.10.1.20"), "büyük/küçük harf fark etmez");
        assert_eq!(parse_arp(ARP, "52:54:00:00:00:65"), None, "tamamlanmamış kayıt (0x0) sayılmaz");
        assert_eq!(parse_arp(ARP, "52:54:00:00:00:66"), None);
        assert_eq!(parse_arp("", "x"), None);
        assert_eq!(parse_arp("başlık\nbozuk satır\n", "x"), None);
    }

    #[test]
    fn subid_ranges() {
        let text = "root:100000:65536\nredox:165536:65536\n1001:231072:1000\n";
        assert_eq!(subid_range(text, "redox", "998"), Some((165_536, 65_536)));
        assert_eq!(subid_range(text, "ayse", "1001"), Some((231_072, 1000)), "sayısal kimlikle de eşleşir");
        assert_eq!(subid_range(text, "yok", "5"), None);
        assert_eq!(subid_range("redox:x:y\n", "redox", "1"), None);
        assert_eq!(subid_range("redoxx:1:2\n", "redox", "1"), None, "ad tam eşleşmeli");
    }

    #[test]
    fn bridge_listing_never_fails() {
        // Windows'ta ya da /sys olmayan ortamda boş döner; Linux'ta yalnızca geçerli adlar.
        assert!(bridges().iter().all(|b| redox_core::valid_bridge(b)));
    }
}
