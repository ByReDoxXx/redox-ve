//! Tek oturum açma: OpenID Connect yetkilendirme kodu akışı (PKCE ile).
//!
//! Kimlik sağlayıcı kullanıcıyı doğrular; RedoX VE dönen kimliği (varsayılan: e-posta) yerel bir
//! kullanıcıyla eşleştirir. Rol ve kapsam yereldir: sağlayıcıda hesabı olan herkes giremez,
//! yalnızca `redox user sso` ile eşlenmiş olanlar girer.
//!
//! Kimlik belirteci (ID token) doğrudan sağlayıcının token uç noktasından, sertifikası doğrulanmış
//! TLS üzerinden alınır. OpenID Connect Core 3.1.3.7 bu durumda imza denetimi yerine TLS sunucu
//! doğrulamasına güvenilmesine izin verir; burada da öyle yapılır ve belirteç başka hiçbir
//! yoldan (tarayıcıdan, sorgudan) kabul edilmez.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Başlatılan bir girişin tamamlanması için tanınan süre.
const PENDING_SECS: u64 = 600;
const MAX_PENDING: usize = 1000;
pub const STATE_COOKIE: &str = "redox_oidc";

#[derive(Clone, PartialEq)]
pub struct Config {
    pub issuer: String,
    pub client_id: String,
    client_secret: String,
    pub redirect_url: String,
    /// Yerel kullanıcıyla eşleştirilen alan: `email`, `preferred_username` ya da `sub`.
    pub claim: String,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Config({} {})", self.issuer, self.client_id)
    }
}

/// `https://…` adresi; şifresiz `http://` yalnızca aynı makinede (deneme ve yerel sağlayıcı).
fn secure_url(url: &str) -> bool {
    let local = ["http://127.0.0.1", "http://localhost"].iter().any(|p| url.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/'])));
    (url.starts_with("https://") && url.len() > 8 || local) && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut map = HashMap::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| format!("oidc dosyası, satır {}: \"anahtar = değer\" bekleniyordu", i + 1))?;
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
        let mut take = |key: &str| map.remove(key).filter(|v| !v.is_empty());
        let need = |value: Option<String>, key: &str| value.ok_or_else(|| format!("oidc dosyası: \"{key}\" eksik"));
        let issuer = need(take("issuer"), "issuer")?.trim_end_matches('/').to_string();
        let redirect_url = need(take("redirect_url"), "redirect_url")?;
        for (name, url) in [("issuer", &issuer), ("redirect_url", &redirect_url)] {
            if !secure_url(url) {
                return Err(format!("oidc dosyası: \"{name}\" https:// ile başlamalı: {url}"));
            }
        }
        if !redirect_url.ends_with("/login/oidc/callback") {
            return Err("oidc dosyası: \"redirect_url\" panelin adresi + /login/oidc/callback olmalı".to_string());
        }
        let claim = take("claim").unwrap_or_else(|| "email".to_string());
        if !matches!(claim.as_str(), "email" | "preferred_username" | "sub") {
            return Err(format!("oidc dosyası: \"claim\" email, preferred_username ya da sub olmalı: {claim}"));
        }
        let config = Config { issuer, client_id: need(take("client_id"), "client_id")?, client_secret: need(take("client_secret"), "client_secret")?, redirect_url, claim };
        if let Some(unknown) = map.keys().next() {
            return Err(format!("oidc dosyası: bilinmeyen anahtar \"{unknown}\""));
        }
        Ok(config)
    }
}

struct Pending {
    nonce: String,
    verifier: String,
    created: u64,
}

pub struct Oidc {
    pub config: Config,
    /// Sağlayıcının keşif belgesinden okunan (yetkilendirme, token) uç noktaları.
    endpoints: Mutex<Option<(String, String)>>,
    pending: Mutex<HashMap<String, Pending>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn random(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| format!("rastgele sayı üretilemedi: {e}"))?;
    Ok(base64url(&buf))
}

/// URL güvenli, dolgusuz base64.
pub fn base64url(bytes: &[u8]) -> String {
    crate::mail::base64(bytes).trim_end_matches('=').replace('+', "-").replace('/', "_")
}

pub fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let values: Option<Vec<u32>> = text.trim_end_matches('=').bytes().map(|b| TABLE.iter().position(|t| *t == b).map(|p| p as u32)).collect();
    let values = values?;
    if values.len() % 4 == 1 {
        return None;
    }
    Some(
        values
            .chunks(4)
            .flat_map(|c| {
                let n = c.iter().enumerate().fold(0u32, |n, (i, v)| n | v << (18 - 6 * i));
                (0..c.len() - 1).map(move |i| (n >> (16 - 8 * i)) as u8).collect::<Vec<_>>()
            })
            .collect(),
    )
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).redirects(0).build()
}

impl Oidc {
    pub fn new(config: Config) -> Self {
        Oidc { config, endpoints: Mutex::new(None), pending: Mutex::new(HashMap::new()) }
    }

    /// Keşif belgesini (bir kez) okur. Belgedeki `issuer` yapılandırılanla aynı olmalıdır.
    fn endpoints(&self) -> Result<(String, String), String> {
        if let Some(known) = lock(&self.endpoints).clone() {
            return Ok(known);
        }
        let url = format!("{}/.well-known/openid-configuration", self.config.issuer);
        let doc: Value = agent().get(&url).call().map_err(|e| format!("kimlik sağlayıcıya erişilemedi: {e}"))?.into_json().map_err(|_| "kimlik sağlayıcının keşif belgesi okunamadı".to_string())?;
        if doc["issuer"].as_str().map(|i| i.trim_end_matches('/')) != Some(self.config.issuer.as_str()) {
            return Err("kimlik sağlayıcının bildirdiği adres yapılandırılanla aynı değil".to_string());
        }
        let field = |name: &str| doc[name].as_str().filter(|u| secure_url(u)).map(str::to_string).ok_or_else(|| format!("keşif belgesinde geçerli \"{name}\" yok"));
        let found = (field("authorization_endpoint")?, field("token_endpoint")?);
        *lock(&self.endpoints) = Some(found.clone());
        Ok(found)
    }

    /// Girişi başlatır: tarayıcının gideceği adresi ve çereze yazılacak durum değerini döndürür.
    pub fn begin(&self, now: u64) -> Result<(String, String), String> {
        let (authorize, _) = self.endpoints()?;
        let (state, nonce, verifier) = (random(24)?, random(24)?, random(48)?);
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        let mut pending = lock(&self.pending);
        pending.retain(|_, p| p.created + PENDING_SECS > now);
        if pending.len() >= MAX_PENDING {
            return Err("çok fazla yarım kalmış giriş var; biraz sonra yeniden deneyin".to_string());
        }
        pending.insert(state.clone(), Pending { nonce: nonce.clone(), verifier, created: now });
        let query = serde_urlencoded::to_string([
            ("response_type", "code"),
            ("client_id", self.config.client_id.as_str()),
            ("redirect_uri", self.config.redirect_url.as_str()),
            ("scope", "openid email profile"),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ])
        .map_err(|e| e.to_string())?;
        let join = if authorize.contains('?') { '&' } else { '?' };
        Ok((format!("{authorize}{join}{query}"), state))
    }

    /// Dönüşü tamamlar: kodu belirteçle değiştirir ve eşleştirilecek kimliği döndürür.
    /// `cookie_state` girişi başlatan tarayıcının çerezidir; sorgudaki `state` ile aynı olmalıdır.
    pub fn finish(&self, code: &str, state: &str, cookie_state: Option<&str>, now: u64) -> Result<String, String> {
        if state.is_empty() || cookie_state != Some(state) {
            return Err("giriş bu tarayıcıda başlatılmamış ya da süresi dolmuş; yeniden deneyin".to_string());
        }
        let pending = lock(&self.pending).remove(state).filter(|p| p.created + PENDING_SECS > now).ok_or("girişin süresi dolmuş; yeniden deneyin")?;
        let (_, token_endpoint) = self.endpoints()?;
        let response: Value = agent()
            .post(&token_endpoint)
            .send_form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.config.redirect_url),
                ("client_id", &self.config.client_id),
                ("client_secret", &self.config.client_secret),
                ("code_verifier", &pending.verifier),
            ])
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => format!("kimlik sağlayıcı kodu kabul etmedi ({code})"),
                ureq::Error::Transport(t) => format!("kimlik sağlayıcıya erişilemedi: {}", t.kind()),
            })?
            .into_json()
            .map_err(|_| "kimlik sağlayıcının yanıtı okunamadı".to_string())?;
        let claims = response["id_token"].as_str().and_then(|t| t.split('.').nth(1)).and_then(base64url_decode).and_then(|b| serde_json::from_slice::<Value>(&b).ok()).ok_or("kimlik sağlayıcı kimlik belirteci döndürmedi")?;
        self.subject(&claims, &pending.nonce, now)
    }

    /// Belirtecin bu istemci ve bu giriş için kesildiğini denetler; eşleştirilecek kimliği döndürür.
    pub fn subject(&self, claims: &Value, nonce: &str, now: u64) -> Result<String, String> {
        let audience_ok = match &claims["aud"] {
            Value::String(aud) => aud == &self.config.client_id,
            // Birden çok alıcı varsa yetkili taraf da bu istemci olmalıdır.
            Value::Array(list) => list.iter().any(|a| a == self.config.client_id.as_str()) && (list.len() == 1 || claims["azp"] == self.config.client_id.as_str()),
            _ => false,
        };
        if claims["iss"].as_str().map(|i| i.trim_end_matches('/')) != Some(self.config.issuer.as_str()) || !audience_ok {
            return Err("kimlik belirteci bu kurulum için kesilmemiş".to_string());
        }
        if claims["exp"].as_u64().is_none_or(|exp| exp <= now) {
            return Err("kimlik belirtecinin süresi dolmuş".to_string());
        }
        if claims["nonce"].as_str() != Some(nonce) {
            return Err("kimlik belirteci bu giriş denemesine ait değil".to_string());
        }
        if self.config.claim == "email" && claims["email_verified"] == false {
            return Err("kimlik sağlayıcı e-posta adresini doğrulamamış".to_string());
        }
        claims[self.config.claim.as_str()].as_str().filter(|s| !s.is_empty()).map(str::to_string).ok_or_else(|| format!("kimlik belirtecinde \"{}\" alanı yok", self.config.claim))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> Config {
        Config::parse("issuer = https://id.example.com/\nclient_id = redox\nclient_secret = cok-gizli\nredirect_url = https://redox.example.com/login/oidc/callback\n").unwrap()
    }

    #[test]
    fn config_is_validated() {
        let c = config();
        assert_eq!((c.issuer.as_str(), c.claim.as_str()), ("https://id.example.com", "email"));
        assert!(!format!("{c:?}").contains("gizli"));
        for (text, needle) in [
            ("client_id = a\nclient_secret = b\nredirect_url = https://r.example.com/login/oidc/callback", "\"issuer\" eksik"),
            ("issuer = http://id.example.com\nclient_id = a\nclient_secret = b\nredirect_url = https://r.example.com/login/oidc/callback", "https:// ile başlamalı"),
            ("issuer = http://127.0.0.1.evil.example\nclient_id = a\nclient_secret = b\nredirect_url = https://r.example.com/login/oidc/callback", "https:// ile başlamalı"),
            ("issuer = https://id.example.com\nclient_id = a\nclient_secret = b\nredirect_url = https://r.example.com/baska", "/login/oidc/callback olmalı"),
            ("issuer = https://id.example.com\nclient_id = a\nredirect_url = https://r.example.com/login/oidc/callback", "\"client_secret\" eksik"),
            ("issuer = https://id.example.com\nclient_id = a\nclient_secret = b\nredirect_url = https://r.example.com/login/oidc/callback\nclaim = groups", "\"claim\""),
            ("issuer = https://id.example.com\nclient_id = a\nclient_secret = b\nredirect_url = https://r.example.com/login/oidc/callback\nscope = x", "bilinmeyen anahtar"),
        ] {
            let err = Config::parse(text).unwrap_err();
            assert!(err.contains(needle), "{text:?}: {err}");
        }
        assert!(Config::parse("issuer = http://127.0.0.1:9000\nclient_id = a\nclient_secret = b\nredirect_url = http://localhost:8006/login/oidc/callback").is_ok(), "yerel deneme");
    }

    #[test]
    fn base64url_round_trips() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"\xff\xfe\xfd\xfc", b"{\"sub\":\"1\"}"] {
            let text = base64url(data);
            assert!(!text.contains(['+', '/', '=']));
            assert_eq!(base64url_decode(&text).unwrap(), data);
        }
        assert_eq!(base64url_decode("a"), None);
        assert_eq!(base64url_decode("ab*d"), None);
    }

    #[test]
    fn claims_must_match_this_client_and_this_login() {
        let oidc = Oidc::new(config());
        let good = json!({"iss": "https://id.example.com", "aud": "redox", "exp": 2_000, "nonce": "n1", "email": "ali@example.com", "email_verified": true});
        assert_eq!(oidc.subject(&good, "n1", 1_000).unwrap(), "ali@example.com");
        let with = |patch: Value| {
            let mut v = good.clone();
            v.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
            v
        };
        assert!(oidc.subject(&with(json!({"aud": ["redox"]})), "n1", 1_000).is_ok());
        assert!(oidc.subject(&with(json!({"aud": ["redox", "baska"], "azp": "redox"})), "n1", 1_000).is_ok());
        for (claims, needle) in [
            (with(json!({"iss": "https://sahte.example.com"})), "bu kurulum için kesilmemiş"),
            (with(json!({"aud": "baska-uygulama"})), "bu kurulum için kesilmemiş"),
            (with(json!({"aud": ["redox", "baska"]})), "bu kurulum için kesilmemiş"),
            (with(json!({"aud": null})), "bu kurulum için kesilmemiş"),
            (with(json!({"exp": 1_000})), "süresi dolmuş"),
            (with(json!({"exp": null})), "süresi dolmuş"),
            (with(json!({"nonce": "baska"})), "bu giriş denemesine ait değil"),
            (with(json!({"email_verified": false})), "doğrulamamış"),
            (with(json!({"email": ""})), "alanı yok"),
            (with(json!({"email": null})), "alanı yok"),
        ] {
            let err = oidc.subject(&claims, "n1", 1_000).unwrap_err();
            assert!(err.contains(needle), "{claims}: {err}");
        }
    }

    #[test]
    fn callback_requires_the_browser_that_started_it() {
        let oidc = Oidc::new(config());
        lock(&oidc.pending).insert("s1".into(), Pending { nonce: "n".into(), verifier: "v".into(), created: 100 });
        assert!(oidc.finish("kod", "s1", None, 200).unwrap_err().contains("bu tarayıcıda başlatılmamış"));
        assert!(oidc.finish("kod", "s1", Some("baska"), 200).unwrap_err().contains("bu tarayıcıda başlatılmamış"));
        assert!(oidc.finish("kod", "", Some(""), 200).is_err());
        assert!(lock(&oidc.pending).contains_key("s1"), "başkasının denemesi bekleyen girişi tüketmez");
        assert!(oidc.finish("kod", "s1", Some("s1"), 100 + PENDING_SECS).unwrap_err().contains("süresi dolmuş"));
        assert!(oidc.finish("kod", "s1", Some("s1"), 200).unwrap_err().contains("süresi dolmuş"), "durum tek kullanımlıktır");
    }
}
