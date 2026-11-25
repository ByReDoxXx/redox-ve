//! Makine durumu: UEFI değişken deposu ve sanal TPM'in yedeklenmesi.

use super::*;

impl QemuDriver {
    /// Yedeğin yanındaki makine durumu arşivi: UEFI değişken deposu ve sanal TPM durumu.
    pub(super) fn machine_path(&self, id: &str) -> Result<PathBuf, String> {
        Ok(self.backup_path(id)?.with_file_name(format!("{id}.machine.tar")))
    }

    /// Konuğun UEFI değişken deposunu ve sanal TPM durumunu yedeğin yanına arşivler. İkisi de yoksa
    /// arşiv yazılmaz ve `false` döner. Disklere göre çok küçüktür (birkaç MB).
    pub(super) fn save_machine(&self, guest: &Guest, id: &str) -> Result<bool, String> {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        if guest.hw.firmware == redox_core::hw::Firmware::Uefi {
            let secure = guest.hw.secure_boot;
            let vars = self.cfg.efivars_path(guest.vmid, secure);
            if vars.is_file() {
                // Konuk çalışıyorsa dosya QEMU'da açıktır: kilit zorlanmadan (-U) bir kopyası alınır.
                let copy = vars.with_extension("qcow2.backup");
                let _ = std::fs::remove_file(&copy);
                let made = self
                    .img(&["convert".as_ref(), "-U".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-O".as_ref(), "qcow2".as_ref(), vars.as_os_str(), copy.as_os_str()], "UEFI değişkenleri yedeklenemedi")
                    .and_then(|()| std::fs::read(&copy).map_err(|e| format!("UEFI değişkenleri okunamadı: {e}")));
                let _ = std::fs::remove_file(&copy);
                entries.push((if secure { "efivars-secure.qcow2" } else { "efivars.qcow2" }.to_string(), made?));
            }
        }
        if guest.hw.tpm {
            let mut files: Vec<PathBuf> = std::fs::read_dir(self.cfg.tpm_dir(guest.vmid)).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect()).unwrap_or_default();
            files.sort();
            for file in files {
                let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                if !machine_member_ok(&name) || name.ends_with(".lock") {
                    continue;
                }
                entries.push((format!("tpm/{name}"), std::fs::read(&file).map_err(|e| format!("TPM durumu okunamadı ({name}): {e}"))?));
            }
        }
        if entries.is_empty() {
            return Ok(false);
        }
        let path = self.machine_path(id)?;
        let temp = path.with_extension("tar.tmp");
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(name, data)| (name.as_str(), data.as_slice())).collect();
        crate::ova::write_tar(&temp, &refs)
            .and_then(|()| std::fs::rename(&temp, &path))
            .map_err(|e| {
                let _ = std::fs::remove_file(&temp);
                format!("makine durumu arşivi yazılamadı: {e}")
            })?;
        Ok(true)
    }

    /// Yedekteki makine durumunu geri koyar. Arşiv yoksa (eski yedek, BIOS'lu ve TPM'siz konuk) bir şey yapmaz.
    /// Konuğun şimdiki donanımına uymayan parça atlanır: UEFI'den BIOS'a geçmiş konuğa değişken deposu,
    /// TPM'i kaldırılmış konuğa TPM durumu konmaz.
    pub(super) fn load_machine(&self, guest: &Guest, id: &str) -> Result<(), String> {
        let path = self.machine_path(id)?;
        if !path.is_file() {
            return Ok(());
        }
        self.load_machine_from(guest, &path)
    }

    /// `load_machine`'in arşiv yolunu doğrudan alan hali (başka sunucudan indirilen arşiv için).
    pub(super) fn load_machine_from(&self, guest: &Guest, path: &Path) -> Result<(), String> {
        use std::io::{Read, Seek, SeekFrom};
        const MAX_MEMBER: u64 = 64 * 1024 * 1024;
        let mut file = std::fs::File::open(path).map_err(|e| format!("makine durumu arşivi açılamadı: {e}"))?;
        let mut tpm: Vec<(String, Vec<u8>)> = Vec::new();
        for member in crate::ova::members(&mut file)? {
            if member.size > MAX_MEMBER {
                return Err(format!("makine durumu arşivindeki {} beklenmedik ölçüde büyük", member.name));
            }
            let mut data = vec![0u8; member.size as usize];
            file.seek(SeekFrom::Start(member.offset)).and_then(|_| file.read_exact(&mut data)).map_err(|e| format!("makine durumu arşivi okunamadı: {e}"))?;
            match member.name.as_str() {
                name @ ("efivars.qcow2" | "efivars-secure.qcow2") => {
                    let secure = name == "efivars-secure.qcow2";
                    if guest.hw.firmware != redox_core::hw::Firmware::Uefi || guest.hw.secure_boot != secure {
                        continue;
                    }
                    let vars = self.cfg.efivars_path(guest.vmid, secure);
                    let staging = vars.with_extension("qcow2.restore");
                    std::fs::write(&staging, &data).map_err(|e| format!("UEFI değişkenleri yazılamadı: {e}"))?;
                    // Arşiv dış hedeften gelmiş olabilir: başka bir dosyaya bağlanan imaj kabul edilmez.
                    let placed = self.check_standalone(&staging).and_then(|()| std::fs::rename(&staging, &vars).map_err(|e| format!("UEFI değişkenleri yerine konamadı: {e}")));
                    if placed.is_err() {
                        let _ = std::fs::remove_file(&staging);
                    }
                    placed?;
                }
                name => {
                    if let Some(file_name) = name.strip_prefix("tpm/").filter(|n| machine_member_ok(n)) {
                        tpm.push((file_name.to_string(), data));
                    }
                }
            }
        }
        if guest.hw.tpm && !tpm.is_empty() {
            let dir = self.cfg.tpm_dir(guest.vmid);
            let staging = dir.with_extension("restore");
            let _ = std::fs::remove_dir_all(&staging);
            let built = (|| {
                std::fs::create_dir_all(&staging)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))?;
                }
                for (name, data) in &tpm {
                    std::fs::write(staging.join(name), data)?;
                }
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::rename(&staging, &dir)
            })();
            built.map_err(|e: std::io::Error| {
                let _ = std::fs::remove_dir_all(&staging);
                format!("TPM durumu yerine konamadı: {e}")
            })?;
        }
        Ok(())
    }
}
