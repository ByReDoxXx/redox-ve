//! Kalıcılık: küme durumu diske yalnızca değiştiğinde yazılır.
//!
//! Arka plan turlarının çoğu hiçbir şeyi değiştirmeden `mutate` çağırır. Her çağrıda dosyayı baştan yazıp diske
//! boşaltmak (fsync) SSD'yi boşuna yıpratır ve küme kilidini bu süre boyunca tutar. Ölçümler (bellek, çalışma
//! süresi, disk doluluğu) küme durumunda sürekli değişir; bunlar boş bir turla dosyaya taşınmaz, ölçüm turu
//! onları kendi aralığında yazar.

use sha2::{Digest, Sha256};

use super::*;

pub(super) type Fingerprint = [u8; 32];

fn encode(cluster: &Cluster) -> std::io::Result<(Vec<u8>, Fingerprint)> {
    let bytes = serde_json::to_vec_pretty(cluster)?;
    let digest = Sha256::digest(&bytes).into();
    Ok((bytes, digest))
}

impl AppState {
    /// Değişiklikten önceki hâlin özeti; kalıcılık kapalıysa `None`.
    pub(super) fn fingerprint(&self, cluster: &Cluster) -> Option<Fingerprint> {
        self.0.state_file.as_ref()?;
        encode(cluster).ok().map(|(_, digest)| digest)
    }

    /// Kümenin son hâlini yazar. Çağıran küme kilidini tutar: yazmalar sıraya girer.
    pub(super) fn persist(&self, cluster: &Cluster) {
        self.persist_unless(cluster, None);
    }

    /// `before`: değişiklikten önceki özet. Küme o hâldeyse (değişiklik hiçbir şeye dokunmadıysa) yazılmaz.
    pub(super) fn persist_unless(&self, cluster: &Cluster, before: Option<Fingerprint>) {
        let Some(path) = &self.0.state_file else { return };
        // Durum dosyası yalnızca hizmet kullanıcısınca okunabilir (0600).
        let outcome = encode(cluster).and_then(|(bytes, digest)| {
            let mut saved = self.0.saved.lock().unwrap_or_else(|e| e.into_inner());
            // Dosya silinmişse ya da son yazma başarısız olduysa içerik aynı olsa da yeniden yazılır.
            let untouched = before == Some(digest) && saved.is_some();
            if (untouched || *saved == Some(digest)) && path.exists() {
                return Ok(());
            }
            // Yazma yarıda kalırsa bir sonraki çağrı yeniden denesin.
            *saved = None;
            write_private(path, &bytes)?;
            *saved = Some(digest);
            Ok(())
        });
        if let Err(e) = outcome {
            eprintln!("uyarı: durum {} dosyasına yazılamadı: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Cost, UserFile};
    use redox_core::SimDriver;

    fn state_at(path: &Path) -> AppState {
        let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
        AppState::new(Cluster::demo(0), Box::new(SimDriver::default()), Duration::ZERO, Some(path.to_path_buf()), auth)
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("redox-persist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_state_file_is_written_only_when_the_cluster_changes() {
        let dir = folder("degisim");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let state = state_at(&path);
        state.mutate(|_| ());
        let first = load_state(&path).unwrap().unwrap();
        assert_eq!(first.name, state.snapshot().name);

        // Dosyaya konan işaret, yeniden yazılıp yazılmadığını gösterir.
        std::fs::write(&path, b"isaret").unwrap();
        for _ in 0..3 {
            state.mutate(|_| ());
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"isaret", "değişiklik yokken yazılmaz");

        state.mutate(|c| c.name = "yeni-ad".into());
        assert_eq!(load_state(&path).unwrap().unwrap().name, "yeni-ad");
        // Eski hâline dönmek de bir değişikliktir.
        state.mutate(|c| c.name = first.name.clone());
        assert_eq!(load_state(&path).unwrap().unwrap().name, first.name);

        std::fs::remove_file(&path).unwrap();
        state.mutate(|_| ());
        assert!(path.exists(), "silinen dosya geri yazılır");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn measurements_are_not_carried_to_disk_by_a_change_that_does_nothing() {
        let dir = folder("olcum");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let state = state_at(&path);
        state.mutate(|_| ());
        let first = std::fs::read(&path).unwrap();
        // Ölçüm turu kümeyi yazmadan günceller.
        state.lock().nodes[0].uptime_secs += 5;
        state.mutate(|_| ());
        assert_eq!(std::fs::read(&path).unwrap(), first);
        // Ölçüm turunun kendi yazması ve gerçek bir değişiklik güncel ölçümü de taşır.
        let uptime = state.snapshot().nodes[0].uptime_secs;
        state.persist(&state.lock());
        assert_eq!(load_state(&path).unwrap().unwrap().nodes[0].uptime_secs, uptime);
        state.lock().nodes[0].uptime_secs += 5;
        state.mutate(|c| c.name = "baska".into());
        assert_eq!(load_state(&path).unwrap().unwrap().nodes[0].uptime_secs, uptime + 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_write_is_tried_again() {
        let dir = folder("hata");
        let path = dir.join("state.json");
        let state = state_at(&path);
        // Dizin yokken yazılamaz.
        state.mutate(|c| c.name = "kayip-olmasin".into());
        assert!(!path.exists());
        std::fs::create_dir_all(&dir).unwrap();
        state.mutate(|_| ());
        assert_eq!(load_state(&path).unwrap().unwrap().name, "kayip-olmasin");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
