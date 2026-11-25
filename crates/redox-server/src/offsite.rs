//! Yedeklerin sunucu dışına kopyalanması: bağlı bir dizine ya da S3 uyumlu bir depoya.
//!
//! İsteğe bağlı şifreleme istemci tarafındadır (ChaCha20-Poly1305, parçalı): hedef yalnızca
//! şifreli veriyi görür ve kesilmiş ya da değiştirilmiş dosya geri yüklenirken reddedilir.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Eski biçim: içerik doğrulanır ama dosyanın adına bağlı değildir.
const MAGIC: &[u8; 8] = b"RDXENC1\n";
/// Şimdiki biçim: her parça, dosyanın hedefteki adıyla birlikte doğrulanır. Hedefe yazabilen biri iki yedeğin
/// dosyalarını yer değiştirirse (ya da birini öbürünün adıyla kopyalarsa) çözme başarısız olur.
const MAGIC_BOUND: &[u8; 8] = b"RDXENC2\n";
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
const HEADER: usize = 8 + 16 + 7;
/// Tek istekle yüklenebilen en büyük S3 nesnesi.
/// S3'te bu boyutu aşan dosyalar parçalı yüklenir (tek istekle yükleme sınırı 5 GB'tır).
const S3_PART: u64 = 64 * 1024 * 1024;
/// S3 bir yüklemede en çok bu kadar parça kabul eder.
const S3_MAX_PARTS: u64 = 10_000;

#[derive(Clone, PartialEq)]
pub enum Target {
    /// Bağlı bir dizin (ikinci disk, NFS, SMB).
    Dir(PathBuf),
    S3 { endpoint: String, host: String, bucket: String, region: String, prefix: String, access_key: String, secret_key: String },
}

#[derive(Clone, PartialEq)]
pub struct Config {
    pub target: Target,
    /// Doluysa yedekler gönderilmeden önce bu parolayla şifrelenir.
    passphrase: Option<String>,
    /// S3'te parçalı yüklemenin parça boyutu; bundan büyük dosyalar parçalara bölünür.
    part_bytes: u64,
    /// Kopya hedefinde saklama süresi (gün): yedek yerelde silinse de alındığı andan başlayarak bu kadar
    /// gün hedefte durur (arşiv). 0: yerelde silinen yedek hedeften de silinir.
    pub keep_days: u32,
}

/// Gizli anahtar ve parola hata ayıklama çıktısına yazılmaz.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Config({})", self.describe())
    }
}

impl Config {
    /// Yapılandırma dosyası: satır başına `anahtar = değer`; `#` ile başlayan satırlar atlanır.
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut map = std::collections::HashMap::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| format!("dış yedek dosyası, satır {}: \"anahtar = değer\" bekleniyordu", i + 1))?;
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
        let mut take = |key: &str| map.remove(key).filter(|v| !v.is_empty());
        let need = |value: Option<String>, key: &str| value.ok_or_else(|| format!("dış yedek dosyası: \"{key}\" eksik"));
        let passphrase = take("passphrase");
        if passphrase.as_ref().is_some_and(|p| p.len() < 12) {
            return Err("dış yedek dosyası: şifreleme parolası en az 12 karakter olmalı".to_string());
        }
        let keep_days = match take("keep_days") {
            Some(days) => days.parse::<u32>().ok().filter(|d| *d <= 36_500).ok_or_else(|| format!("dış yedek dosyası: \"keep_days\" 0 ile 36500 arasında bir gün sayısı olmalı: {days}"))?,
            None => 0,
        };
        let kind = need(take("kind"), "kind")?;
        let target = match kind.as_str() {
            "dir" => {
                let path = PathBuf::from(need(take("path"), "path")?);
                if !path.is_absolute() {
                    return Err("dış yedek dosyası: \"path\" tam yol olmalı".to_string());
                }
                Target::Dir(path)
            }
            "s3" => {
                let endpoint = crate::peers::normalize_url(&need(take("endpoint"), "endpoint")?).map_err(|e| format!("dış yedek dosyası: {e}"))?;
                let host = endpoint.split_once("://").map(|(_, h)| h.to_string()).unwrap_or_default();
                let bucket = need(take("bucket"), "bucket")?;
                if !bucket.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-') || bucket.len() < 3 {
                    return Err(format!("dış yedek dosyası: geçersiz kova adı \"{bucket}\""));
                }
                let prefix = take("prefix").unwrap_or_default();
                if !prefix.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b)) || prefix.starts_with('/') || prefix.contains("..") {
                    return Err(format!("dış yedek dosyası: geçersiz önek \"{prefix}\" (harf, rakam, nokta, tire, eğik çizgi)"));
                }
                let region = take("region").unwrap_or_else(|| "us-east-1".to_string());
                let (access_key, secret_key) = (need(take("access_key"), "access_key")?, need(take("secret_key"), "secret_key")?);
                if ![&region, &access_key, &secret_key].iter().all(|v| v.bytes().all(|b| b.is_ascii_graphic())) {
                    return Err("dış yedek dosyası: bölge ve anahtarlar boşluk içeremez".to_string());
                }
                Target::S3 { endpoint, host, bucket, region, prefix, access_key, secret_key }
            }
            other => return Err(format!("dış yedek dosyası: bilinmeyen tür \"{other}\"; \"dir\" ya da \"s3\" olmalı")),
        };
        if let Some(unknown) = map.keys().next() {
            return Err(format!("dış yedek dosyası: bilinmeyen anahtar \"{unknown}\""));
        }
        Ok(Config { target, passphrase, part_bytes: S3_PART, keep_days })
    }

    /// Bu hedef, `created` anında alınmış yedeği yerelde silinse de hâlâ saklıyor mu?
    pub fn keeps(&self, created: u64, now: u64) -> bool {
        self.keep_days > 0 && created + u64::from(self.keep_days) * 86_400 > now
    }

    pub fn encrypted(&self) -> bool {
        self.passphrase.is_some()
    }

    /// Arayüzde gösterilen kısa tanım (gizli bilgi içermez).
    pub fn describe(&self) -> String {
        let place = match &self.target {
            Target::Dir(path) => path.display().to_string(),
            Target::S3 { bucket, prefix, .. } => format!("s3://{bucket}/{prefix}"),
        };
        format!("{place}{}", if self.encrypted() { " (şifreli)" } else { "" })
    }

    /// Yerel dosyanın hedefteki adı: dosya adının kendisi.
    fn name_of(local: &Path) -> Result<&str, String> {
        local.file_name().and_then(|n| n.to_str()).ok_or_else(|| "yedek dosyasının adı okunamadı".to_string())
    }

    /// Hedefte saklanan ad: şifreliyse `.enc` eklenir. Ad en çok üç bileşenlidir (`chunks/ab/<özet>`);
    /// her bileşen yalnızca güvenli karakterler içerir, böylece hedef dizinin dışına çıkılamaz.
    fn stored_name(&self, name: &str) -> Result<String, String> {
        let parts: Vec<&str> = name.split('/').collect();
        let safe = |part: &&str| !part.is_empty() && !part.starts_with('.') && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if parts.len() > 3 || !parts.iter().all(safe) {
            return Err(format!("geçersiz yedek dosyası adı: {name}"));
        }
        Ok(if self.encrypted() { format!("{name}.enc") } else { name.to_string() })
    }

    /// Yerel yedek dosyasını hedefe kopyalar ve boyutunu doğrular. Gönderilen bayt sayısını döndürür.
    pub fn push(&self, local: &Path) -> Result<u64, String> {
        self.put_as(Self::name_of(local)?, local)
    }

    /// `local` dosyasını hedefe `name` adıyla kopyalar.
    pub fn put_as(&self, name: &str, local: &Path) -> Result<u64, String> {
        let bound_name = name;
        let name = self.stored_name(name)?;
        let file = std::fs::File::open(local).map_err(|e| format!("{} açılamadı: {e}", local.display()))?;
        let plain_len = file.metadata().map_err(|e| e.to_string())?.len();
        let (mut body, len): (Box<dyn Read + Send + Sync>, u64) = match &self.passphrase {
            Some(pass) => (Box::new(EncryptReader::new(file, pass, bound_name)?), encrypted_len(plain_len)),
            None => (Box::new(file), plain_len),
        };
        match &self.target {
            Target::Dir(dir) => {
                if !dir.is_dir() {
                    return Err(format!("hedef dizin yok ya da bağlı değil: {}", dir.display()));
                }
                // Önce geçici ada yaz: yarım kalan kopya geçerli bir yedek gibi görünmesin.
                let dest = dir.join(&name);
                let base = dest.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                let tmp = dest.with_file_name(format!(".{base}.tmp"));
                if let Some(parent) = dest.parent().filter(|parent| *parent != dir.as_path()) {
                    std::fs::create_dir_all(parent).map_err(|e| format!("hedefe yazılamadı: {e}"))?;
                }
                let copied = std::fs::File::create(&tmp)
                    .and_then(|mut out| {
                        let n = std::io::copy(&mut body, &mut out)?;
                        out.sync_all()?;
                        Ok(n)
                    })
                    .map_err(|e| format!("hedefe yazılamadı: {e}"));
                match copied {
                    Ok(n) if n == len => std::fs::rename(&tmp, &dest).map_err(|e| format!("hedefe yazılamadı: {e}"))?,
                    other => {
                        let _ = std::fs::remove_file(&tmp);
                        return Err(other.err().unwrap_or_else(|| "hedefe yazılan boyut beklenenden farklı".to_string()));
                    }
                }
            }
            Target::S3 { .. } => {
                if len > self.part_bytes {
                    self.s3_multipart(&name, &mut body, len)?;
                } else {
                    self.s3_query("PUT", &name, "", Some((&mut body, len)))?;
                }
                let stored = self.s3("HEAD", &name, None)?.header("content-length").and_then(|v| v.parse::<u64>().ok());
                if stored != Some(len) {
                    return Err(format!("hedefteki boyut doğrulanamadı (beklenen {len}, bulunan {stored:?})"));
                }
            }
        }
        Ok(len)
    }

    /// Hedefteki kopyayı yerel yedek dosyasına indirir (gerekiyorsa şifresini çözer).
    pub fn pull(&self, local: &Path) -> Result<(), String> {
        self.get_as(Self::name_of(local)?, local)
    }

    /// Hedefteki `name` dosyasını `local` yoluna indirir.
    pub fn get_as(&self, name: &str, local: &Path) -> Result<(), String> {
        self.get_checked(name, local, false)
    }

    /// `pull` gibi; `require_bound` ise ada bağlı olmayan (eski biçimli) şifreli kopya kabul edilmez.
    pub fn pull_bound(&self, local: &Path, require_bound: bool) -> Result<(), String> {
        self.get_checked(Self::name_of(local)?, local, require_bound)
    }

    fn get_checked(&self, name: &str, local: &Path, require_bound: bool) -> Result<(), String> {
        let bound_name = name;
        let name = self.stored_name(name)?;
        let mut source: Box<dyn Read> = match &self.target {
            Target::Dir(dir) => Box::new(std::fs::File::open(dir.join(&name)).map_err(|e| format!("dış kopya açılamadı ({name}): {e}"))?),
            Target::S3 { .. } => Box::new(self.s3("GET", &name, None)?.into_reader()),
        };
        let tmp = local.with_extension("indiriliyor");
        let written = std::fs::File::create(&tmp).map_err(|e| format!("{} oluşturulamadı: {e}", tmp.display())).and_then(|mut out| {
            match &self.passphrase {
                Some(pass) => decrypt(&mut source, pass, &mut out, bound_name, require_bound),
                None => std::io::copy(&mut source, &mut out).map(|_| ()).map_err(|e| format!("dış kopya okunamadı: {e}")),
            }?;
            out.sync_all().map_err(|e| e.to_string())
        });
        match written {
            Ok(()) => std::fs::rename(&tmp, local).map_err(|e| format!("{} yazılamadı: {e}", local.display())),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// Hedefteki kopyayı siler; zaten yoksa hata değildir.
    pub fn delete(&self, local: &Path) -> Result<(), String> {
        self.delete_named(Self::name_of(local)?)
    }

    pub fn delete_named(&self, name: &str) -> Result<(), String> {
        let name = self.stored_name(name)?;
        match &self.target {
            Target::Dir(dir) => match std::fs::remove_file(dir.join(&name)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("dış kopya silinemedi: {e}")),
            },
            Target::S3 { .. } => match self.s3("DELETE", &name, None) {
                Ok(_) => Ok(()),
                Err(e) if e == NOT_FOUND => Ok(()),
                Err(e) => Err(e),
            },
        }
    }

    /// Hedefte `name` adlı dosya var mı? Hedefe erişilemiyorsa hata (yok sayılmaz).
    pub fn exists_named(&self, name: &str) -> Result<bool, String> {
        let name = self.stored_name(name)?;
        match &self.target {
            Target::Dir(dir) => {
                if !dir.is_dir() {
                    return Err(format!("hedef dizin yok ya da bağlı değil: {}", dir.display()));
                }
                Ok(dir.join(&name).is_file())
            }
            Target::S3 { .. } => match self.s3("HEAD", &name, None) {
                Ok(_) => Ok(true),
                Err(e) if e == NOT_FOUND => Ok(false),
                Err(e) => Err(e),
            },
        }
    }

    /// Büyük dosyayı parça parça yükler: başlat, parçaları sırayla gönder, tamamla. Parçalar bellekte
    /// tutulmaz, akıtılır. Herhangi bir adım başarısız olursa yükleme iptal edilir (hedefte yarım parça kalmaz).
    fn s3_multipart(&self, name: &str, body: &mut dyn Read, len: u64) -> Result<(), String> {
        // Parça sayısı sınırı aşılmasın diye çok büyük dosyalarda parça boyutu büyütülür.
        let part = self.part_bytes.max(len.div_ceil(S3_MAX_PARTS));
        let started = self.s3_query("POST", name, "uploads=", Some((&mut std::io::empty(), 0)))?.into_string().map_err(|e| format!("hedefin yanıtı okunamadı: {e}"))?;
        let upload = xml_value(&started, "UploadId").ok_or("hedef parçalı yüklemeyi başlatmadı (UploadId yok)")?;
        let id = uri_encode(&upload);
        let sent = (|| {
            let mut etags: Vec<String> = Vec::new();
            let mut left = len;
            while left > 0 {
                let size = left.min(part);
                let number = etags.len() + 1;
                let mut chunk = (&mut *body).take(size);
                let response = self.s3_query("PUT", name, &format!("partNumber={number}&uploadId={id}"), Some((&mut chunk, size))).map_err(|e| format!("{number}. parça gönderilemedi: {e}"))?;
                etags.push(response.header("etag").ok_or_else(|| format!("hedef {number}. parça için kimlik (ETag) döndürmedi"))?.to_string());
                left -= size;
            }
            let list: String = etags.iter().enumerate().map(|(i, tag)| format!("<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>", i + 1, xml_escape(tag))).collect();
            let complete = format!("<CompleteMultipartUpload>{list}</CompleteMultipartUpload>");
            let done = self.s3_query("POST", name, &format!("uploadId={id}"), Some((&mut complete.as_bytes(), complete.len() as u64)))?.into_string().map_err(|e| format!("hedefin yanıtı okunamadı: {e}"))?;
            // S3, tamamlama hatasını 200 yanıtının gövdesinde de bildirebilir.
            match xml_value(&done, "Code") {
                Some(code) if done.contains("<Error>") => Err(format!("hedef parçaları birleştiremedi: {code}")),
                _ => Ok(()),
            }
        })();
        if sent.is_err() {
            let _ = self.s3_query("DELETE", name, &format!("uploadId={id}"), None);
        }
        sent
    }

    fn s3(&self, method: &str, name: &str, body: Option<(Box<dyn Read + Send + Sync>, u64)>) -> Result<ureq::Response, String> {
        match body {
            Some((mut reader, len)) => self.s3_query(method, name, "", Some((&mut reader, len))),
            None => self.s3_query(method, name, "", None),
        }
    }

    /// `query`: kurallı sorgu dizgisi (ada göre sıralı, değerleri `uri_encode` ile kodlanmış); boş olabilir.
    fn s3_query(&self, method: &str, name: &str, query: &str, body: Option<(&mut dyn Read, u64)>) -> Result<ureq::Response, String> {
        let Target::S3 { endpoint, host, bucket, region, prefix, access_key, secret_key } = &self.target else {
            return Err("hedef S3 değil".to_string());
        };
        let path = format!("/{bucket}/{prefix}{name}");
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        const UNSIGNED: &str = "UNSIGNED-PAYLOAD";
        let headers = [("host", host.as_str()), ("x-amz-content-sha256", UNSIGNED), ("x-amz-date", amz_date.as_str())];
        let authorization = sigv4_query(method, &path, query, &headers, UNSIGNED, &amz_date, region, access_key, secret_key);
        let url = if query.is_empty() { format!("{endpoint}{path}") } else { format!("{endpoint}{path}?{query}") };
        // Büyük dosyalar için toplam süre sınırı yok; bağlantı ve okuma sınırları var.
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(120)).redirects(0).build();
        let request = agent
            .request(method, &url)
            .set("x-amz-content-sha256", UNSIGNED)
            .set("x-amz-date", &amz_date)
            .set("Authorization", &authorization);
        let sent = match body {
            Some((reader, len)) => request.set("Content-Length", &len.to_string()).send(reader),
            None => request.call(),
        };
        sent.map_err(|e| match e {
            ureq::Error::Status(404, _) => NOT_FOUND.to_string(),
            ureq::Error::Status(403, _) => "hedef erişimi reddetti (403): anahtarları ve kova iznini denetleyin".to_string(),
            ureq::Error::Status(code, _) => format!("hedef {code} yanıtı verdi"),
            ureq::Error::Transport(t) => format!("hedefe erişilemiyor: {}", t.kind()),
        })
    }
}

const NOT_FOUND: &str = "dış kopya hedefte yok (404)";

/// Artımlı yedeklerin parça parça eşitlenmesi için hedefe adla erişim.
impl redox_core::Remote for Config {
    fn exists(&self, name: &str) -> Result<bool, String> {
        self.exists_named(name)
    }
    fn put(&self, name: &str, local: &Path) -> Result<u64, String> {
        self.put_as(name, local)
    }
    fn get(&self, name: &str, local: &Path) -> Result<(), String> {
        self.get_as(name, local)
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.delete_named(name)
    }
}

/// AWS'nin URI kodlaması: harf, rakam ve `-_.~` dışındaki her bayt `%XX` olur.
pub fn uri_encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// XML yanıtındaki ilk `<etiket>değer</etiket>` değerini döndürür.
fn xml_value(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].replace("&quot;", "\"").replace("&amp;", "&"))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC her uzunlukta anahtar kabul eder");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// AWS Signature Version 4 `Authorization` başlığı. `headers` ada göre sıralı ve küçük harfli olmalıdır.
#[allow(clippy::too_many_arguments)]
pub fn sigv4(method: &str, path: &str, headers: &[(&str, &str)], payload_hash: &str, amz_date: &str, region: &str, access_key: &str, secret_key: &str) -> String {
    sigv4_query(method, path, "", headers, payload_hash, amz_date, region, access_key, secret_key)
}

/// Sorgu dizgili istekler için imza. `query` kurallı biçimde olmalıdır: ada göre sıralı `ad=değer` çiftleri,
/// ad ve değerler `uri_encode` ile kodlanmış, değersiz parametre `ad=` olarak.
#[allow(clippy::too_many_arguments)]
pub fn sigv4_query(method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload_hash: &str, amz_date: &str, region: &str, access_key: &str, secret_key: &str) -> String {
    let date = &amz_date[..8];
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let signed = headers.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
    let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\n{payload_hash}");
    let scope = format!("{date}/{region}/s3/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
    let key = ["s3", "aws4_request"].iter().fold(hmac(&hmac(format!("AWS4{secret_key}").as_bytes(), date), region), |k, part| hmac(&k, part));
    format!("AWS4-HMAC-SHA256 Credential={access_key}/{scope},SignedHeaders={signed},Signature={}", hex(&hmac(&key, &to_sign)))
}

/// Anahtar türetme (Argon2id) bilerek pahalıdır; artımlı yedekte binlerce küçük dosya gittiği için
/// aynı parola ve tuzla türetilen anahtar bellekte tutulur.
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<ChaCha20Poly1305, String> {
    type Entry = ([u8; 32], Vec<u8>, ChaCha20Poly1305);
    static CACHE: std::sync::Mutex<Vec<Entry>> = std::sync::Mutex::new(Vec::new());
    let owner: [u8; 32] = Sha256::digest(passphrase.as_bytes()).into();
    let cached = |cache: &Vec<Entry>| cache.iter().find(|(who, s, _)| *who == owner && s.as_slice() == salt).map(|(_, _, cipher)| cipher.clone());
    if let Some(cipher) = cached(&CACHE.lock().unwrap_or_else(|e| e.into_inner())) {
        return Ok(cipher);
    }
    let cipher = derive_key_uncached(passphrase, salt)?;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 16 {
        cache.remove(0);
    }
    cache.push((owner, salt.to_vec(), cipher.clone()));
    Ok(cipher)
}

fn derive_key_uncached(passphrase: &str, salt: &[u8]) -> Result<ChaCha20Poly1305, String> {
    let params = argon2::Params::new(19_456, 2, 1, Some(32)).map_err(|e| e.to_string())?;
    let mut key = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| format!("şifreleme anahtarı türetilemedi: {e}"))?;
    Ok(ChaCha20Poly1305::new(Key::from_slice(&key)))
}

/// Parça nonce'u: rastgele önek + parça sayacı + "son parça" baytı. Son parça işareti kesilmeyi,
/// sayaç parçaların yer değiştirmesini yakalar.
fn nonce(prefix: &[u8; 7], counter: u32, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..7].copy_from_slice(prefix);
    n[7..11].copy_from_slice(&counter.to_be_bytes());
    n[11] = last as u8;
    n
}

/// Şifreli dosyanın boyutu: başlık + veri + her parça için doğrulama etiketi.
pub fn encrypted_len(plain: u64) -> u64 {
    HEADER as u64 + plain + TAG as u64 * (plain / CHUNK as u64 + 1)
}

/// Kaynağı okurken şifreler; böylece şifreli kopya diske yazılmadan doğrudan hedefe akar.
pub struct EncryptReader<R: Read> {
    inner: R,
    cipher: ChaCha20Poly1305,
    prefix: [u8; 7],
    /// Dosyanın hedefteki adı: her parçanın doğrulamasına katılır.
    name: Vec<u8>,
    counter: u32,
    out: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> EncryptReader<R> {
    /// `name`: dosyanın hedefteki (şifreleme eki olmayan) adı.
    pub fn new(inner: R, passphrase: &str, name: &str) -> Result<Self, String> {
        // Tuz sunucu her başladığında bir kez üretilir (anahtar bir kez türetilsin diye); dosyaları
        // birbirinden ayıran, her dosya için yeniden üretilen 56 bitlik nonce önekidir.
        static SALT: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();
        let random = |buf: &mut [u8]| getrandom::getrandom(buf).map_err(|e| format!("rastgele sayı üretilemedi: {e}"));
        let salt = match SALT.get() {
            Some(salt) => *salt,
            None => {
                let mut fresh = [0u8; 16];
                random(&mut fresh)?;
                *SALT.get_or_init(|| fresh)
            }
        };
        let mut prefix = [0u8; 7];
        random(&mut prefix)?;
        let mut out = MAGIC_BOUND.to_vec();
        out.extend_from_slice(&salt);
        out.extend_from_slice(&prefix);
        Ok(EncryptReader { inner, cipher: derive_key(passphrase, &salt)?, prefix, name: name.as_bytes().to_vec(), counter: 0, out, pos: 0, done: false })
    }
}

impl<R: Read> Read for EncryptReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.out.len() {
            if self.done {
                return Ok(0);
            }
            let mut chunk = vec![0u8; CHUNK];
            let mut filled = 0;
            while filled < CHUNK {
                match self.inner.read(&mut chunk[filled..])? {
                    0 => break,
                    n => filled += n,
                }
            }
            // Tam dolu parça son değildir: ardından (boş da olsa) işaretli bir son parça gelir.
            let last = filled < CHUNK;
            let sealed = self
                .cipher
                .encrypt(Nonce::from_slice(&nonce(&self.prefix, self.counter, last)), Payload { msg: &chunk[..filled], aad: &self.name })
                .map_err(|_| std::io::Error::other("şifreleme başarısız"))?;
            self.counter = self.counter.checked_add(1).ok_or_else(|| std::io::Error::other("dosya şifrelemek için çok büyük"))?;
            self.done = last;
            self.out = sealed;
            self.pos = 0;
        }
        let n = buf.len().min(self.out.len() - self.pos);
        buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn read_full(source: &mut dyn Read, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match source.read(&mut buf[filled..]).map_err(|e| format!("dış kopya okunamadı: {e}"))? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Şifreli akışı çözer. Yanlış parola, değiştirilmiş ya da kesilmiş veri ve başka bir adla gönderilmiş dosya
/// hata verir. `name`: dosyanın hedefteki adı. `require_bound`: ada bağlı olmayan eski biçim kabul edilmez
/// (yoksa hedefe yazabilen biri, yeni bir kopyanın yerine eski biçimli başka bir yedeği koyabilirdi).
pub fn decrypt(source: &mut dyn Read, passphrase: &str, out: &mut dyn Write, name: &str, require_bound: bool) -> Result<(), String> {
    const BAD: &str = "dış kopya çözülemedi: parola yanlış, dosya bozulmuş ya da başka bir yedeğe ait";
    let mut header = [0u8; HEADER];
    let read = read_full(source, &mut header)?;
    let bound = read == HEADER && &header[..8] == MAGIC_BOUND;
    if !bound && (read != HEADER || &header[..8] != MAGIC) {
        return Err("dış kopya şifreli RedoX yedeği değil".to_string());
    }
    if !bound && require_bound {
        return Err("dış kopya bu yedeğin adına bağlı değil (eski biçim): hedefteki dosya başka bir yedekle değiştirilmiş olabilir".to_string());
    }
    let aad: &[u8] = if bound { name.as_bytes() } else { &[] };
    let cipher = derive_key(passphrase, &header[8..24])?;
    let prefix: [u8; 7] = header[24..].try_into().expect("7 bayt");
    let mut buf = vec![0u8; CHUNK + TAG];
    for counter in 0u32.. {
        let n = read_full(source, &mut buf)?;
        if n < TAG {
            return Err("dış kopya eksik (kesilmiş dosya)".to_string());
        }
        let last = n < CHUNK + TAG;
        let plain = cipher.decrypt(Nonce::from_slice(&nonce(&prefix, counter, last)), Payload { msg: &buf[..n], aad }).map_err(|_| BAD.to_string())?;
        out.write_all(&plain).map_err(|e| format!("yedek dosyası yazılamadı: {e}"))?;
        if last {
            return Ok(());
        }
    }
    Err(BAD.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_days_makes_a_copy_target_an_archive() {
        let base = format!("kind = dir
path = {}
", std::env::temp_dir().display());
        let cfg = Config::parse(&format!("{base}keep_days = 30
")).unwrap();
        assert_eq!(cfg.keep_days, 30);
        assert!(cfg.keeps(1_000_000, 1_000_000 + 29 * 86_400) && !cfg.keeps(1_000_000, 1_000_000 + 30 * 86_400));
        assert!(!Config::parse(&base).unwrap().keeps(1_000_000, 1_000_001), "süre yoksa saklanmaz");
        assert!(Config::parse(&format!("{base}keep_days = cok
")).unwrap_err().contains("keep_days"));
    }

    const PASS: &str = "dogru-at-pil-zimba";
    const NAME: &str = "vm-103-1.qcow2";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("redox-offsite-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("yerel")).unwrap();
        std::fs::create_dir_all(dir.join("uzak")).unwrap();
        dir
    }

    fn dir_config(dir: &Path, encrypted: bool) -> Config {
        let pass = if encrypted { format!("passphrase = {PASS}\n") } else { String::new() };
        Config::parse(&format!("kind = dir\npath = {}\n{pass}", dir.join("uzak").display())).unwrap()
    }

    fn encrypt_all(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        EncryptReader::new(data, PASS, NAME).unwrap().read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn parses_and_validates_the_config_file() {
        let s3 = Config::parse("# hedef\nkind = s3\nendpoint = https://s3.eu-central-1.amazonaws.com/\nbucket = yedek-kovam\nregion = eu-central-1\nprefix = redox/\naccess_key = AKIA123\nsecret_key = gizli/anahtar+1\npassphrase = cok-gizli-parola\n").unwrap();
        assert_eq!(s3.describe(), "s3://yedek-kovam/redox/ (şifreli)");
        let debug = format!("{s3:?}");
        assert!(!debug.contains("gizli") && !debug.contains("AKIA"), "{debug}");
        assert!(matches!(&s3.target, Target::S3 { host, .. } if host == "s3.eu-central-1.amazonaws.com"));

        let abs = std::env::temp_dir().display().to_string();
        for (text, needle) in [
            ("", "\"kind\" eksik"),
            ("kind = ftp", "bilinmeyen tür"),
            ("kind = dir", "\"path\" eksik"),
            ("kind = dir\npath = goreli/yol", "tam yol"),
            (format!("kind = dir\npath = {abs}\nfazla = 1").as_str(), "bilinmeyen anahtar"),
            ("kind dir", "satır 1"),
            (format!("kind = dir\npath = {abs}\npassphrase = kisa").as_str(), "en az 12 karakter"),
            ("kind = s3\nendpoint = http://s3.example.com\nbucket = abc\naccess_key = a\nsecret_key = b", "şifresiz http"),
            ("kind = s3\nendpoint = https://s3.example.com\nbucket = Kova_1\naccess_key = a\nsecret_key = b", "geçersiz kova"),
            ("kind = s3\nendpoint = https://s3.example.com\nbucket = abc\nprefix = ../x/\naccess_key = a\nsecret_key = b", "geçersiz önek"),
            ("kind = s3\nendpoint = https://s3.example.com\nbucket = abc\naccess_key = a", "\"secret_key\" eksik"),
        ] {
            let err = Config::parse(text).unwrap_err();
            assert!(err.contains(needle), "{text:?}: {err}");
        }
    }

    #[test]
    fn signature_matches_the_aws_documentation_example() {
        // AWS belgelerindeki "GET Object" örneği (Signature Version 4, S3).
        let auth = sigv4(
            "GET",
            "/test.txt",
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
                ("x-amz-date", "20130524T000000Z"),
            ],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "20130524T000000Z",
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn query_signature_matches_the_aws_documentation_examples() {
        const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let sign = |query: &str| {
            sigv4_query(
                "GET",
                "/",
                query,
                &[("host", "examplebucket.s3.amazonaws.com"), ("x-amz-content-sha256", EMPTY), ("x-amz-date", "20130524T000000Z")],
                EMPTY,
                "20130524T000000Z",
                "us-east-1",
                "AKIAIOSFODNN7EXAMPLE",
                "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            )
        };
        // "GET Bucket Lifecycle" (değersiz parametre) ve "Get Bucket (List Objects)" (iki parametre) örnekleri.
        assert!(sign("lifecycle=").ends_with("Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"), "{}", sign("lifecycle="));
        assert!(sign("max-keys=2&prefix=J").ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"), "{}", sign("max-keys=2&prefix=J"));
        assert_eq!(uri_encode("aZ09-_.~ /+="), "aZ09-_.~%20%2F%2B%3D");
        assert_eq!(xml_value("<R><UploadId>ab&amp;c</UploadId></R>", "UploadId").as_deref(), Some("ab&c"));
        assert_eq!(xml_value("<R></R>", "UploadId"), None);
        assert_eq!(xml_escape("\"e1\""), "&quot;e1&quot;");
    }

    #[test]
    fn encryption_round_trips_at_chunk_boundaries() {
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 2 * CHUNK + 777] {
            let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let sealed = encrypt_all(&data);
            assert_eq!(sealed.len() as u64, encrypted_len(len as u64), "{len}");
            assert!(len < 64 || !sealed.windows(32).any(|w| w == &data[..32]), "düz metin sızmaz");
            let mut plain = Vec::new();
            decrypt(&mut sealed.as_slice(), PASS, &mut plain, NAME, true).unwrap();
            assert_eq!(plain, data, "{len}");
        }
        assert_ne!(encrypt_all(b"ayni veri"), encrypt_all(b"ayni veri"), "her şifrelemede nonce yenilenir");
    }

    #[test]
    fn tampering_truncation_and_wrong_passphrase_are_rejected() {
        let data = vec![7u8; 2 * CHUNK + 100];
        let sealed = encrypt_all(&data);
        let try_decrypt = |bytes: &[u8], pass: &str| decrypt(&mut &bytes[..], pass, &mut Vec::new(), NAME, true);
        assert!(try_decrypt(&sealed, PASS).is_ok());
        assert!(try_decrypt(&sealed, "yanlis-parola-123").unwrap_err().contains("parola yanlış"));

        // Ada bağlılık: aynı dosya başka bir yedeğin adıyla (hedefte yer değiştirilmiş) çözülmez.
        let as_other = decrypt(&mut sealed.as_slice(), PASS, &mut Vec::new(), "vm-104-9.qcow2", true).unwrap_err();
        assert!(as_other.contains("başka bir yedeğe ait"), "{as_other}");
        assert!(decrypt(&mut sealed.as_slice(), PASS, &mut Vec::new(), "vm-103-1.disk1.qcow2", false).is_err(), "aynı yedeğin başka diski de sayılmaz");
        assert!(sealed.starts_with(MAGIC_BOUND));

        let mut flipped = sealed.clone();
        flipped[HEADER + 10] ^= 1;
        assert!(try_decrypt(&flipped, PASS).unwrap_err().contains("bozulmuş"));
        // Başlığı eski biçim gibi gösterme (bağlılığı atlatma denemesi): parçalar adla mühürlü olduğu için çözülmez.
        let mut downgraded = sealed.clone();
        downgraded[..8].copy_from_slice(MAGIC);
        assert!(decrypt(&mut downgraded.as_slice(), PASS, &mut Vec::new(), NAME, false).unwrap_err().contains("çözülemedi"));
        assert!(decrypt(&mut downgraded.as_slice(), PASS, &mut Vec::new(), NAME, true).unwrap_err().contains("adına bağlı değil"));

        // Parça sınırında kesme: son parça işareti olmadığı için fark edilir.
        let first_two = HEADER + 2 * (CHUNK + TAG);
        assert!(try_decrypt(&sealed[..first_two], PASS).is_err(), "son parça atılmış");
        assert!(try_decrypt(&sealed[..HEADER + CHUNK + TAG], PASS).is_err(), "son iki parça atılmış");
        assert!(try_decrypt(&sealed[..sealed.len() - 1], PASS).is_err());

        // Parçaların yeri değiştirilirse sayaç tutmaz.
        let mut swapped = sealed.clone();
        let (a, b) = (HEADER, HEADER + CHUNK + TAG);
        let first: Vec<u8> = swapped[a..b].to_vec();
        swapped.copy_within(b..b + CHUNK + TAG, a);
        swapped[b..b + CHUNK + TAG].copy_from_slice(&first);
        assert!(try_decrypt(&swapped, PASS).is_err());

        assert!(try_decrypt(b"qcow2 gibi duz bir dosya, sifreli degil..", PASS).unwrap_err().contains("şifreli RedoX yedeği değil"));
    }

    #[test]
    fn directory_target_push_pull_delete() {
        for encrypted in [false, true] {
            let dir = temp(if encrypted { "dir-enc" } else { "dir" });
            let cfg = dir_config(&dir, encrypted);
            let local = dir.join("yerel/vm-103-1.qcow2");
            let data = vec![42u8; CHUNK + 5];
            std::fs::write(&local, &data).unwrap();

            let remote = dir.join("uzak").join(if encrypted { "vm-103-1.qcow2.enc" } else { "vm-103-1.qcow2" });
            assert_eq!(cfg.push(&local).unwrap(), std::fs::metadata(&remote).unwrap().len());
            assert_eq!(std::fs::read(&remote).unwrap() == data, !encrypted, "şifreliyse hedefteki içerik farklıdır");
            assert_eq!(std::fs::read_dir(dir.join("uzak")).unwrap().count(), 1, "geçici dosya kalmaz");

            std::fs::remove_file(&local).unwrap();
            cfg.pull(&local).unwrap();
            assert_eq!(std::fs::read(&local).unwrap(), data);

            cfg.delete(&local).unwrap();
            assert!(!remote.exists());
            cfg.delete(&local).unwrap();
            assert!(cfg.pull(&dir.join("yerel/yok.qcow2")).unwrap_err().contains("dış kopya açılamadı"));
            assert!(!dir.join("yerel/yok.indiriliyor").exists(), "başarısız indirme iz bırakmaz");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn missing_target_directory_and_bad_names_are_errors() {
        let dir = temp("eksik");
        let cfg = dir_config(&dir, false);
        std::fs::remove_dir_all(dir.join("uzak")).unwrap();
        let local = dir.join("yerel/vm-1-1.qcow2");
        std::fs::write(&local, "x").unwrap();
        assert!(cfg.push(&local).unwrap_err().contains("bağlı değil"));
        let odd = dir.join("yerel/garip ad.qcow2");
        std::fs::write(&odd, "x").unwrap();
        assert!(cfg.push(&odd).unwrap_err().contains("geçersiz yedek dosyası adı"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Bellekte nesne tutan asgari S3 taklidi: PUT/GET/HEAD/DELETE, imza başlığı zorunlu.
    type Store = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>;

    fn fake_s3() -> (u16, Store) {
        use std::io::{BufRead, BufReader};
        let store = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<String, Vec<u8>>::new()));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let objects = store.clone();
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let mut reader = BufReader::new(socket);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let (path, query) = target.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((target.clone(), String::new()));
                let param = |name: &str| query.split('&').find_map(|pair| pair.strip_prefix(&format!("{name}=")).map(str::to_string));
                let (mut length, mut signed) = (0usize, false);
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim().to_ascii_lowercase();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(v) = header.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    signed |= header.starts_with("authorization: aws4-hmac-sha256 credential=test-key/") && header.contains("signedheaders=host;x-amz-content-sha256;x-amz-date");
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let mut map = objects.lock().unwrap();
                // Parçalı yükleme: parçalar `yol#yükleme#numara` adıyla bekler, tamamlanınca birleştirilir.
                let mut etag = None;
                let (status, payload): (&str, Vec<u8>) = match (signed, method.as_str()) {
                    (false, _) => ("403 Forbidden", Vec::new()),
                    (_, "POST") if query == "uploads=" => ("200 OK", b"<InitiateMultipartUploadResult><UploadId>yukleme/1+a=</UploadId></InitiateMultipartUploadResult>".to_vec()),
                    (_, "PUT") if param("partNumber").is_some() => {
                        let number = param("partNumber").unwrap();
                        if param("uploadId").as_deref() != Some("yukleme%2F1%2Ba%3D") {
                            ("404 Not Found", Vec::new())
                        } else if path.contains("parca-hatasi") && number == "2" {
                            ("500 Internal Server Error", Vec::new())
                        } else {
                            etag = Some(format!("\"e{number}\""));
                            map.insert(format!("{path}#yukleme#{number}"), body);
                            ("200 OK", Vec::new())
                        }
                    }
                    (_, "POST") if param("uploadId").is_some() => {
                        let listed = String::from_utf8_lossy(&body).to_string();
                        let numbers: Vec<String> = listed.split("<PartNumber>").skip(1).filter_map(|p| p.split('<').next().map(str::to_string)).collect();
                        let tags_ok = numbers.iter().all(|n| listed.contains(&format!("<ETag>&quot;e{n}&quot;</ETag>")));
                        let mut whole = Vec::new();
                        let all = numbers.iter().all(|n| map.remove(&format!("{path}#yukleme#{n}")).map(|part| whole.extend(part)).is_some());
                        if all && tags_ok && !numbers.is_empty() && !path.contains("birlesme-hatasi") {
                            map.insert(path, whole);
                            ("200 OK", b"<CompleteMultipartUploadResult><ETag>\"tamam\"</ETag></CompleteMultipartUploadResult>".to_vec())
                        } else {
                            ("200 OK", b"<Error><Code>InvalidPart</Code></Error>".to_vec())
                        }
                    }
                    (_, "DELETE") if param("uploadId").is_some() => {
                        map.retain(|key, _| !key.starts_with(&format!("{path}#yukleme#")));
                        ("204 No Content", Vec::new())
                    }
                    (_, "PUT") => {
                        map.insert(path, body);
                        ("200 OK", Vec::new())
                    }
                    (_, "GET" | "HEAD") => match map.get(&path) {
                        Some(data) => ("200 OK", data.clone()),
                        None => ("404 Not Found", Vec::new()),
                    },
                    (_, "DELETE") => {
                        map.remove(&path);
                        ("204 No Content", Vec::new())
                    }
                    _ => ("405 Method Not Allowed", Vec::new()),
                };
                let mut socket = reader.into_inner();
                let etag = etag.map(|tag| format!("ETag: {tag}\r\n")).unwrap_or_default();
                let _ = write!(socket, "HTTP/1.1 {status}\r\n{etag}Content-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
                if method != "HEAD" {
                    let _ = socket.write_all(&payload);
                }
            }
        });
        (port, store)
    }

    #[test]
    fn s3_target_push_pull_delete_against_a_fake_server() {
        let (port, store) = fake_s3();
        let dir = temp("s3");
        let cfg = Config::parse(&format!(
            "kind = s3\nendpoint = http://127.0.0.1:{port}\nbucket = yedekler\nprefix = redox/\naccess_key = test-key\nsecret_key = test-secret\npassphrase = {PASS}\n"
        ))
        .unwrap();
        let local = dir.join("yerel/ct-200-5.tar.gz");
        let data = vec![9u8; 3000];
        std::fs::write(&local, &data).unwrap();

        assert_eq!(cfg.push(&local).unwrap(), encrypted_len(3000));
        let stored = store.lock().unwrap().get("/yedekler/redox/ct-200-5.tar.gz.enc").cloned().expect("nesne önekli yolda durur");
        assert!(stored.starts_with(MAGIC_BOUND) && !stored.windows(64).any(|w| w == &data[..64]));

        std::fs::remove_file(&local).unwrap();
        cfg.pull(&local).unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), data);

        cfg.delete(&local).unwrap();
        assert!(store.lock().unwrap().is_empty());
        assert!(cfg.pull(&local).unwrap_err().contains("hedefte yok (404)"));
        assert_eq!(std::fs::read(&local).unwrap(), data, "başarısız indirme yerel dosyayı bozmaz");

        // Kapalı kapı: açık bir hata, takılma yok.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let down = Config::parse(&format!("kind = s3\nendpoint = http://127.0.0.1:{closed}\nbucket = yedekler\naccess_key = a\nsecret_key = b\n")).unwrap();
        assert!(down.push(&local).unwrap_err().contains("erişilemiyor"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn large_files_are_uploaded_to_s3_in_parts() {
        let (port, store) = fake_s3();
        let dir = temp("s3-parca");
        let mut cfg = Config::parse(&format!(
            "kind = s3\nendpoint = http://127.0.0.1:{port}\nbucket = yedekler\nprefix = redox/\naccess_key = test-key\nsecret_key = test-secret\npassphrase = {PASS}\n"
        ))
        .unwrap();
        assert_eq!(cfg.part_bytes, 64 * 1024 * 1024, "varsayılan parça 64 MiB");
        assert_eq!(cfg.keep_days, 0);
        cfg.part_bytes = 1000;
        let local = dir.join("yerel/vm-103-7.qcow2");
        let data: Vec<u8> = (0..2500u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, &data).unwrap();

        // 2500 bayt şifrelenince 2547 bayt: 1000 + 1000 + 547 olarak üç parça.
        let sent = cfg.push(&local).unwrap();
        assert_eq!(sent, encrypted_len(2500));
        {
            let map = store.lock().unwrap();
            assert_eq!(map.keys().collect::<Vec<_>>(), ["/yedekler/redox/vm-103-7.qcow2.enc"], "parçalar birleşti, artık parça kalmadı");
            assert_eq!(map["/yedekler/redox/vm-103-7.qcow2.enc"].len() as u64, sent);
        }
        std::fs::remove_file(&local).unwrap();
        cfg.pull(&local).unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), data, "parçalı yüklenen dosya bayt bayt geri gelir");
        // Sınırdaki dosya tek istekle gider (parçalı yükleme yalnızca sınır aşılınca).
        cfg.part_bytes = sent;
        cfg.delete(&local).unwrap();
        cfg.push(&local).unwrap();
        assert_eq!(store.lock().unwrap().len(), 1);

        // Bir parça gönderilemezse ya da birleştirme reddedilirse yükleme iptal edilir: hedefte yarım parça kalmaz.
        cfg.part_bytes = 1000;
        for (name, needle) in [("parca-hatasi.qcow2", "2. parça gönderilemedi"), ("birlesme-hatasi.qcow2", "birleştiremedi: InvalidPart")] {
            let bad = dir.join("yerel").join(name);
            std::fs::write(&bad, &data).unwrap();
            let err = cfg.push(&bad).unwrap_err();
            assert!(err.contains(needle), "{name}: {err}");
            let map = store.lock().unwrap();
            assert!(map.keys().all(|key| !key.contains(name)), "{name}: {:?}", map.keys().collect::<Vec<_>>());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn named_files_live_in_subdirectories_and_cannot_escape_the_target() {
        use redox_core::Remote;
        for encrypted in [false, true] {
            let dir = temp(if encrypted { "ad-enc" } else { "ad" });
            let cfg = dir_config(&dir, encrypted);
            let local = dir.join("yerel/parca");
            std::fs::write(&local, b"parca verisi").unwrap();
            let name = "chunks/ab/abcdef0123";
            assert!(!cfg.exists(name).unwrap());
            cfg.put(name, &local).unwrap();
            assert!(cfg.exists(name).unwrap());
            let stored = dir.join("uzak/chunks/ab").join(if encrypted { "abcdef0123.enc" } else { "abcdef0123" });
            assert!(stored.is_file());
            assert_eq!(std::fs::read(&stored).unwrap() == b"parca verisi", !encrypted);
            assert_eq!(std::fs::read_dir(dir.join("uzak/chunks/ab")).unwrap().count(), 1, "geçici dosya kalmaz");
            let back = dir.join("yerel/.geri.tmp");
            cfg.get(name, &back).unwrap();
            assert_eq!(std::fs::read(&back).unwrap(), b"parca verisi");
            Remote::delete(&cfg, name).unwrap();
            assert!(!cfg.exists(name).unwrap());
            Remote::delete(&cfg, name).unwrap();

            for bad in ["../x", "chunks/../x", "/etc/passwd", "a//b", "a/b/c/d", ".gizli", "chunks/.gizli", "bos luk", ""] {
                assert!(cfg.put(bad, &local).unwrap_err().contains("geçersiz yedek dosyası adı"), "{bad}");
                assert!(cfg.exists(bad).is_err() && cfg.get(bad, &back).is_err() && Remote::delete(&cfg, bad).is_err(), "{bad}");
            }
            // Hedef bağlı değilse "yok" denmez: hata verilir (yoksa bütün parçalar yeniden gönderilirdi).
            std::fs::remove_dir_all(dir.join("uzak")).unwrap();
            assert!(cfg.exists(name).unwrap_err().contains("bağlı değil"));
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn key_is_derived_once_per_run_but_every_file_gets_its_own_nonce() {
        let (a, b) = (encrypt_all(b"bir"), encrypt_all(b"iki"));
        assert_eq!(a[8..24], b[8..24], "tuz çalışma boyunca aynıdır (anahtar bir kez türetilir)");
        assert_ne!(a[24..31], b[24..31], "nonce öneki her dosyada yenilenir");
        // Başka bir tuzla (önceki çalışmadan kalma dosya) şifrelenmiş veri de çözülür.
        let mut old = MAGIC.to_vec();
        let (salt, prefix) = ([5u8; 16], [9u8; 7]);
        old.extend_from_slice(&salt);
        old.extend_from_slice(&prefix);
        let cipher = derive_key_uncached(PASS, &salt).unwrap();
        old.extend(cipher.encrypt(Nonce::from_slice(&nonce(&prefix, 0, true)), &b"eski dosya"[..]).unwrap());
        let mut plain = Vec::new();
        decrypt(&mut old.as_slice(), PASS, &mut plain, "herhangi-bir-ad.qcow2", false).unwrap();
        assert_eq!(plain, b"eski dosya", "eski biçimli kopyalar okunmaya devam eder");
        assert!(decrypt(&mut old.as_slice(), "yanlis-parola-123", &mut Vec::new(), NAME, false).is_err(), "önbellek parolayı karıştırmaz");
        // Adına bağlı gönderildiği bilinen bir yedeğin yerine konmuş eski biçimli dosya kabul edilmez.
        assert!(decrypt(&mut old.as_slice(), PASS, &mut Vec::new(), NAME, true).unwrap_err().contains("adına bağlı değil"));
    }

    #[test]
    fn s3_target_reports_presence_and_tolerates_deleting_missing_objects() {
        use redox_core::Remote;
        let (port, store) = fake_s3();
        let dir = temp("s3-ad");
        let cfg = Config::parse(&format!("kind = s3\nendpoint = http://127.0.0.1:{port}\nbucket = yedekler\nprefix = redox/\naccess_key = test-key\nsecret_key = test-secret\n")).unwrap();
        let local = dir.join("yerel/parca");
        std::fs::write(&local, b"parca").unwrap();
        let name = "chunks/ab/abcdef0123";
        assert!(!cfg.exists(name).unwrap());
        cfg.put(name, &local).unwrap();
        assert!(cfg.exists(name).unwrap());
        assert_eq!(store.lock().unwrap().get("/yedekler/redox/chunks/ab/abcdef0123").map(Vec::as_slice), Some(&b"parca"[..]));
        let back = dir.join("yerel/geri");
        cfg.get(name, &back).unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), b"parca");
        Remote::delete(&cfg, name).unwrap();
        assert!(!cfg.exists(name).unwrap());
        // Erişilemeyen hedef "yok" sayılmaz.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let down = Config::parse(&format!("kind = s3\nendpoint = http://127.0.0.1:{closed}\nbucket = yedekler\naccess_key = a\nsecret_key = b\n")).unwrap();
        assert!(down.exists(name).unwrap_err().contains("erişilemiyor"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn swapped_remote_copies_are_rejected_on_pull() {
        let dir = temp("bagli");
        let cfg = dir_config(&dir, true);
        let (a, b) = (dir.join("vm-103-1.qcow2"), dir.join("vm-104-2.qcow2"));
        std::fs::write(&a, b"yuz uc numarali konugun diski").unwrap();
        std::fs::write(&b, b"yuz dort numarali konugun diski").unwrap();
        cfg.push(&a).unwrap();
        cfg.push(&b).unwrap();
        // Hedefe yazabilen biri iki dosyanın yerini değiştirir.
        let remote = |name: &str| dir.join("uzak").join(format!("{name}.enc"));
        let hold = dir.join("uzak").join("gecici");
        std::fs::rename(remote("vm-103-1.qcow2"), &hold).unwrap();
        std::fs::rename(remote("vm-104-2.qcow2"), remote("vm-103-1.qcow2")).unwrap();
        std::fs::rename(&hold, remote("vm-104-2.qcow2")).unwrap();

        std::fs::remove_file(&a).unwrap();
        for require in [true, false] {
            let err = cfg.pull_bound(&a, require).unwrap_err();
            assert!(err.contains("başka bir yedeğe ait"), "{require}: {err}");
            assert!(!a.exists(), "çözülemeyen dosya yerel diske konmaz");
        }
        // Yerine konunca yeniden iner.
        std::fs::rename(remote("vm-103-1.qcow2"), &hold).unwrap();
        std::fs::rename(remote("vm-104-2.qcow2"), remote("vm-103-1.qcow2")).unwrap();
        std::fs::rename(&hold, remote("vm-104-2.qcow2")).unwrap();
        cfg.pull_bound(&a, true).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"yuz uc numarali konugun diski");
        // Şifresiz hedefte bağlama yoktur (doğrulama da yoktur); istek yok sayılır.
        let plain = dir_config(&dir.join("duz"), false);
        std::fs::create_dir_all(dir.join("duz").join("uzak")).unwrap();
        let c = dir.join("duz").join("vm-1-1.qcow2");
        std::fs::write(&c, b"duz").unwrap();
        plain.push(&c).unwrap();
        std::fs::remove_file(&c).unwrap();
        plain.pull_bound(&c, true).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
