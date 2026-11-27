//! TLS vekilinin tanınması.
//!
//! Vekil (ör. `tailscale serve`) aynı sunucuda çalışır ve gerçek istemci adresini `X-Forwarded-For` ile bildirir.
//! Panel yerel adreste dinlediği için sunucudaki başka bir hesap da ona doğrudan bağlanıp bu başlığı uydurabilir.
//! `tailscale serve` isteklere gizli bir başlık ekleyemediği için vekil, bağlantıyı açan sürecin **kullanıcı
//! kimliğiyle** tanınır (`--proxy-uid`; `tailscaled` kök olarak çalışır, yani 0). Kimlik çekirdeğin bağlantı
//! tablosundan (`/proc/net/tcp`) okunur; yalnızca Linux'ta bulunur.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

/// Bağlantı tablosunda kurulu (ESTABLISHED) bağlantının durum kodu.
const ESTABLISHED: &str = "01";

/// `X-Forwarded-For`'una güvenilecek vekil.
#[derive(Debug, Clone)]
pub struct ProxyOwner {
    /// Vekil sürecinin kullanıcı kimliği.
    pub uid: u32,
    /// Panelin dinlediği kapı.
    pub port: u16,
    /// Okunan bağlantı tabloları (testler kendi dosyalarını verir).
    pub tables: Vec<PathBuf>,
}

impl ProxyOwner {
    pub fn new(uid: u32, port: u16) -> Self {
        ProxyOwner { uid, port, tables: vec!["/proc/net/tcp".into(), "/proc/net/tcp6".into()] }
    }

    /// `peer` adresinden gelen bağlantıyı vekilin kullanıcısı mı açtı? Tablo okunamazsa ya da bağlantı
    /// bulunamazsa hayır.
    pub fn opened(&self, peer: SocketAddr) -> bool {
        self.tables.iter().filter_map(|path| std::fs::read_to_string(path).ok()).find_map(|table| owner_of(&table, peer, self.port)) == Some(self.uid)
    }
}

/// Tablodaki adres: sekizlik gruplar sunucunun kendi bayt sırasıyla yazılmış 32 bitlik sayılardır.
fn address(text: &str) -> Option<SocketAddr> {
    let (ip, port) = text.split_once(':')?;
    let mut bytes = Vec::with_capacity(16);
    for at in (0..ip.len()).step_by(8) {
        bytes.extend(u32::from_str_radix(ip.get(at..at + 8)?, 16).ok()?.to_ne_bytes());
    }
    let ip = match bytes.len() {
        4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => return None,
    };
    Some(SocketAddr::new(ip.to_canonical(), u16::from_str_radix(port, 16).ok()?))
}

/// İstemci ucu `peer` olan ve panelin `port` kapısına giden **açık** bağlantının sahibi.
///
/// Yalnızca kurulu ve bir sokete bağlı (inode'u olan) satırlar sayılır: kapatılmış bağlantıları çekirdek 0
/// numaralı (kök) kullanıcıya aitmiş gibi gösterir; onlar sayılsaydı isteği yollayıp bağlantıyı hemen kapatan
/// sıradan bir hesap kök sanılırdı.
fn owner_of(table: &str, peer: SocketAddr, port: u16) -> Option<u32> {
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
    table.lines().find_map(|line| {
        let mut fields = line.split_whitespace().skip(1);
        let (local, remote, state) = (fields.next()?, fields.next()?, fields.next()?);
        let uid = fields.nth(3)?;
        let inode = fields.nth(1)?;
        (state == ESTABLISHED && inode != "0" && address(local)? == peer && address(remote)?.port() == port).then(|| uid.parse().ok())?
    })
}

#[cfg(all(test, target_endian = "little"))]
mod tests {
    use super::*;

    const HEAD: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";

    fn row(local: &str, remote: &str, state: &str, uid: u32, inode: u32) -> String {
        format!("   0: {local} {remote} {state} 00000000:00000000 00:00000000 00000000 {uid:5}        0 {inode} 1 0000000000000000 20 4 30 10 -1\n")
    }

    #[test]
    fn only_the_open_connection_from_that_address_has_an_owner() {
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        // 127.0.0.1:40000 -> 127.0.0.1:8006
        let (mine, panel) = ("0100007F:9C40", "0100007F:1F46");
        let table = |state: &str, uid, inode| format!("{HEAD}{}{}", row(panel, mine, "01", 999, 11), row(mine, panel, state, uid, inode));
        assert_eq!(owner_of(&table("01", 1000, 12), peer, 8006), Some(1000), "sunucunun kendi ucu (999) değil, istemci ucu");
        assert_eq!(owner_of(&table("01", 0, 12), peer, 8006), Some(0));
        // Kapatılmış bağlantı kök gibi görünür; sayılmaz.
        for state in ["04", "05", "06", "08"] {
            assert_eq!(owner_of(&table(state, 0, 12), peer, 8006), None, "durum {state}");
        }
        assert_eq!(owner_of(&table("01", 0, 0), peer, 8006), None, "sokete bağlı olmayan satır");
        // Aynı istemci adresinden başka bir kapıya giden bağlantı panelinki değildir.
        assert_eq!(owner_of(&table("01", 0, 12), peer, 9000), None);
        assert_eq!(owner_of(&table("01", 0, 12), "127.0.0.1:40001".parse().unwrap(), 8006), None);
        for broken in ["", HEAD, "   0: bozuk satır\n", "   0: 0100007F 0100007F:1F46 01\n"] {
            assert_eq!(owner_of(broken, peer, 8006), None);
        }
    }

    #[test]
    fn ipv6_and_mapped_addresses_are_read() {
        let v6 = row("00000000000000000000000001000000:9C40", "00000000000000000000000001000000:1F46", "01", 7, 3);
        assert_eq!(owner_of(&v6, "[::1]:40000".parse().unwrap(), 8006), Some(7));
        // IPv6 soketinden IPv4 adresine bağlanan istemci.
        let mapped = row("0000000000000000FFFF00000100007F:9C40", "0000000000000000FFFF00000100007F:1F46", "01", 0, 3);
        assert_eq!(owner_of(&mapped, "127.0.0.1:40000".parse().unwrap(), 8006), Some(0));
    }

    /// Gerçek çekirdek tablosu: bu sürecin açtığı bağlantı kendi kullanıcısına aittir, kapatılınca kimseye.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_kernel_table_names_the_user_who_opened_a_connection() {
        use std::os::unix::fs::MetadataExt;
        let me = std::fs::metadata("/proc/self").unwrap().uid();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (_server, peer) = listener.accept().unwrap();
        assert!(ProxyOwner::new(me, port).opened(peer));
        assert!(!ProxyOwner::new(me.wrapping_add(1), port).opened(peer));
        drop(client);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!ProxyOwner::new(me, port).opened(peer));
        assert!(!ProxyOwner::new(0, port).opened(peer), "kapatılmış bağlantı kök sayılmaz");
    }
}
