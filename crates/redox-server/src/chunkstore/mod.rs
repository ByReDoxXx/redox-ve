//! Artımlı yedek deposu.
//!
//! Disk imajı sabit boyutlu parçalara bölünür; her parça içeriğinin SHA-256 özetiyle
//! adlandırılır ve depoda yalnızca bir kez (sıkıştırılmış) durur. Bir yedek, parça
//! özetlerinin sıralı listesidir (dizin dosyası). Sonraki yedekler değişmeyen parçaları
//! yeniden yazmaz: yalnızca değişen kısım yer kaplar.
//!
//! Zincir yoktur: her dizin tek başına bütün diski tarif eder, bu yüzden herhangi bir
//! yedek silinince diğerleri bozulmaz. Silinen yedeğin, başka hiçbir dizinin anmadığı
//! parçaları çöp toplamada kaldırılır.
//!
//! Güvenlik kuralları:
//! - Parça ve dizin önce geçici adla yazılır, diske işlenir, sonra yeniden adlandırılır;
//!   yarım kalan yazma hiçbir zaman geçerli bir ad altında görünmez.
//! - Dizin en son yazılır: dizini olan yedeğin bütün parçaları depodadır.
//! - Okurken her parçanın özeti yeniden hesaplanır; bozuk veri sessizce geri yüklenmez.
//! - Çöp toplama, okunamayan tek bir dizin bile varsa hiçbir şey silmez ve yedekleme ya da
//!   geri yükleme sürerken çalışmaz.
//!
//! Dış hedefle eşitleme aynı düzeni korur: hedefte de parçalar `chunks/` altında bir kez durur,
//! her yedeğin dizini ayrıdır. Gönderirken yalnızca hedefte olmayan parçalar gider; dizin en son
//! gönderilir. İndirilen her parça yerine konmadan önce özetiyle doğrulanır. Hedefte silerken,
//! başka bir yedeğin kullandığı (ya da kullanıp kullanmadığı belirlenemeyen) parçalara dokunulmaz.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock, TryLockError};

use redox_core::Remote;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod remote;
mod seal;
mod upkeep;

pub use seal::marked as seal_marked;

/// Parça boyutu: 4 MiB.
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
const FORMAT: u32 = 1;
const INDEX_SUFFIX: &str = ".rdxi.json";
const CHUNKS_DIR: &str = "chunks";
const ZSTD_LEVEL: i32 = 3;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize)]
struct Index {
    format: u32,
    /// Dizinin ait olduğu yedek. Dosya başka bir adla (başka bir yedeğin yerine) konmuşsa fark edilir.
    #[serde(default)]
    backup: String,
    chunk_size: u64,
    disks: Vec<DiskIndex>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskIndex {
    /// Disk numarası: 0 asıl disk, diğerleri ek disk. (Tek diskli eski dizinlerde alan yoktur.)
    #[serde(default)]
    id: u8,
    /// Diskin ham boyutu (bayt).
    size: u64,
    /// Sırayla parça özetleri; boş dizge tümü sıfır olan (saklanmayan) parçadır.
    chunks: Vec<String>,
}

/// Bir yedeğin depoya etkisi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Diskin ham boyutu.
    pub size: u64,
    /// Bu yedeğin andığı parçaların depoda kapladığı yer (tek başına dursaydı kaplayacağı).
    pub stored: u64,
    /// Bu yedekle depoya yeni eklenen bayt.
    pub added: u64,
    /// Sıfır olmayan, birbirinden farklı parça sayısı.
    pub chunks: usize,
    pub new_chunks: usize,
}

/// Değişen blok takibiyle alınan yedekte bir disk: yalnızca `ranges` içindeki baytlar `raw` dosyasında
/// geçerlidir; geri kalanı temel yedekten gelir.
pub struct DeltaDisk<'a> {
    pub id: u8,
    /// Diskin ham boyutu; temel yedekteki boyutla aynı olmalıdır.
    pub size: u64,
    pub raw: &'a mut std::fs::File,
    /// Değişen bölgeler: (başlangıç, uzunluk).
    pub ranges: Vec<(u64, u64)>,
}

pub struct Store {
    dir: PathBuf,
    chunk_size: usize,
    /// Yedekleme, geri yükleme ve doğrulama paylaşımlı; çöp toplama tek başına tutar.
    gate: RwLock<()>,
    /// Dış hedefe gönderme ve hedeften silme sırayla yapılır: biri, diğerinin az önce
    /// "hedefte var" diye atladığı parçayı silemesin.
    sync: Mutex<()>,
    /// Yerel şifreleme anahtarı; varsa yeni parçalar diske şifreli yazılır.
    seal: Option<seal::Seal>,
}

/// Hedefteki parça adı (yereldeki düzenle aynı).
fn remote_chunk(hash: &str) -> String {
    format!("{CHUNKS_DIR}/{}/{hash}", &hash[..2])
}

fn remote_index(id: &str) -> String {
    format!("{id}{INDEX_SUFFIX}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Dizinin kendisini diske işler (yeniden adlandırmanın kalıcı olması için); desteklemeyen sistemde yok sayılır.
fn sync_dir(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

/// `path` dosyasını önce geçici adla yazıp diske işler, sonra yerine koyar.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("geçersiz yol")?;
    let name = path.file_name().and_then(|n| n.to_str()).ok_or("geçersiz dosya adı")?;
    let temp = dir.join(format!(".{name}.{}-{}.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("{} yazılamadı: {e}", path.display()));
    }
    Ok(())
}

impl Store {
    /// `dir`: yedek dizini. Dizinler `dir/<kimlik>.rdxi.json`, parçalar `dir/chunks/` altında durur.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Store::with_chunk_size(dir, CHUNK_SIZE)
    }

    pub fn with_chunk_size(dir: impl Into<PathBuf>, chunk_size: usize) -> Self {
        Store { dir: dir.into(), chunk_size: chunk_size.max(1), gate: RwLock::new(()), sync: Mutex::new(()), seal: None }
    }

    /// Depoyu parolayla şifreli kullanır: bundan sonra yazılan parçalar şifrelenir, şifreli parçalar okunabilir.
    /// Önceden şifresiz yazılmış parçalar olduğu gibi kalır ve okunmaya devam eder.
    pub fn sealed(mut self, passphrase: &str) -> Result<Self, String> {
        self.seal = Some(seal::Seal::open(&self.dir, passphrase)?);
        Ok(self)
    }

    pub fn is_sealed(&self) -> bool {
        self.seal.is_some()
    }

    fn index_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}{INDEX_SUFFIX}"))
    }

    fn chunk_path(&self, hash: &str) -> PathBuf {
        self.dir.join(CHUNKS_DIR).join(&hash[..2]).join(hash)
    }

    /// Bu kimlikle artımlı bir yedek var mı?
    pub fn has(&self, id: &str) -> bool {
        self.index_path(id).is_file()
    }

    /// Ham diskleri `id` adıyla depoya yazar. Her öge: (disk numarası, kaynak, bayt boyutu).
    /// Diskler birlikte tek yedektir: dizin, hepsi eksiksiz yazıldıktan sonra konur.
    pub fn write(&self, id: &str, disks: &mut [(u8, &mut dyn Read, u64)]) -> Result<Stats, String> {
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let index_path = self.index_path(id);
        if index_path.exists() {
            return Err(format!("{} zaten var", index_path.display()));
        }
        std::fs::create_dir_all(self.dir.join(CHUNKS_DIR)).map_err(|e| format!("yedek deposu oluşturulamadı: {e}"))?;
        let mut stats = Stats::default();
        let mut seen: HashSet<String> = HashSet::new();
        let mut buf = vec![0u8; self.chunk_size];
        let mut indexed = Vec::with_capacity(disks.len());
        // Yeni parça konan dizinler: dizin dosyası yazılmadan önce diske işlenir.
        let mut touched: HashSet<PathBuf> = HashSet::new();
        for (disk_id, source, size) in disks.iter_mut() {
            let size = *size;
            stats.size += size;
            let mut chunks = Vec::with_capacity(size.div_ceil(self.chunk_size as u64) as usize);
            let mut remaining = size;
            while remaining > 0 {
                let want = remaining.min(self.chunk_size as u64) as usize;
                let chunk = &mut buf[..want];
                source.read_exact(chunk).map_err(|e| format!("disk okunamadı (beklenenden kısa olabilir): {e}"))?;
                remaining -= want as u64;
                if chunk.iter().all(|b| *b == 0) {
                    chunks.push(String::new());
                    continue;
                }
                let hash = hex(&Sha256::digest(&*chunk));
                if seen.insert(hash.clone()) {
                    let (bytes, new) = self.put(&hash, chunk)?;
                    stats.chunks += 1;
                    stats.stored += bytes;
                    if new {
                        stats.new_chunks += 1;
                        stats.added += bytes;
                        touched.extend(self.chunk_path(&hash).parent().map(Path::to_path_buf));
                    }
                }
                chunks.push(hash);
            }
            // Kaynak bildirilen boyuttan uzunsa yedek eksik olurdu: kabul edilmez.
            let mut extra = [0u8; 1];
            match source.read(&mut extra) {
                Ok(0) => {}
                Ok(_) => return Err("disk bildirilen boyuttan uzun; yedek alınmadı".to_string()),
                Err(e) => return Err(format!("disk okunamadı: {e}")),
            }
            indexed.push(DiskIndex { id: *disk_id, size, chunks });
        }
        let index = Index { format: FORMAT, backup: id.to_string(), chunk_size: self.chunk_size as u64, disks: indexed };
        let text = serde_json::to_vec(&index).map_err(|e| format!("yedek dizini yazılamadı: {e}"))?;
        // Yazdığımız dizin kendi kurallarımızdan geçmeli (ör. aynı disk iki kez verilmişse reddedilir).
        Self::parse(&text).map_err(|e| format!("yedek dizini geçersiz: {e}"))?;
        // Parçaların yeniden adlandırılması kalıcı olmadan dizin yazılmaz: yoksa elektrik kesintisinden
        // sonra dizini duran ama parçası kaybolmuş bir yedek kalabilirdi.
        self.settle(&touched);
        // Dizin en son ve atomik yazılır: var olan dizinin bütün parçaları depodadır.
        write_atomic(&index_path, &text)?;
        sync_dir(&self.dir);
        Ok(stats)
    }

    /// `base` yedeğinin üzerine yalnızca değişen bölgeleri işleyerek `id` yedeğini yazar: değişmeyen parçalar
    /// temel yedeğin parçalarıdır (okunmadan, özetleriyle); değişen her parça temel içerik + yeni baytlardan
    /// yeniden kurulur. Sonuç, diskin tamamı okunmuş gibi eksiksiz ve kendi başına bir yedektir.
    pub fn write_delta(&self, id: &str, base: &str, disks: &mut [DeltaDisk<'_>]) -> Result<Stats, String> {
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let index_path = self.index_path(id);
        if index_path.exists() {
            return Err(format!("{} zaten var", index_path.display()));
        }
        let prior = self.load(base)?;
        if prior.chunk_size != self.chunk_size as u64 || prior.disks.len() != disks.len() {
            return Err("temel yedek bu konuğun şimdiki diskleriyle uyuşmuyor".to_string());
        }
        let cs = self.chunk_size as u64;
        let mut stats = Stats::default();
        let mut touched: HashSet<PathBuf> = HashSet::new();
        let mut indexed = Vec::with_capacity(disks.len());
        for disk in disks.iter_mut() {
            let old = Self::disk(&prior, disk.id)?;
            if old.size != disk.size {
                return Err(format!("disk boyutu temel yedekten beri değişmiş ({} → {} bayt)", old.size, disk.size));
            }
            stats.size += disk.size;
            let mut ranges: Vec<(u64, u64)> = disk.ranges.iter().copied().filter(|(_, len)| *len > 0).collect();
            ranges.sort_unstable();
            if ranges.iter().any(|(start, len)| start.checked_add(*len).is_none_or(|end| end > disk.size)) {
                return Err("değişen bölge diskin dışına taşıyor".to_string());
            }
            let mut dirty: Vec<u64> = ranges.iter().flat_map(|(start, len)| (start / cs)..=((start + len - 1) / cs)).collect();
            dirty.sort_unstable();
            dirty.dedup();
            let mut chunks = old.chunks.clone();
            for i in dirty {
                let begin = i * cs;
                let len = (disk.size - begin).min(cs) as usize;
                let mut buf = match chunks[i as usize].as_str() {
                    "" => vec![0u8; len],
                    hash => self.get(hash, len)?,
                };
                for (start, length) in &ranges {
                    let (from, to) = ((*start).max(begin), (start + length).min(begin + len as u64));
                    if from < to {
                        disk.raw.seek(SeekFrom::Start(from)).map_err(|e| format!("değişen bloklar okunamadı: {e}"))?;
                        disk.raw.read_exact(&mut buf[(from - begin) as usize..(to - begin) as usize]).map_err(|e| format!("değişen bloklar okunamadı: {e}"))?;
                    }
                }
                chunks[i as usize] = if buf.iter().all(|b| *b == 0) {
                    String::new()
                } else {
                    let hash = hex(&Sha256::digest(&buf));
                    let (bytes, new) = self.put(&hash, &buf)?;
                    if new {
                        stats.new_chunks += 1;
                        stats.added += bytes;
                        touched.extend(self.chunk_path(&hash).parent().map(Path::to_path_buf));
                    }
                    hash
                };
            }
            indexed.push(DiskIndex { id: disk.id, size: disk.size, chunks });
        }
        let index = Index { format: FORMAT, backup: id.to_string(), chunk_size: cs, disks: indexed };
        // Yedeğin kapladığı yer: andığı bütün (farklı) parçaların depodaki boyutu. Biri bile yoksa yedek eksiktir.
        for (hash, _) in Self::unique_chunks(&index) {
            let meta = std::fs::metadata(self.chunk_path(&hash)).map_err(|e| format!("parça eksik ({}): {e}", &hash[..12]))?;
            stats.chunks += 1;
            stats.stored += meta.len();
        }
        let text = serde_json::to_vec(&index).map_err(|e| format!("yedek dizini yazılamadı: {e}"))?;
        Self::parse(&text).map_err(|e| format!("yedek dizini geçersiz: {e}"))?;
        self.settle(&touched);
        write_atomic(&index_path, &text)?;
        sync_dir(&self.dir);
        Ok(stats)
    }

    /// Parça dizinlerini (ve yeni alt dizinler için üst dizini) diske işler.
    fn settle(&self, touched: &HashSet<PathBuf>) {
        for dir in touched {
            sync_dir(dir);
        }
        if !touched.is_empty() {
            sync_dir(&self.dir.join(CHUNKS_DIR));
        }
    }

    /// Parçayı depoya koyar; `(depodaki boyutu, yeni mi)` döndürür.
    fn put(&self, hash: &str, data: &[u8]) -> Result<(u64, bool), String> {
        let path = self.chunk_path(hash);
        if let Ok(meta) = std::fs::metadata(&path) {
            return Ok((meta.len(), false));
        }
        let mut packed = zstd::bulk::compress(data, ZSTD_LEVEL).map_err(|e| format!("parça sıkıştırılamadı: {e}"))?;
        if let Some(seal) = &self.seal {
            packed = seal.seal(&packed, hash.as_bytes())?;
        }
        let parent = path.parent().ok_or("geçersiz parça yolu")?;
        std::fs::create_dir_all(parent).map_err(|e| format!("{} oluşturulamadı: {e}", parent.display()))?;
        write_atomic(&path, &packed)?;
        Ok((packed.len() as u64, true))
    }

    /// Parçayı okur, açar ve özetini doğrular.
    fn get(&self, hash: &str, expected_len: usize) -> Result<Vec<u8>, String> {
        self.decode(&self.chunk_path(hash), hash, expected_len)
    }

    /// `path` dosyasındaki (şifreliyse çözülen) sıkıştırılmış parçayı açar; uzunluğu ve özeti tutmuyorsa hata verir.
    fn decode(&self, path: &Path, hash: &str, expected_len: usize) -> Result<Vec<u8>, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("parça eksik ({}): {e}", &hash[..12]))?;
        // Bozuk bir dosya belleği tüketemesin: sıkıştırılmış parça, ham boyutundan ancak biraz büyük olabilir.
        let mut packed = Vec::new();
        file.take(expected_len as u64 + expected_len as u64 / 8 + 4096).read_to_end(&mut packed).map_err(|e| format!("parça okunamadı ({}): {e}", &hash[..12]))?;
        if packed.starts_with(seal::MAGIC) {
            let seal = self.seal.as_ref().ok_or_else(|| format!("parça şifreli ({}): sunucuyu --backup-passphrase-file ile başlatın", &hash[..12]))?;
            packed = seal.unseal(&packed, hash.as_bytes()).map_err(|()| format!("parça bozuk ({}): şifresi çözülemedi", &hash[..12]))?;
        }
        let decoder = zstd::stream::read::Decoder::new(&packed[..]).map_err(|e| format!("parça açılamadı ({}): {e}", &hash[..12]))?;
        let mut data = Vec::with_capacity(expected_len);
        // Bozuk bir parça belleği tüketemesin: beklenenden bir bayt fazlası okunur, o kadar.
        decoder.take(expected_len as u64 + 1).read_to_end(&mut data).map_err(|e| format!("parça bozuk ({}): {e}", &hash[..12]))?;
        if data.len() != expected_len || hex(&Sha256::digest(&data)) != hash {
            return Err(format!("parça bozuk ({}): içerik özetiyle uyuşmuyor", &hash[..12]));
        }
        Ok(data)
    }

    fn load(&self, id: &str) -> Result<Index, String> {
        let path = self.index_path(id);
        let text = std::fs::read(&path).map_err(|e| format!("yedek dizini okunamadı ({}): {e}", path.display()))?;
        Self::parse(&text).and_then(|index| Self::owned_by(index, id)).map_err(|e| format!("yedek dizini geçersiz ({}): {e}", path.display()))
    }

    /// Dizin gerçekten `id` yedeğine mi ait? (Eski dizinlerde alan boştur ve denetlenemez.)
    fn owned_by(index: Index, id: &str) -> Result<Index, String> {
        if index.backup.is_empty() || index.backup == id {
            Ok(index)
        } else {
            Err(format!("başka bir yedeğe ait ({})", index.backup))
        }
    }

    fn parse(text: &[u8]) -> Result<Index, String> {
        let index: Index = serde_json::from_slice(text).map_err(|e| e.to_string())?;
        if index.format != FORMAT {
            return Err(format!("bilinmeyen biçim sürümü {}", index.format));
        }
        if index.chunk_size == 0 || index.chunk_size > (1 << 30) {
            return Err("geçersiz parça boyutu".to_string());
        }
        if index.disks.is_empty() {
            return Err("yedek dizini disk içermiyor".to_string());
        }
        let mut ids: Vec<u8> = index.disks.iter().map(|d| d.id).collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != index.disks.len() {
            return Err("yedek dizininde aynı disk iki kez geçiyor".to_string());
        }
        for disk in &index.disks {
            if disk.chunks.len() as u64 != disk.size.div_ceil(index.chunk_size) {
                return Err("parça sayısı disk boyutuyla uyuşmuyor".to_string());
            }
            // Özetler dosya yoluna dönüşür: yalnızca 64 onaltılık hane kabul edilir.
            if disk.chunks.iter().any(|h| !h.is_empty() && !valid_hash(h)) {
                return Err("geçersiz parça özeti".to_string());
            }
        }
        Ok(index)
    }

    fn disk(index: &Index, disk: u8) -> Result<&DiskIndex, String> {
        index.disks.iter().find(|d| d.id == disk).ok_or_else(|| match disk {
            0 => "yedekte asıl disk yok".to_string(),
            n => format!("yedekte ek disk {n} yok"),
        })
    }

    /// Yedekteki diskler: (numara, ham boyut).
    pub fn disks(&self, id: &str) -> Result<Vec<(u8, u64)>, String> {
        Ok(self.load(id)?.disks.iter().map(|d| (d.id, d.size)).collect())
    }

    /// Yedekteki `disk` numaralı diski ham olarak `out` dosyasına yazar (sıfır bölgeler boşluk bırakılır);
    /// disk boyutunu döndürür.
    pub fn read(&self, id: &str, disk: u8, out: &mut std::fs::File) -> Result<u64, String> {
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let index = self.load(id)?;
        let disk = Self::disk(&index, disk)?;
        let io = |e: std::io::Error| format!("geri yükleme dosyası yazılamadı: {e}");
        out.set_len(0).map_err(io)?;
        let mut offset = 0u64;
        for hash in &disk.chunks {
            let len = (disk.size - offset).min(index.chunk_size);
            if !hash.is_empty() {
                let data = self.get(hash, len as usize)?;
                out.seek(SeekFrom::Start(offset)).map_err(io)?;
                out.write_all(&data).map_err(io)?;
            }
            offset += len;
        }
        out.set_len(disk.size).map_err(io)?;
        out.sync_all().map_err(io)?;
        Ok(disk.size)
    }
}

#[cfg(test)]
mod tests;
