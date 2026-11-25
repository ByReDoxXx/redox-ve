//! Askı, şablon ve klonlama.

use super::*;

impl Cluster {
    /// Askıdaki konuğun kayıtlı bellek durumu şimdiki diskine ve donanımına aittir: bunları değiştiren işlemler
    /// reddedilir (yoksa konuk, kaldığı yerden bambaşka bir diskle devam ederdi).
    pub fn not_suspended(&self, vmid: VmId) -> Result<(), Error> {
        if self.guest(vmid)?.hw.suspended {
            return Err(Error::InvalidSpec(format!("{vmid} askıda: önce başlatın ya da askı durumunu atın")));
        }
        Ok(())
    }

    /// Askıya alma tamamlandı: konuk kapalı görünür, bellek durumu kayıtlıdır.
    pub fn after_suspend(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.status = GuestStatus::Stopped;
            g.cpu_pct = 0;
            g.mem_used_mb = 0;
            g.started_at = None;
            g.ip = None;
            g.agent = None;
            g.hw.suspended = true;
        }
    }

    /// Kayıtlı bellek durumu kullanıldı (konuk devam etti) ya da atıldı.
    pub fn clear_suspended(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.suspended = false;
        }
    }

    /// Şablonun diski değişmemelidir (bağlı klonlar ona dayanır): diski değiştiren işlemler bununla reddedilir.
    pub fn not_template(&self, vmid: VmId) -> Result<(), Error> {
        if self.guest(vmid)?.hw.template {
            return Err(Error::InvalidSpec(format!("{vmid} bir şablon: diski değiştirilemez. Önce şablonluktan çıkarın.")));
        }
        Ok(())
    }

    /// `vmid` şablonuna dayanan bağlı klonlar varsa hata verir (`what`: iletideki eylem).
    pub fn no_linked_clones(&self, vmid: VmId, what: &str) -> Result<(), Error> {
        let linked: Vec<String> = self.guests.iter().filter(|g| g.hw.base == Some(vmid)).map(|g| g.vmid.to_string()).collect();
        if linked.is_empty() {
            return Ok(());
        }
        Err(Error::InvalidSpec(format!("{vmid} {what}: bağlı klonları ona dayanıyor ({}). Önce onları silin.", linked.join(", "))))
    }

    /// Kapalı sanal makineyi şablona çevirir ya da şablonluktan çıkarır.
    pub fn set_template(&mut self, vmid: VmId, on: bool) -> Result<(), Error> {
        let guest = self.guest(vmid)?;
        if guest.kind != GuestKind::Vm {
            return Err(Error::InvalidSpec("yalnızca sanal makineler şablon olabilir".to_string()));
        }
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if guest.status != GuestStatus::Stopped {
            return Err(Error::RequiresStopped { vmid, what: "şablona çevirmek".to_string() });
        }
        if on && guest.hw.suspended {
            return Err(Error::InvalidSpec(format!("{vmid} askıda: önce başlatın ya da askı durumunu atın")));
        }
        if on && guest.hw.base.is_some() {
            return Err(Error::InvalidSpec("bağlı klon şablona çevrilemez; önce tam klonunu alın".to_string()));
        }
        if !on {
            self.no_linked_clones(vmid, "şablonluktan çıkarılamaz")?;
        }
        let g = self.guest_mut(vmid)?;
        g.hw.template = on;
        if on {
            // Şablon kendiliğinden açılmaz ve zamanlanmış yedeği olmaz (diski değişmez).
            g.onboot = false;
            g.backup_time = None;
        }
        Ok(())
    }

    /// Kaynağın ayarlarıyla, durdurulmuş yeni bir konuk kaydı ekler ve kimliğini döndürür.
    /// Anlık görüntüler ve port yönlendirmeleri kopyalanmaz (portlar çakışırdı).
    pub fn clone_guest(&mut self, source: VmId, name: &str) -> Result<VmId, Error> {
        self.clone_guest_as(source, name, false)
    }

    /// `linked`: disk kopyalanmaz; klon şablonun diskine dayanır ve yalnızca farkları tutar.
    pub fn clone_guest_as(&mut self, source: VmId, name: &str, linked: bool) -> Result<VmId, Error> {
        let src = self.guest(source)?.clone();
        if linked && !src.hw.template {
            return Err(Error::InvalidSpec("bağlı klon yalnızca şablondan oluşturulur; önce konuğu şablona çevirin".to_string()));
        }
        if !valid_name(name) {
            return Err(Error::InvalidName(name.to_string()));
        }
        // Klon kaynağın havuzunu devralır; kota doluysa klonlanmaz.
        if let Some(pool) = &src.pool {
            self.pool_allows(pool, None, src.cores, src.mem_total_mb, src.total_disk_gb())?;
        }
        let si = self.storage_index(&src.storage, &src.node)?;
        let free_gb = self.storages[si].free_gb();
        let needed_gb = src.total_disk_gb();
        if needed_gb > free_gb {
            return Err(Error::InsufficientStorage { storage: src.storage.clone(), needed_gb, free_gb });
        }
        let vmid = self.next_vmid();
        self.storages[si].used_gb += needed_gb;
        let copy = Guest {
            vmid,
            name: name.to_string(),
            status: GuestStatus::Stopped,
            cpu_pct: 0,
            mem_used_mb: 0,
            ip: None,
            agent: None,
            started_at: None,
            lock: None,
            forwards: Vec::new(),
            snapshots: Vec::new(),
            // Kopya kendi zamanlamasını sizden ister; kaynağınki devralınmaz.
            backup_time: None,
            last_auto_backup: None,
            hw: crate::hw::Hardware { template: false, suspended: false, base: linked.then_some(source), ..src.hw.clone() },
            ..src
        };
        let at = self.guests.partition_point(|g| g.vmid < vmid);
        self.guests.insert(at, copy);
        Ok(vmid)
    }

    /// Yarım kalan klonu geri alır: hedef kayıt silinir, disk payı iade edilir.
    pub fn discard_clone(&mut self, vmid: VmId) {
        let Ok(g) = self.guest(vmid) else { return };
        let (node, storage, disk_gb) = (g.node.clone(), g.storage.clone(), g.total_disk_gb());
        if let Ok(si) = self.storage_index(&storage, &node) {
            self.storages[si].used_gb = self.storages[si].used_gb.saturating_sub(disk_gb);
        }
        self.guests.retain(|g| g.vmid != vmid);
        self.usage.retain(|u| u.vmid != vmid);
    }

    /// İçe aktarılan konuğun kaydını tamamlamak için değiştirilebilir erişim.
    pub fn guest_mut_for_import(&mut self, vmid: VmId) -> Result<&mut Guest, Error> {
        self.guest_mut(vmid)
    }
}
