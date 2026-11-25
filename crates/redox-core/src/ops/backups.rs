//! Yedek kayıtları: saklama, kilit, arşiv, korumasız konuklar ve geri yükleme kuralları.

use super::*;

impl Cluster {
    /// Sınama konuğunun denetlenecek kapısına açılan sunucu tarafı kapıyı kaydeder (konuk başlatılmadan önce).
    pub fn set_test_forward(&mut self, vmid: VmId, host_port: u16) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.test_fwd = host_port;
        }
    }

    /// Saklama sayısını aşan otomatik yedekler (en eskiler). Elle alınan yedeklere ve
    /// silinmiş konukların yedeklerine dokunulmaz. Konuğun kendi zamanlamasıyla alınanlar konuğun,
    /// bir yedek işinin aldıkları o işin saklama sayısına uyar; işi silinmiş yedekler kalır.
    ///
    /// "Son N"e ek olarak haftalık, aylık ve yıllık yedekler (GFS) saklanır; kilitli yedekler süresi dolana kadar
    /// budanmaz (ama sayıya dahildir: kilit bitince fazlalıksa silinir).
    pub fn prunable_backups(&self, now: u64) -> Vec<String> {
        let mut doomed = Vec::new();
        let mut prune = |group: Vec<&Backup>, keep: u32, gfs: crate::retention::Gfs| {
            // `self.backups` en yeni başta tutulur; yine de alınma anına göre sıralanır.
            let mut group = group;
            group.sort_by_key(|b| std::cmp::Reverse(b.created));
            let created: Vec<u64> = group.iter().map(|b| b.created).collect();
            let kept = crate::retention::kept(&created, keep, gfs);
            // Kilitli ve üzerinde anında kurtarılmış bir konuk çalışan yedekler budanmaz.
            let in_use = |id: &str| self.guests.iter().any(|g| g.hw.instant.as_deref() == Some(id));
            doomed.extend(group.iter().zip(kept).filter(|(b, kept)| !kept && !b.is_locked(now) && !in_use(&b.id)).map(|(b, _)| b.id.clone()));
        };
        for g in &self.guests {
            prune(self.backups.iter().filter(|b| b.vmid == g.vmid && b.auto && b.job.is_none()).collect(), g.backup_keep, g.backup_gfs);
            for j in &self.jobs {
                prune(self.backups.iter().filter(|b| b.vmid == g.vmid && b.auto && b.job.as_deref() == Some(j.name.as_str())).collect(), j.keep, j.gfs);
            }
        }
        doomed
    }

    /// Yedeği `days` gün boyunca silinemez yapar. Kilit yalnızca uzatılabilir: var olan kilit kısaltılamaz
    /// ve kaldırılamaz (yanlışlıkla ya da kötü niyetle silmeye karşı korumanın anlamı budur).
    pub fn lock_backup(&mut self, id: &str, days: u32, now: u64) -> Result<u64, Error> {
        if days == 0 || days > crate::retention::MAX_LOCK_DAYS {
            return Err(Error::InvalidSpec(format!("kilit süresi 1 ile {} gün arasında olmalı", crate::retention::MAX_LOCK_DAYS)));
        }
        let until = now + u64::from(days) * crate::retention::DAY_SECS;
        let backup = self.backups.iter_mut().find(|b| b.id == id).ok_or_else(|| Error::BackupNotFound(id.to_string()))?;
        if backup.locked_until.is_some_and(|old| old >= until) {
            return Err(Error::InvalidSpec(format!("yedek zaten {} tarihine kadar kilitli; kilit kısaltılamaz", crate::retention::date_label(backup.locked_until.unwrap_or(0)))));
        }
        backup.locked_until = Some(until);
        Ok(until)
    }

    /// Korumasız konuklar: son `max_age_days` gün içinde hiç yedeği alınmamış olanlar (şablonlar hariç).
    /// `(kimlik, ad, son yedeğin anı)`; hiç yedeği yoksa an `None`. Kimlik sırasıyla döner.
    ///
    /// Konuğun kendi yedek yaşı hedefi (RPO, saat) varsa o geçerlidir; yoksa `max_age_days`. İkisi de 0 ise
    /// konuk bu denetimin dışındadır.
    pub fn unprotected(&self, now: u64, max_age_days: u32) -> Vec<(VmId, String, Option<u64>)> {
        self.guests
            .iter()
            .filter(|g| !g.hw.template)
            .filter_map(|g| {
                let allowed = match g.hw.rpo_hours {
                    0 => u64::from(max_age_days) * crate::retention::DAY_SECS,
                    hours => u64::from(hours) * 3600,
                };
                if allowed == 0 {
                    return None;
                }
                let limit = now.saturating_sub(allowed);
                let last = self.backups.iter().filter(|b| b.vmid == g.vmid).map(|b| b.created).max();
                (last.is_none_or(|ts| ts < limit)).then(|| (g.vmid, g.name.clone(), last))
            })
            .collect()
    }

    /// Konuk yedekten ayrılabilir mi (anında kurtarılmış mı)?
    pub fn check_detach(&self, vmid: VmId) -> Result<(), Error> {
        match self.guest(vmid)?.hw.instant {
            Some(_) => Ok(()),
            None => Err(Error::InvalidSpec("bu konuk bir yedeğin üzerinde çalışmıyor".to_string())),
        }
    }

    /// Diskler bağımsızlaştı: konuk artık yedeğe dayanmıyor.
    pub fn apply_detach(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.instant = None;
        }
    }

    /// Ek dış hedeflerdeki kopya sayısını kaydeder; yedek silinmişse sessizce geçer.
    pub fn set_backup_copies(&mut self, id: &str, copies: u8) {
        if let Some(b) = self.backups.iter_mut().find(|b| b.id == id) {
            b.copies = copies;
        }
    }

    /// Olağan dışı değişim uyarısını kapatır (yönetici baktı, sorun yok). Uyarı notunu döndürür.
    pub fn clear_backup_suspicion(&mut self, id: &str) -> Result<String, Error> {
        let b = self.backups.iter_mut().find(|b| b.id == id).ok_or_else(|| Error::BackupNotFound(id.to_string()))?;
        b.suspicious.take().ok_or_else(|| Error::InvalidSpec(format!("{id} yedeğinde kapatılacak bir uyarı yok")))
    }

    /// Zamanlanmış sınama için sıradaki yedek: her sanal makinenin **en yeni** yedeği, hiç sınanmadıysa ya da
    /// son sınaması `every_days` günden eskiyse adaydır. Hiç sınanmamışlar önce, sonra sınaması en eski olan gelir.
    /// Şablonların ve konteynerlerin yedekleri açılarak sınanmaz.
    pub fn next_backup_to_test(&self, now: u64, every_days: u32) -> Option<String> {
        let limit = now.saturating_sub(u64::from(every_days) * crate::retention::DAY_SECS);
        self.guests
            .iter()
            .filter(|g| g.kind == GuestKind::Vm && !g.hw.template && !g.hw.isolated)
            .filter_map(|g| self.backups.iter().filter(|b| b.vmid == g.vmid && b.id.starts_with("vm-")).max_by_key(|b| b.created))
            .filter(|b| b.tested.as_ref().is_none_or(|t| t.at <= limit))
            .min_by_key(|b| (b.tested.as_ref().map_or(0, |t| t.at), b.vmid))
            .map(|b| b.id.clone())
    }

    /// Sınama sonucunu yedeğe yazar; yedek o arada silinmişse sessizce geçer.
    pub fn set_backup_test(&mut self, id: &str, test: BackupTest) {
        if let Some(b) = self.backups.iter_mut().find(|b| b.id == id) {
            b.tested = Some(test);
        }
    }

    /// Yedek silinebilir mi? Kilitliyse ne zamana kadar kilitli olduğunu söyleyen bir hata döner.
    pub fn backup_deletable(&self, id: &str, now: u64) -> Result<(), Error> {
        if let Some(user) = self.guests.iter().find(|g| g.hw.instant.as_deref() == Some(id)) {
            return Err(Error::InvalidSpec(format!(
                "{} {} konuğu bu yedeğin üzerinde çalışıyor (anında kurtarma); önce konuğu yedekten ayırın ya da silin",
                user.vmid, user.name
            )));
        }
        match self.backup(id)?.locked_until {
            Some(until) if until > now => Err(Error::InvalidSpec(format!("yedek {} tarihine kadar kilitli; o güne kadar silinemez", crate::retention::date_label(until)))),
            _ => Ok(()),
        }
    }

    pub fn backup(&self, id: &str) -> Result<&Backup, Error> {
        self.backups.iter().find(|b| b.id == id).ok_or_else(|| Error::BackupNotFound(id.to_string()))
    }

    pub fn backups_of(&self, vmid: VmId) -> impl Iterator<Item = &Backup> {
        self.backups.iter().filter(move |b| b.vmid == vmid)
    }

    /// Yedeği kaydeder (en yeni başta).
    pub fn add_backup(&mut self, mut backup: Backup) {
        // Artımlı yedekte değişen veri önceki yedeklere göre olağan dışıysa yedek işaretlenir.
        if let (None, Some(new_mb)) = (&backup.suspicious, backup.new_mb) {
            let history: Vec<u64> = self.backups.iter().filter(|b| b.vmid == backup.vmid).filter_map(|b| b.new_mb).take(7).collect();
            backup.suspicious = crate::retention::change_anomaly(&history, new_mb, backup.size_mb);
        }
        self.backups.insert(0, backup);
    }

    /// Dış kopyanın durumunu kaydeder; yedek silinmişse sessizce geçer.
    pub fn set_backup_offsite(&mut self, id: &str, offsite: bool) {
        if let Some(b) = self.backups.iter_mut().find(|b| b.id == id) {
            b.offsite = offsite;
            // Bu sürüm her dış kopyayı adına bağlı gönderir.
            b.offsite_bound = offsite;
        }
    }

    pub fn remove_backup(&mut self, id: &str) -> Result<Backup, Error> {
        let at = self.backups.iter().position(|b| b.id == id).ok_or_else(|| Error::BackupNotFound(id.to_string()))?;
        Ok(self.backups.remove(at))
    }

    /// Yerelde silinen yedeği arşive alır: dosyaları kopya hedefinde durmaya devam eder.
    pub fn archive_backup(&mut self, backup: Backup) {
        self.archive.retain(|b| b.id != backup.id);
        self.archive.push(backup);
        self.archive.sort_by_key(|b| std::cmp::Reverse(b.created));
    }

    /// Arşivdeki yedeği yeniden yedek listesine alır. Dosyaları yerelde yoktur: geri yüklenirken kopya
    /// hedefinden iner. Kayıt "elle alınmış" sayılır (otomatik budama onu yeniden silmesin) ve asıl dış
    /// hedefte kopyası olmadığı işaretlenir (orada silinmişti).
    pub fn unarchive_backup(&mut self, id: &str) -> Result<(), Error> {
        let at = self.archive.iter().position(|b| b.id == id).ok_or_else(|| Error::BackupNotFound(id.to_string()))?;
        if self.backups.iter().any(|b| b.id == id) {
            return Err(Error::InvalidSpec(format!("{id} zaten yedek listesinde")));
        }
        let mut backup = self.archive.remove(at);
        (backup.auto, backup.offsite, backup.job, backup.locked_until) = (false, false, None, None);
        let pos = self.backups.iter().position(|b| b.created <= backup.created).unwrap_or(self.backups.len());
        self.backups.insert(pos, backup);
        Ok(())
    }

    /// Saklama süresi dolan arşiv kaydını düşürür.
    pub fn drop_archived(&mut self, id: &str) {
        self.archive.retain(|b| b.id != id);
    }

    /// Geri yüklemede yerine konacak ek diskler: yedekte bulunan ve konuğa hâlâ takılı olanlar.
    /// Yedekten sonra kaldırılmış diskler geri getirilmez, sonradan eklenenlere dokunulmaz; aynı numaraya
    /// sonradan takılmış disk de (kaldırılıp yeniden eklenmiş) başka bir disktir ve üzerine yazılmaz.
    pub fn restore_disks(&self, backup_id: &str) -> Result<Vec<u8>, Error> {
        let backup = self.backup(backup_id)?;
        let guest = self.guest(backup.vmid)?;
        let mut ids = Vec::new();
        for saved in &backup.disks {
            match guest.hw.disks.iter().find(|d| d.id == saved.id) {
                Some(now) if now.serial != saved.serial => {}
                // Sıra numarası tutmayan eski kayıtlarda ayırt edilemez: boyut farklıysa açık hata.
                Some(now) if now.size_gb != saved.size_gb => {
                    return Err(Error::InvalidSpec(format!(
                        "ek disk {} yedek alındıktan sonra değiştirilmiş (yedekte {} GB, şimdi {} GB); yedeği geri yüklemeden önce bu diski kaldırın",
                        saved.id, saved.size_gb, now.size_gb
                    )));
                }
                Some(_) => ids.push(saved.id),
                None => {}
            }
        }
        Ok(ids)
    }

    /// Geri yüklemeden sonra: disk yedekteki haline döndüğü için anlık görüntü listesi boşalır.
    /// `disk_gb`: asıl diskin yedekteki boyutu (0: bilinmiyor). Yedekten sonra büyütülmüş disk geri
    /// yüklenince eski boyutuna döner; kayıt ve depolama kullanımı da buna uydurulur.
    pub fn after_restore(&mut self, vmid: VmId, disk_gb: u64) {
        let Ok(g) = self.guest_mut(vmid) else { return };
        g.snapshots.clear();
        if disk_gb == 0 || g.disk_gb == disk_gb {
            return;
        }
        let (was, storage, node) = (g.disk_gb, g.storage.clone(), g.node.clone());
        g.disk_gb = disk_gb;
        if let Ok(si) = self.storage_index(&storage, &node) {
            self.storages[si].used_gb = self.storages[si].used_gb.saturating_sub(was).saturating_add(disk_gb);
        }
    }
}
