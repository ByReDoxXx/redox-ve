//! Diskler ve ağ kartları: büyütme, depolama taşıma, ekleme ve çıkarma.

use super::*;

impl Cluster {
    /// Disk bu boyuta büyütülebilir mi? (Küçültme veri kaybettirir; desteklenmez.)
    pub fn check_grow(&self, vmid: VmId, new_gb: u64) -> Result<(), Error> {
        let guest = self.guest(vmid)?;
        if new_gb <= guest.disk_gb {
            return Err(Error::InvalidSpec(format!(
                "yeni boyut şimdikinden ({} GB) büyük olmalı; disk küçültülemez",
                guest.disk_gb
            )));
        }
        self.pool_allows_disk(vmid, new_gb - guest.disk_gb)?;
        let storage = self.storage_for(&guest.storage, &guest.node)?;
        let extra = new_gb - guest.disk_gb;
        if extra > storage.free_gb() {
            return Err(Error::InsufficientStorage {
                storage: guest.storage.clone(),
                needed_gb: extra,
                free_gb: storage.free_gb(),
            });
        }
        Ok(())
    }

    pub fn apply_grow(&mut self, vmid: VmId, new_gb: u64) {
        let Ok(g) = self.guest(vmid) else { return };
        let (node, storage, extra) = (g.node.clone(), g.storage.clone(), new_gb.saturating_sub(g.disk_gb));
        if let Ok(si) = self.storage_index(&storage, &node) {
            self.storages[si].used_gb += extra;
        }
        if let Ok(g) = self.guest_mut(vmid) {
            g.disk_gb = new_gb;
        }
    }

    /// Konuğun diskleri `target` depolama alanına taşınabilir mi? Konuk kapalı olmalıdır (`begin_op` denetler).
    /// Şablonun bağlı klonları ve bağlı klonun kendisi disk yoluna bağlı olduğu için taşınmaz.
    pub fn check_storage_move(&self, vmid: VmId, target: &str) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        if g.storage == target {
            return Err(Error::InvalidSpec(format!("konuk zaten {target} alanında")));
        }
        let to = self.storage_for(target, &g.node)?;
        if !to.content.iter().any(|c| c == "disk") {
            return Err(Error::InvalidSpec(format!("{target} alanı konuk diski tutmuyor")));
        }
        self.not_suspended(vmid)?;
        if g.hw.base.is_some() {
            return Err(Error::InvalidSpec("bağlı klon taşınamaz: diski şablonun diskine dayanır".to_string()));
        }
        if self.guests.iter().any(|c| c.hw.base == Some(vmid)) {
            return Err(Error::InvalidSpec("bağlı klonları olan şablon taşınamaz: klonların diskleri bu diskin yerine bağlıdır".to_string()));
        }
        let needed_gb = g.total_disk_gb();
        if needed_gb > to.free_gb() {
            return Err(Error::InsufficientStorage { storage: target.to_string(), needed_gb, free_gb: to.free_gb() });
        }
        Ok(())
    }

    /// Taşıma bitti: konuğun payı eski alandan düşülür, yenisine eklenir.
    pub fn apply_storage_move(&mut self, vmid: VmId, target: &str) {
        let Ok(g) = self.guest(vmid) else { return };
        let (node, from, size) = (g.node.clone(), g.storage.clone(), g.total_disk_gb());
        if let Ok(si) = self.storage_index(&from, &node) {
            self.storages[si].used_gb = self.storages[si].used_gb.saturating_sub(size);
        }
        if let Ok(si) = self.storage_index(target, &node) {
            self.storages[si].used_gb += size;
        }
        if let Ok(g) = self.guest_mut(vmid) {
            g.storage = target.to_string();
        }
    }

    /// Disk ekleme ve çıkarma için ortak ön koşullar: sanal makine, kapalı, kilitsiz.
    pub(super) fn disk_change_allowed(&self, vmid: VmId) -> Result<&Guest, Error> {
        self.device_change_allowed(vmid, false, "disk eklemek ya da çıkarmak")
    }

    /// Aygıt ekleme ve çıkarma kuralı. `hot`: sürücü çalışırken takabiliyorsa çalışan konuk da kabul edilir
    /// (duraklatılmış konuk edilmez: aygıtı konuğun işletim sistemi karşılamalıdır).
    pub(super) fn device_change_allowed(&self, vmid: VmId, hot: bool, what: &str) -> Result<&Guest, Error> {
        let guest = self.guest(vmid)?;
        if guest.kind != GuestKind::Vm {
            return Err(Error::InvalidSpec("ek disk ve ek ağ kartı yalnızca sanal makinelere eklenebilir".to_string()));
        }
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if guest.status != GuestStatus::Stopped && !(hot && guest.status == GuestStatus::Running) {
            return Err(Error::RequiresStopped { vmid, what: what.to_string() });
        }
        self.not_template(vmid)?;
        self.not_suspended(vmid)?;
        Ok(guest)
    }

    /// Yeni bir ek disk kaydeder ve depolamadan yer ayırır; diskin numarasını döndürür.
    pub fn add_disk(&mut self, vmid: VmId, size_gb: u64) -> Result<u8, Error> {
        self.add_disk_with(vmid, size_gb, false)
    }

    /// `add_disk`'in çalışan konuğu da kabul eden hali (sürücü çalışırken takabiliyorsa).
    pub fn add_disk_hot(&mut self, vmid: VmId, size_gb: u64) -> Result<u8, Error> {
        self.add_disk_with(vmid, size_gb, true)
    }

    pub(super) fn add_disk_with(&mut self, vmid: VmId, size_gb: u64, hot: bool) -> Result<u8, Error> {
        let guest = self.device_change_allowed(vmid, hot, "disk eklemek ya da çıkarmak")?;
        if size_gb == 0 {
            return Err(Error::InvalidSpec("disk boyutu en az 1 GB olmalı".to_string()));
        }
        self.pool_allows_disk(vmid, size_gb)?;
        if guest.hw.disks.len() >= crate::hw::MAX_EXTRA_DISKS {
            return Err(Error::InvalidSpec(format!("en fazla {} ek disk eklenebilir", crate::hw::MAX_EXTRA_DISKS)));
        }
        let id = (1..=crate::hw::MAX_EXTRA_DISKS as u8).find(|i| guest.hw.disks.iter().all(|d| d.id != *i)).expect("sınırın altında boş numara vardır");
        let (storage, node) = (guest.storage.clone(), guest.node.clone());
        let si = self.storage_index(&storage, &node)?;
        let free_gb = self.storages[si].free_gb();
        if size_gb > free_gb {
            return Err(Error::InsufficientStorage { storage, needed_gb: size_gb, free_gb });
        }
        self.storages[si].used_gb += size_gb;
        let g = self.guest_mut(vmid)?;
        g.hw.disk_seq += 1;
        g.hw.disks.push(crate::hw::ExtraDisk { id, size_gb, serial: g.hw.disk_seq });
        g.hw.disks.sort_by_key(|d| d.id);
        Ok(id)
    }

    /// Başka sunucudan gelen ek diski numarası ve seri numarasıyla aynen kaydeder (taşıma). Bellek durumuyla
    /// gelen konukta aygıtların yeri kaynaktakiyle aynı olmalıdır; bu yüzden numara yeniden verilmez.
    pub fn add_disk_exact(&mut self, vmid: VmId, disk: &crate::hw::ExtraDisk) -> Result<(), Error> {
        let guest = self.guest(vmid)?;
        if !(1..=crate::hw::MAX_EXTRA_DISKS as u8).contains(&disk.id) || disk.size_gb == 0 || guest.hw.disks.iter().any(|d| d.id == disk.id) {
            return Err(Error::InvalidSpec(format!("ek disk {} bu konuğa eklenemez (numara geçersiz ya da kullanılıyor)", disk.id)));
        }
        let (storage, node) = (guest.storage.clone(), guest.node.clone());
        let si = self.storage_index(&storage, &node)?;
        let free_gb = self.storages[si].free_gb();
        if disk.size_gb > free_gb {
            return Err(Error::InsufficientStorage { storage, needed_gb: disk.size_gb, free_gb });
        }
        self.storages[si].used_gb += disk.size_gb;
        let g = self.guest_mut(vmid)?;
        g.hw.disk_seq = g.hw.disk_seq.max(disk.serial);
        g.hw.disks.push(disk.clone());
        g.hw.disks.sort_by_key(|d| d.id);
        Ok(())
    }

    /// Ek disk kaydını siler ve ayrılan yeri iade eder.
    pub fn remove_disk(&mut self, vmid: VmId, id: u8) -> Result<crate::hw::ExtraDisk, Error> {
        self.disk_change_allowed(vmid)?;
        self.forget_disk(vmid, id)
    }

    /// Disk kaydını ve ayrılan yeri durum denetimi yapmadan geri alır (takılamayan diskin kaydı için).
    pub fn forget_disk(&mut self, vmid: VmId, id: u8) -> Result<crate::hw::ExtraDisk, Error> {
        let guest = self.guest(vmid)?;
        let at = guest.hw.disks.iter().position(|d| d.id == id).ok_or_else(|| Error::InvalidSpec(format!("{vmid} konuğunda {id} numaralı ek disk yok")))?;
        let (storage, node) = (guest.storage.clone(), guest.node.clone());
        let disk = self.guest_mut(vmid)?.hw.disks.remove(at);
        if let Ok(si) = self.storage_index(&storage, &node) {
            self.storages[si].used_gb = self.storages[si].used_gb.saturating_sub(disk.size_gb);
        }
        Ok(disk)
    }

    /// Konuğa ek ağ kartı kaydeder; kartın sırasını (1 ilk ek kart) döndürür. `hot`: çalışan konuk da kabul edilir.
    pub fn add_nic(&mut self, vmid: VmId, network: &str, hot: bool) -> Result<u8, Error> {
        let guest = self.device_change_allowed(vmid, hot, "ağ kartı eklemek")?;
        if crate::hw::parse_nic(network).is_none() {
            return Err(Error::InvalidSpec(format!("geçersiz ağ ayarı: {network}")));
        }
        if guest.hw.nics.len() >= crate::hw::MAX_EXTRA_NICS {
            return Err(Error::InvalidSpec(format!("en fazla {} ek ağ kartı eklenebilir", crate::hw::MAX_EXTRA_NICS)));
        }
        let g = self.guest_mut(vmid)?;
        g.hw.nics.push(network.to_string());
        Ok(g.hw.nics.len() as u8)
    }

    /// Son eklenen ağ kartının kaydını geri alır (takılamayan kart için).
    pub fn forget_nic(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.nics.pop();
        }
    }
}
