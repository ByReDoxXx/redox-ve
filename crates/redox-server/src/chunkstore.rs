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
        Store { dir: dir.into(), chunk_size: chunk_size.max(1), gate: RwLock::new(()), sync: Mutex::new(()) }
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
        let packed = zstd::bulk::compress(data, ZSTD_LEVEL).map_err(|e| format!("parça sıkıştırılamadı: {e}"))?;
        let parent = path.parent().ok_or("geçersiz parça yolu")?;
        std::fs::create_dir_all(parent).map_err(|e| format!("{} oluşturulamadı: {e}", parent.display()))?;
        write_atomic(&path, &packed)?;
        Ok((packed.len() as u64, true))
    }

    /// Parçayı okur, açar ve özetini doğrular.
    fn get(&self, hash: &str, expected_len: usize) -> Result<Vec<u8>, String> {
        Self::decode(&self.chunk_path(hash), hash, expected_len)
    }

    /// `path` dosyasındaki sıkıştırılmış parçayı açar; uzunluğu ve özeti tutmuyorsa hata verir.
    fn decode(path: &Path, hash: &str, expected_len: usize) -> Result<Vec<u8>, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("parça eksik ({}): {e}", &hash[..12]))?;
        let decoder = zstd::stream::read::Decoder::new(file).map_err(|e| format!("parça açılamadı ({}): {e}", &hash[..12]))?;
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

    /// Yedeğin (bütün disklerinin) parçalarını okuyup özetlerini doğrular; doğrulanan parça sayısını döndürür.
    pub fn verify(&self, id: &str) -> Result<usize, String> {
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let index = self.load(id)?;
        let mut checked: HashSet<&str> = HashSet::new();
        for disk in &index.disks {
            let mut offset = 0u64;
            for hash in &disk.chunks {
                let len = (disk.size - offset).min(index.chunk_size);
                // Aynı özet farklı uzunlukta anılamaz; son (kısa) parça ayrıca denetlenir.
                if !hash.is_empty() && (checked.insert(hash) || len != index.chunk_size) {
                    self.get(hash, len as usize)?;
                }
                offset += len;
            }
        }
        Ok(checked.len())
    }

    /// Dizindeki birbirinden farklı (sıfır olmayan) parçalar, ilk geçtikleri yerdeki uzunluklarıyla.
    fn unique_chunks(index: &Index) -> Vec<(String, usize)> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut unique = Vec::new();
        for disk in &index.disks {
            let mut offset = 0u64;
            for hash in &disk.chunks {
                let len = (disk.size - offset).min(index.chunk_size);
                if !hash.is_empty() && seen.insert(hash) {
                    unique.push((hash.clone(), len as usize));
                }
                offset += len;
            }
        }
        unique
    }

    /// Yereldeki bütün dizinlerin andığı parçalar. Tek bir dizin bile okunamıyorsa hata:
    /// hangi parçaların gerekli olduğu bilinmeden hiçbir şey silinmemelidir.
    fn referenced(&self) -> Result<HashSet<String>, String> {
        let mut used: HashSet<String> = HashSet::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(used),
            Err(e) => return Err(format!("yedek dizini okunamadı: {e}")),
        };
        for entry in entries {
            let entry = entry.map_err(|e| format!("yedek dizini okunamadı: {e}"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') || !name.ends_with(INDEX_SUFFIX) {
                continue;
            }
            let text = std::fs::read(entry.path()).map_err(|e| format!("{name} okunamadı: {e}"))?;
            let index = Self::parse(&text).map_err(|e| format!("{name} geçersiz: {e}"))?;
            used.extend(index.disks.into_iter().flat_map(|d| d.chunks).filter(|h| !h.is_empty()));
        }
        Ok(used)
    }

    /// Hedefteki bir dizini indirip doğrular; andığı parçaları uzunluklarıyla döndürür.
    fn remote_chunks(&self, id: &str, remote: &dyn Remote) -> Result<(Vec<(String, usize)>, PathBuf), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("yedek dizini oluşturulamadı: {e}"))?;
        let staged = self.dir.join(format!(".{id}.{}-{}.rdxi.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
        let parsed = remote.get(&remote_index(id), &staged).and_then(|()| {
            let text = std::fs::read(&staged).map_err(|e| format!("indirilen dizin okunamadı: {e}"))?;
            let index = Self::parse(&text).and_then(|index| Self::owned_by(index, id)).map_err(|e| format!("dış hedefteki yedek dizini geçersiz ({id}): {e}"))?;
            Ok(Self::unique_chunks(&index))
        });
        match parsed {
            Ok(chunks) => Ok((chunks, staged)),
            Err(e) => {
                let _ = std::fs::remove_file(&staged);
                Err(e)
            }
        }
    }

    /// Yedeği dış hedefe gönderir: yalnızca hedefte olmayan parçalar gider, dizin en son gönderilir.
    /// `known`: dış kopyası olduğu bilinen diğer yedekler; onların parçaları hedefe sorulmadan atlanır.
    /// `(gönderilen parça sayısı, gönderilen bayt)` döndürür.
    pub fn push(&self, id: &str, known: &[String], remote: &dyn Remote) -> Result<(usize, u64), String> {
        let _sync = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let index = self.load(id)?;
        self.check_remote_store(remote)?;
        let mut there: HashSet<String> = HashSet::new();
        for other in known.iter().filter(|other| other.as_str() != id) {
            // Yerelde dizini olmayan (ya da okunamayan) yedek bilgi vermez; parçaları hedefe sorulur.
            // Kayıttaki "dış kopyası var" işaretine körü körüne güvenilmez: hedef değiştirilmiş ya da
            // boşaltılmış olabilir. Yedeğin dizini hedefte gerçekten duruyorsa parçaları da oradadır.
            if let Ok(index) = self.load(other) {
                if remote.exists(&remote_index(other))? {
                    there.extend(Self::unique_chunks(&index).into_iter().map(|(hash, _)| hash));
                }
            }
        }
        let (mut sent, mut bytes) = (0usize, 0u64);
        for (hash, _) in Self::unique_chunks(&index) {
            if there.contains(&hash) {
                continue;
            }
            let name = remote_chunk(&hash);
            if !remote.exists(&name)? {
                bytes += remote.put(&name, &self.chunk_path(&hash))?;
                sent += 1;
            }
        }
        // Dizin en son: hedefte dizini olan yedeğin bütün parçaları da hedeftedir.
        bytes += remote.put(&remote_index(id), &self.index_path(id))?;
        Ok((sent, bytes))
    }

    /// Hedefteki depo işareti: ilk göndermede konur, sonrakilerde okunur. Şifreleme parolası değişmişse
    /// okunamaz; böylece eski parolayla şifrelenmiş parçalar "hedefte var" sayılıp yeni yedeğin dış kopyası
    /// açılamaz halde bırakılmaz.
    fn check_remote_store(&self, remote: &dyn Remote) -> Result<(), String> {
        const MARKER: &str = "redox-store.json";
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("yedek dizini oluşturulamadı: {e}"))?;
        let probe = self.dir.join(format!(".store.{}-{}.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
        let checked = if remote.exists(MARKER)? {
            remote.get(MARKER, &probe).map_err(|e| {
                format!("dış hedefteki yedek deposu açılamıyor; şifreleme parolası değiştiyse eski parolayı geri yazın ya da boş bir hedef kullanın: {e}")
            })
        } else {
            std::fs::write(&probe, b"{\"redox\":1}\n").map_err(|e| format!("{} yazılamadı: {e}", probe.display())).and_then(|()| remote.put(MARKER, &probe).map(|_| ()))
        };
        let _ = std::fs::remove_file(&probe);
        checked
    }

    /// Dış hedefteki yedeği yerel depoya indirir: yerelde olmayan ya da bozulmuş parçalar gelir ve her biri
    /// yerine konmadan önce özetiyle doğrulanır. Hedefte bu yedeğin dizini yoksa `false`.
    pub fn pull(&self, id: &str, remote: &dyn Remote) -> Result<bool, String> {
        // Hedefte silme (`unpush`) ile aynı anda çalışmaz: yoksa silme, az sonra yerele inecek yedeğin
        // parçalarını "kimse kullanmıyor" sanabilirdi.
        let _sync = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        if !remote.exists(&remote_index(id))? {
            return Ok(false);
        }
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let (chunks, staged) = self.remote_chunks(id, remote)?;
        let fetched = (|| {
            let mut touched: HashSet<PathBuf> = HashSet::new();
            for (hash, len) in &chunks {
                let path = self.chunk_path(hash);
                // Yerelde duran parça da doğrulanır: bozulmuşsa hedefteki sağlam kopyayla değiştirilir.
                if path.is_file() && Self::decode(&path, hash, *len).is_ok() {
                    continue;
                }
                let parent = path.parent().ok_or("geçersiz parça yolu")?;
                std::fs::create_dir_all(parent).map_err(|e| format!("{} oluşturulamadı: {e}", parent.display()))?;
                let temp = parent.join(format!(".{hash}.{}-{}.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
                let checked = remote.get(&remote_chunk(hash), &temp).and_then(|()| Self::decode(&temp, hash, *len).map(|_| ()));
                let placed = checked.and_then(|()| std::fs::rename(&temp, &path).map_err(|e| format!("parça yerine konamadı: {e}")));
                if let Err(e) = placed {
                    let _ = std::fs::remove_file(&temp);
                    return Err(format!("dış kopya indirilemedi: {e}"));
                }
                touched.insert(parent.to_path_buf());
            }
            self.settle(&touched);
            // Dizin en son yerine konur: yarım kalan indirme geçerli bir yedek gibi görünmez.
            std::fs::rename(&staged, self.index_path(id)).map_err(|e| format!("yedek dizini yerine konamadı: {e}"))
        })();
        if let Err(e) = fetched {
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
        sync_dir(&self.dir);
        Ok(true)
    }

    /// Dış hedefteki yedeği siler: önce dizini, sonra yalnızca ona özgü parçaları. Korunan parçalar:
    /// yereldeki bütün dizinlerin ve `others` içinde yalnızca hedefte kalmış yedeklerin andıkları.
    /// Bunlar belirlenemezse hiçbir parça silinmez (hedefte artık parça kalır, veri kaybı olmaz).
    /// Hedefte bu yedeğin dizini yoksa `false`.
    pub fn unpush(&self, id: &str, others: &[String], remote: &dyn Remote) -> Result<bool, String> {
        let _sync = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        if !remote.exists(&remote_index(id))? {
            return Ok(false);
        }
        let (victims, staged) = self.remote_chunks(id, remote)?;
        let _ = std::fs::remove_file(&staged);
        remote.delete(&remote_index(id))?;
        let keep = (|| {
            let mut keep = self.referenced()?;
            for other in others.iter().filter(|other| other.as_str() != id && !self.has(other)) {
                // Tam (tek dosyalık) dış kopyaların dizini yoktur; onlar parça kullanmaz.
                if remote.exists(&remote_index(other))? {
                    let (chunks, staged) = self.remote_chunks(other, remote)?;
                    let _ = std::fs::remove_file(&staged);
                    keep.extend(chunks.into_iter().map(|(hash, _)| hash));
                }
            }
            Ok::<_, String>(keep)
        })()
        .map_err(|e| format!("dış kopyanın dizini silindi ama parçaları hedefte bırakıldı (hangilerinin gerekli olduğu belirlenemedi): {e}"))?;
        for (hash, _) in victims {
            if !keep.contains(&hash) {
                remote.delete(&remote_chunk(&hash))?;
            }
        }
        Ok(true)
    }

    /// Yedeğin dizinini siler ve artık anılmayan parçaları toplar. Yedek yoksa `false`.
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let path = self.index_path(id);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(format!("{} silinemedi: {e}", path.display())),
        }
        sync_dir(&self.dir);
        // Çöp toplanamazsa yedek yine silinmiştir; artık parçalar bir sonraki silmede gider.
        let _ = self.gc();
        Ok(true)
    }

    /// Hiçbir dizinin anmadığı parçaları siler; `(silinen dosya, bayt)` döndürür.
    /// Yedekleme ya da geri yükleme sürüyorsa hiçbir şey yapmaz (`None`).
    pub fn gc(&self) -> Result<Option<(usize, u64)>, String> {
        let _gate = match self.gate.try_write() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return Ok(None),
        };
        // Okunamayan tek bir dizin, hangi parçaların gerekli olduğunu belirsiz kılar: hiçbir şey silinmez.
        let used = self.referenced().map_err(|e| format!("çöp toplama atlandı: {e}"))?;
        let (mut files, mut bytes) = (0usize, 0u64);
        let Ok(groups) = std::fs::read_dir(self.dir.join(CHUNKS_DIR)) else {
            return Ok(Some((0, 0)));
        };
        for group in groups.flatten() {
            let Ok(chunks) = std::fs::read_dir(group.path()) else { continue };
            for chunk in chunks.flatten() {
                // Kilit bizdeyken yazan yoktur: geçici dosyalar da kalıntıdır.
                if chunk.file_name().to_str().is_some_and(|name| used.contains(name)) {
                    continue;
                }
                let len = chunk.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(chunk.path()).is_ok() {
                    files += 1;
                    bytes += len;
                }
            }
            let _ = std::fs::remove_dir(group.path()); // yalnızca boşsa silinir
        }
        Ok(Some((files, bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static DIRS: AtomicU64 = AtomicU64::new(0);

    /// 8 baytlık parçalarla geçici bir depo.
    fn store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rdx-chunks-{}-{}", std::process::id(), DIRS.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Store::with_chunk_size(&dir, 8), dir)
    }

    fn put(s: &Store, id: &str, data: &[u8]) -> Result<Stats, String> {
        s.write(id, &mut [(0, &mut &data[..], data.len() as u64)])
    }

    fn restored(s: &Store, dir: &Path, id: &str) -> Result<Vec<u8>, String> {
        let path = dir.join("geri.raw");
        let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        // Eski içerik kalmamalı: dosya önce çöple doldurulur.
        file.write_all(&[0xff; 100]).unwrap();
        s.read(id, 0, &mut file)?;
        drop(file);
        Ok(std::fs::read(&path).unwrap())
    }

    fn chunk_files(dir: &Path) -> Vec<PathBuf> {
        let mut all: Vec<PathBuf> = std::fs::read_dir(dir.join(CHUNKS_DIR))
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|group| std::fs::read_dir(group.path()).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>())
            .collect();
        all.sort();
        all
    }

    /// Dört parça: veri, sıfır, veri, kısa son parça (3 bayt).
    fn disk() -> Vec<u8> {
        let mut d = b"AAAAAAAA".to_vec();
        d.extend([0u8; 8]);
        d.extend(b"BBBBBBBB");
        d.extend(b"CCC");
        d
    }

    #[test]
    fn roundtrip_keeps_every_byte_and_skips_zero_chunks() {
        let (s, dir) = store();
        let data = disk();
        let stats = put(&s, "vm-1-1", &data).unwrap();
        assert_eq!((stats.size, stats.chunks, stats.new_chunks), (27, 3, 3), "sıfır parça saklanmaz");
        assert_eq!(stats.stored, stats.added);
        assert!(s.has("vm-1-1") && !s.has("vm-1-2"));
        assert_eq!(chunk_files(&dir).len(), 3);
        assert_eq!(restored(&s, &dir, "vm-1-1").unwrap(), data);
        assert_eq!(s.verify("vm-1-1").unwrap(), 3);
        assert!(put(&s, "vm-1-1", &data).unwrap_err().contains("zaten var"), "var olan yedeğin üzerine yazılmaz");

        // Tümü sıfır ve boş diskler de çalışır.
        assert_eq!(put(&s, "sifir", &[0u8; 20]).unwrap().chunks, 0);
        assert_eq!(restored(&s, &dir, "sifir").unwrap(), vec![0u8; 20]);
        put(&s, "bos", &[]).unwrap();
        assert_eq!(restored(&s, &dir, "bos").unwrap(), Vec::<u8>::new());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn several_disks_form_one_backup() {
        let (s, dir) = store();
        let (main, data) = (disk(), b"VERIVERIAAAAAAAAson".to_vec());
        let stats = s.write("a", &mut [(0, &mut &main[..], main.len() as u64), (2, &mut &data[..], data.len() as u64)]).unwrap();
        // İkinci diskteki "AAAAAAAA" parçası asıl diskte de var: bir kez saklanır.
        assert_eq!((stats.size, stats.chunks), (46, 5));
        assert_eq!(s.disks("a").unwrap(), [(0, 27), (2, 19)]);
        let read = |disk: u8| -> Result<Vec<u8>, String> {
            let path = dir.join("geri.raw");
            let mut file = std::fs::File::create(&path).unwrap();
            s.read("a", disk, &mut file)?;
            Ok(std::fs::read(&path).unwrap())
        };
        assert_eq!(read(0).unwrap(), main);
        assert_eq!(read(2).unwrap(), data);
        assert!(read(1).unwrap_err().contains("ek disk 1 yok"));
        assert_eq!(s.verify("a").unwrap(), 5);

        // Ek diske özgü bir parça bozulursa doğrulama yakalar; asıl disk yine okunur.
        let only_extra = chunk_files(&dir).into_iter().find(|p| {
            let packed = std::fs::read(p).unwrap();
            zstd::bulk::decompress(&packed, 64).unwrap() == b"VERIVERI"
        });
        std::fs::write(only_extra.unwrap(), b"bozuk").unwrap();
        assert!(s.verify("a").is_err() && read(2).is_err());
        assert_eq!(read(0).unwrap(), main);

        // Aynı disk numarası iki kez verilemez; yarım yedek kaydedilmez.
        assert!(s.write("b", &mut [(1, &mut &main[..], 27), (1, &mut &main[..], 27)]).unwrap_err().contains("iki kez"));
        assert!(!s.has("b"));
        // Tek diskli eski dizin (disk numarası alanı yok) asıl disk sayılır.
        put(&s, "eski", &main).unwrap();
        let path = dir.join(format!("eski{INDEX_SUFFIX}"));
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["disks"][0].as_object_mut().unwrap().remove("id");
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(s.disks("eski").unwrap(), [(0, 27)]);
        assert_eq!(restored(&s, &dir, "eski").unwrap(), main);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `image` üzerindeki değişiklikleri (bölge listesiyle) temel yedeğe işler ve geri okunan diski döndürür.
    fn delta(s: &Store, dir: &Path, id: &str, base: &str, image: &[u8], ranges: &[(u64, u64)]) -> Result<Vec<u8>, String> {
        // Ham dosyada yalnızca değişen bölgeler geçerlidir: geri kalanı çöple doldurulur ki okunursa fark edilsin.
        let mut sparse = vec![0xEEu8; image.len()];
        for (start, len) in ranges {
            let (a, b) = (*start as usize, ((*start + *len) as usize).min(image.len()));
            if a < b {
                sparse[a..b].copy_from_slice(&image[a..b]);
            }
        }
        let path = dir.join("delta.raw");
        std::fs::write(&path, &sparse).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        s.write_delta(id, base, &mut [DeltaDisk { id: 0, size: image.len() as u64, raw: &mut file, ranges: ranges.to_vec() }])?;
        restored(s, dir, id)
    }

    #[test]
    fn delta_backup_equals_a_full_read() {
        let (s, dir) = store();
        let first = disk(); // AAAAAAAA 00000000 BBBBBBBB CCC
        put(&s, "a", &first).unwrap();
        let before = chunk_files(&dir).len();

        // Parçanın ortasında birkaç bayt, parça sınırını aşan bir yazma ve kısa son parça.
        let mut second = first.clone();
        second[2..5].copy_from_slice(b"xyz");
        second[14..18].copy_from_slice(b"1234");
        second[25..27].copy_from_slice(b"ZZ");
        let ranges = [(2, 3), (14, 4), (25, 2)];
        assert_eq!(delta(&s, &dir, "b", "a", &second, &ranges).unwrap(), second);
        assert_eq!(s.verify("b").unwrap(), 4);
        assert_eq!(restored(&s, &dir, "a").unwrap(), first, "temel yedek değişmez");

        // Hiçbir şey değişmemişse yeni parça yazılmaz; yedek yine eksiksizdir.
        let stats = {
            let path = dir.join("bos.raw");
            std::fs::write(&path, vec![0xEEu8; second.len()]).unwrap();
            let mut file = std::fs::File::open(&path).unwrap();
            s.write_delta("c", "b", &mut [DeltaDisk { id: 0, size: second.len() as u64, raw: &mut file, ranges: Vec::new() }]).unwrap()
        };
        assert_eq!((stats.new_chunks, stats.added, stats.chunks), (0, 0, 4));
        assert_eq!(restored(&s, &dir, "c").unwrap(), second);

        // Bir parça tümüyle sıfırlanır (saklanmaz), sıfır olan parça dolar.
        let mut third = second.clone();
        third[0..8].fill(0);
        third[8..16].copy_from_slice(b"DOLUDOLU");
        assert_eq!(delta(&s, &dir, "d", "c", &third, &[(0, 16)]).unwrap(), third);
        let index: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(format!("d{INDEX_SUFFIX}"))).unwrap()).unwrap();
        assert_eq!(index["disks"][0]["chunks"][0], "", "sıfırlanan parça saklanmaz");
        assert!(chunk_files(&dir).len() > before);

        // Zincir değil: aradaki yedekler silinse de sonuncusu tam geri yüklenir.
        for id in ["a", "b", "c"] {
            s.delete(id).unwrap();
        }
        assert_eq!(restored(&s, &dir, "d").unwrap(), third);
        assert_eq!(s.verify("d").unwrap(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn delta_backup_refuses_a_base_that_does_not_fit() {
        let (s, dir) = store();
        let first = disk();
        put(&s, "a", &first).unwrap();
        let mut bigger = first.clone();
        bigger.extend(b"EKLENDI!");
        assert!(delta(&s, &dir, "b", "a", &bigger, &[(0, 4)]).unwrap_err().contains("boyutu"), "büyütülmüş disk");
        assert!(delta(&s, &dir, "b", "yok", &first, &[(0, 4)]).is_err(), "silinmiş temel yedek");
        assert!(delta(&s, &dir, "b", "a", &first, &[(20, 100)]).unwrap_err().contains("dışına"));
        assert!(delta(&s, &dir, "a", "a", &first, &[(0, 4)]).unwrap_err().contains("zaten var"));
        // Temel yedeğin bozuk parçası değişen parçanın altındaysa yeni yedek alınmaz.
        let victim = chunk_files(&dir).into_iter().find(|p| zstd::bulk::decompress(&std::fs::read(p).unwrap(), 64).unwrap() == b"AAAAAAAA").unwrap();
        std::fs::write(&victim, b"bozuk").unwrap();
        assert!(delta(&s, &dir, "b", "a", &first, &[(0, 4)]).unwrap_err().contains("parça bozuk"));
        // Temelin parçası tümüyle kaybolmuşsa, o parça değişmemiş olsa bile yeni yedek eksik kalırdı: reddedilir.
        std::fs::remove_file(&victim).unwrap();
        assert!(delta(&s, &dir, "b", "a", &first, &[(16, 4)]).unwrap_err().contains("parça eksik"));
        assert!(!s.has("b"), "yarım yedek kaydedilmez");
        // Disk sayısı değişmişse de temel kullanılamaz.
        let path = dir.join("iki.raw");
        std::fs::write(&path, &first).unwrap();
        let (mut f0, mut f1) = (std::fs::File::open(&path).unwrap(), std::fs::File::open(&path).unwrap());
        let mut two = [DeltaDisk { id: 0, size: 27, raw: &mut f0, ranges: vec![] }, DeltaDisk { id: 1, size: 27, raw: &mut f1, ranges: vec![] }];
        assert!(s.write_delta("c", "a", &mut two).unwrap_err().contains("uyuşmuyor"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn second_backup_stores_only_changed_chunks() {
        let (s, dir) = store();
        let first = disk();
        put(&s, "a", &first).unwrap();
        let mut second = first.clone();
        second[16..24].copy_from_slice(b"DDDDDDDD"); // üçüncü parça değişti
        let stats = put(&s, "b", &second).unwrap();
        assert_eq!((stats.chunks, stats.new_chunks), (3, 1), "yalnızca değişen parça yazılır");
        assert!(stats.added < stats.stored);
        assert_eq!(chunk_files(&dir).len(), 4);
        // Değişmeyen disk hiç yer kaplamaz.
        let again = put(&s, "c", &second).unwrap();
        assert_eq!((again.new_chunks, again.added), (0, 0));
        // Aynı içerik disk içinde yinelenirse bir kez saklanır.
        let twins = put(&s, "ikiz", b"EEEEEEEEEEEEEEEE").unwrap();
        assert_eq!((twins.chunks, twins.new_chunks), (1, 1));

        assert_eq!(restored(&s, &dir, "a").unwrap(), first, "eski yedek yeni yedekten etkilenmez");
        assert_eq!(restored(&s, &dir, "b").unwrap(), second);
        assert_eq!(restored(&s, &dir, "ikiz").unwrap(), b"EEEEEEEEEEEEEEEE");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_a_backup_never_breaks_the_others() {
        let (s, dir) = store();
        let first = disk();
        let mut second = first.clone();
        second[0..8].copy_from_slice(b"ZZZZZZZZ");
        put(&s, "a", &first).unwrap();
        put(&s, "b", &second).unwrap();
        assert_eq!(chunk_files(&dir).len(), 4);

        // Eski (temel) yedek silinir: yenisi tam olarak geri yüklenir.
        assert!(s.delete("a").unwrap());
        assert!(!s.has("a"));
        assert_eq!(chunk_files(&dir).len(), 3, "yalnızca a'ya özgü parça gider");
        assert_eq!(restored(&s, &dir, "b").unwrap(), second);
        assert_eq!(s.verify("b").unwrap(), 3);
        assert!(restored(&s, &dir, "a").is_err());
        assert!(!s.delete("a").unwrap(), "olmayanı silmek hata değildir");

        assert!(s.delete("b").unwrap());
        assert!(chunk_files(&dir).is_empty(), "son yedek gidince depo boşalır");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corruption_is_detected_not_restored() {
        let (s, dir) = store();
        let data = disk();
        put(&s, "a", &data).unwrap();
        let victim = chunk_files(&dir).remove(0);
        let good = std::fs::read(&victim).unwrap();

        // Geçerli ama başka içerikli parça: özet uyuşmaz.
        std::fs::write(&victim, zstd::bulk::compress(b"XXXXXXXX", 3).unwrap()).unwrap();
        assert!(restored(&s, &dir, "a").unwrap_err().contains("parça bozuk"));
        assert!(s.verify("a").unwrap_err().contains("parça bozuk"));
        // Açıldığında beklenenden uzun çıkan parça.
        std::fs::write(&victim, zstd::bulk::compress(&[7u8; 4096], 3).unwrap()).unwrap();
        assert!(s.verify("a").is_err());
        // Kesilmiş ve anlamsız veri.
        std::fs::write(&victim, &good[..good.len() / 2]).unwrap();
        assert!(s.verify("a").is_err());
        std::fs::write(&victim, b"zstd degil").unwrap();
        assert!(s.verify("a").is_err());
        // Eksik parça.
        std::fs::remove_file(&victim).unwrap();
        assert!(restored(&s, &dir, "a").unwrap_err().contains("parça eksik"));
        // Parça yerine konunca yedek yeniden sağlamdır.
        std::fs::write(&victim, &good).unwrap();
        assert_eq!(restored(&s, &dir, "a").unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_source_length_leaves_no_backup() {
        let (s, dir) = store();
        let data = disk();
        assert!(s.write("kisa", &mut [(0, &mut &data[..], 40)]).unwrap_err().contains("okunamadı"));
        assert!(s.write("uzun", &mut [(0, &mut &data[..], 16)]).unwrap_err().contains("uzun"));
        assert!(!s.has("kisa") && !s.has("uzun"), "yarım yedek kaydedilmez");
        // Yarım kalanların parçaları bir sonraki çöp toplamada gider.
        assert!(!chunk_files(&dir).is_empty());
        s.gc().unwrap().unwrap();
        assert!(chunk_files(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn gc_deletes_nothing_when_unsure() {
        let (s, dir) = store();
        put(&s, "a", &disk()).unwrap();
        put(&s, "b", b"QQQQQQQQ").unwrap();
        // b'nin dizini okunamaz hale gelir: hangi parçaların gerektiği bilinemez.
        std::fs::write(dir.join(format!("b{INDEX_SUFFIX}")), b"{ bozuk").unwrap();
        std::fs::remove_file(dir.join(format!("a{INDEX_SUFFIX}"))).unwrap();
        assert!(s.gc().unwrap_err().contains("çöp toplama atlandı"));
        assert_eq!(chunk_files(&dir).len(), 4, "belirsizlikte hiçbir parça silinmez");

        // Yedekleme ya da geri yükleme sürerken çöp toplama çalışmaz.
        std::fs::remove_file(dir.join(format!("b{INDEX_SUFFIX}"))).unwrap();
        {
            let _busy = s.gate.read().unwrap();
            assert_eq!(s.gc().unwrap(), None);
            assert_eq!(chunk_files(&dir).len(), 4);
        }
        // Yarım kalmış geçici dosyalar da toplanır.
        let stray = chunk_files(&dir)[0].with_file_name(".yarim.tmp");
        std::fs::write(&stray, b"x").unwrap();
        assert_eq!(s.gc().unwrap().unwrap().0, 5);
        assert!(chunk_files(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tampered_index_is_rejected() {
        let (s, dir) = store();
        put(&s, "a", &disk()).unwrap();
        let path = dir.join(format!("a{INDEX_SUFFIX}"));
        let original: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let broken = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut v = original.clone();
            edit(&mut v);
            std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
            s.verify("a").unwrap_err()
        };
        // Özet dosya yoluna dönüşür: dizin dışına çıkan değer kabul edilmez.
        assert!(broken(&|v| v["disks"][0]["chunks"][0] = "../../../etc/passwd".into()).contains("geçersiz parça özeti"));
        assert!(broken(&|v| v["disks"][0]["chunks"][0] = "AB".repeat(32).into()).contains("geçersiz parça özeti"));
        assert!(broken(&|v| v["disks"][0]["size"] = 4096.into()).contains("parça sayısı"));
        assert!(broken(&|v| v["format"] = 9.into()).contains("biçim sürümü"));
        assert!(broken(&|v| v["chunk_size"] = 0.into()).contains("parça boyutu"));
        assert!(broken(&|v| v["disks"] = serde_json::json!([])).contains("disk içermiyor"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Dizin tabanlı sahte dış hedef; hangi işlemin kaç kez yapıldığını sayar.
    struct FakeRemote {
        dir: PathBuf,
        log: Mutex<Vec<String>>,
    }

    impl FakeRemote {
        fn new(dir: &Path) -> Self {
            let dir = dir.join("uzak");
            std::fs::create_dir_all(&dir).unwrap();
            FakeRemote { dir, log: Mutex::new(Vec::new()) }
        }
        /// Belirtilen türden işlemleri döndürür ve günlüğü boşaltır.
        fn take(&self, verb: &str) -> Vec<String> {
            let all: Vec<String> = self.log.lock().unwrap().drain(..).collect();
            all.into_iter().filter(|line| line.starts_with(verb)).collect()
        }
        fn note(&self, verb: &str, name: &str) {
            self.log.lock().unwrap().push(format!("{verb} {name}"));
        }
        fn chunks(&self) -> usize {
            chunk_files(&self.dir).len()
        }
    }

    impl Remote for FakeRemote {
        fn exists(&self, name: &str) -> Result<bool, String> {
            self.note("exists", name);
            Ok(self.dir.join(name).is_file())
        }
        fn put(&self, name: &str, local: &Path) -> Result<u64, String> {
            self.note("put", name);
            let dest = self.dir.join(name);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::copy(local, dest).map_err(|e| format!("gönderilemedi: {e}"))
        }
        fn get(&self, name: &str, local: &Path) -> Result<(), String> {
            self.note("get", name);
            std::fs::copy(self.dir.join(name), local).map(|_| ()).map_err(|e| format!("hedefte yok ({name}): {e}"))
        }
        fn delete(&self, name: &str) -> Result<(), String> {
            self.note("delete", name);
            let _ = std::fs::remove_file(self.dir.join(name));
            Ok(())
        }
    }

    /// Aynı hedefe bakan, yerel deposu boş ikinci bir sunucu.
    fn fresh(dir: &Path, name: &str) -> (Store, PathBuf) {
        let local = dir.join(name);
        std::fs::create_dir_all(&local).unwrap();
        (Store::with_chunk_size(&local, 8), local)
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn push_sends_only_what_the_target_lacks() {
        let (s, dir) = store();
        let remote = FakeRemote::new(&dir);
        let first = disk();
        let mut second = first.clone();
        second[16..24].copy_from_slice(b"DDDDDDDD");
        put(&s, "a", &first).unwrap();
        put(&s, "b", &second).unwrap();

        assert_eq!(s.push("a", &[], &remote).unwrap().0, 3);
        let puts = remote.take("put");
        assert_eq!(puts.len(), 5, "depo işareti, üç parça ve dizin: {puts:?}");
        assert!(puts[0].ends_with("redox-store.json"));
        assert!(puts.last().unwrap().ends_with("a.rdxi.json"), "dizin en son gönderilir");
        assert_eq!(remote.chunks(), 3);

        // İkinci yedek: a'nın dış kopyası olduğu biliniyor; ortak parçalar sorulmadan atlanır.
        assert_eq!(s.push("b", &ids(&["a"]), &remote).unwrap().0, 1);
        let log: Vec<String> = remote.log.lock().unwrap().clone();
        assert_eq!(log.iter().filter(|l| l.starts_with("exists")).count(), 3, "depo işareti, a'nın dizini ve yalnızca yeni parça sorulur: {log:?}");
        assert_eq!(remote.take("put").len(), 2);
        assert_eq!(remote.chunks(), 4);

        // Yeniden gönderme (bilinen yok): her parça sorulur, eksik olan tamamlanır.
        let lost = chunk_files(&remote.dir).remove(0);
        std::fs::remove_file(&lost).unwrap();
        remote.take("exists");
        let resent = s.push("a", &[], &remote).unwrap().0 + s.push("b", &[], &remote).unwrap().0;
        assert_eq!(resent, 1, "hedefte kaybolan parça yeniden gider");
        assert_eq!(remote.take("exists").len(), 8, "bilinen yokken her parça sorulur (ve iki kez depo işareti)");
        assert_eq!(remote.chunks(), 4);

        // Yerelde parçası eksik yedek gönderilemez ve hedefe dizini konmaz.
        put(&s, "c", b"KKKKKKKK").unwrap();
        for path in chunk_files(&dir) {
            if !path.starts_with(&remote.dir) && !chunk_files(&remote.dir).iter().any(|r| r.file_name() == path.file_name()) {
                std::fs::remove_file(path).unwrap();
            }
        }
        assert!(s.push("c", &[], &remote).is_err());
        assert!(!remote.dir.join("c.rdxi.json").exists());
        assert!(s.push("yok", &[], &remote).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn push_does_not_trust_the_offsite_flag_when_the_target_was_replaced() {
        let (s, dir) = store();
        let old = FakeRemote::new(&dir);
        put(&s, "a", &disk()).unwrap();
        put(&s, "b", &disk()).unwrap();
        s.push("a", &[], &old).unwrap();
        // Hedef başka (boş) bir yere çevrilir; a hâlâ "dış kopyası var" diye biliniyor.
        let fresh_dir = dir.join("yeni-hedef");
        let new = FakeRemote::new(&fresh_dir);
        assert_eq!(s.push("b", &ids(&["a"]), &new).unwrap().0, 3, "a'nın dizini yeni hedefte yok: bütün parçalar gönderilir");
        let (other, local) = fresh(&dir, "baska");
        assert!(other.pull("b", &new).unwrap());
        assert_eq!(restored(&other, &local, "b").unwrap(), disk());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pull_restores_a_lost_store_and_verifies_every_chunk() {
        let (s, dir) = store();
        let remote = FakeRemote::new(&dir);
        let data = disk();
        put(&s, "a", &data).unwrap();
        s.push("a", &[], &remote).unwrap();

        let (other, local) = fresh(&dir, "yeni");
        assert!(!other.pull("yok", &remote).unwrap(), "hedefte olmayan yedek: yanlış, hata değil");
        assert!(other.pull("a", &remote).unwrap());
        assert_eq!(restored(&other, &local, "a").unwrap(), data);
        assert_eq!(other.verify("a").unwrap(), 3);
        remote.take("get");
        // Parçalar zaten yereldeyse yalnızca dizin iner.
        std::fs::remove_file(local.join(format!("a{INDEX_SUFFIX}"))).unwrap();
        assert!(other.pull("a", &remote).unwrap());
        assert_eq!(remote.take("get").len(), 1);

        // Hedefte bozulmuş parça: indirme reddedilir, yerelde ne dizin ne bozuk parça kalır.
        let (third, local3) = fresh(&dir, "ucuncu");
        let victim = chunk_files(&remote.dir).remove(1);
        let good = std::fs::read(&victim).unwrap();
        std::fs::write(&victim, zstd::bulk::compress(b"XXXXXXXX", 3).unwrap()).unwrap();
        assert!(third.pull("a", &remote).unwrap_err().contains("parça bozuk"));
        assert!(!third.has("a"));
        let leftovers: Vec<PathBuf> = chunk_files(&local3).into_iter().filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with('.')).collect();
        assert!(leftovers.is_empty() && std::fs::read_dir(&local3).unwrap().flatten().all(|e| !e.file_name().to_str().unwrap().starts_with('.')), "geçici dosya kalmaz");
        // Hedefte eksik parça.
        std::fs::remove_file(&victim).unwrap();
        assert!(third.pull("a", &remote).unwrap_err().contains("indirilemedi"));
        assert!(!third.has("a"));
        std::fs::write(&victim, &good).unwrap();
        assert!(third.pull("a", &remote).unwrap());
        assert_eq!(restored(&third, &local3, "a").unwrap(), data);

        // Hedefteki dizin değiştirilmişse (dizin dışına çıkan ad) reddedilir.
        let index = remote.dir.join(format!("a{INDEX_SUFFIX}"));
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
        v["disks"][0]["chunks"][0] = "../../x".into();
        std::fs::write(&index, serde_json::to_vec(&v).unwrap()).unwrap();
        let (fourth, _) = fresh(&dir, "dorduncu");
        assert!(fourth.pull("a", &remote).unwrap_err().contains("geçersiz"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pull_heals_corrupt_local_chunks_and_rejects_a_swapped_index() {
        let (s, dir) = store();
        let remote = FakeRemote::new(&dir);
        let data = disk();
        put(&s, "a", &data).unwrap();
        put(&s, "b", b"BASKAVERI").unwrap();
        s.push("a", &[], &remote).unwrap();
        s.push("b", &[], &remote).unwrap();
        // Yerelde bir parça bozulur: yedek okunamaz; hedeften çekince onarılır.
        let victim = chunk_files(&dir).into_iter().find(|p| !p.starts_with(&remote.dir) && zstd::bulk::decompress(&std::fs::read(p).unwrap(), 64).unwrap() == b"AAAAAAAA").unwrap();
        std::fs::write(&victim, b"bozuldu").unwrap();
        assert!(restored(&s, &dir, "a").is_err());
        remote.take("get");
        assert!(s.pull("a", &remote).unwrap());
        assert_eq!(remote.take("get").len(), 2, "dizin ve yalnızca bozuk parça iner");
        assert_eq!(restored(&s, &dir, "a").unwrap(), data);

        // Hedefte a'nın dizini b'ninkiyle değiştirilirse (geçerli, parçaları sağlam bir dizin) fark edilir.
        std::fs::copy(remote.dir.join(format!("b{INDEX_SUFFIX}")), remote.dir.join(format!("a{INDEX_SUFFIX}"))).unwrap();
        let (other, _) = fresh(&dir, "yeni");
        assert!(other.pull("a", &remote).unwrap_err().contains("başka bir yedeğe ait"));
        assert!(!other.has("a"));
        // Yerelde de aynı: dizin dosyası başka bir yedeğin adına kopyalanırsa okunmaz.
        std::fs::copy(dir.join(format!("b{INDEX_SUFFIX}")), dir.join(format!("c{INDEX_SUFFIX}"))).unwrap();
        assert!(s.verify("c").unwrap_err().contains("başka bir yedeğe ait"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn push_refuses_a_target_written_with_another_passphrase() {
        let (s, dir) = store();
        let target = dir.join("hedef");
        std::fs::create_dir_all(&target).unwrap();
        let config = |pass: &str| crate::offsite::Config::parse(&format!("kind = dir\npath = {}\npassphrase = {pass}\n", target.display())).unwrap();
        put(&s, "a", &disk()).unwrap();
        put(&s, "b", &disk()).unwrap();
        s.push("a", &[], &config("ilk-parola-123456")).unwrap();
        // Parola değişti: eski parçalar yeni parolayla açılamaz; "hedefte var" diye atlanmamalı.
        let err = s.push("b", &[], &config("yeni-parola-654321")).unwrap_err();
        assert!(err.contains("parolası değiştiyse"), "{err}");
        assert!(!target.join(format!("b{INDEX_SUFFIX}.enc")).exists(), "açılamayacak bir dış kopya bırakılmaz");
        s.push("b", &[], &config("ilk-parola-123456")).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unpush_removes_only_chunks_no_other_backup_needs() {
        let (s, dir) = store();
        let remote = FakeRemote::new(&dir);
        let first = disk();
        let mut second = first.clone();
        second[0..8].copy_from_slice(b"ZZZZZZZZ");
        put(&s, "a", &first).unwrap();
        put(&s, "b", &second).unwrap();
        s.push("a", &[], &remote).unwrap();
        s.push("b", &ids(&["a"]), &remote).unwrap();
        assert_eq!(remote.chunks(), 4);
        assert!(!s.unpush("yok", &[], &remote).unwrap());

        // a yerelde ve hedefte silinir: b'nin kullandığı parçalar hedefte kalır.
        s.delete("a").unwrap();
        assert!(s.unpush("a", &ids(&["b"]), &remote).unwrap());
        assert_eq!(remote.chunks(), 3);
        assert!(!remote.dir.join(format!("a{INDEX_SUFFIX}")).exists());
        let (other, local) = fresh(&dir, "yeni");
        assert!(other.pull("b", &remote).unwrap());
        assert_eq!(restored(&other, &local, "b").unwrap(), second);

        // Yerel depo tümüyle kaybolmuş olsa da: hedefte kalan diğer yedeğin dizini okunur, parçaları korunur.
        put(&s, "c", &first).unwrap();
        s.push("c", &[], &remote).unwrap();
        assert_eq!(remote.chunks(), 4);
        let (empty, _) = fresh(&dir, "bos");
        assert!(empty.unpush("c", &ids(&["b"]), &remote).unwrap());
        assert_eq!(remote.chunks(), 3, "yalnızca c'ye özgü parça gider");
        let (again, local2) = fresh(&dir, "yine");
        assert!(again.pull("b", &remote).unwrap());
        assert_eq!(restored(&again, &local2, "b").unwrap(), second);

        // Son yedek de silinince hedef boşalır.
        s.delete("b").unwrap();
        s.delete("c").unwrap();
        assert!(s.unpush("b", &[], &remote).unwrap());
        assert_eq!(remote.chunks(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unpush_keeps_every_chunk_when_it_cannot_tell_what_is_needed() {
        let (s, dir) = store();
        let remote = FakeRemote::new(&dir);
        put(&s, "a", &disk()).unwrap();
        put(&s, "b", &disk()).unwrap();
        s.push("a", &[], &remote).unwrap();
        s.push("b", &ids(&["a"]), &remote).unwrap();
        // b yalnızca hedefte kalmış ve dizini okunamıyor: a silinirken hiçbir parça silinmez.
        let (empty, _) = fresh(&dir, "bos");
        std::fs::write(remote.dir.join(format!("b{INDEX_SUFFIX}")), b"{ bozuk").unwrap();
        let err = empty.unpush("a", &ids(&["b"]), &remote).unwrap_err();
        assert!(err.contains("parçaları hedefte bırakıldı"), "{err}");
        assert_eq!(remote.chunks(), 3);
        assert!(!remote.dir.join(format!("a{INDEX_SUFFIX}")).exists(), "dizin silinir: yedek artık listelenmez");

        // Yerelde okunamayan bir dizin de aynı sonucu verir.
        s.push("a", &[], &remote).unwrap();
        std::fs::write(dir.join(format!("b{INDEX_SUFFIX}")), b"{ bozuk").unwrap();
        assert!(s.unpush("a", &[], &remote).unwrap_err().contains("bırakıldı"));
        assert_eq!(remote.chunks(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
