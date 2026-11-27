//! Birleşik görünüm: başka RedoX VE sunucularının düğüm ve konuklarını salt okunur olarak toplar.
//!
//! Bu gerçek bir küme değildir: ortak durum ve çoğunluk oylaması yoktur. (Çalışan konuk sunucular arasında
//! canlı taşınabilir; bkz. `state/migrate.rs`.) Her sunucu kendi konuklarını yönetir; burası yalnızca hepsini tek
//! sayfada gösterir ve yönetmek için ilgili sunucuya bağlantı verir.

use std::sync::Mutex;
use std::time::Duration;

use redox_core::{Guest, GuestStatus, Node};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Bir eş sunucudan okunan yanıtın üst sınırı.
const MAX_BODY: u64 = 4 * 1024 * 1024;
pub const MAX_PEERS: usize = 32;

/// Yapılandırılmış eş sunucu. Token yalnızca istek başlığında kullanılır; hiçbir çıktıya girmez.
#[derive(Clone)]
pub struct Peer {
    pub name: String,
    pub url: String,
    token: String,
}

/// Token hata ayıklama çıktısına da yazılmaz.
impl std::fmt::Debug for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer({} {})", self.name, self.url)
    }
}

/// Bir eşin son okunan durumu (arayüz ve API bunu gösterir).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct PeerView {
    pub name: String,
    pub url: String,
    /// Son denemenin zamanı; hiç denenmediyse `None`.
    pub checked: Option<u64>,
    /// Son deneme başarısızsa nedeni.
    pub error: Option<String>,
    /// Eşin kendi küme adı.
    pub cluster: String,
    pub nodes: Vec<Node>,
    pub guests: Vec<Guest>,
    /// Eşin dağıtık anahtarları; okunamadıysa (ör. eski sürüm) `None`.
    pub vswitches: Option<Vec<redox_core::VSwitch>>,
}

impl PeerView {
    pub fn online(&self) -> bool {
        self.checked.is_some() && self.error.is_none()
    }

    pub fn running(&self) -> usize {
        self.guests.iter().filter(|g| g.status == GuestStatus::Running).count()
    }
}

fn valid_peer_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Adresi doğrular ve sondaki `/` işaretini atar. Token ağdan geçtiği için şifresiz HTTP
/// yalnızca aynı makinedeki (loopback) eşlerde kabul edilir.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let url = raw.trim_end_matches('/');
    let (secure, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (_, Some(rest)) => (false, rest),
        _ => return Err(format!("adres https:// ile başlamalı: {raw}")),
    };
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b)) {
        return Err(format!("adres yalnızca sunucu adı ve kapı içermeli (yol, sorgu ya da kullanıcı bilgisi olmadan): {raw}"));
    }
    let host = match rest.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => rest.split(':').next().unwrap_or(""),
    };
    if !secure && !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Err(format!("şifresiz http yalnızca aynı makinedeki eşlerde kullanılabilir; https kullanın: {raw}"));
    }
    Ok(url.to_string())
}

/// Eş dosyasını okur: her satır `AD ADRES TOKEN`; boş satırlar ve `#` ile başlayanlar atlanır.
pub fn parse_file(text: &str) -> Result<Vec<Peer>, String> {
    let mut peers: Vec<Peer> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |message: String| format!("eş dosyası, satır {}: {message}", i + 1);
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [name, url, token] = parts[..] else {
            return Err(at("\"AD ADRES TOKEN\" bekleniyordu".to_string()));
        };
        if !valid_peer_name(name) {
            return Err(at(format!("geçersiz ad \"{name}\" (küçük harf, rakam ve tire; en çok 32 karakter)")));
        }
        if peers.iter().any(|p| p.name == name) {
            return Err(at(format!("\"{name}\" adı iki kez geçiyor")));
        }
        // Token başlığa yazılır: yalnızca yazdırılabilir ASCII (satır sonu kaçırılamaz).
        if token.len() < 16 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(at("token geçersiz (redox token create ile üretilen değeri yazın)".to_string()));
        }
        peers.push(Peer { name: name.to_string(), url: normalize_url(url).map_err(at)?, token: token.to_string() });
    }
    if peers.len() > MAX_PEERS {
        return Err(format!("eş dosyası: en çok {MAX_PEERS} eş tanımlanabilir"));
    }
    Ok(peers)
}

#[derive(Default)]
pub struct Peers {
    peers: Mutex<Vec<Peer>>,
    views: Mutex<Vec<PeerView>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Peers {
    pub fn configure(&self, peers: Vec<Peer>) {
        *lock(&self.views) = peers.iter().map(|p| PeerView { name: p.name.clone(), url: p.url.clone(), ..Default::default() }).collect();
        *lock(&self.peers) = peers;
    }

    pub fn views(&self) -> Vec<PeerView> {
        lock(&self.views).clone()
    }

    /// Bütün eşleri sırayla okur (engelleyen ağ çağrıları; arka plan iş parçacığında çağrılır).
    pub fn poll(&self, now: u64) {
        let peers = lock(&self.peers).clone();
        for peer in peers {
            let view = fetch(&peer, now);
            if let Some(slot) = lock(&self.views).iter_mut().find(|v| v.name == peer.name) {
                *slot = view;
            }
        }
    }
}

fn fetch(peer: &Peer, now: u64) -> PeerView {
    let mut view = PeerView { name: peer.name.clone(), url: peer.url.clone(), checked: Some(now), ..Default::default() };
    // Yönlendirme izlenmez: token başka bir adrese gitmemeli.
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).redirects(0).build();
    let result = (|| -> Result<(String, Vec<Node>, Vec<Guest>), String> {
        let summary: serde_json::Value = get(&agent, peer, "/api/v1/cluster")?;
        Ok((
            summary["name"].as_str().unwrap_or("").chars().take(64).collect(),
            get(&agent, peer, "/api/v1/nodes")?,
            get(&agent, peer, "/api/v1/guests")?,
        ))
    })();
    match result {
        Ok((cluster, nodes, guests)) => {
            view.cluster = cluster;
            view.nodes = nodes;
            view.guests = guests;
            // İsteğe bağlı bilgi: bunu sunmayan eş yine "erişilebilir" sayılır. Rapordaki ek alanlar atılır.
            view.vswitches = get::<Vec<serde_json::Value>>(&agent, peer, "/api/v1/vswitches").ok().and_then(|all| all.into_iter().map(switch_of).collect());
        }
        Err(e) => view.error = Some(e),
    }
    view
}

/// Eşin anahtar raporundan yalnızca tanım alanlarını alır.
fn switch_of(report: serde_json::Value) -> Option<redox_core::VSwitch> {
    let only = serde_json::json!({ "name": report["name"], "vni": report["vni"], "mtu": report["mtu"], "members": report["members"] });
    serde_json::from_value(only).ok()
}

fn get<T: DeserializeOwned>(agent: &ureq::Agent, peer: &Peer, path: &str) -> Result<T, String> {
    use std::io::Read;
    let response = agent
        .get(&format!("{}{path}", peer.url))
        .set("Authorization", &format!("Bearer {}", peer.token))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => "token reddedildi (401)".to_string(),
            ureq::Error::Status(code, _) => format!("sunucu {code} yanıtı verdi"),
            ureq::Error::Transport(t) => format!("erişilemiyor: {}", t.kind()),
        })?;
    if response.status() != 200 {
        return Err(format!("sunucu {} yanıtı verdi", response.status()));
    }
    let mut body = Vec::new();
    response.into_reader().take(MAX_BODY + 1).read_to_end(&mut body).map_err(|e| format!("yanıt okunamadı: {e}"))?;
    if body.len() as u64 > MAX_BODY {
        return Err("yanıt çok büyük".to_string());
    }
    serde_json::from_slice(&body).map_err(|_| "yanıt anlaşılamadı (RedoX VE sunucusu mu?)".to_string())
}

/// Eş sunucuya yapılan çağrıların hata iletisi.
fn peer_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => "eş sunucu token'ı reddetti (401)".to_string(),
        ureq::Error::Status(403, _) => "eş sunucudaki token yönetici yetkili değil (403); taşıma için yönetici token'ı gerekir".to_string(),
        ureq::Error::Status(code, response) => {
            let detail = response.into_json::<serde_json::Value>().ok().and_then(|v| v["error"].as_str().map(str::to_string));
            match detail {
                Some(text) => format!("eş sunucu isteği reddetti ({code}): {text}"),
                None => format!("eş sunucu {code} yanıtı verdi"),
            }
        }
        ureq::Error::Transport(t) => format!("eş sunucuya erişilemiyor: {}", t.kind()),
    }
}

/// Yedek sunucunun ikiz için verdiği yanıt.
pub struct FtOpened {
    pub nbd_port: u16,
    pub migrate_port: u16,
    pub host: Option<String>,
}

/// Hedefin kopya için verdiği yanıt.
pub struct MirrorOpened {
    pub generation: u32,
    pub port: u16,
    /// Kaynağın bağlanacağı adres; hedef bildirmediyse eşin yönetim adresindeki ad kullanılır.
    pub host: Option<String>,
}

/// `https://ad:8006/…` adresindeki sunucu adı ya da IP (köşeli ayraçsız).
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?']).next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.0,
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    redox_core::valid_migrate_host(host).then(|| host.to_string())
}

impl Peer {
    pub fn ft_prepare(&self, request: &crate::state::ft::FtPrepare) -> Result<FtOpened, String> {
        let body = serde_json::to_value(request).map_err(|e| e.to_string())?;
        let reply: serde_json::Value = self.request("POST", "/api/v1/ft/standby", Duration::from_secs(60)).send_json(body).map_err(peer_error)?.into_json().map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())?;
        let port = |name: &str| reply[name].as_u64().and_then(|p| u16::try_from(p).ok()).ok_or_else(|| "eş sunucunun yanıtı anlaşılamadı".to_string());
        Ok(FtOpened { nbd_port: port("nbd_port")?, migrate_port: port("migrate_port")?, host: reply["host"].as_str().filter(|h| redox_core::valid_migrate_host(h)).map(str::to_string) })
    }

    /// Kısa zaman aşımlı çağrı: hata toleransının kararları saniyeler içinde verilmelidir.
    fn quick(&self, method: &str, path: &str, body: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
        let request = ureq::AgentBuilder::new().timeout(Duration::from_secs(3)).redirects(0).build().request(method, &format!("{}{path}", self.url)).set("Authorization", &format!("Bearer {}", self.token));
        let response = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        }
        .map_err(peer_error)?;
        if response.status() == 204 {
            return Ok(serde_json::Value::Null);
        }
        response.into_json().map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())
    }

    /// Dönen değer: yedek sunucuda ikiz duruyor mu?
    pub fn ft_beat(&self, origin: &str, vmid: redox_core::VmId, generation: u32, alive: bool) -> Result<bool, String> {
        let reply = self.quick("POST", &format!("/api/v1/ft/standby/{origin}/{vmid}/beat"), Some(serde_json::json!({ "generation": generation, "alive": alive })))?;
        reply["standby"].as_bool().ok_or_else(|| "eş sunucunun yanıtı anlaşılamadı".to_string())
    }

    pub fn ft_discard(&self, origin: &str, vmid: redox_core::VmId, generation: u32) -> Result<(), String> {
        self.request("DELETE", &format!("/api/v1/ft/standby/{origin}/{vmid}?generation={generation}"), Duration::from_secs(30)).call().map(|_| ()).map_err(peer_error)
    }

    /// Hakemden oy ister; kazanan tarafı döndürür.
    pub fn ft_arbitrate(&self, key: &str, side: redox_core::FtSide) -> Result<redox_core::FtSide, String> {
        let reply = self.quick("POST", "/api/v1/ft/arbitrate", Some(serde_json::json!({ "key": key, "side": side })))?;
        serde_json::from_value(reply["winner"].clone()).map_err(|_| "hakemin yanıtı anlaşılamadı".to_string())
    }

    /// Oy vermeden sorar: karar verildiyse kazanan.
    pub fn ft_winner(&self, key: &str) -> Result<Option<redox_core::FtSide>, String> {
        let reply = self.quick("GET", &format!("/api/v1/ft/arbitrate/{key}"), None)?;
        serde_json::from_value(reply["winner"].clone()).map_err(|_| "hakemin yanıtı anlaşılamadı".to_string())
    }

    pub fn host_name(&self) -> Option<String> {
        url_host(&self.url)
    }

    pub fn mirror_prepare(&self, origin: &str, guest: &Guest, sizes: &[u64], key: &str) -> Result<MirrorOpened, String> {
        let body = serde_json::json!({ "origin": origin, "guest": guest, "sizes": sizes, "key": key });
        let reply: serde_json::Value = self
            .request("POST", "/api/v1/mirror-stores", Duration::from_secs(60))
            .send_json(body)
            .map_err(peer_error)?
            .into_json()
            .map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())?;
        let number = |name: &str| reply[name].as_u64().ok_or_else(|| "eş sunucunun yanıtı anlaşılamadı".to_string());
        Ok(MirrorOpened {
            generation: u32::try_from(number("generation")?).map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())?,
            port: u16::try_from(number("port")?).map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())?,
            host: reply["host"].as_str().filter(|h| redox_core::valid_migrate_host(h)).map(str::to_string),
        })
    }

    pub fn mirror_report(&self, origin: &str, vmid: redox_core::VmId, generation: u32, in_sync: bool) -> Result<(), String> {
        let body = serde_json::json!({ "generation": generation, "in_sync": in_sync });
        self.request("POST", &format!("/api/v1/mirror-stores/{origin}/{vmid}/report"), Duration::from_secs(20)).send_json(body).map(|_| ()).map_err(peer_error)
    }

    pub fn mirror_delete(&self, origin: &str, vmid: redox_core::VmId) -> Result<(), String> {
        self.request("DELETE", &format!("/api/v1/mirror-stores/{origin}/{vmid}"), Duration::from_secs(60)).call().map(|_| ()).map_err(peer_error)
    }

    fn request(&self, method: &str, path: &str, read_timeout: Duration) -> ureq::Request {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(read_timeout)
            .redirects(0)
            .build()
            .request(method, &format!("{}{path}", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
    }

    fn json(&self, method: &str, path: &str) -> Result<serde_json::Value, String> {
        let response = self.request(method, path, Duration::from_secs(30)).call().map_err(peer_error)?;
        if response.status() == 204 {
            return Ok(serde_json::Value::Null);
        }
        response.into_json().map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())
    }

    pub fn vswitch_put(&self, switch: &serde_json::Value) -> Result<(), String> {
        self.request("POST", "/api/v1/vswitches", Duration::from_secs(30)).send_json(switch.clone()).map(|_| ()).map_err(peer_error)
    }

    /// Eşte anahtarı siler; eşte zaten yoksa başarı sayılır.
    pub fn vswitch_delete(&self, name: &str) -> Result<(), String> {
        let listed = self.json("GET", "/api/v1/vswitches")?;
        if !listed.as_array().is_some_and(|all| all.iter().any(|s| s["name"] == name)) {
            return Ok(());
        }
        self.request("DELETE", &format!("/api/v1/vswitches/{name}"), Duration::from_secs(30)).call().map(|_| ()).map_err(peer_error)
    }

    /// Bu eşin kendi eşlerini nasıl gördüğü: (ad, adres, ulaşılıyor mu). Devralmada hakem oyu olarak kullanılır.
    pub fn peers_seen(&self) -> Result<Vec<(String, String, bool)>, String> {
        let listed = self.json("GET", "/api/v1/peers")?;
        let text = |p: &serde_json::Value, key: &str| p[key].as_str().unwrap_or_default().to_string();
        Ok(listed.as_array().ok_or("eş sunucunun yanıtı anlaşılamadı")?.iter().map(|p| (text(p, "name"), text(p, "url"), !p["checked"].is_null() && p["error"].is_null())).collect())
    }

    pub fn guest(&self, vmid: redox_core::VmId) -> Result<Guest, String> {
        serde_json::from_value(self.json("GET", &format!("/api/v1/guests/{vmid}"))?).map_err(|_| "eş sunucudaki konuk kaydı okunamadı".to_string())
    }

    /// Eşte yedek başlatır ve tamamlanmasını bekler; yedeğin kimliğini döndürür.
    pub fn backup_and_wait(&self, vmid: redox_core::VmId, timeout: Duration) -> Result<String, String> {
        let id = self.json("POST", &format!("/api/v1/guests/{vmid}/backups"))?["id"].as_str().map(str::to_string).ok_or("eş sunucu yedek kimliği döndürmedi")?;
        let command = format!("vm backup {vmid}");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let backups = self.json("GET", "/api/v1/backups")?;
            if backups.as_array().is_some_and(|list| list.iter().any(|b| b["id"] == id.as_str())) {
                return Ok(id);
            }
            // En yeni yedek görevi başarısızsa beklemenin anlamı yok.
            let tasks = self.json("GET", "/api/v1/tasks")?;
            if let Some(task) = tasks.as_array().and_then(|list| list.iter().find(|t| t["command"] == command.as_str())) {
                if task["status"] == "failed" {
                    return Err(format!("eş sunucuda yedek alınamadı: {}", task["message"].as_str().unwrap_or("neden bildirilmedi")));
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("eş sunucudaki yedek zamanında tamamlanmadı".to_string());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Yedek dosyasını indirir: önce geçici ada, bitince asıl ada. İndirilen bayt sayısını döndürür.
    /// Yedekteki diskin ham boyutu ve blok özetleri (artımlı çoğaltma).
    pub fn backup_blocks(&self, id: &str, disk: u8) -> Result<(u64, Vec<String>), String> {
        let v = self.request("GET", &format!("/api/v1/backups/{id}/blocks?disk={disk}"), Duration::from_secs(3600)).call().map_err(peer_error)?.into_json::<serde_json::Value>().map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())?;
        let size = v["size"].as_u64().ok_or("eş sunucu disk boyutunu bildirmedi")?;
        let hashes = v["hashes"].as_array().ok_or("eş sunucu blok özetlerini bildirmedi")?.iter().filter_map(|h| h.as_str().map(str::to_string)).collect();
        Ok((size, hashes))
    }

    /// Yedekteki diskin bir bloğu.
    pub fn download_block(&self, id: &str, disk: u8, index: u64) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let response = self.request("GET", &format!("/api/v1/backups/{id}/block?disk={disk}&index={index}"), Duration::from_secs(300)).call().map_err(peer_error)?;
        let mut data = Vec::new();
        response.into_reader().take(crate::delta::BLOCK + 1).read_to_end(&mut data).map_err(|e| format!("blok indirilemedi: {e}"))?;
        Ok(data)
    }

    pub fn download_backup(&self, id: &str, dest: &std::path::Path) -> Result<u64, String> {
        self.download_backup_disk(id, 0, dest)
    }

    /// Yedeğin makine durumu arşivini (UEFI değişkenleri, sanal TPM) indirir; yedekte arşiv yoksa `false`.
    pub fn download_machine(&self, id: &str, dest: &std::path::Path) -> Result<bool, String> {
        match self.request("GET", &format!("/api/v1/backups/{id}/machine"), Duration::from_secs(120)).call() {
            Ok(response) => {
                let mut data = Vec::new();
                use std::io::Read;
                response.into_reader().take(256 * 1024 * 1024).read_to_end(&mut data).map_err(|e| format!("makine durumu indirilemedi: {e}"))?;
                std::fs::write(dest, data).map(|()| true).map_err(|e| format!("{} yazılamadı: {e}", dest.display()))
            }
            Err(ureq::Error::Status(404, _)) => Ok(false),
            Err(e) => Err(peer_error(e)),
        }
    }

    /// Yedekteki bir diski indirir: 0 asıl disk, 1 ve üstü ek diskler.
    pub fn download_backup_disk(&self, id: &str, disk: u8, dest: &std::path::Path) -> Result<u64, String> {
        let query = if disk == 0 { String::new() } else { format!("?disk={disk}") };
        let response = self.request("GET", &format!("/api/v1/backups/{id}/download{query}"), Duration::from_secs(120)).call().map_err(peer_error)?;
        let expected: Option<u64> = response.header("content-length").and_then(|v| v.parse().ok());
        let tmp = dest.with_extension("indiriliyor");
        let copied = std::fs::File::create(&tmp).and_then(|mut out| {
            let n = std::io::copy(&mut response.into_reader(), &mut out)?;
            out.sync_all()?;
            Ok(n)
        });
        match copied {
            Ok(n) if expected.is_none_or(|e| e == n) => std::fs::rename(&tmp, dest).map(|()| n).map_err(|e| format!("{} yazılamadı: {e}", dest.display())),
            other => {
                let _ = std::fs::remove_file(&tmp);
                Err(match other {
                    Ok(_) => "indirme yarıda kesildi (boyut tutmuyor)".to_string(),
                    Err(e) => format!("yedek indirilemedi: {e}"),
                })
            }
        }
    }

    /// Eşteki konuk canlı taşınabilir mi? Taşınabiliyorsa hedefin açması gereken donanım yerleşimi döner.
    pub fn migrate_prepare(&self, vmid: redox_core::VmId) -> Result<u8, String> {
        let reply = self.json("POST", &format!("/api/v1/guests/{vmid}/migrate/prepare"))?;
        reply["layout"].as_u64().and_then(|layout| u8::try_from(layout).ok()).ok_or_else(|| "eş sunucu donanım yerleşimini bildirmedi (canlı taşımayı bilmeyen eski bir sürüm olabilir)".to_string())
    }

    /// Eşe, konuğu taşımayı bekleyen bu sunucuya göndermesini söyler ve görevin bitmesini bekler. `host`
    /// verilmezse eş, isteğin geldiği adrese bağlanır.
    pub fn migrate_and_wait(&self, vmid: redox_core::VmId, host: Option<&str>, nbd_port: u16, ram_port: u16, key: &str, timeout: Duration) -> Result<(), String> {
        let body = serde_json::json!({ "host": host, "nbd_port": nbd_port, "ram_port": ram_port, "key": key });
        let started: serde_json::Value = self
            .request("POST", &format!("/api/v1/guests/{vmid}/migrate"), Duration::from_secs(30))
            .send_json(body)
            .map_err(peer_error)?
            .into_json()
            .map_err(|_| "eş sunucunun yanıtı anlaşılamadı".to_string())?;
        let task = started["id"].as_u64().ok_or("eş sunucu taşıma görevini bildirmedi")?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let tasks = self.json("GET", "/api/v1/tasks")?;
            if let Some(found) = tasks.as_array().and_then(|list| list.iter().find(|t| t["id"] == task)) {
                match found["status"].as_str() {
                    Some("ok") => return Ok(()),
                    Some("failed") => return Err(format!("eş sunucu konuğu gönderemedi: {}", found["message"].as_str().unwrap_or("neden bildirilmedi"))),
                    _ => {}
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("eş sunucudaki taşıma zamanında tamamlanmadı".to_string());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Eşe, bu sunucudaki konuğu kendine canlı taşımasını söyler (`me`: bu sunucunun eşteki adı). İş eşte arka
    /// planda yürür; sonucu `task_by_command` ile izlenir.
    pub fn migrate_pull(&self, me: &str, vmid: redox_core::VmId) -> Result<(), String> {
        self.request("POST", &format!("/api/v1/peers/{me}/migrate/{vmid}"), Duration::from_secs(30)).call().map(|_| ()).map_err(peer_error)
    }

    /// Eşin görev günlüğünde bu komutun en yeni kaydı: `(numara, durum, ileti)`.
    pub fn task_by_command(&self, command: &str) -> Result<Option<(u64, String, String)>, String> {
        let tasks = self.json("GET", "/api/v1/tasks")?;
        Ok(tasks.as_array().and_then(|list| list.iter().find(|t| t["command"] == command)).and_then(|t| Some((t["id"].as_u64()?, t["status"].as_str()?.to_string(), t["message"].as_str().unwrap_or_default().to_string()))))
    }

    /// Eşteki çalışan konuğu askıya alır ve askıya alınmasını bekler.
    pub fn suspend_and_wait(&self, vmid: redox_core::VmId, timeout: Duration) -> Result<(), String> {
        self.json("POST", &format!("/api/v1/guests/{vmid}/suspend"))?;
        let command = format!("vm suspend {vmid}");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let guest = self.guest(vmid)?;
            if guest.hw.suspended && guest.lock.is_none() {
                return Ok(());
            }
            let tasks = self.json("GET", "/api/v1/tasks")?;
            if let Some(task) = tasks.as_array().and_then(|list| list.iter().find(|t| t["command"] == command.as_str())) {
                if task["status"] == "failed" {
                    return Err(format!("eş sunucuda konuk askıya alınamadı: {}", task["message"].as_str().unwrap_or("neden bildirilmedi")));
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("eş sunucudaki konuk zamanında askıya alınmadı".to_string());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Askıdaki konuğun bellek durumunu indirir; `(çalışırken mi askıya alındı, donanım yerleşimi)` döndürür.
    pub fn download_state(&self, vmid: redox_core::VmId, dest: &std::path::Path) -> Result<(bool, u8), String> {
        let response = self.request("GET", &format!("/api/v1/guests/{vmid}/state"), Duration::from_secs(120)).call().map_err(peer_error)?;
        let expected: Option<u64> = response.header("content-length").and_then(|v| v.parse().ok());
        let running = response.header("x-redox-state-running") == Some("1");
        let layout: u8 = response.header("x-redox-state-layout").and_then(|v| v.parse().ok()).ok_or("eş sunucu bellek durumunun yerleşimini bildirmedi (eski sürüm olabilir)")?;
        let copied = std::fs::File::create(dest).and_then(|mut out| {
            let n = std::io::copy(&mut response.into_reader(), &mut out)?;
            out.sync_all()?;
            Ok(n)
        });
        match copied {
            Ok(n) if expected.is_none_or(|e| e == n) => Ok((running, layout)),
            other => {
                let _ = std::fs::remove_file(dest);
                Err(match other {
                    Ok(_) => "bellek durumu indirmesi yarıda kesildi (boyut tutmuyor)".to_string(),
                    Err(e) => format!("bellek durumu indirilemedi: {e}"),
                })
            }
        }
    }

    pub fn delete_backup(&self, id: &str) -> Result<(), String> {
        self.json("DELETE", &format!("/api/v1/backups/{id}")).map(|_| ())
    }
}

impl Peers {
    pub fn peer(&self, name: &str) -> Option<Peer> {
        lock(&self.peers).iter().find(|p| p.name == name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "rdx_0123456789abcdef0123456789abcdef";

    #[test]
    fn parses_peer_file() {
        let text = format!("# eşler\n\nankara https://ankara.example.com:8443/ {TOKEN}\n  yerel   http://127.0.0.1:8007 {TOKEN}\n");
        let peers = parse_file(&text).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!((peers[0].name.as_str(), peers[0].url.as_str()), ("ankara", "https://ankara.example.com:8443"));
        assert_eq!(peers[1].url, "http://127.0.0.1:8007");
        assert!(parse_file("").unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_peer_lines_with_line_numbers() {
        for (line, needle) in [
            ("ankara https://a.example.com".to_string(), "bekleniyordu"),
            (format!("Ankara https://a.example.com {TOKEN}"), "geçersiz ad"),
            (format!("a http://a.example.com {TOKEN}"), "şifresiz http"),
            (format!("a ftp://a.example.com {TOKEN}"), "https:// ile başlamalı"),
            (format!("a https://a.example.com/yol {TOKEN}"), "yalnızca sunucu adı"),
            (format!("a https://kullanici@a.example.com {TOKEN}"), "yalnızca sunucu adı"),
            (format!("a https://a.example.com?x=1 {TOKEN}"), "yalnızca sunucu adı"),
            ("a https://a.example.com kisa".to_string(), "token geçersiz"),
        ] {
            let err = parse_file(&format!("# açıklama\n{line}")).err().unwrap_or_else(|| panic!("{line} kabul edildi"));
            assert!(err.contains("satır 2") && err.contains(needle), "{line}: {err}");
        }
        let twice = format!("a https://a.example.com {TOKEN}\na https://b.example.com {TOKEN}");
        assert!(parse_file(&twice).unwrap_err().contains("iki kez"));
        let many: String = (0..=MAX_PEERS).map(|i| format!("p{i} https://p{i}.example.com {TOKEN}\n")).collect();
        assert!(parse_file(&many).unwrap_err().contains("en çok"));
    }

    #[test]
    fn plain_http_is_only_for_loopback() {
        for ok in ["http://127.0.0.1:8006", "http://localhost:8006", "http://[::1]:8006", "https://10.0.0.5"] {
            assert!(normalize_url(ok).is_ok(), "{ok}");
        }
        for bad in ["http://10.0.0.5:8006", "http://127.0.0.1.evil.example:80", "http://localhost.evil.example", "https://"] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unreachable_peer_is_reported_not_fatal() {
        // Dinleyen yok: bağlantı hemen reddedilir.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let peers = Peers::default();
        peers.configure(parse_file(&format!("kapali http://127.0.0.1:{port} {TOKEN}")).unwrap());
        assert_eq!(peers.views()[0].checked, None, "ilk turdan önce durum bilinmez");
        assert!(!peers.views()[0].online());
        peers.poll(42);
        let v = &peers.views()[0];
        assert_eq!(v.checked, Some(42));
        assert!(v.error.as_deref().unwrap().starts_with("erişilemiyor"), "{:?}", v.error);
        assert!(!v.online() && v.guests.is_empty());
    }
}
