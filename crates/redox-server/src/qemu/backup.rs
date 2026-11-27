//! Yedek dosyaları: tam, parçalı ve artımlı yedek, dışa aktarma biçimleri.

use super::*;

impl QemuDriver {
    /// Tam yedekte `n` numaralı diskin dosyası.
    pub(super) fn backup_part(&self, id: &str, n: u8) -> Result<PathBuf, String> {
        let main = self.backup_path(id)?;
        Ok(if n == 0 { main } else { self.cfg.backups_dir().join(format!("{id}.disk{n}.qcow2")) })
    }

    /// Diskleri hedef dosyalara döker: çalışan konukta hepsi aynı andaki tutarlı görüntüdür
    /// (tek QMP işlemi), kapalı konukta `qemu-img` ile. `raw`: sıkıştırılmış qcow2 yerine ham imaj.
    /// Dönen değer: konuk çalışıyorsa dosya sistemi donduruldu mu (kapalı konukta `None`).
    pub(super) fn dump(&self, guest: &Guest, parts: &[(PathBuf, PathBuf)], live: bool, raw: bool, mode: Cbt) -> Result<Option<bool>, String> {
        if live {
            #[cfg(unix)]
            {
                return imp::live_backup(self, guest, parts, raw, mode).map(Some);
            }
            #[cfg(not(unix))]
            {
                let _ = (guest, mode);
                return Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string());
            }
        }
        // Kapalı konuk: qemu-img kopyalar; hız sınırı varsa `-r` (bayt/sn) ile uygulanır.
        let rate = (self.cfg.backup_mbps > 0).then(|| (self.cfg.backup_mbps * 1024 * 1024).to_string());
        for (disk, target) in parts {
            if guest.hw.encrypted {
                if raw {
                    return Err("şifreli diskli konukta ham döküm alınmaz".to_string());
                }
                // Dosyanın kopyası şifreli kalır; `qemu-img convert` ise çözüp şifresiz yazardı.
                std::fs::copy(disk, target).map_err(|e| format!("yedek alınamadı: {e}"))?;
                continue;
            }
            let mut args: Vec<&std::ffi::OsStr> = vec!["convert".as_ref()];
            if let Some(rate) = &rate {
                args.extend::<[&std::ffi::OsStr; 2]>(["-r".as_ref(), rate.as_ref()]);
            }
            args.extend::<[&std::ffi::OsStr; 2]>(["-O".as_ref(), if raw { "raw" } else { "qcow2" }.as_ref()]);
            if !raw {
                args.push("-c".as_ref());
            }
            args.extend([disk.as_os_str(), target.as_os_str()]);
            self.img(&args, "yedek alınamadı")?;
        }
        Ok(None)
    }

    /// Artımlı yedek: diskler önce ham dosyalara dökülür, sonra birlikte parça deposuna işlenir.
    /// Ham dosyalar geçicidir.
    pub(super) fn backup_chunks(&self, guest: &Guest, id: &str, live: bool, ids: &[u8]) -> Result<BackupSize, String> {
        const MB: u64 = 1024 * 1024;
        let parts: Vec<(PathBuf, PathBuf)> =
            ids.iter().map(|n| (self.guest_disk(guest, *n), self.cfg.backups_dir().join(format!(".{id}.d{n}.raw.tmp")))).collect();
        for (_, raw) in &parts {
            let _ = std::fs::remove_file(raw);
        }
        // Değişen blok takibi: durum dosyası önce silinir, yalnızca başarılı yedekten sonra yeniden yazılır.
        // Böylece yarıda kalan her deneme, bir sonraki yedeğin diski baştan sona okumasına yol açar.
        let prior = if self.cfg.cbt { self.cbt_state(guest.vmid) } else { None };
        self.cbt_invalidate(guest.vmid);
        #[cfg(unix)]
        if let Some(prior) = prior.filter(|p| live && p.streak < CBT_MAX_STREAK && p.disks == ids && self.store.has(&p.base)) {
            match self.backup_delta(guest, id, &prior.base, ids) {
                Ok(size) => {
                    self.cbt_record(guest.vmid, &CbtState { base: id.to_string(), disks: ids.to_vec(), streak: prior.streak + 1 });
                    return Ok(size);
                }
                // Harita kullanılamıyorsa (eski QEMU, tutarsız harita, bozuk temel) yedek yine alınır: tam okumayla.
                Err(e) => eprintln!("{} için değişen blok takibi kullanılamadı, disk baştan sona okunacak: {e}", guest.vmid),
            }
        }
        #[cfg(not(unix))]
        let _ = prior;
        let mut armed = self.cfg.cbt;
        let dumped = match self.dump(guest, &parts, live, true, if armed { Cbt::Arm } else { Cbt::Off }) {
            // Haritayı sıfırlama isteği reddedildiyse (desteklemeyen QEMU) yedek haritasız alınır.
            Err(e) if armed && live && e.starts_with("yedekleme başlatılamadı") => {
                armed = false;
                for (_, raw) in &parts {
                    let _ = std::fs::remove_file(raw);
                }
                self.dump(guest, &parts, live, true, Cbt::Off)
            }
            other => other,
        };
        if armed && !live {
            // Kapalı konukta harita doğrudan imajda sıfırlanır (konuk kapalı: okuma ile sıfırlama arasında yazma olmaz).
            armed = dumped.is_ok() && parts.iter().all(|(disk, _)| self.arm_offline(disk));
        }
        let stored = dumped.and_then(|quiesced| {
            let mut files = Vec::with_capacity(parts.len());
            for (n, (_, raw)) in ids.iter().zip(&parts) {
                let file = std::fs::File::open(raw).map_err(|e| format!("ham disk açılamadı: {e}"))?;
                let size = file.metadata().map_err(|e| format!("ham disk okunamadı: {e}"))?.len();
                files.push((*n, file, size));
            }
            let mut sources: Vec<(u8, &mut dyn std::io::Read, u64)> = files.iter_mut().map(|(n, file, size)| (*n, file as &mut dyn std::io::Read, *size)).collect();
            self.store.write(id, &mut sources).map(|stats| (stats, quiesced))
        });
        for (_, raw) in &parts {
            let _ = std::fs::remove_file(raw);
        }
        if stored.is_err() {
            // Yarım kalan yedeğin depoya yazdığı parçalar hiçbir dizinde anılmaz: hemen toplanır.
            let _ = self.store.gc();
        }
        let (stats, quiesced) = stored?;
        if armed {
            self.cbt_record(guest.vmid, &CbtState { base: id.to_string(), disks: ids.to_vec(), streak: 0 });
        }
        Ok(BackupSize { size_mb: stats.stored.div_ceil(MB), new_mb: Some(stats.added.div_ceil(MB)), quiesced, cbt: false })
    }

    /// Çalışan konuğun yalnızca son yedekten beri yazılan bloklarını okuyup `base` yedeğinin üzerine işler.
    #[cfg(unix)]
    pub(super) fn backup_delta(&self, guest: &Guest, id: &str, base: &str, ids: &[u8]) -> Result<BackupSize, String> {
        const MB: u64 = 1024 * 1024;
        let dir = self.cfg.backups_dir();
        let parts: Vec<(PathBuf, PathBuf)> = ids.iter().map(|n| (self.guest_disk(guest, *n), dir.join(format!(".{id}.d{n}.delta.tmp")))).collect();
        let raws: Vec<PathBuf> = ids.iter().map(|n| dir.join(format!(".{id}.d{n}.raw.tmp"))).collect();
        let cleanup = || {
            for path in parts.iter().map(|(_, delta)| delta).chain(&raws) {
                let _ = std::fs::remove_file(path);
            }
        };
        cleanup();
        let result = (|| {
            let quiesced = imp::live_backup(self, guest, &parts, false, Cbt::Delta)?;
            let mut files = Vec::with_capacity(ids.len());
            for ((n, (_, delta)), raw) in ids.iter().zip(&parts).zip(&raws) {
                let qcow2: [&std::ffi::OsStr; 2] = ["-f".as_ref(), "qcow2".as_ref()];
                let map = self.img_json(&["map".as_ref(), "--output=json".as_ref(), qcow2[0], qcow2[1], delta.as_os_str()], "değişen bloklar okunamadı")?;
                let ranges = present_ranges(&map).ok_or("bu qemu-img sürümü yazılmış bölgeleri bildirmiyor (present alanı yok)")?;
                let info = self.img_json(&["info".as_ref(), "--output=json".as_ref(), qcow2[0], qcow2[1], delta.as_os_str()], "değişen bloklar okunamadı")?;
                let size = info["virtual-size"].as_u64().ok_or("disk boyutu okunamadı")?;
                self.img(&["convert".as_ref(), qcow2[0], qcow2[1], "-O".as_ref(), "raw".as_ref(), delta.as_os_str(), raw.as_os_str()], "değişen bloklar okunamadı")?;
                let file = std::fs::File::open(raw).map_err(|e| format!("değişen bloklar açılamadı: {e}"))?;
                files.push((*n, size, file, ranges));
            }
            let mut disks: Vec<crate::chunkstore::DeltaDisk<'_>> =
                files.iter_mut().map(|(n, size, file, ranges)| crate::chunkstore::DeltaDisk { id: *n, size: *size, raw: file, ranges: std::mem::take(ranges) }).collect();
            let stats = self.store.write_delta(id, base, &mut disks)?;
            Ok(BackupSize { size_mb: stats.stored.div_ceil(MB), new_mb: Some(stats.added.div_ceil(MB)), quiesced: Some(quiesced), cbt: true })
        })();
        cleanup();
        result
    }

    /// Artımlı yedekteki `disk` numaralı diski (özetleri doğrulayarak) ham disk dosyasına açar.
    pub(super) fn unpack(&self, id: &str, disk: u8, raw: &Path) -> Result<(), String> {
        let mut file = std::fs::File::create(raw).map_err(|e| format!("{} oluşturulamadı: {e}", raw.display()))?;
        self.store.read(id, disk, &mut file).map(|_| ())
    }

    /// Ham diski qcow2'ye çevirir. Biçim açıkça verilir: ham içerik konuğun elindedir ve
    /// biçim tahminine bırakılırsa başka bir imaj gibi yorumlanabilir.
    pub(super) fn raw_to_qcow2(&self, raw: &Path, to: &Path, compress: bool, what: &str) -> Result<(), String> {
        let mut args: Vec<&std::ffi::OsStr> = vec!["convert".as_ref(), "-f".as_ref(), "raw".as_ref(), "-O".as_ref(), "qcow2".as_ref()];
        if compress {
            args.push("-c".as_ref());
        }
        args.extend([raw.as_os_str(), to.as_os_str()]);
        self.img(&args, what)
    }

    /// Konuğun disklerini yedekler (makine durumu ayrıca `save_machine` ile eklenir).
    pub(super) fn backup_disks(&self, guest: &Guest, id: &str) -> Result<BackupSize, String> {
        let target = self.backup_path(id)?;
        std::fs::create_dir_all(self.cfg.backups_dir()).map_err(|e| format!("yedek dizini oluşturulamadı: {e}"))?;
        if target.exists() || self.store.has(id) {
            return Err(format!("{} zaten var", target.display()));
        }
        #[cfg(unix)]
        let live = self.alive(guest.vmid);
        #[cfg(not(unix))]
        let live = false;
        let ids = disk_ids(guest);
        if self.cfg.incremental_backups && !guest.hw.encrypted {
            return self.backup_chunks(guest, id, live, &ids);
        }
        // Her disk ayrı dosya: asıl disk `<yedek>.qcow2`, ek diskler `<yedek>.diskN.qcow2`. Önce geçici
        // adla yazılır: yarıda kesilen yedek, geçerli bir yedek dosyası gibi görünmez.
        let mut finals = Vec::with_capacity(ids.len());
        let mut parts = Vec::with_capacity(ids.len());
        for n in &ids {
            let target = self.backup_part(id, *n)?;
            let name = target.file_name().and_then(|f| f.to_str()).unwrap_or_default().to_string();
            parts.push((self.guest_disk(guest, *n), target.with_file_name(format!(".{name}.tmp"))));
            finals.push(target);
        }
        let placed = self.dump(guest, &parts, live, false, Cbt::Off).and_then(|quiesced| {
            parts.iter().zip(&finals).try_for_each(|((_, temp), target)| std::fs::rename(temp, target).map_err(|e| format!("yedek dosyası yerine konamadı: {e}")))?;
            Ok(quiesced)
        });
        let quiesced = match placed {
            Ok(quiesced) => quiesced,
            Err(e) => {
                for path in parts.iter().map(|(_, temp)| temp).chain(&finals) {
                    let _ = std::fs::remove_file(path);
                }
                return Err(e);
            }
        };
        let bytes: u64 = finals.iter().map(|part| std::fs::metadata(part).map(|m| m.len()).unwrap_or(0)).sum();
        Ok(BackupSize { quiesced, ..BackupSize::full(bytes.div_ceil(1024 * 1024)) })
    }

    /// Yedek kimliği dosya adına dönüşür; yol kaçışına karşı yalnızca güvenli karakterler.
    pub(super) fn backup_path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_backup_id(id) {
            return Err(format!("geçersiz yedek kimliği: {id}"));
        }
        Ok(self.cfg.backups_dir().join(format!("{id}.qcow2")))
    }
}
