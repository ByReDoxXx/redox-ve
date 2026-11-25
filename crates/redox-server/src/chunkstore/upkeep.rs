//! Bakım: doğrulama, başvuru sayımı, silme ve çöp toplama.

use super::*;

impl Store {
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
    pub(super) fn unique_chunks(index: &Index) -> Vec<(String, usize)> {
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
    pub(super) fn referenced(&self) -> Result<HashSet<String>, String> {
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
