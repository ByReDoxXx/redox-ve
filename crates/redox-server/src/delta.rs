//! Artımlı çoğaltma: iki sunucudaki diskin yalnızca farklı bloklarını taşır.
//!
//! Kaynak, yedeğin ham içeriğini sabit boyutlu bloklara böler ve her bloğun SHA-256 özetini verir. Hedef,
//! elindeki eski kopyanın aynı bloklarını özetler; özeti tutan blok yerinde kalır, tutmayan kaynaktan
//! indirilir. Değişmeyen veri ağdan geçmez; indirilen her blok özetiyle doğrulanır.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Blok boyutu: 4 MiB.
pub const BLOCK: u64 = 4 * 1024 * 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_block(file: &mut std::fs::File, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    file.seek(SeekFrom::Start(offset))?;
    let mut filled = 0;
    while filled < len {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    // Dosyanın sonundan öteye düşen kısım sıfır sayılır.
    Ok(buf)
}

/// Dosyanın boyutu ve blok özetleri.
pub fn block_hashes(path: &Path) -> Result<(u64, Vec<String>), String> {
    let err = |e: std::io::Error| format!("{} okunamadı: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(err)?;
    let size = file.metadata().map_err(err)?.len();
    let mut hashes = Vec::with_capacity(size.div_ceil(BLOCK) as usize);
    let mut offset = 0;
    while offset < size {
        let len = (size - offset).min(BLOCK) as usize;
        hashes.push(hex(&Sha256::digest(read_block(&mut file, offset, len).map_err(err)?)));
        offset += len as u64;
    }
    Ok((size, hashes))
}

/// Dosyanın bir bloğu (son blok kısa olabilir); dosyanın dışındaysa hata.
pub fn block(path: &Path, index: u64) -> Result<Vec<u8>, String> {
    let err = |e: std::io::Error| format!("{} okunamadı: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(err)?;
    let size = file.metadata().map_err(err)?.len();
    let offset = index.checked_mul(BLOCK).filter(|o| *o < size).ok_or("istenen blok dosyanın dışında")?;
    read_block(&mut file, offset, (size - offset).min(BLOCK) as usize).map_err(err)
}

/// `path` dosyasını kaynağın içeriğine getirir: boyutu `size` yapılır, özeti tutmayan bloklar `fetch` ile
/// alınıp yazılır. İndirilen blok sayısını ve toplam blok sayısını döndürür.
pub fn patch(path: &Path, size: u64, hashes: &[String], fetch: &dyn Fn(u64) -> Result<Vec<u8>, String>) -> Result<(u64, u64), String> {
    if hashes.len() as u64 != size.div_ceil(BLOCK) {
        return Err("blok sayısı disk boyutuyla uyuşmuyor".to_string());
    }
    let err = |e: std::io::Error| format!("{} yazılamadı: {e}", path.display());
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(err)?;
    file.set_len(size).map_err(err)?;
    let mut fetched = 0;
    for (index, wanted) in hashes.iter().enumerate() {
        let offset = index as u64 * BLOCK;
        let len = (size - offset).min(BLOCK) as usize;
        if hex(&Sha256::digest(read_block(&mut file, offset, len).map_err(err)?)) == *wanted {
            continue;
        }
        let data = fetch(index as u64)?;
        if data.len() != len || hex(&Sha256::digest(&data)) != *wanted {
            return Err(format!("{index}. blok kaynaktan bozuk geldi (özeti tutmuyor)"));
        }
        file.seek(SeekFrom::Start(offset)).map_err(err)?;
        file.write_all(&data).map_err(err)?;
        fetched += 1;
    }
    file.sync_all().map_err(err)?;
    Ok((fetched, hashes.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("redox-delta-{}-{name}", std::process::id()))
    }

    #[test]
    fn only_changed_blocks_are_fetched_and_every_block_is_verified() {
        let (source, copy) = (temp("kaynak"), temp("kopya"));
        // Üç blok: iki tam, biri kısa.
        let mut data = vec![1u8; (2 * BLOCK + 1000) as usize];
        std::fs::write(&source, &data).unwrap();
        std::fs::write(&copy, &data).unwrap();
        let (size, hashes) = block_hashes(&source).unwrap();
        assert_eq!((size, hashes.len()), (2 * BLOCK + 1000, 3));
        let fetch = |i: u64| block(&source, i);
        assert_eq!(patch(&copy, size, &hashes, &fetch).unwrap(), (0, 3), "aynı içerik: hiçbir şey inmez");

        // Kaynakta ortadaki blok değişir ve disk bir blok büyür.
        data[BLOCK as usize + 5] = 9;
        data.extend(vec![7u8; BLOCK as usize]);
        std::fs::write(&source, &data).unwrap();
        let (size, hashes) = block_hashes(&source).unwrap();
        assert_eq!(patch(&copy, size, &hashes, &|i| block(&source, i)).unwrap(), (3, 4), "değişen blok, uzayan son blok ve yeni blok");
        assert_eq!(std::fs::read(&copy).unwrap(), data);

        // Küçülen disk kısaltılır; boş kopyadan başlamak tam indirmedir.
        data.truncate(BLOCK as usize + 10);
        std::fs::write(&source, &data).unwrap();
        let (size, hashes) = block_hashes(&source).unwrap();
        patch(&copy, size, &hashes, &|i| block(&source, i)).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), data);
        std::fs::write(&copy, b"").unwrap();
        assert_eq!(patch(&copy, size, &hashes, &|i| block(&source, i)).unwrap(), (2, 2));

        // Bozuk gelen blok reddedilir; tutarsız özet listesi de.
        std::fs::write(&copy, b"").unwrap();
        assert!(patch(&copy, size, &hashes, &|_| Ok(vec![0u8; BLOCK as usize])).unwrap_err().contains("bozuk geldi"));
        assert!(patch(&copy, size, &hashes[..1], &|i| block(&source, i)).unwrap_err().contains("uyuşmuyor"));
        assert!(block(&source, 99).is_err());
        for f in [source, copy] {
            let _ = std::fs::remove_file(f);
        }
    }
}
