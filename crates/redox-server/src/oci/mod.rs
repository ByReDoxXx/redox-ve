//! OCI (Docker) imajlarından konteyner: imajı kayıt sunucusundan çekme, katmanlarını güvenle açma ve
//! çalışma ayarını LXC'ye çevirme. Yalnızca `linux/amd64` ve herkese açık imajlar.

pub mod config;
pub mod registry;
pub mod unpack;

use std::path::{Path, PathBuf};

use redox_core::oci::ImageRef;
use serde_json::Value;

pub use config::ImageConfig;

const MAX_LAYERS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Zstd,
}

impl Compression {
    pub fn name(self) -> &'static str {
        match self {
            Compression::None => "tar",
            Compression::Gzip => "gzip",
            Compression::Zstd => "zstd",
        }
    }

    pub fn parse(name: &str) -> Option<Compression> {
        [Compression::None, Compression::Gzip, Compression::Zstd].into_iter().find(|c| c.name() == name)
    }

    fn from_media_type(media: &str) -> Result<Compression, String> {
        if media.ends_with("+gzip") || media.ends_with(".tar.gzip") {
            Ok(Compression::Gzip)
        } else if media.ends_with("+zstd") {
            Ok(Compression::Zstd)
        } else if media.ends_with(".tar") {
            Ok(Compression::None)
        } else {
            Err(format!("desteklenmeyen katman türü: {media}"))
        }
    }
}

#[derive(Debug)]
pub struct Pulled {
    /// İndirilmiş katmanlar, uygulanma sırasıyla.
    pub layers: Vec<(PathBuf, Compression)>,
    pub config: ImageConfig,
}

/// Çok platformlu dizinden `linux/amd64` olanın özeti.
fn pick_platform(index: &Value) -> Result<String, String> {
    index["manifests"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| m["platform"]["os"] == "linux" && m["platform"]["architecture"] == "amd64")
        .and_then(|m| m["digest"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "imajın linux/amd64 sürümü yok".to_string())
}

/// İmajı `cache` dizinine çeker. `limit`: sıkıştırılmış katmanların toplam üst sınırı.
pub fn pull(image: &ImageRef, cache: &Path, limit: u64) -> Result<Pulled, String> {
    let mut client = registry::Client::new(image);
    let mut manifest = client.manifest(&image.reference)?;
    if manifest["manifests"].is_array() {
        let digest = pick_platform(&manifest)?;
        manifest = client.manifest(&digest)?;
    }
    let digest_of = |v: &Value| v["digest"].as_str().map(str::to_string).ok_or_else(|| "imaj belgesi eksik".to_string());
    std::fs::create_dir_all(cache).map_err(|e| format!("{} oluşturulamadı: {e}", cache.display()))?;
    let config_path = cache.join("config.json");
    client.blob(&digest_of(&manifest["config"])?, &config_path, 4 * 1024 * 1024)?;
    let document: Value = std::fs::read(&config_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or("imaj ayarı anlaşılamadı")?;
    if document["architecture"].as_str().is_some_and(|a| a != "amd64") || document["os"].as_str().is_some_and(|o| o != "linux") {
        return Err("imaj linux/amd64 değil".to_string());
    }
    let config = ImageConfig::parse(&document)?;
    let listed = manifest["layers"].as_array().filter(|l| !l.is_empty() && l.len() <= MAX_LAYERS).ok_or("imajın katman listesi okunamadı")?;
    let mut remaining = limit;
    let mut layers = Vec::new();
    for (i, layer) in listed.iter().enumerate() {
        let compression = Compression::from_media_type(layer["mediaType"].as_str().unwrap_or_default())?;
        let path = cache.join(format!("katman-{i}"));
        remaining -= client.blob(&digest_of(layer)?, &path, remaining)?;
        layers.push((path, compression));
    }
    Ok(Pulled { layers, config })
}

/// `redox oci-unpack <kök> <sıkıştırma> <bayt-sınırı>`: katmanı standart girdiden açar ve kalan sınırı yazar.
/// Sunucu bunu konteynerin kimlik eşlemesi içinde (`lxc-usernsexec`) çalıştırır; sahiplikler doğru olur.
#[cfg(unix)]
pub fn unpack_command(root: &str, compression: &str, budget: &str) -> Result<(), String> {
    let root = Path::new(root);
    if !root.is_absolute() || !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        return Err(format!("kök dizin yok: {}", root.display()));
    }
    let mut budget: u64 = budget.parse().map_err(|_| "geçersiz sınır".to_string())?;
    let compression = Compression::parse(compression).ok_or("geçersiz sıkıştırma")?;
    let stdin = std::io::stdin();
    match compression {
        Compression::None => unpack::unpack(&mut stdin.lock(), root, &mut budget, true)?,
        Compression::Zstd => {
            let mut input = zstd::stream::read::Decoder::new(stdin.lock()).map_err(|e| format!("zstd açılamadı: {e}"))?;
            unpack::unpack(&mut input, root, &mut budget, true)?;
        }
        Compression::Gzip => {
            let mut child = std::process::Command::new("gzip").arg("-dc").stdin(std::process::Stdio::inherit()).stdout(std::process::Stdio::piped()).spawn().map_err(|e| format!("gzip çalıştırılamadı: {e}"))?;
            let mut output = child.stdout.take().expect("gzip çıktısı");
            let result = unpack::unpack(&mut output, root, &mut budget, true);
            drop(output);
            let status = child.wait().map_err(|e| format!("gzip beklenemedi: {e}"))?;
            result?;
            if !status.success() {
                return Err("katman gzip ile açılamadı".to_string());
            }
        }
    }
    println!("{budget}");
    Ok(())
}

#[cfg(not(unix))]
pub fn unpack_command(_root: &str, _compression: &str, _budget: &str) -> Result<(), String> {
    Err("katman açma yalnızca Linux'ta çalışır".to_string())
}
