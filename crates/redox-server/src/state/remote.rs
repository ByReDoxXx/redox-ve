//! Sunucu dışı yedek hedefleri: asıl hedef, kopya hedefleri ve gönderme.

use super::*;

impl AppState {
    /// Ek bir dış hedef ekler: asıl hedefe giden her yedek buraya da kopyalanır.
    pub fn add_offsite_copy(&self, config: crate::offsite::Config) {
        self.0.offsite_copies.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::new(config));
    }

    pub fn offsite_copies(&self) -> Vec<Arc<crate::offsite::Config>> {
        self.0.offsite_copies.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Yedeğin geri indirilebileceği hedefler, deneme sırasıyla: asıl hedef, sonra kopya hedefleri.
    pub(super) fn remotes_for(&self, backup: &Backup) -> Vec<Arc<crate::offsite::Config>> {
        let mut remotes: Vec<_> = if backup.offsite { self.offsite().into_iter().collect() } else { Vec::new() };
        if backup.copies > 0 {
            remotes.extend(self.offsite_copies());
        }
        remotes
    }

    /// Asıl hedefe gitmiş yedeği ek hedeflere de kopyalar; başarılı kopya sayısını döndürür.
    /// Başarısız kopya görev günlüğüne yazılır; asıl dış kopya ve yerel yedek yine geçerlidir.
    pub(super) fn push_copies(&self, id: &str, disks: &[u8], vmid: Option<VmId>) -> u8 {
        let mut made = 0u8;
        for copy in self.offsite_copies() {
            // Kopya hedefinde hangi parçaların durduğu bilinmediği için hepsi hedefe sorulur.
            let outcome = self.push_to(&copy, id, disks, Vec::new());
            if outcome.is_ok() {
                made += 1;
            } else {
                self.mutate(|c| {
                    let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                    c.log(&node, SCHEDULER, format!("backup copy {id} ({})", copy.describe()), &outcome, now(), vmid);
                });
            }
        }
        made
    }

    pub fn set_offsite(&self, config: crate::offsite::Config) {
        *self.0.offsite.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(config));
    }

    pub fn offsite(&self) -> Option<Arc<crate::offsite::Config>> {
        self.0.offsite.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Dış kopyası olan diğer yedeklerin kimlikleri.
    pub(super) fn offsite_others(&self, id: &str) -> Vec<String> {
        self.lock().backups.iter().filter(|b| b.offsite && b.id != id).map(|b| b.id.clone()).collect()
    }

    /// Yedeği dış hedefe kopyalar. Hedef yapılandırılmadıysa `None`. Artımlı yedek parça parça gider
    /// (yalnızca hedefte olmayan kısım); diğerleri tek dosya olarak. `recheck`: diğer yedeklerden
    /// "hedefte vardır" diye bilinen parçalar da hedefe sorulur (elle yeniden gönderme).
    pub(super) fn push_file(&self, id: &str, disks: &[u8], recheck: bool) -> Option<Result<(), String>> {
        let config = self.offsite()?;
        let known = if recheck { Vec::new() } else { self.offsite_others(id) };
        Some(self.push_to(&config, id, disks, known))
    }

    /// Yedeğin disklerini ve (varsa) makine durumu arşivini verilen hedefe gönderir.
    pub(super) fn push_to(&self, config: &Arc<crate::offsite::Config>, id: &str, disks: &[u8], known: Vec<String>) -> Result<(), String> {
        self.push_disks(config, id, disks, known)?;
        // Makine durumu arşivi (varsa) disklerden sonra gider.
        match self.driver().backup_machine_file(id) {
            Some(path) if path.is_file() => config.push(&path).map(|_| ()),
            _ => Ok(()),
        }
    }

    pub(super) fn push_disks(&self, config: &Arc<crate::offsite::Config>, id: &str, disks: &[u8], known: Vec<String>) -> Result<(), String> {
        match self.driver().push_backup(id, &known, &**config) {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(e) => return Err(e),
        }
        // Tam yedek: asıl disk ve her ek disk ayrı dosya olarak gider.
        let mut sent: Vec<std::path::PathBuf> = Vec::new();
        for disk in std::iter::once(0).chain(disks.iter().copied()) {
            let pushed = match self.driver().export_backup(id, disk) {
                Ok(Some((path, temporary))) => {
                    let pushed = config.push(&path).map(|_| ());
                    if temporary {
                        discard_export(&path);
                    }
                    pushed.map(|()| path)
                }
                Ok(None) => Err("bu sürücü yedek dosyası tutmuyor".to_string()),
                Err(e) => Err(e),
            };
            match pushed {
                Ok(path) => sent.push(path),
                Err(e) => {
                    // Yarım dış kopya bırakılmaz: gönderilmiş dosyalar geri alınır (kayıt "dış kopya yok" kalır).
                    for path in &sent {
                        let _ = config.delete(path);
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}
