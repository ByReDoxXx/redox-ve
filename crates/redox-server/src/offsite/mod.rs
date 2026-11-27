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

mod crypto;
mod sign;

pub use crypto::*;

/// Dosya başı, parolayla şifrelenmiş bir RedoX dosyasına mı ait?
pub fn is_encrypted(head: &[u8]) -> bool {
    head.starts_with(MAGIC_FILE_KEY) || head.starts_with(MAGIC_BOUND) || head.starts_with(MAGIC)
}
pub use sign::*;

/// Eski biçim: içerik doğrulanır ama dosyanın adına bağlı değildir.
const MAGIC: &[u8; 8] = b"RDXENC1\n";
/// Şimdiki biçim: her parça, dosyanın hedefteki adıyla birlikte doğrulanır. Hedefe yazabilen biri iki yedeğin
/// dosyalarını yer değiştirirse (ya da birini öbürünün adıyla kopyalarsa) çözme başarısız olur.
const MAGIC_BOUND: &[u8; 8] = b"RDXENC2\n";
/// Güncel biçim: ada bağlı ve her dosya kendi anahtarıyla şifreli. Önceki biçimde bütün dosyalar aynı anahtarı
/// kullanır, dosyaları yalnızca 56 bitlik rastgele bir önek ayırırdı; çok sayıda parça dosyası gönderildiğinde iki
/// dosyanın aynı öneki alma olasılığı (ve o iki dosyanın içeriğinin açığa çıkması) göz ardı edilemez olurdu.
const MAGIC_FILE_KEY: &[u8; 8] = b"RDXENC3\n";
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

#[cfg(test)]
mod tests;
