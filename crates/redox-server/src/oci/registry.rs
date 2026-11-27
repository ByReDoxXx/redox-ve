//! OCI dağıtım protokolü (Docker Registry HTTP API v2) istemcisi: yalnızca okuma, adsız erişim.
//!
//! Her indirilen parça, adresindeki SHA-256 özetiyle doğrulanır. Bağlantı HTTPS'tir; şifresiz HTTP yalnızca
//! aynı makinedeki kayıt sunucusuna (localhost) yapılır. Yönlendirmeler elle izlenir: yetki belirteci kayıt
//! sunucusundan başka bir adrese gönderilmez.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use redox_core::oci::{valid_digest, ImageRef};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MANIFEST_TYPES: &str = "application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json";
const MAX_DOCUMENT: u64 = 4 * 1024 * 1024;
const MAX_HOPS: usize = 6;

fn local(url: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"].iter().any(|p| url.strip_prefix(p).is_some_and(|rest| rest.starts_with([':', '/'])))
}

/// `şema://sunucu[:port]`.
fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    url[after..].find(['/', '?']).map_or(url, |i| &url[..after + i])
}

pub fn sha256_of(bytes: &[u8]) -> String {
    format!("sha256:{}", Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// `Bearer realm="…",service="…"` başlığının alanları.
pub fn challenge_fields(header: &str) -> Option<Vec<(String, String)>> {
    let mut rest = header.strip_prefix("Bearer ")?.trim();
    let mut out = Vec::new();
    while !rest.is_empty() {
        let (key, after) = rest.split_once("=\"")?;
        let (value, after) = after.split_once('"')?;
        out.push((key.trim().to_string(), value.to_string()));
        rest = after.trim_start_matches([',', ' ']);
    }
    Some(out)
}

pub struct Client {
    base: String,
    repository: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(image: &ImageRef) -> Client {
        let host = image.registry.split(':').next().unwrap_or_default();
        let scheme = if host == "localhost" || host == "127.0.0.1" { "http" } else { "https" };
        Client {
            base: format!("{scheme}://{}", image.registry),
            repository: image.repository.clone(),
            token: None,
            agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(120)).redirects(0).build(),
        }
    }

    /// Uzak adres yalnızca HTTPS olabilir; kayıt sunucusu bu makinedeyse yine bu makineye düz HTTP de olur.
    fn permitted(&self, url: &str) -> Result<(), String> {
        if url.starts_with("https://") || (local(url) && local(&self.base)) {
            Ok(())
        } else {
            Err(format!("kayıt sunucusu güvenli olmayan bir adrese yönlendirdi: {}", origin(url)))
        }
    }

    fn fetch_token(&mut self, challenge: &str) -> Result<String, String> {
        let fields = challenge_fields(challenge).ok_or("kayıt sunucusu desteklenmeyen bir kimlik doğrulama istiyor")?;
        let find = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        let realm = find("realm").ok_or("kayıt sunucusu belirteç adresi vermedi")?;
        self.permitted(realm)?;
        let scope = format!("repository:{}:pull", self.repository);
        let mut request = self.agent.get(realm).query("scope", &scope);
        if let Some(service) = find("service") {
            request = request.query("service", service);
        }
        let reply = request.call().map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("kayıt sunucusu belirteç vermedi ({code}); özel imajlar desteklenmiyor"),
            ureq::Error::Transport(t) => format!("belirteç sunucusuna erişilemedi: {}", t.kind()),
        })?;
        let body: Value = serde_json::from_slice(&read_limited(reply.into_reader(), MAX_DOCUMENT)?).map_err(|_| "belirteç yanıtı anlaşılamadı".to_string())?;
        body["token"].as_str().or_else(|| body["access_token"].as_str()).map(str::to_string).ok_or_else(|| "belirteç yanıtı boş".to_string())
    }

    fn get(&mut self, path: &str, accept: &str) -> Result<ureq::Response, String> {
        let mut url = format!("{}/v2/{}/{path}", self.base, self.repository);
        let mut asked = false;
        for _ in 0..MAX_HOPS {
            self.permitted(&url)?;
            let mut request = self.agent.get(&url).set("Accept", accept);
            let at_registry = origin(&url) == self.base;
            if let Some(token) = self.token.as_deref().filter(|_| at_registry) {
                request = request.set("Authorization", &format!("Bearer {token}"));
            }
            let reply = match request.call() {
                Ok(reply) => reply,
                Err(ureq::Error::Status(_, reply)) => reply,
                Err(ureq::Error::Transport(t)) => return Err(format!("kayıt sunucusuna erişilemedi ({}): {}", origin(&url), t.kind())),
            };
            match reply.status() {
                200 => return Ok(reply),
                301 | 302 | 303 | 307 | 308 => {
                    let location = reply.header("location").ok_or("yönlendirme adresi yok")?;
                    url = if location.starts_with('/') { format!("{}{location}", origin(&url)) } else { location.to_string() };
                }
                401 if at_registry && !asked => {
                    asked = true;
                    let challenge = reply.header("www-authenticate").ok_or("kayıt sunucusu yetki istedi (401)")?.to_string();
                    self.token = Some(self.fetch_token(&challenge)?);
                }
                401 | 403 => return Err("kayıt sunucusu erişimi reddetti: imaj yok ya da özel (özel imajlar desteklenmiyor)".to_string()),
                404 => return Err("imaj bulunamadı; adı ve etiketi denetleyin".to_string()),
                429 => return Err("kayıt sunucusu istek sınırına ulaşıldığını bildirdi (429); daha sonra yeniden deneyin".to_string()),
                code => return Err(format!("kayıt sunucusu {code} yanıtı verdi")),
            }
        }
        Err("çok fazla yönlendirme".to_string())
    }

    /// İmaj belgesi (dizin ya da manifest). `reference` özet ise içerik özetle doğrulanır.
    pub fn manifest(&mut self, reference: &str) -> Result<Value, String> {
        let reply = self.get(&format!("manifests/{reference}"), MANIFEST_TYPES)?;
        let bytes = read_limited(reply.into_reader(), MAX_DOCUMENT)?;
        if valid_digest(reference) && sha256_of(&bytes) != reference {
            return Err("imaj belgesi özetiyle uyuşmuyor".to_string());
        }
        serde_json::from_slice(&bytes).map_err(|_| "imaj belgesi anlaşılamadı".to_string())
    }

    /// Bir parçayı dosyaya indirir ve özetini doğrular. `limit`: kabul edilen en büyük boyut.
    pub fn blob(&mut self, digest: &str, dest: &Path, limit: u64) -> Result<u64, String> {
        if !valid_digest(digest) {
            return Err("imaj belgesinde geçersiz özet var".to_string());
        }
        let reply = self.get(&format!("blobs/{digest}"), "*/*")?;
        let mut file = std::fs::File::create(dest).map_err(|e| format!("{} oluşturulamadı: {e}", dest.display()))?;
        let mut input = reply.into_reader().take(limit.saturating_add(1));
        let (mut hasher, mut total, mut buffer) = (Sha256::new(), 0u64, vec![0u8; 256 * 1024]);
        loop {
            let n = input.read(&mut buffer).map_err(|e| format!("indirme yarıda kesildi: {e}"))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > limit {
                return Err("imaj konteynere ayrılan diske sığmıyor; disk boyutunu büyütün".to_string());
            }
            hasher.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(|e| format!("diske yazılamadı: {e}"))?;
        }
        let found = format!("sha256:{}", hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>());
        if found != digest {
            return Err("indirilen parça özetiyle uyuşmuyor".to_string());
        }
        Ok(total)
    }
}

fn read_limited(input: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    input.take(limit + 1).read_to_end(&mut bytes).map_err(|e| format!("yanıt okunamadı: {e}"))?;
    if bytes.len() as u64 > limit {
        return Err("kayıt sunucusunun yanıtı çok büyük".to_string());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_only_for_this_machine() {
        let remote = Client::new(&ImageRef::parse("ornek.net/a").unwrap());
        assert!(remote.permitted("https://cdn.ornek.net/x").is_ok());
        for bad in ["http://cdn.ornek.net/x", "http://127.0.0.1:80/x", "http://localhost/x", "file:///etc/passwd", "ftp://a/b"] {
            assert!(remote.permitted(bad).is_err(), "{bad}");
        }
        let here = Client::new(&ImageRef::parse("localhost:5000/a").unwrap());
        assert!(here.permitted("http://127.0.0.1:5001/x").is_ok() && here.permitted("http://localhost.ornek.net/x").is_err());
        assert_eq!(origin("https://a.net:5/v2/x?y"), "https://a.net:5");
        assert_eq!(origin("https://a.net"), "https://a.net");
    }

    #[test]
    fn bearer_challenges_are_parsed_including_commas_inside_values() {
        let fields = challenge_fields(r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:a/b:pull,push""#).unwrap();
        assert_eq!(fields[0], ("realm".into(), "https://auth.docker.io/token".into()));
        assert_eq!(fields[2].1, "repository:a/b:pull,push");
        assert_eq!(challenge_fields("Basic realm=\"x\""), None);
        assert_eq!(challenge_fields("Bearer realm=x"), None);
    }
}
