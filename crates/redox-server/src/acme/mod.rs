//! Yerleşik ACME istemcisi (RFC 8555): Let's Encrypt'ten, certbot gerekmeden, HTTP-01 sınamasıyla sertifika.
//!
//! `redox acme issue` sertifikayı `<dizin>/tls.crt` ve `tls.key` olarak yazar; `redox acme renew` bitişine 30
//! günden az kaldıysa yeniler. Yerleşik TLS dinleyicisi dosyalar değişince yenisini yeniden başlamadan kullanır.
//! Sağlayıcının kullanım koşulları ancak yönetici `--agree-tos` yazarsa kabul edilmiş sayılır.

pub mod challenge;
pub mod der;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P256_SHA256_FIXED_SIGNING};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::oidc::base64url;

pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";
/// Bu kadar günden az kalmışsa yenilenir.
pub const RENEW_DAYS: u64 = 30;
const WAIT: Duration = Duration::from_secs(120);
const MAX_REPLY: u64 = 256 * 1024;

/// Yenilemenin aynı ayarla yapılabilmesi için sertifikanın yanında saklanır.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub names: Vec<String>,
    pub directory: String,
    #[serde(default)]
    pub email: String,
    pub listen: String,
}

pub fn valid_domain(name: &str) -> bool {
    name.len() <= 253
        && name.contains('.')
        && name.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
        // IP adresi için bu yolla sertifika alınmaz.
        && !name.rsplit('.').next().unwrap_or_default().bytes().all(|b| b.is_ascii_digit())
}

/// Dizin adresi HTTPS olmalı; şifresiz yalnızca bu makinedeki deneme sunucusu.
fn secure(url: &str) -> bool {
    url.starts_with("https://") || ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| url.starts_with(p))
}

fn write_secret(path: &Path, content: &[u8]) -> Result<(), String> {
    crate::auth::write_private(path, content).map_err(|e| format!("{} yazılamadı: {e}", path.display()))
}

fn pem(label: &str, der: &[u8]) -> String {
    let text = crate::mail::base64(der);
    let lines: Vec<&str> = text.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// Hesap anahtarı: ilk kullanımda üretilir ve dizinde saklanır.
fn account_key(dir: &Path) -> Result<EcdsaKeyPair, String> {
    let path = dir.join("acme-account.key");
    let rng = SystemRandom::new();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let fresh = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).map_err(|_| "anahtar üretilemedi")?;
            write_secret(&path, fresh.as_ref())?;
            fresh.as_ref().to_vec()
        }
        Err(e) => return Err(format!("{} okunamadı: {e}", path.display())),
    };
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &bytes, &rng).map_err(|_| format!("{} bozuk", path.display()))
}

struct Client {
    agent: ureq::Agent,
    key: EcdsaKeyPair,
    rng: SystemRandom,
    directory: Value,
    nonce: Option<String>,
    /// Hesabın adresi; hesap açılana kadar `None`.
    account: Option<String>,
}

struct Reply {
    location: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|_| "ACME yanıtı anlaşılamadı".to_string())
    }
}

fn read_body(response: ureq::Response) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut body = Vec::new();
    response.into_reader().take(MAX_REPLY).read_to_end(&mut body).map_err(|e| format!("ACME yanıtı okunamadı: {e}"))?;
    Ok(body)
}

impl Client {
    fn new(directory_url: &str, key: EcdsaKeyPair) -> Result<Client, String> {
        if !secure(directory_url) {
            return Err("ACME dizin adresi https:// ile başlamalı".to_string());
        }
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).redirects(0).build();
        let reply = agent.get(directory_url).call().map_err(|e| format!("ACME dizinine erişilemedi: {}", describe(e)))?;
        let directory: Value = serde_json::from_slice(&read_body(reply)?).map_err(|_| "ACME dizini anlaşılamadı".to_string())?;
        Ok(Client { agent, key, rng: SystemRandom::new(), directory, nonce: None, account: None })
    }

    fn endpoint(&self, name: &str) -> Result<String, String> {
        url_of(&self.directory[name])
    }

    /// Anahtarın JWK gösterimi; alanlar özet için sözlük sırasındadır.
    fn jwk(&self) -> (String, Value) {
        let point = self.key.public_key().as_ref();
        let (x, y) = (base64url(&point[1..33]), base64url(&point[33..65]));
        (format!("{{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"{x}\",\"y\":\"{y}\"}}"), json!({ "crv": "P-256", "kty": "EC", "x": x, "y": y }))
    }

    pub fn thumbprint(&self) -> String {
        base64url(&Sha256::digest(self.jwk().0.as_bytes()))
    }

    fn fresh_nonce(&mut self) -> Result<String, String> {
        if let Some(nonce) = self.nonce.take() {
            return Ok(nonce);
        }
        let url = self.endpoint("newNonce")?;
        let reply = self.agent.head(&url).call().map_err(|e| format!("ACME sunucusu: {}", describe(e)))?;
        reply.header("replay-nonce").map(str::to_string).ok_or_else(|| "ACME sunucusu tek kullanımlık sayı vermedi".to_string())
    }

    /// İmzalı istek. `payload` yoksa "POST-as-GET".
    fn post(&mut self, url: &str, payload: Option<&Value>) -> Result<Reply, String> {
        if !secure(url) {
            return Err("ACME sunucusu şifresiz bir adres verdi".to_string());
        }
        for attempt in 0..3 {
            let nonce = self.fresh_nonce()?;
            let mut header = json!({ "alg": "ES256", "nonce": nonce, "url": url });
            match &self.account {
                Some(kid) => header["kid"] = json!(kid),
                None => header["jwk"] = self.jwk().1,
            }
            let protected = base64url(header.to_string().as_bytes());
            let body = payload.map(|p| base64url(p.to_string().as_bytes())).unwrap_or_default();
            let signature = self.key.sign(&self.rng, format!("{protected}.{body}").as_bytes()).map_err(|_| "ACME isteği imzalanamadı")?;
            let jws = json!({ "protected": protected, "payload": body, "signature": base64url(signature.as_ref()) });
            let sent = self.agent.post(url).set("Content-Type", "application/jose+json").send_string(&jws.to_string());
            match sent {
                Ok(response) => {
                    self.nonce = response.header("replay-nonce").map(str::to_string);
                    let location = response.header("location").map(str::to_string);
                    return Ok(Reply { location, body: read_body(response)? });
                }
                Err(ureq::Error::Status(code, response)) => {
                    self.nonce = response.header("replay-nonce").map(str::to_string);
                    let problem: Value = read_body(response).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
                    // Sayı eskimişse sunucu yenisini verir; istek yinelenir.
                    if problem["type"].as_str().is_some_and(|t| t.ends_with(":badNonce")) && attempt < 2 {
                        continue;
                    }
                    return Err(format!("ACME sunucusu isteği reddetti ({code}): {}", problem["detail"].as_str().unwrap_or("ayrıntı yok")));
                }
                Err(e) => return Err(format!("ACME sunucusuna erişilemedi: {}", describe(e))),
            }
        }
        Err("ACME sunucusu tek kullanımlık sayıyı kabul etmiyor".to_string())
    }

    /// Nesnenin durumu `wanted` listesinden biri olana kadar yoklar.
    fn wait(&mut self, url: &str, wanted: &[&str]) -> Result<Value, String> {
        let deadline = Instant::now() + WAIT;
        let mut pause = Duration::from_millis(200);
        loop {
            let value = self.post(url, None)?.json()?;
            let status = value["status"].as_str().unwrap_or_default();
            if wanted.contains(&status) {
                return Ok(value);
            }
            if status == "invalid" {
                let detail = value["challenges"].as_array().into_iter().flatten().find_map(|c| c["error"]["detail"].as_str()).or_else(|| value["error"]["detail"].as_str()).unwrap_or("ayrıntı yok");
                return Err(format!("doğrulama başarısız: {detail}"));
            }
            if Instant::now() >= deadline {
                return Err(format!("ACME sunucusu {} saniyede sonuç vermedi", WAIT.as_secs()));
            }
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_secs(5));
        }
    }
}

fn describe(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => format!("{code} yanıtı"),
        ureq::Error::Transport(t) => t.kind().to_string(),
    }
}

fn url_of(value: &Value) -> Result<String, String> {
    value.as_str().filter(|u| secure(u)).map(str::to_string).ok_or_else(|| "ACME yanıtında beklenen adres yok".to_string())
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 256 && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Sertifika alır ve `dir` içine yazar. Başarısızlıkta var olan sertifikaya dokunulmaz.
pub fn issue(settings: &Settings, dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    if settings.names.is_empty() || settings.names.len() > 20 {
        return Err("bir ile yirmi arasında alan adı verilmeli".to_string());
    }
    if let Some(bad) = settings.names.iter().find(|n| !valid_domain(n)) {
        return Err(format!("geçersiz alan adı: {bad} (IP adresi ve joker ad desteklenmez)"));
    }
    if !settings.email.is_empty() && (!settings.email.contains('@') || settings.email.chars().any(|c| c.is_whitespace() || c.is_control())) {
        return Err("geçersiz e-posta adresi".to_string());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    let responder = challenge::Responder::start(&settings.listen)?;
    let mut client = Client::new(&settings.directory, account_key(dir)?)?;

    let mut registration = json!({ "termsOfServiceAgreed": true });
    if !settings.email.is_empty() {
        registration["contact"] = json!([format!("mailto:{}", settings.email)]);
    }
    let url = client.endpoint("newAccount")?;
    client.account = Some(client.post(&url, Some(&registration))?.location.ok_or("ACME sunucusu hesap adresi vermedi")?);

    let identifiers: Vec<Value> = settings.names.iter().map(|n| json!({ "type": "dns", "value": n })).collect();
    let url = client.endpoint("newOrder")?;
    let reply = client.post(&url, Some(&json!({ "identifiers": identifiers })))?;
    let order_url = reply.location.clone().filter(|u| secure(u)).ok_or("ACME sunucusu sipariş adresi vermedi")?;
    let order = reply.json()?;

    for authorization in order["authorizations"].as_array().ok_or("siparişte yetkilendirme yok")? {
        let url = url_of(authorization)?;
        let detail = client.post(&url, None)?.json()?;
        if detail["status"] == "valid" {
            continue;
        }
        let http = detail["challenges"].as_array().into_iter().flatten().find(|c| c["type"] == "http-01").ok_or("sunucu http-01 sınaması sunmuyor")?;
        let token = http["token"].as_str().filter(|t| valid_token(t)).ok_or("sınama belirteci geçersiz")?;
        responder.publish(token, format!("{token}.{}", client.thumbprint()));
        client.post(&url_of(&http["url"])?, Some(&json!({})))?;
        client.wait(&url, &["valid"])?;
    }

    // Sertifikanın anahtarı her seferinde yenidir.
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).map_err(|_| "anahtar üretilemedi")?;
    let certificate_key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).map_err(|_| "anahtar kurulamadı")?;
    let csr = der::request(certificate_key.public_key().as_ref(), &settings.names, |info| certificate_key.sign(&rng, info).map(|s| s.as_ref().to_vec()).map_err(|_| "istek imzalanamadı".to_string()))?;
    client.wait(&order_url, &["ready", "valid"])?;
    client.post(&url_of(&order["finalize"])?, Some(&json!({ "csr": base64url(&csr) })))?;
    let done = client.wait(&order_url, &["valid"])?;
    let chain = client.post(&url_of(&done["certificate"])?, None)?.body;
    let chain = String::from_utf8(chain).map_err(|_| "sertifika anlaşılamadı".to_string())?;

    // Yeni dosyalar önce yanda kurulup denetlenir: anahtarla eşleşmeyen sertifika eskisinin yerine geçemez.
    let (new_cert, new_key) = (dir.join("tls.crt.yeni"), dir.join("tls.key.yeni"));
    write_secret(&new_cert, chain.as_bytes())?;
    write_secret(&new_key, pem("PRIVATE KEY", pkcs8.as_ref()).as_bytes())?;
    if let Err(e) = crate::tls::config(&new_cert, &new_key) {
        let _ = (std::fs::remove_file(&new_cert), std::fs::remove_file(&new_key));
        return Err(format!("alınan sertifika kullanılamıyor: {e}"));
    }
    let (cert, key) = (dir.join("tls.crt"), dir.join("tls.key"));
    std::fs::rename(&new_key, &key).and_then(|()| std::fs::rename(&new_cert, &cert)).map_err(|e| format!("sertifika yerine konamadı: {e}"))?;
    write_secret(&dir.join("acme.json"), &serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?)?;
    Ok((cert, key))
}

/// Sertifikanın bitişine kalan gün.
pub fn days_left(cert: &Path, now: u64) -> Result<u64, String> {
    let first = CertificateDer::pem_file_iter(cert).ok().and_then(|mut i| i.next()).and_then(Result::ok).ok_or_else(|| format!("{} okunamadı", cert.display()))?;
    let end = der::not_after(&first).ok_or("sertifikanın bitiş tarihi okunamadı")?;
    Ok(end.saturating_sub(now) / 86_400)
}

/// Bitişine `RENEW_DAYS` günden az kaldıysa yeniler. `Ok(None)`: henüz gerek yok.
pub fn renew(dir: &Path, now: u64) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let path = dir.join("acme.json");
    let settings: Settings = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or_else(|| format!("{} okunamadı; önce redox acme issue çalıştırın", path.display()))?;
    if days_left(&dir.join("tls.crt"), now)? >= RENEW_DAYS {
        return Ok(None);
    }
    issue(&settings, dir).map(Some)
}

const USAGE: &str = "kullanım: redox acme issue --domain <ad>[,<ad>...] --dir <dizin> --agree-tos [--email <adres>] [--staging | --directory <url>] [--listen <adres:kapı>]\n          redox acme renew --dir <dizin>\n          redox acme status --dir <dizin>";

/// `redox acme …` komutu.
pub fn command(args: &[&str]) -> Result<String, String> {
    let (action, rest) = args.split_first().ok_or(USAGE)?;
    let (mut names, mut dir, mut email, mut agreed) = (Vec::new(), None, String::new(), false);
    let (mut directory, mut listen) = (LETS_ENCRYPT.to_string(), "0.0.0.0:80".to_string());
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().map(|v| v.to_string()).ok_or_else(|| format!("{flag} bir değer ister"));
        match *flag {
            "--domain" => names = value()?.split(',').map(|n| n.trim().to_ascii_lowercase()).filter(|n| !n.is_empty()).collect(),
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--email" => email = value()?,
            "--directory" => directory = value()?,
            "--staging" => directory = LETS_ENCRYPT_STAGING.to_string(),
            "--listen" => listen = value()?,
            "--agree-tos" => agreed = true,
            _ => return Err(USAGE.to_string()),
        }
    }
    let dir = dir.ok_or(USAGE)?;
    let now = crate::state::now();
    let written = |(cert, key): (PathBuf, PathBuf)| format!("sertifika yazıldı: {} {}", cert.display(), key.display());
    match *action {
        "issue" => {
            if !agreed {
                return Err("sertifika sağlayıcısının kullanım koşullarını kabul etmek için --agree-tos gerekli (Let's Encrypt: https://letsencrypt.org/repository/)".to_string());
            }
            issue(&Settings { names, directory, email, listen }, &dir).map(written)
        }
        "renew" => renew(&dir, now).map(|done| done.map(written).unwrap_or_else(|| "yenileme henüz gerekmiyor".to_string())),
        "status" => days_left(&dir.join("tls.crt"), now).map(|days| format!("sertifikanın bitmesine {days} gün var")),
        _ => Err(USAGE.to_string()),
    }
}
