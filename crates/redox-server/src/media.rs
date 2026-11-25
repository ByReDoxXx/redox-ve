//! Kurulum kalıpları (ISO) ve disk imajları: listeleme, tarayıcıdan ya da API ile yükleme, bir adresten
//! indirme ve silme.
//!
//! Dosya adı türü belirler: `.iso` kalıp dizinine, disk imajı uzantıları (`.qcow2`, `.vmdk`, …) imaj
//! dizinine gider. Ad yalnızca güvenli karakterlerden oluşabilir; böylece istek dizin dışına çıkamaz.
//! Yükleme ve indirme önce gizli bir geçici dosyaya yazılır, tamamlanınca (var olan bir dosyanın üzerine
//! yazmadan) yerine konur: yarım dosya hiçbir zaman seçilebilir bir kalıp olarak görünmez.

use std::io::Write;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use redox_core::{Error, VmId};
use sha2::{Digest, Sha256};

use crate::state::AppState;

/// Tek dosya için üst sınır (yanlışlıkla diski dolduran bir yüklemeye karşı).
pub const MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Media {
    pub name: String,
    /// `true`: kurulum kalıbı (ISO); `false`: disk imajı.
    pub iso: bool,
    pub bytes: u64,
    /// Bu dosyayı kullanan konuklar (takılı kalıp ya da kaynak imaj).
    pub used_by: Vec<VmId>,
    /// OVA arşiviyse tanımındaki donanım önerisi.
    pub ova: Option<crate::ova::OvfInfo>,
}

/// Adın gösterdiği dosyanın sunucudaki yolu. Ad geçersizse ya da sürücü dosya tutmuyorsa hata.
/// Hazır bulut imajı kataloğu: `(anahtar, görünen ad, kaydedilecek dosya adı, adres)`. Dağıtımların kendi
/// sunucularındaki "en güncel" adresleridir; imajlar cloud-init ile açılır (kullanıcı ve SSH anahtarı
/// konuk oluşturulurken verilir). İçerik sürekli güncellendiği için sabit bir özetle doğrulanamaz;
/// bütünlük yalnızca https bağlantısına dayanır.
pub const CATALOG: [(&str, &str, &str, &str); 6] = [
    ("debian-12", "Debian 12 (bookworm)", "debian-12-genericcloud-amd64.qcow2", "https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-amd64.qcow2"),
    ("debian-13", "Debian 13 (trixie)", "debian-13-genericcloud-amd64.qcow2", "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"),
    ("ubuntu-24.04", "Ubuntu 24.04 LTS (noble)", "ubuntu-24.04-server-cloudimg-amd64.qcow2", "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img"),
    ("ubuntu-22.04", "Ubuntu 22.04 LTS (jammy)", "ubuntu-22.04-server-cloudimg-amd64.qcow2", "https://cloud-images.ubuntu.com/jammy/current/jammy-server-cloudimg-amd64.img"),
    ("almalinux-9", "AlmaLinux 9", "almalinux-9-genericcloud-x86_64.qcow2", "https://repo.almalinux.org/almalinux/9/cloud/x86_64/images/AlmaLinux-9-GenericCloud-latest.x86_64.qcow2"),
    ("rocky-9", "Rocky Linux 9", "rocky-9-genericcloud-x86_64.qcow2", "https://dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2"),
];

pub fn target(st: &AppState, name: &str) -> Result<(PathBuf, bool), Error> {
    let driver = st.driver();
    let found = if redox_core::valid_iso_name(name) {
        driver.iso_file(name).map(|p| (p, true))
    } else if redox_core::hw::valid_image_name(name) {
        driver.image_file(name).map(|p| (p, false))
    } else {
        return Err(Error::InvalidSpec(format!(
            "geçersiz dosya adı \"{name}\": harf, rakam, nokta, tire ve alt çizgi; uzantı .iso ya da .qcow2/.img/.raw/.vmdk/.vdi/.vhdx/.vhd/.ova olmalı"
        )));
    };
    found.ok_or_else(|| Error::InvalidSpec("bu sürücü kalıp ve imaj dosyası tutmuyor (gerçek sürücü gerekir)".to_string()))
}

pub fn list(st: &AppState) -> Vec<Media> {
    let (driver, cluster) = (st.driver(), st.snapshot());
    let users = |name: &str| -> Vec<VmId> { cluster.guests.iter().filter(|g| g.iso.as_deref() == Some(name) || g.hw.image.as_deref() == Some(name)).map(|g| g.vmid).collect() };
    let entry = |name: String, iso: bool, path: Option<PathBuf>| Media {
        ova: path.as_ref().filter(|_| name.to_ascii_lowercase().ends_with(".ova")).and_then(|p| crate::ova::inspect(p).ok()),
        bytes: path.and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0),
        used_by: users(&name),
        name,
        iso,
    };
    let mut all: Vec<Media> = driver.isos().into_iter().map(|n| entry(n.clone(), true, driver.iso_file(&n))).collect();
    all.extend(driver.images().into_iter().map(|n| entry(n.clone(), false, driver.image_file(&n))));
    all
}

fn temp_for(dest: &Path) -> Result<PathBuf, String> {
    let dir = dest.parent().ok_or("geçersiz hedef yolu")?;
    let name = dest.file_name().and_then(|n| n.to_str()).ok_or("geçersiz dosya adı")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    Ok(dir.join(format!(".{name}.{}-{}.upload.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))))
}

/// Tamamlanan geçici dosyayı yerine koyar. Var olan dosyanın üzerine yazmaz (aynı anda iki yükleme yarışsa bile).
fn place(temp: &Path, dest: &Path) -> Result<(), String> {
    let placed = std::fs::hard_link(temp, dest).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{} zaten var; önce onu silin ya da başka bir ad verin", dest.file_name().and_then(|n| n.to_str()).unwrap_or("dosya")),
        _ => format!("dosya yerine konamadı: {e}"),
    });
    let _ = std::fs::remove_file(temp);
    placed
}

fn check_digest(actual: &str, expected: Option<&str>) -> Result<(), String> {
    match expected.map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()) {
        Some(e) if e.len() != 64 || !e.bytes().all(|b| b.is_ascii_hexdigit()) => Err("SHA-256 özeti 64 onaltılık hane olmalı".to_string()),
        Some(e) if e != actual => Err(format!("SHA-256 özeti uyuşmuyor (beklenen {e}, bulunan {actual}); dosya kaydedilmedi")),
        _ => Ok(()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parça parça gelen bir yüklemeyi diske yazar.
#[derive(Debug)]
pub struct Upload {
    file: Option<std::fs::File>,
    temp: PathBuf,
    dest: PathBuf,
    written: u64,
    hasher: Sha256,
}

impl Upload {
    pub fn begin(dest: PathBuf) -> Result<Upload, String> {
        if dest.exists() {
            return Err(format!("{} zaten var; önce onu silin ya da başka bir ad verin", dest.file_name().and_then(|n| n.to_str()).unwrap_or("dosya")));
        }
        let temp = temp_for(&dest)?;
        let file = std::fs::File::create(&temp).map_err(|e| format!("geçici dosya oluşturulamadı: {e}"))?;
        Ok(Upload { file: Some(file), temp, dest, written: 0, hasher: Sha256::new() })
    }

    pub async fn write(&mut self, chunk: axum::body::Bytes) -> Result<(), String> {
        self.written += chunk.len() as u64;
        if self.written > MAX_BYTES {
            return Err("dosya 64 GB sınırını aşıyor".to_string());
        }
        self.hasher.update(&chunk);
        let mut file = self.file.take().ok_or("yükleme yarıda kesildi")?;
        // Disk yazması istek iş parçacığını tutmasın.
        let written = tokio::task::spawn_blocking(move || file.write_all(&chunk).map(|()| file)).await;
        match written {
            Ok(Ok(file)) => {
                self.file = Some(file);
                Ok(())
            }
            Ok(Err(e)) => Err(format!("diske yazılamadı: {e}")),
            Err(e) => Err(format!("diske yazılamadı: {e}")),
        }
    }

    /// Dosyayı diske işler, (verildiyse) özetini doğrular ve yerine koyar; bayt sayısını döndürür.
    pub async fn finish(mut self, sha256: Option<&str>) -> Result<u64, String> {
        if self.written == 0 {
            return Err("boş dosya yüklenemez".to_string());
        }
        check_digest(&hex(&self.hasher.clone().finalize()), sha256)?;
        let file = self.file.take().ok_or("yükleme yarıda kesildi")?;
        let (temp, dest) = (self.temp.clone(), self.dest.clone());
        tokio::task::spawn_blocking(move || {
            file.sync_all().map_err(|e| format!("diske yazılamadı: {e}"))?;
            place(&temp, &dest)
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(self.written)
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        // Yarıda kalan (ya da reddedilen) yüklemenin geçici dosyası kalmaz; başarılıysa zaten taşınmıştır.
        let _ = std::fs::remove_file(&self.temp);
    }
}

/// Adres genel internete mi gidiyor? Sunucunun kendi ağındaki hizmetlere (yönetim arayüzleri, bulut
/// üstveri adresleri) istek yaptırılamasın diye yalnızca HTTPS ve genel adresler kabul edilir.
pub fn check_public_url(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or("adres https:// ile başlamalı")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err("adres geçersiz (kullanıcı bilgisi içeremez)".to_string());
    }
    let with_port = if authority.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { authority.to_string() } else { format!("{authority}:443") };
    let addresses: Vec<IpAddr> = with_port.to_socket_addrs().map_err(|e| format!("adres çözülemedi ({authority}): {e}"))?.map(|a| a.ip()).collect();
    if addresses.is_empty() {
        return Err(format!("adres çözülemedi: {authority}"));
    }
    match addresses.iter().find(|ip| !is_public(ip)) {
        Some(ip) => Err(format!("{authority} genel bir internet adresi değil ({ip}); iç ağdaki adreslerden indirme yapılmaz")),
        None => Ok(()),
    }
}

pub fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() || v4.is_documentation()
                || (o[0] == 100 && (64..128).contains(&o[1])) // taşıyıcı NAT (100.64.0.0/10)
                || o[0] == 0
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public(&IpAddr::V4(v4)),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

/// Yönlendirmedeki `Location` değerini tam adrese çevirir.
fn resolve_location(current: &str, location: &str) -> Option<String> {
    if location.contains("://") {
        return Some(location.to_string());
    }
    let scheme_end = current.find("://")? + 3;
    let origin_end = current[scheme_end..].find('/').map(|i| scheme_end + i).unwrap_or(current.len());
    location.starts_with('/').then(|| format!("{}{location}", &current[..origin_end]))
}

/// `url` adresindeki dosyayı `dest` yoluna indirir; bayt sayısını döndürür. Her adres (yönlendirmeler
/// dahil) `allowed` denetiminden geçer.
pub fn fetch(url: &str, dest: &Path, sha256: Option<&str>, allowed: &dyn Fn(&str) -> Result<(), String>) -> Result<u64, String> {
    if dest.exists() {
        return Err(format!("{} zaten var", dest.display()));
    }
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(120)).redirects(0).build();
    let mut current = url.to_string();
    let mut response = None;
    for _ in 0..6 {
        allowed(&current)?;
        let reply = match agent.get(&current).call() {
            Ok(reply) => reply,
            Err(ureq::Error::Status(code, reply)) if (300..400).contains(&code) => reply,
            Err(ureq::Error::Status(code, _)) => return Err(format!("sunucu {code} yanıtı verdi")),
            Err(ureq::Error::Transport(t)) => return Err(format!("indirilemedi: {}", t.kind())),
        };
        if (300..400).contains(&reply.status()) {
            let location = reply.header("location").ok_or("yönlendirme adresi yok")?;
            current = resolve_location(&current, location).ok_or("yönlendirme adresi anlaşılamadı")?;
            continue;
        }
        response = Some(reply);
        break;
    }
    let response = response.ok_or("çok fazla yönlendirme")?;
    let temp = temp_for(dest)?;
    let saved = (|| {
        let mut file = std::fs::File::create(&temp).map_err(|e| format!("geçici dosya oluşturulamadı: {e}"))?;
        let mut reader = std::io::Read::take(response.into_reader(), MAX_BYTES + 1);
        let (mut hasher, mut total, mut buf) = (Sha256::new(), 0u64, vec![0u8; 256 * 1024]);
        loop {
            let n = std::io::Read::read(&mut reader, &mut buf).map_err(|e| format!("indirme yarıda kesildi: {e}"))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_BYTES {
                return Err("dosya 64 GB sınırını aşıyor".to_string());
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| format!("diske yazılamadı: {e}"))?;
        }
        if total == 0 {
            return Err("sunucu boş dosya gönderdi".to_string());
        }
        check_digest(&hex(&hasher.finalize()), sha256)?;
        file.sync_all().map_err(|e| format!("diske yazılamadı: {e}"))?;
        place(&temp, dest).map(|()| total)
    })();
    let _ = std::fs::remove_file(&temp);
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_are_public_https_images_with_valid_names() {
        let mut keys: Vec<&str> = CATALOG.iter().map(|e| e.0).collect();
        keys.dedup();
        assert_eq!(keys.len(), CATALOG.len(), "anahtarlar benzersiz");
        for (key, label, file, url) in CATALOG {
            assert!(!label.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'), "{key}");
            assert!(redox_core::hw::valid_image_name(file), "{file}");
            assert!(url.starts_with("https://"), "{url}");
        }
    }
    use std::io::{BufRead, BufReader, Read};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("redox-media-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hidden(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).count()
    }

    #[test]
    fn only_public_https_addresses_are_fetched() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "255.255.255.255", "::1", "fe80::1", "fd00::1", "::ffff:10.0.0.1", "::"] {
            assert!(!is_public(&ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "151.101.1.69", "2606:4700::1111", "::ffff:1.1.1.1"] {
            assert!(is_public(&ip.parse().unwrap()), "{ip}");
        }
        assert!(check_public_url("http://example.com/a.iso").unwrap_err().contains("https://"));
        assert!(check_public_url("https://kullanici:parola@example.com/a.iso").unwrap_err().contains("kullanıcı bilgisi"));
        assert!(check_public_url("https://127.0.0.1/a.iso").unwrap_err().contains("genel bir internet adresi değil"));
        assert!(check_public_url("https://169.254.169.254/latest/meta-data").unwrap_err().contains("genel"), "bulut üstveri adresi");
        assert!(check_public_url("https://[::1]:8443/a.iso").unwrap_err().contains("genel"));
        assert!(check_public_url("https://localhost:8006/").unwrap_err().contains("genel"));
        assert!(check_public_url("https:///a.iso").is_err());
    }

    #[test]
    fn redirect_targets_are_resolved() {
        assert_eq!(resolve_location("https://a.example/x/y.iso", "https://b.example/z.iso").as_deref(), Some("https://b.example/z.iso"));
        assert_eq!(resolve_location("https://a.example/x/y.iso", "/z.iso").as_deref(), Some("https://a.example/z.iso"));
        assert_eq!(resolve_location("https://a.example", "/z.iso").as_deref(), Some("https://a.example/z.iso"));
        assert_eq!(resolve_location("https://a.example/x/", "z.iso"), None, "göreli yol desteklenmez: açık hata");
    }

    /// Tek seferlik sahte HTTP sunucusu: yollara göre yanıt verir.
    fn server(routes: Vec<(&'static str, String)>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let mut reader = BufReader::new(socket);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                        break;
                    }
                }
                let reply = routes.iter().find(|(p, _)| *p == path).map(|(_, r)| r.clone()).unwrap_or_else(|| "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string());
                let mut socket = reader.into_inner();
                let _ = socket.write_all(reply.as_bytes());
            }
        });
        port
    }

    fn ok_reply(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    #[test]
    fn fetch_follows_checked_redirects_and_verifies_the_digest() {
        let dir = temp("fetch");
        let port = server(vec![
            ("/a.iso", ok_reply("kalip icerigi")),
            ("/yon", "HTTP/1.1 302 Found\r\nLocation: /a.iso\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()),
            ("/ic", "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/gizli\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()),
            ("/bos", ok_reply("")),
        ]);
        let base = format!("http://127.0.0.1:{port}");
        let any = |_: &str| Ok(());
        let digest = hex(&Sha256::digest(b"kalip icerigi"));

        let dest = dir.join("a.iso");
        assert_eq!(fetch(&format!("{base}/yon"), &dest, Some(&digest.to_uppercase()), &any).unwrap(), 13);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "kalip icerigi");
        assert!(fetch(&format!("{base}/a.iso"), &dest, None, &any).unwrap_err().contains("zaten var"), "var olanın üzerine yazılmaz");

        // Özet uyuşmazsa dosya kaydedilmez ve geçici dosya kalmaz.
        let other = dir.join("b.iso");
        assert!(fetch(&format!("{base}/a.iso"), &other, Some(&"0".repeat(64)), &any).unwrap_err().contains("uyuşmuyor"));
        assert!(fetch(&format!("{base}/a.iso"), &other, Some("kisa"), &any).unwrap_err().contains("64 onaltılık"));
        assert!(fetch(&format!("{base}/bos"), &other, None, &any).unwrap_err().contains("boş dosya"));
        assert!(fetch(&format!("{base}/yok"), &other, None, &any).unwrap_err().contains("404"));
        assert!(!other.exists() && hidden(&dir) == 0);

        // Yönlendirmenin gösterdiği adres de denetlenir: iç ağa sıçranamaz.
        let seen = std::sync::Mutex::new(Vec::new());
        let strict = |url: &str| {
            seen.lock().unwrap().push(url.to_string());
            if url.contains("169.254.") { Err("iç adres".to_string()) } else { Ok(()) }
        };
        assert_eq!(fetch(&format!("{base}/ic"), &other, None, &strict).unwrap_err(), "iç adres");
        assert_eq!(seen.lock().unwrap().len(), 2, "ilk adres ve yönlendirme hedefi");
        assert!(!other.exists() && hidden(&dir) == 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn uploads_are_atomic_and_never_overwrite() {
        let dir = temp("upload");
        let dest = dir.join("yeni.iso");
        let mut up = Upload::begin(dest.clone()).unwrap();
        up.write(axum::body::Bytes::from_static(b"bir ")).await.unwrap();
        assert!(!dest.exists(), "tamamlanmadan görünmez");
        up.write(axum::body::Bytes::from_static(b"iki")).await.unwrap();
        assert_eq!(up.finish(Some(&hex(&Sha256::digest(b"bir iki")))).await.unwrap(), 7);
        let mut text = String::new();
        std::fs::File::open(&dest).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!((text.as_str(), hidden(&dir)), ("bir iki", 0));

        assert!(Upload::begin(dest.clone()).unwrap_err().contains("zaten var"));
        // Yarıda bırakılan ve özeti tutmayan yükleme iz bırakmaz.
        let other = dir.join("diger.iso");
        let mut dropped = Upload::begin(other.clone()).unwrap();
        dropped.write(axum::body::Bytes::from_static(b"yarim")).await.unwrap();
        drop(dropped);
        let mut wrong = Upload::begin(other.clone()).unwrap();
        wrong.write(axum::body::Bytes::from_static(b"veri")).await.unwrap();
        assert!(wrong.finish(Some(&"a".repeat(64))).await.unwrap_err().contains("uyuşmuyor"));
        assert!(Upload::begin(other.clone()).unwrap().finish(None).await.unwrap_err().contains("boş dosya"));
        assert!(!other.exists() && hidden(&dir) == 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
