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

mod keys;
mod password;
mod users;

pub use keys::*;
pub use password::*;
pub use users::*;

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
    /// Salt okunur token: sahibi yönetici olsa da yalnızca okuma yapabilir (izleme araçları için).
    #[serde(default)]
    pub read_only: bool,
    /// Bu andan (Unix saniyesi) sonra token geçmez; `None` süresizdir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
}

impl ApiToken {
    pub fn expired(&self, now: u64) -> bool {
        self.expires.is_some_and(|end| now >= end)
    }
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
    /// Parolası LDAP / Active Directory'de doğrulanır; yerel parola özeti kullanılmaz.
    #[serde(default)]
    pub ldap: bool,
    /// Kayıtlı güvenlik anahtarları (WebAuthn). Doluysa giriş parolaya ek olarak anahtar ya da uygulama kodu ister.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<SecurityKey>,
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
    /// Parola doğru; oturum için kayıtlı güvenlik anahtarı gerekiyor.
    SecondStep(Box<KeyChallenge>),
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
    /// Sunucu TLS'i kendisi sonlandırıyor: çerezler `Secure` olur (X-Forwarded-For'a güvenilmez).
    pub direct_tls: bool,
    /// Dizin sunucusu (`--ldap`); `ldap` işaretli kullanıcılar parolalarıyla yalnızca buradan doğrulanır.
    pub ldap: Option<crate::ldap::Config>,
    /// Güvenlik anahtarları için panelin adresi (`--webauthn-origin`).
    pub webauthn: Option<crate::webauthn::Relying>,
    /// Yanıt bekleyen güvenlik anahtarı sınamaları (bilet → sınama).
    key_challenges: Mutex<HashMap<String, keys::Pending>>,
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

/// Deneme sayacının anahtarı: istemci ve kullanıcı adı.
fn attempt_key(client: &str, name: &str) -> String {
    format!("{client}|{}", name.to_lowercase())
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Auth {
    /// `path` verilirse dosya değiştikçe (ör. `redox user passwd`) yeniden okunur.
    pub fn secure_cookies(&self) -> bool {
        self.behind_tls_proxy || self.direct_tls
    }

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
            direct_tls: false,
            ldap: None,
            webauthn: None,
            key_challenges: Mutex::new(HashMap::new()),
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
        self.users().find(name).is_some_and(|u| self.password_ok(u, password))
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
        let Some(user) = user else { return true };
        let Some(secret) = &user.totp_secret else {
            // Yalnızca güvenlik anahtarı olan hesapta kod ikinci adımın yerine geçmez.
            return user.keys.is_empty();
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

    /// Kullanıcının parolası doğru mu? Dizin kullanıcısında yanıtı dizin verir; dizine ulaşılamazsa giriş yoktur.
    fn password_ok(&self, user: &User, password: &str) -> bool {
        if !user.ldap {
            return verify_password(&user.password_hash, password);
        }
        let Some(directory) = &self.ldap else {
            eprintln!("uyarı: {} dizin kullanıcısı ama sunucu --ldap olmadan çalışıyor", user.name);
            return false;
        };
        match directory.bind(&user.name, password) {
            Ok(()) => true,
            Err(crate::ldap::BindError::Refused) => false,
            Err(crate::ldap::BindError::Unavailable(why)) => {
                eprintln!("uyarı: LDAP: {why}");
                false
            }
        }
    }

    pub fn login(&self, client: &str, name: &str, password: &str, now: u64) -> Result<String, LoginError> {
        self.login_with_code(client, name, password, "", now)
    }

    /// Parolayı (ve açıksa doğrulama kodunu) denetler; yeni bir oturum token'ı döndürür.
    pub fn login_with_code(&self, client: &str, name: &str, password: &str, code: &str, now: u64) -> Result<String, LoginError> {
        let key = attempt_key(client, name);
        if let Some(wait) = self.locked_for(&key, now) {
            return Err(LoginError::Locked(wait));
        }

        let user = self.users().find(name).cloned();
        // Var olmayan kullanıcıda da özet hesaplanır: yanıt süresi kullanıcı adlarını ele vermesin.
        let password_ok = match &user {
            Some(user) => self.password_ok(user, password),
            None => verify_password(&self.dummy_hash, password),
        };
        // Güvenlik anahtarı olan hesapta kod yazılmadıysa sıradaki adım anahtardır. Deneme sayacı burada ne artar
        // ne sıfırlanır: oturum henüz açılmamıştır.
        if let Some(user) = user.as_ref().filter(|u| password_ok && code.is_empty() && !u.keys.is_empty()) {
            return Err(match self.begin_key_login(user, now) {
                Ok(challenge) => LoginError::SecondStep(Box::new(challenge)),
                Err(why) => {
                    // Anahtar denetlenemiyorsa giriş yoktur.
                    eprintln!("uyarı: {}: {why}", user.name);
                    LoginError::Invalid
                }
            });
        }
        // Hangi adımın yanlış olduğu bilerek söylenmez.
        let ok = password_ok && user.is_some() && self.second_factor_ok(user.as_ref(), code, now);
        if !ok {
            return Err(self.count_failure(key, now));
        }
        self.forget_failures(&key);

        Ok(self.open_session(user.expect("ok ise kullanıcı vardır"), now))
    }

    fn locked_for(&self, key: &str, now: u64) -> Option<u64> {
        lock(&self.attempts).get(key).filter(|a| a.locked_until > now).map(|a| a.locked_until - now)
    }

    fn forget_failures(&self, key: &str) {
        lock(&self.attempts).remove(key);
    }

    /// Hatalı bir denemeyi sayar; sınır aşılırsa istemci o kullanıcı için bir süre kilitlenir.
    fn count_failure(&self, key: String, now: u64) -> LoginError {
        let mut attempts = lock(&self.attempts);
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
            return LoginError::Locked(LOCK_SECS);
        }
        LoginError::Invalid
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

    /// `session` gibi denetler ama oturumu "görüldü" saymaz ve değiştirmez. Açık bir terminal, kendi yetki
    /// denetimiyle web oturumunun hareketsizlik süresini uzatmasın diye.
    pub fn peek_session(&self, token: &str, now: u64) -> Option<Identity> {
        let users = self.users();
        let sessions = lock(&self.sessions);
        let s = sessions.get(&sha256_hex(token))?;
        let user = users.find(&s.user).filter(|u| u.password_hash == s.password_hash)?;
        (!expired(s, now)).then(|| Identity { user: user.name.clone(), role: user.role, scope: user.scope.clone() })
    }

    pub fn logout(&self, token: &str) {
        let mut sessions = lock(&self.sessions);
        sessions.remove(&sha256_hex(token));
        let now = *lock(&self.sessions_saved);
        self.save_sessions(&sessions, now);
    }

    /// `Authorization: Bearer` token'ını doğrular (sabit zamanlı karşılaştırma). Süresi dolan token geçmez.
    pub fn bearer(&self, token: &str) -> Option<Identity> {
        self.bearer_at(token, crate::state::now())
    }

    pub fn bearer_at(&self, token: &str, now: u64) -> Option<Identity> {
        let digest = sha256_hex(token);
        let users = self.users();
        let mut found = None;
        for u in &users.users {
            for t in &u.tokens {
                if bool::from(t.sha256.as_bytes().ct_eq(digest.as_bytes())) && !t.expired(now) {
                    // Salt okunur token, sahibinin rolünü izleyiciye indirir.
                    let role = if t.read_only { Role::Viewer } else { u.role };
                    found = Some(Identity { user: u.name.clone(), role, scope: u.scope.clone() });
                }
            }
        }
        found
    }

    pub fn session_cookie(&self, token: &str) -> String {
        format!(
            "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ABSOLUTE_SECS}{}",
            if self.secure_cookies() { "; Secure" } else { "" }
        )
    }

    pub fn clear_cookie(&self) -> String {
        format!(
            "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
            if self.secure_cookies() { "; Secure" } else { "" }
        )
    }
}

fn expired(s: &Session, now: u64) -> bool {
    now >= s.last_seen + IDLE_SECS || now >= s.created + ABSOLUTE_SECS
}

#[cfg(test)]
mod tests;
