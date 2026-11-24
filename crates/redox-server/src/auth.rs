//! Kimlik doğrulama: kullanıcı dosyası, parola özetleri, oturumlar, API token'ları
//! ve giriş denemesi sınırlayıcı.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const SESSION_COOKIE: &str = "redox_session";
/// Hareketsiz oturumun ömrü.
pub const IDLE_SECS: u64 = 2 * 3600;
/// Oturumun, kullanılsa bile, en uzun ömrü.
pub const ABSOLUTE_SECS: u64 = 12 * 3600;
pub const MIN_PASSWORD_CHARS: usize = 12;
const MAX_FAILURES: u32 = 5;
const WINDOW_SECS: u64 = 15 * 60;
const LOCK_SECS: u64 = 15 * 60;
const MAX_SESSIONS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Her şeyi yapabilir.
    Admin,
    /// Kapsamındaki konukları açıp kapatır, konsolunu kullanır, anlık görüntü ve yedek alır;
    /// konuk oluşturamaz, silemez, ayar değiştiremez.
    Operator,
    /// Yalnızca görüntüler; durum değiştiren istekler reddedilir.
    Viewer,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Admin => "yönetici",
            Role::Operator => "operatör",
            Role::Viewer => "izleyici",
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "admin" => Ok(Role::Admin),
            "operator" => Ok(Role::Operator),
            "viewer" => Ok(Role::Viewer),
            _ => Err(format!("rol \"admin\", \"operator\" ya da \"viewer\" olmalı: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiToken {
    pub name: String,
    /// Token'ın kendisi saklanmaz; yalnızca SHA-256 özeti.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    /// Argon2id, PHC biçiminde.
    pub password_hash: String,
    pub role: Role,
    #[serde(default)]
    pub tokens: Vec<ApiToken>,
    /// Etkin iki adımlı doğrulama anahtarı (base32).
    #[serde(default)]
    pub totp_secret: Option<String>,
    /// Kurulumu başlatılmış ama henüz doğrulanmamış anahtar.
    #[serde(default)]
    pub totp_pending: Option<String>,
    /// Operatörün yetkili olduğu konuk etiketleri; boşsa bütün konuklar. Öteki rollerde kullanılmaz.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Tek oturum açmada bu kullanıcıyla eşleşen kimlik (ör. e-posta adresi).
    #[serde(default)]
    pub sso: Option<String>,
}

/// Parola özetinin maliyeti. `Test` yalnızca testleri hızlandırmak içindir.
#[derive(Debug, Clone, Copy)]
pub enum Cost {
    Production,
    Test,
}

fn argon(cost: Cost) -> Argon2<'static> {
    let params = match cost {
        // OWASP önerisi: Argon2id, 19 MiB bellek, 2 tur, 1 iş parçacığı.
        Cost::Production => Params::new(19_456, 2, 1, None),
        Cost::Test => Params::new(8, 1, 1, None),
    }
    .expect("sabit Argon2 parametreleri geçerli");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn fill_random(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("işletim sistemi rastgele sayı üretemedi");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hash_password(password: &str, cost: Cost) -> String {
    let mut salt = [0u8; 16];
    fill_random(&mut salt);
    let salt = SaltString::encode_b64(&salt).expect("16 baytlık tuz kodlanabilir");
    argon(cost)
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2 özetleme başarısız olmamalı")
        .to_string()
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// 256 bit rastgele, onaltılık token (oturum ve API için).
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes);
    hex(&bytes)
}

pub fn sha256_hex(input: &str) -> String {
    hex(&Sha256::digest(input.as_bytes()))
}

pub fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

pub fn check_password_policy(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!("parola en az {MIN_PASSWORD_CHARS} karakter olmalı"));
    }
    if password.len() > 1024 {
        return Err("parola 1024 bayttan uzun olamaz".to_string());
    }
    Ok(())
}

/// Dosyayı yalnızca sahibinin okuyabileceği izinlerle ve atomik olarak yazar.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserFile {
    pub users: Vec<User>,
}

impl UserFile {
    /// Dosya yoksa boş liste döner; bozuksa hata.
    pub fn load(path: &Path) -> Result<UserFile, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{} okunamadı: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserFile::default()),
            Err(e) => Err(format!("{} açılamadı: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        write_private(path, &json).map_err(|e| format!("{} yazılamadı: {e}", path.display()))
    }

    pub fn find(&self, name: &str) -> Option<&User> {
        self.users.iter().find(|u| u.name == name)
    }

    fn find_mut(&mut self, name: &str) -> Result<&mut User, String> {
        self.users
            .iter_mut()
            .find(|u| u.name == name)
            .ok_or_else(|| format!("kullanıcı bulunamadı: {name}"))
    }

    pub fn add_user(&mut self, name: &str, password: &str, role: Role, cost: Cost) -> Result<(), String> {
        if !valid_username(name) {
            return Err(format!(
                "geçersiz kullanıcı adı \"{name}\": 1-32 karakter; küçük harf, rakam, nokta, tire ve alt çizgi kullanın"
            ));
        }
        if self.find(name).is_some() {
            return Err(format!("{name} adında bir kullanıcı zaten var"));
        }
        check_password_policy(password)?;
        self.users.push(User {
            name: name.to_string(),
            password_hash: hash_password(password, cost),
            role,
            tokens: Vec::new(),
            totp_secret: None,
            totp_pending: None,
            scope: Vec::new(),
            sso: None,
        });
        Ok(())
    }

    /// Parolayı değiştirir. Özet değiştiği için kullanıcının açık oturumları da düşer.
    pub fn set_password(&mut self, name: &str, password: &str, cost: Cost) -> Result<(), String> {
        check_password_policy(password)?;
        self.find_mut(name)?.password_hash = hash_password(password, cost);
        Ok(())
    }

    /// Son yönetici silinemez: aksi halde sisteme kimse giremez.
    pub fn remove_user(&mut self, name: &str) -> Result<(), String> {
        let user = self.find(name).ok_or_else(|| format!("kullanıcı bulunamadı: {name}"))?;
        let admins = self.users.iter().filter(|u| u.role == Role::Admin).count();
        if user.role == Role::Admin && admins == 1 {
            return Err(format!("{name} son yönetici; önce başka bir yönetici ekleyin"));
        }
        self.users.retain(|u| u.name != name);
        Ok(())
    }

    /// Rolü değiştirir. Son yönetici izleyiciye çevrilemez.
    pub fn set_role(&mut self, name: &str, role: Role) -> Result<(), String> {
        let admins = self.users.iter().filter(|u| u.role == Role::Admin).count();
        let user = self.find_mut(name)?;
        if user.role == Role::Admin && role != Role::Admin && admins == 1 {
            return Err(format!("{name} son yönetici; önce başka bir yönetici ekleyin"));
        }
        user.role = role;
        Ok(())
    }

    /// Operatörün yetki kapsamını (konuk etiketleri) belirler; boş liste bütün konuklar demektir.
    pub fn set_scope(&mut self, name: &str, mut tags: Vec<String>) -> Result<(), String> {
        if tags.len() > redox_core::MAX_TAGS {
            return Err(format!("en fazla {} etiket yazılabilir", redox_core::MAX_TAGS));
        }
        if let Some(bad) = tags.iter().find(|t| !redox_core::valid_tag(t)) {
            return Err(format!("geçersiz etiket \"{bad}\": küçük harf, rakam ve tire kullanın"));
        }
        tags.sort();
        tags.dedup();
        self.find_mut(name)?.scope = tags;
        Ok(())
    }

    /// Tek oturum açma kimliğini belirler; boş değer eşleşmeyi kaldırır.
    pub fn set_sso(&mut self, name: &str, subject: &str) -> Result<(), String> {
        let subject = subject.trim();
        if subject.len() > 254 || subject.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("geçersiz tek oturum açma kimliği".to_string());
        }
        if !subject.is_empty() && self.users.iter().any(|u| u.name != name && u.sso.as_deref() == Some(subject)) {
            return Err(format!("{subject} başka bir kullanıcıyla eşleşmiş"));
        }
        self.find_mut(name)?.sso = Some(subject.to_string()).filter(|s| !s.is_empty());
        Ok(())
    }

    /// İki adımlı doğrulama kurulumunu başlatır: yeni gizli anahtar üretir ama henüz etkinleştirmez.
    pub fn begin_totp(&mut self, name: &str) -> Result<String, String> {
        let secret = crate::totp::generate_secret();
        self.find_mut(name)?.totp_pending = Some(secret.clone());
        Ok(secret)
    }

    /// Kullanıcı uygulamasındaki kodu girince kurulumu tamamlar.
    pub fn confirm_totp(&mut self, name: &str, code: &str, now: u64) -> Result<(), String> {
        let user = self.find_mut(name)?;
        let pending = user.totp_pending.clone().ok_or("önce kurulumu başlatın")?;
        if crate::totp::verify(&pending, code, now).is_none() {
            return Err("kod doğrulanamadı; uygulamadaki güncel 6 haneli kodu girin".to_string());
        }
        user.totp_secret = Some(pending);
        user.totp_pending = None;
        Ok(())
    }

    pub fn disable_totp(&mut self, name: &str) -> Result<(), String> {
        let user = self.find_mut(name)?;
        user.totp_secret = None;
        user.totp_pending = None;
        Ok(())
    }

    /// Yeni API token'ı üretir. Düz metni yalnızca bu çağrı döndürür.
    pub fn create_token(&mut self, user: &str, label: &str) -> Result<String, String> {
        let plain = format!("redox_{}", random_token());
        self.add_token(user, label, &plain)?;
        Ok(plain)
    }

    pub fn add_token(&mut self, user: &str, label: &str, plain: &str) -> Result<(), String> {
        if label.is_empty() || label.len() > 64 {
            return Err("token etiketi 1-64 karakter olmalı".to_string());
        }
        let user = self.find_mut(user)?;
        if user.tokens.iter().any(|t| t.name == label) {
            return Err(format!("\"{label}\" etiketli bir token zaten var"));
        }
        user.tokens.push(ApiToken { name: label.to_string(), sha256: sha256_hex(plain) });
        Ok(())
    }

    pub fn revoke_token(&mut self, user: &str, label: &str) -> Result<(), String> {
        let user = self.find_mut(user)?;
        let before = user.tokens.len();
        user.tokens.retain(|t| t.name != label);
        if user.tokens.len() == before {
            return Err(format!("\"{label}\" etiketli token bulunamadı"));
        }
        Ok(())
    }
}

/// Doğrulanmış istek sahibi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user: String,
    pub role: Role,
    /// Operatörün yetkili olduğu etiketler (boşsa hepsi).
    pub scope: Vec<String>,
}

impl Identity {
    pub fn can_write(&self) -> bool {
        self.role == Role::Admin
    }

    /// Bu etiketleri taşıyan konuk üzerinde güç, konsol, anlık görüntü ve yedek işlemleri yapabilir mi?
    pub fn can_operate(&self, tags: &[String]) -> bool {
        match self.role {
            Role::Admin => true,
            Role::Operator => self.scope.is_empty() || tags.iter().any(|t| self.scope.contains(t)),
            Role::Viewer => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginError {
    /// Kullanıcı yok ya da parola yanlış (hangisi olduğu bilerek söylenmez).
    Invalid,
    /// Çok fazla hatalı deneme; saniye cinsinden kalan bekleme.
    Locked(u64),
}

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    user: String,
    /// Giriş anındaki parola özeti; parola değişirse oturum geçersizleşir.
    password_hash: String,
    created: u64,
    last_seen: u64,
}

#[derive(Default)]
struct Attempts {
    failures: u32,
    first: u64,
    locked_until: u64,
}

struct Cached {
    file: UserFile,
    mtime: Option<SystemTime>,
}

pub struct Auth {
    path: Option<PathBuf>,
    users: Mutex<Cached>,
    /// Anahtar oturum token'ının SHA-256 özetidir: dosya sızsa da oturum çalınamaz.
    sessions: Mutex<HashMap<String, Session>>,
    /// Oturumların diskteki yeri (kullanıcı dosyasının yanında); bellekte çalışırken `None`.
    sessions_path: Option<PathBuf>,
    /// Oturumların diske son yazıldığı an.
    sessions_saved: Mutex<u64>,
    attempts: Mutex<HashMap<String, Attempts>>,
    /// TLS arkasında: çerezler `Secure` olur, HSTS gönderilir, X-Forwarded-For'a güvenilir.
    pub behind_tls_proxy: bool,
    /// Var olmayan kullanıcıda da aynı süreyi harcamak için.
    dummy_hash: String,
    cost: Cost,
    /// Kullanıcı başına en son kabul edilen TOTP adımı (yeniden kullanım engeli).
    totp_used: Mutex<HashMap<String, u64>>,
    pub audit: crate::audit::Audit,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Auth {
    /// `path` verilirse dosya değiştikçe (ör. `redox user passwd`) yeniden okunur.
    pub fn new(file: UserFile, path: Option<PathBuf>, behind_tls_proxy: bool, cost: Cost) -> Self {
        let mtime = path.as_deref().and_then(mtime);
        // Denetim günlüğü kullanıcı dosyasının yanında durur.
        let audit = crate::audit::Audit::new(path.as_ref().map(|p| p.with_file_name("audit.log")));
        let sessions_path = path.as_ref().map(|p| p.with_file_name("sessions.json"));
        // Hizmet yeniden başlayınca açık oturumlar sürer. Bozuk dosya yalnızca herkesi yeniden girişe yollar.
        let stored: HashMap<String, Session> = sessions_path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Auth {
            sessions_path,
            sessions_saved: Mutex::new(0),
            audit,
            cost,
            totp_used: Mutex::new(HashMap::new()),
            path,
            users: Mutex::new(Cached { file, mtime }),
            sessions: Mutex::new(stored),
            attempts: Mutex::new(HashMap::new()),
            behind_tls_proxy,
            dummy_hash: hash_password("var-olmayan-kullanici-icin", cost),
        }
    }

    fn users(&self) -> UserFile {
        let mut cached = lock(&self.users);
        if let Some(path) = &self.path {
            let current = mtime(path);
            if current != cached.mtime {
                match UserFile::load(path) {
                    Ok(file) => cached.file = file,
                    // Yarım yazılmış ya da bozuk dosya: eldeki listeyle devam et.
                    Err(e) => eprintln!("uyarı: kullanıcı dosyası yeniden okunamadı: {e}"),
                }
                cached.mtime = current;
            }
        }
        cached.file.clone()
    }

    pub fn has_users(&self) -> bool {
        !self.users().users.is_empty()
    }

    pub fn list(&self) -> Vec<User> {
        self.users().users
    }

    pub fn cost(&self) -> Cost {
        self.cost
    }

    pub fn check_password(&self, name: &str, password: &str) -> bool {
        self.users().find(name).is_some_and(|u| verify_password(&u.password_hash, password))
    }

    /// Kullanıcı listesini değiştirir ve dosyaya yazar. Hata dönerse hiçbir şey değişmez.
    pub fn edit<R>(&self, f: impl FnOnce(&mut UserFile) -> Result<R, String>) -> Result<R, String> {
        let mut cached = lock(&self.users);
        if let Some(path) = &self.path {
            // Bu arada komut satırından değişmiş olabilir: üzerine yazmadan önce güncelini al.
            if mtime(path) != cached.mtime {
                cached.file = UserFile::load(path)?;
            }
        }
        let mut copy = cached.file.clone();
        let result = f(&mut copy)?;
        if let Some(path) = &self.path {
            copy.save(path)?;
            cached.mtime = mtime(path);
        }
        cached.file = copy;
        Ok(result)
    }

    /// İkinci adım: kullanıcıda açıksa kod doğru ve daha önce kullanılmamış olmalı.
    fn second_factor_ok(&self, user: Option<&User>, code: &str, now: u64) -> bool {
        let Some((user, secret)) = user.and_then(|u| u.totp_secret.as_ref().map(|s| (u, s))) else {
            return true;
        };
        let Some(step) = crate::totp::verify(secret, code, now) else {
            return false;
        };
        let mut used = lock(&self.totp_used);
        if used.get(&user.name).is_some_and(|last| *last >= step) {
            return false; // aynı kod ikinci kez kullanılamaz
        }
        used.insert(user.name.clone(), step);
        true
    }

    pub fn login(&self, client: &str, name: &str, password: &str, now: u64) -> Result<String, LoginError> {
        self.login_with_code(client, name, password, "", now)
    }

    /// Parolayı (ve açıksa doğrulama kodunu) denetler; yeni bir oturum token'ı döndürür.
    pub fn login_with_code(&self, client: &str, name: &str, password: &str, code: &str, now: u64) -> Result<String, LoginError> {
        let key = format!("{client}|{}", name.to_lowercase());
        if let Some(a) = lock(&self.attempts).get(&key) {
            if a.locked_until > now {
                return Err(LoginError::Locked(a.locked_until - now));
            }
        }

        let user = self.users().find(name).cloned();
        let hash = user.as_ref().map(|u| u.password_hash.as_str()).unwrap_or(&self.dummy_hash);
        // Hangi adımın yanlış olduğu bilerek söylenmez.
        let ok = verify_password(hash, password) && user.is_some() && self.second_factor_ok(user.as_ref(), code, now);

        let mut attempts = lock(&self.attempts);
        if !ok {
            if attempts.len() > MAX_SESSIONS {
                attempts.retain(|_, a| a.locked_until > now || a.first + WINDOW_SECS > now);
            }
            let a = attempts.entry(key).or_default();
            if a.first + WINDOW_SECS <= now {
                *a = Attempts { failures: 0, first: now, locked_until: 0 };
            }
            a.failures += 1;
            if a.failures >= MAX_FAILURES {
                a.locked_until = now + LOCK_SECS;
                a.failures = 0;
                a.first = now;
                return Err(LoginError::Locked(LOCK_SECS));
            }
            return Err(LoginError::Invalid);
        }
        attempts.remove(&key);
        drop(attempts);

        Ok(self.open_session(user.expect("ok ise kullanıcı vardır"), now))
    }

    fn open_session(&self, user: User, now: u64) -> String {
        let token = random_token();
        let mut sessions = lock(&self.sessions);
        // Süresi dolanlar her girişte atılır: dosya da kendiliğinden küçülür.
        sessions.retain(|_, s| !expired(s, now));
        sessions.insert(sha256_hex(&token), Session { user: user.name, password_hash: user.password_hash, created: now, last_seen: now });
        self.save_sessions(&sessions, now);
        token
    }

    /// Tek oturum açma: kimlik sağlayıcının doğruladığı kimlikle eşleşen yerel kullanıcıya oturum açar.
    pub fn login_sso(&self, subject: &str, now: u64) -> Option<(String, String)> {
        let user = self.users().users.iter().find(|u| u.sso.as_deref() == Some(subject) && !subject.is_empty()).cloned()?;
        let name = user.name.clone();
        Some((self.open_session(user, now), name))
    }

    fn save_sessions(&self, sessions: &HashMap<String, Session>, now: u64) {
        let Some(path) = &self.sessions_path else { return };
        *lock(&self.sessions_saved) = now;
        let written = serde_json::to_vec(sessions).map_err(|e| e.to_string()).and_then(|bytes| write_private(path, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = written {
            eprintln!("uyarı: oturumlar {} dosyasına yazılamadı: {e}", path.display());
        }
    }

    /// Oturum çerezini doğrular. Kullanıcı silinmiş ya da parolası değişmişse oturum düşer.
    pub fn session(&self, token: &str, now: u64) -> Option<Identity> {
        let users = self.users();
        let mut sessions = lock(&self.sessions);
        let key = sha256_hex(token);
        let s = sessions.get_mut(&key)?;
        let user = users.find(&s.user).filter(|u| u.password_hash == s.password_hash);
        match user {
            Some(u) if !expired(s, now) => {
                s.last_seen = now;
                let identity = Identity { user: u.name.clone(), role: u.role, scope: u.scope.clone() };
                // Son görülme anı beş dakikada bir diske yazılır: her istekte yazmak gereksizdir.
                if now >= *lock(&self.sessions_saved) + 300 {
                    self.save_sessions(&sessions, now);
                }
                Some(identity)
            }
            _ => {
                sessions.remove(&key);
                self.save_sessions(&sessions, now);
                None
            }
        }
    }

    pub fn logout(&self, token: &str) {
        let mut sessions = lock(&self.sessions);
        sessions.remove(&sha256_hex(token));
        let now = *lock(&self.sessions_saved);
        self.save_sessions(&sessions, now);
    }

    /// `Authorization: Bearer` token'ını doğrular (sabit zamanlı karşılaştırma).
    pub fn bearer(&self, token: &str) -> Option<Identity> {
        let digest = sha256_hex(token);
        let users = self.users();
        let mut found = None;
        for u in &users.users {
            for t in &u.tokens {
                if bool::from(t.sha256.as_bytes().ct_eq(digest.as_bytes())) {
                    found = Some(Identity { user: u.name.clone(), role: u.role, scope: u.scope.clone() });
                }
            }
        }
        found
    }

    pub fn session_cookie(&self, token: &str) -> String {
        format!(
            "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ABSOLUTE_SECS}{}",
            if self.behind_tls_proxy { "; Secure" } else { "" }
        )
    }

    pub fn clear_cookie(&self) -> String {
        format!(
            "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
            if self.behind_tls_proxy { "; Secure" } else { "" }
        )
    }
}

fn expired(s: &Session, now: u64) -> bool {
    now >= s.last_seen + IDLE_SECS || now >= s.created + ABSOLUTE_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "dogru-at-pil-zimba";

    fn users() -> UserFile {
        let mut f = UserFile::default();
        f.add_user("admin", PW, Role::Admin, Cost::Test).unwrap();
        f.add_user("izleyici", "izleyici-parolasi-1", Role::Viewer, Cost::Test).unwrap();
        f
    }

    fn auth() -> Auth {
        Auth::new(users(), None, false, Cost::Test)
    }

    #[test]
    fn sessions_survive_a_restart_without_storing_the_token() {
        let dir = std::env::temp_dir().join(format!("redox-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.json");
        let mut file = UserFile::default();
        file.add_user("admin", PW, Role::Admin, Cost::Test).unwrap();
        file.save(&path).unwrap();

        let first = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
        let token = first.login("ip", "admin", PW, 1_000).unwrap();
        let other = first.login("ip", "admin", PW, 1_000).unwrap();
        let stored = std::fs::read_to_string(dir.join("sessions.json")).unwrap();
        assert!(!stored.contains(&token) && stored.contains(&sha256_hex(&token)), "dosyada token'ın kendisi değil özeti durur");
        drop(first);

        // Yeniden başlayan hizmet oturumu tanır; süre sınırları kaldığı yerden işler.
        let second = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
        assert_eq!(second.session(&token, 1_500).map(|i| i.user), Some("admin".to_string()));
        assert!(second.session(&token, 1_500 + IDLE_SECS).is_none(), "boşta kalma süresi yeniden başlamaz");
        // Çıkış kalıcıdır.
        second.logout(&other);
        drop(second);
        let third = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
        assert!(third.session(&other, 1_600).is_none() && third.session(&token, 1_600).is_none());

        // Bozuk dosya hizmeti durdurmaz: yalnızca herkes yeniden giriş yapar.
        std::fs::write(dir.join("sessions.json"), "{ bozuk").unwrap();
        let fourth = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
        assert!(fourth.login("ip", "admin", PW, 2_000).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scope_and_sso_are_validated() {
        let mut f = UserFile::default();
        f.add_user("nobet", PW, Role::Operator, Cost::Test).unwrap();
        f.add_user("ikinci", PW, Role::Viewer, Cost::Test).unwrap();
        f.set_scope("nobet", vec!["web".into(), "db".into(), "web".into()]).unwrap();
        assert_eq!(f.find("nobet").unwrap().scope, ["db", "web"], "sıralı ve yinelemesiz");
        assert!(f.set_scope("nobet", vec!["Büyük Harf".into()]).unwrap_err().contains("geçersiz etiket"));
        assert!(f.set_scope("yok", Vec::new()).is_err());
        assert_eq!("operator".parse::<Role>(), Ok(Role::Operator));

        let op = Identity { user: "nobet".into(), role: Role::Operator, scope: vec!["web".into()] };
        assert!(op.can_operate(&["web".into(), "x".into()]) && !op.can_operate(&["db".into()]) && !op.can_operate(&[]) && !op.can_write());
        let all = Identity { scope: Vec::new(), ..op.clone() };
        assert!(all.can_operate(&[]), "boş kapsam bütün konuklar demektir");
        let viewer = Identity { role: Role::Viewer, ..all.clone() };
        assert!(!viewer.can_operate(&["web".into()]));

        f.set_sso("nobet", "nobet@example.com").unwrap();
        assert!(f.set_sso("ikinci", "nobet@example.com").unwrap_err().contains("başka bir kullanıcıyla"));
        assert!(f.set_sso("ikinci", "iki satır\nx").is_err());
        f.set_sso("nobet", "").unwrap();
        assert_eq!(f.find("nobet").unwrap().sso, None);
    }

    #[test]
    fn password_hash_verifies_and_is_salted() {
        let a = hash_password(PW, Cost::Test);
        let b = hash_password(PW, Cost::Test);
        assert!(a.starts_with("$argon2id$"));
        assert_ne!(a, b, "aynı parola farklı tuzla farklı özet vermeli");
        assert!(verify_password(&a, PW) && verify_password(&b, PW));
        assert!(!verify_password(&a, "yanlis-parola-123"));
        assert!(!verify_password("bozuk-ozet", PW));
        assert!(!verify_password("", ""));
    }

    #[test]
    fn production_cost_uses_owasp_parameters() {
        let h = hash_password(PW, Cost::Production);
        assert!(h.contains("m=19456,t=2,p=1"), "{h}");
        assert!(verify_password(&h, PW));
    }

    #[test]
    fn tokens_are_random_and_long() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn user_rules() {
        let mut f = users();
        assert!(f.add_user("admin", PW, Role::Admin, Cost::Test).unwrap_err().contains("zaten var"));
        assert!(f.add_user("Büyük", PW, Role::Admin, Cost::Test).unwrap_err().contains("geçersiz kullanıcı adı"));
        assert!(f.add_user("", PW, Role::Admin, Cost::Test).is_err());
        assert!(f.add_user("yeni", "kisa", Role::Viewer, Cost::Test).unwrap_err().contains("en az 12 karakter"));
        assert!(f.set_password("admin", "kisa", Cost::Test).is_err());
        assert!(f.set_password("yok", PW, Cost::Test).unwrap_err().contains("bulunamadı"));

        assert!(f.remove_user("admin").unwrap_err().contains("son yönetici"));
        f.remove_user("izleyici").unwrap();
        f.add_user("ikinci", PW, Role::Admin, Cost::Test).unwrap();
        f.remove_user("admin").unwrap();
        assert_eq!(f.users.len(), 1);
    }

    #[test]
    fn login_and_session_lifecycle() {
        let a = auth();
        assert_eq!(a.login("ip", "admin", "yanlis-parola-123", 100), Err(LoginError::Invalid));
        assert_eq!(a.login("ip", "yok", PW, 100), Err(LoginError::Invalid));

        let t = a.login("ip", "admin", PW, 100).unwrap();
        assert_eq!(a.session(&t, 200), Some(Identity { user: "admin".into(), role: Role::Admin, scope: Vec::new() }));
        assert_eq!(a.session("uydurma", 200), None);

        let t2 = a.login("ip", "admin", PW, 100).unwrap();
        assert_ne!(t, t2, "her giriş yeni token üretir");
        a.logout(&t);
        assert_eq!(a.session(&t, 200), None);
        assert!(a.session(&t2, 200).is_some(), "çıkış yalnızca o oturumu kapatır");
    }

    #[test]
    fn sessions_expire_when_idle_or_too_old() {
        let a = auth();
        let t = a.login("ip", "admin", PW, 0).unwrap();
        assert!(a.session(&t, IDLE_SECS - 1).is_some());
        assert!(a.session(&t, 2 * IDLE_SECS - 2).is_some(), "kullanım hareketsizlik süresini uzatır");
        assert!(a.session(&t, 2 * IDLE_SECS - 2 + IDLE_SECS).is_none(), "hareketsiz oturum düşer");

        let t = a.login("ip", "admin", PW, 0).unwrap();
        let mut now = 0;
        while now + IDLE_SECS / 2 < ABSOLUTE_SECS {
            now += IDLE_SECS / 2;
            assert!(a.session(&t, now).is_some(), "{now}");
        }
        assert!(a.session(&t, ABSOLUTE_SECS).is_none(), "sürekli kullanılsa da mutlak süre dolunca düşer");
    }

    #[test]
    fn lockout_after_repeated_failures_is_per_client() {
        let a = auth();
        for _ in 0..MAX_FAILURES - 1 {
            assert_eq!(a.login("1.1.1.1", "admin", "yanlis-parola-123", 0), Err(LoginError::Invalid));
        }
        assert_eq!(a.login("1.1.1.1", "admin", "yanlis-parola-123", 0), Err(LoginError::Locked(LOCK_SECS)));
        assert_eq!(a.login("1.1.1.1", "admin", PW, 10), Err(LoginError::Locked(LOCK_SECS - 10)), "kilitliyken doğru parola da reddedilir");
        assert_eq!(a.login("1.1.1.1", "ADMIN", PW, 10), Err(LoginError::Locked(LOCK_SECS - 10)), "büyük/küçük harf kilidi aşmaz");
        assert!(a.login("2.2.2.2", "admin", PW, 10).is_ok(), "başka istemci etkilenmez");
        assert!(a.login("1.1.1.1", "admin", PW, LOCK_SECS).is_ok(), "süre dolunca açılır");
    }

    #[test]
    fn failures_outside_window_do_not_accumulate() {
        let a = auth();
        for i in 0..20 {
            let now = i * (WINDOW_SECS + 1);
            assert_eq!(a.login("ip", "admin", "yanlis-parola-123", now), Err(LoginError::Invalid));
        }
        // Başarılı giriş sayacı sıfırlar.
        let now = 21 * (WINDOW_SECS + 1);
        for _ in 0..MAX_FAILURES - 1 {
            a.login("ip", "admin", "yanlis-parola-123", now).unwrap_err();
        }
        a.login("ip", "admin", PW, now).unwrap();
        assert_eq!(a.login("ip", "admin", "yanlis-parola-123", now), Err(LoginError::Invalid));
    }

    #[test]
    fn bearer_tokens_are_stored_hashed() {
        let mut f = users();
        let plain = f.create_token("izleyici", "ci").unwrap();
        assert!(plain.starts_with("redox_") && plain.len() == 70);
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains(&plain), "düz token dosyaya yazılmamalı");
        assert!(f.create_token("izleyici", "ci").unwrap_err().contains("zaten var"));
        assert!(f.create_token("yok", "ci").is_err());

        let a = Auth::new(f.clone(), None, false, Cost::Test);
        assert_eq!(a.bearer(&plain), Some(Identity { user: "izleyici".into(), role: Role::Viewer, scope: Vec::new() }));
        assert_eq!(a.bearer("redox_yanlis"), None);
        assert_eq!(a.bearer(""), None);

        f.revoke_token("izleyici", "ci").unwrap();
        assert!(f.revoke_token("izleyici", "ci").is_err());
        assert_eq!(Auth::new(f, None, false, Cost::Test).bearer(&plain), None);
    }

    #[test]
    fn cookie_flags() {
        let plain = Auth::new(users(), None, false, Cost::Test).session_cookie("t");
        assert!(plain.contains("HttpOnly") && plain.contains("SameSite=Strict") && !plain.contains("Secure"));
        let tls = Auth::new(users(), None, true, Cost::Test);
        assert!(tls.session_cookie("t").ends_with("; Secure"));
        assert!(tls.clear_cookie().contains("Max-Age=0"));
    }

    #[test]
    fn user_file_changes_take_effect_without_restart() {
        let dir = std::env::temp_dir().join(format!("redox-auth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.json");
        assert_eq!(UserFile::load(&path), Ok(UserFile::default()), "dosya yoksa boş liste");

        let mut f = users();
        f.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let a = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
        let t = a.login("ip", "admin", PW, 0).unwrap();
        assert!(a.session(&t, 1).is_some());

        // Parola değişince (dosya yeniden yazılır) eski oturum düşer, eski parola geçmez.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        f.set_password("admin", "yepyeni-parola-456", Cost::Test).unwrap();
        f.save(&path).unwrap();
        assert!(a.session(&t, 2).is_none());
        assert_eq!(a.login("ip", "admin", PW, 2), Err(LoginError::Invalid));
        assert!(a.login("ip", "admin", "yepyeni-parola-456", 2).is_ok());

        std::fs::write(&path, b"{ bozuk").unwrap();
        assert!(UserFile::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn second_factor_is_required_once_enabled() {
        let mut f = users();
        let secret = f.begin_totp("admin").unwrap();
        assert!(f.find("admin").unwrap().totp_secret.is_none(), "doğrulanana kadar etkin değil");
        assert!(f.confirm_totp("admin", "000000", 1000).is_err() || crate::totp::verify(&secret, "000000", 1000).is_some());
        let code = |now: u64| format!("{:06}", crate::totp::code_at(&crate::totp::base32_decode(&secret).unwrap(), now / 30));
        assert!(f.confirm_totp("izleyici", &code(1000), 1000).unwrap_err().contains("önce kurulumu"));
        f.confirm_totp("admin", &code(1000), 1000).unwrap();
        assert_eq!(f.find("admin").unwrap().totp_pending, None);

        let a = Auth::new(f.clone(), None, false, Cost::Test);
        assert_eq!(a.login("ip", "admin", PW, 2000), Err(LoginError::Invalid), "kod olmadan parola yetmez");
        assert_eq!(a.login_with_code("ip", "admin", PW, "123", 2000), Err(LoginError::Invalid));
        assert_eq!(a.login_with_code("ip", "admin", "yanlis-parola-123", &code(2000), 2000), Err(LoginError::Invalid));
        let token = a.login_with_code("ip2", "admin", PW, &code(2000), 2000).unwrap();
        assert!(a.session(&token, 2001).is_some());
        // Aynı kod (ya da daha eski bir adımın kodu) ikinci kez kullanılamaz.
        assert_eq!(a.login_with_code("ip3", "admin", PW, &code(2000), 2001), Err(LoginError::Invalid));
        assert_eq!(a.login_with_code("ip4", "admin", PW, &code(1970), 2001), Err(LoginError::Invalid));
        assert!(a.login_with_code("ip5", "admin", PW, &code(2060), 2060).is_ok(), "sonraki adımın kodu geçer");
        // Kodu olmayan kullanıcı etkilenmez.
        assert!(a.login("ip", "izleyici", "izleyici-parolasi-1", 2000).is_ok());

        f.disable_totp("admin").unwrap();
        assert!(Auth::new(f, None, false, Cost::Test).login("ip", "admin", PW, 2000).is_ok());
    }

    #[test]
    fn wrong_codes_count_toward_lockout() {
        let mut f = users();
        f.begin_totp("admin").unwrap();
        let secret = f.find("admin").unwrap().totp_pending.clone().unwrap();
        let code = format!("{:06}", crate::totp::code_at(&crate::totp::base32_decode(&secret).unwrap(), 0));
        f.confirm_totp("admin", &code, 0).unwrap();
        let a = Auth::new(f, None, false, Cost::Test);
        for _ in 0..MAX_FAILURES - 1 {
            assert_eq!(a.login_with_code("ip", "admin", PW, "000001", 90_000), Err(LoginError::Invalid));
        }
        assert_eq!(a.login_with_code("ip", "admin", PW, "000001", 90_000), Err(LoginError::Locked(LOCK_SECS)));
    }

    #[test]
    fn roles_and_edits() {
        let mut f = users();
        assert!(f.set_role("admin", Role::Viewer).unwrap_err().contains("son yönetici"));
        f.set_role("izleyici", Role::Admin).unwrap();
        f.set_role("admin", Role::Viewer).unwrap();
        assert!(f.set_role("yok", Role::Admin).is_err());

        let a = auth();
        assert!(a.check_password("admin", PW) && !a.check_password("admin", "x") && !a.check_password("yok", PW));
        // Başarısız düzenleme hiçbir şeyi değiştirmez.
        let err = a.edit(|f| { f.remove_user("izleyici")?; f.remove_user("admin") }).unwrap_err();
        assert!(err.contains("son yönetici"));
        assert_eq!(a.list().len(), 2);
        a.edit(|f| f.add_user("yeni", "yeni-kullanici-parolasi", Role::Viewer, Cost::Test)).unwrap();
        assert_eq!(a.list().len(), 3);
        assert!(a.login("ip", "yeni", "yeni-kullanici-parolasi", 0).is_ok());
    }

    #[test]
    fn edits_are_written_to_the_user_file() {
        let dir = std::env::temp_dir().join(format!("redox-auth-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.json");
        users().save(&path).unwrap();
        let a = Auth::new(UserFile::load(&path).unwrap(), Some(path.clone()), false, Cost::Test);
        let t = a.login("ip", "izleyici", "izleyici-parolasi-1", 0).unwrap();

        a.edit(|f| f.set_role("izleyici", Role::Admin)).unwrap();
        assert_eq!(UserFile::load(&path).unwrap().find("izleyici").unwrap().role, Role::Admin);
        assert_eq!(a.session(&t, 1).unwrap().role, Role::Admin, "rol değişikliği açık oturuma hemen yansır");
        a.edit(|f| f.remove_user("izleyici")).unwrap();
        assert!(a.session(&t, 2).is_none(), "silinen kullanıcının oturumu düşer");

        a.audit.record(5, "admin", "ip", "deneme", true);
        assert!(dir.join("audit.log").is_file(), "denetim günlüğü kullanıcı dosyasının yanına yazılır");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
