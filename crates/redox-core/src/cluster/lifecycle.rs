//! Konuk oluşturma, silme, güç işlemleri ve görevlerin tamamlanması.

use super::*;

impl Cluster {
    /// Silinebilir mi? (durdurulmuş ve kilitsiz). Sürücü kaynakları silmeden önce sorulur.
    pub fn check_deletable(&self, vmid: VmId) -> Result<&Guest, Error> {
        let guest = self.guest(vmid)?;
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if guest.status != GuestStatus::Stopped {
            return Err(Error::NotStopped(vmid));
        }
        if self.protection(vmid).is_some() {
            return Err(Error::InvalidSpec(format!("{vmid} hata toleransı açıkken silinemez; önce hata toleransını kapatın")));
        }
        // Sürücü diskleri bu denetimden hemen sonra siler: bağlı klonların dayandığı şablon burada korunmalı.
        self.no_linked_clones(vmid, "silinemez")?;
        Ok(guest)
    }

    /// Sürücü yeni konuğun kaynaklarını hazırlayamadıysa oluşturmayı geri alır:
    /// konuk silinir, disk payı iade edilir, görev başarısız işaretlenir.
    pub fn abort_create(&mut self, vmid: VmId, message: &str, now: u64) {
        let Ok(guest) = self.guest(vmid) else { return };
        let (node, storage, disk_gb) = (guest.node.clone(), guest.storage.clone(), guest.disk_gb);
        if let Ok(si) = self.storage_index(&storage, &node) {
            self.storages[si].used_gb = self.storages[si].used_gb.saturating_sub(disk_gb);
        }
        self.guests.retain(|g| g.vmid != vmid);
        self.usage.retain(|u| u.vmid != vmid);
        let suffix = format!(" create {vmid}");
        if let Some(task) = self.tasks.iter_mut().find(|t| t.vmid == Some(vmid) && t.command.ends_with(&suffix)) {
            task.status = TaskStatus::Failed;
            task.message = Some(message.to_string());
            task.ended = Some(now);
            task.vmid = None;
        }
    }

    /// Yeni konuğu doğrular ve durdurulmuş halde ekler.
    pub fn create_guest(&mut self, spec: &GuestSpec, user: &str, now: u64) -> Result<VmId, Error> {
        let node = self.node(&spec.node)?;
        if node.status != NodeStatus::Online {
            return Err(Error::NodeOffline(spec.node.clone()));
        }
        let (node_cores, node_mem) = (node.cores, node.mem_total_mb);

        let vmid = spec.vmid.unwrap_or_else(|| self.next_vmid());
        if !(VMID_MIN..=VMID_MAX).contains(&vmid) {
            return Err(Error::InvalidVmid(vmid));
        }
        if self.guests.iter().any(|g| g.vmid == vmid) {
            return Err(Error::VmidTaken(vmid));
        }
        if !valid_name(&spec.name) {
            return Err(Error::InvalidName(spec.name.clone()));
        }
        if spec.cores == 0 || spec.cores > node_cores {
            return Err(Error::InvalidSpec(format!(
                "çekirdek sayısı 1 ile {node_cores} arasında olmalı ({} düğümünün çekirdek sayısı)",
                spec.node
            )));
        }
        let min_mem = spec.kind.min_memory_mb();
        if spec.memory_mb < min_mem || spec.memory_mb > node_mem {
            return Err(Error::InvalidSpec(format!(
                "bellek {min_mem} MB ile {node_mem} MB arasında olmalı"
            )));
        }
        if spec.disk_gb == 0 {
            return Err(Error::InvalidSpec("disk boyutu en az 1 GB olmalı".to_string()));
        }
        if !valid_bridge(&spec.bridge) {
            return Err(Error::InvalidSpec(format!("geçersiz ağ ayarı: {}", spec.bridge)));
        }
        if let Some(iso) = &spec.iso {
            if spec.kind != GuestKind::Vm {
                return Err(Error::InvalidSpec("ISO yalnızca sanal makinelere takılabilir".to_string()));
            }
            if !valid_iso_name(iso) {
                return Err(Error::InvalidSpec(format!("geçersiz ISO adı: {iso}")));
            }
        }
        spec.hw.validate(spec.kind).map_err(Error::InvalidSpec)?;
        if spec.hw.vlan > 0 && spec.bridge == crate::model::NAT {
            return Err(Error::InvalidSpec("VLAN etiketi yalnızca köprüye bağlı kartta kullanılır; birinci kart NAT'ta".to_string()));
        }
        if !spec.hw.net_limit_applies(&spec.bridge) {
            return Err(Error::InvalidSpec("ağ hız sınırı yalnızca köprüye bağlı kartlarda uygulanır; bu konuğun bütün kartları NAT'ta".to_string()));
        }
        let si = self.storage_index(&spec.storage, &spec.node)?;
        let free_gb = self.storages[si].free_gb();
        if spec.disk_gb > free_gb {
            return Err(Error::InsufficientStorage {
                storage: spec.storage.clone(),
                needed_gb: spec.disk_gb,
                free_gb,
            });
        }

        self.storages[si].used_gb += spec.disk_gb;
        let guest = Guest {
            vmid,
            name: spec.name.clone(),
            kind: spec.kind,
            node: spec.node.clone(),
            status: GuestStatus::Stopped,
            cores: spec.cores,
            cpu_pct: 0,
            mem_used_mb: 0,
            mem_total_mb: spec.memory_mb,
            disk_gb: spec.disk_gb,
            storage: spec.storage.clone(),
            os: spec.os.clone(),
            ip: None,
            agent: None,
            bridge: spec.bridge.clone(),
            onboot: spec.onboot,
            started_at: None,
            lock: None,
            iso: spec.iso.clone(),
            tags: Vec::new(),
            forwards: Vec::new(),
            snapshots: Vec::new(),
            backup_time: None,
            backup_keep: DEFAULT_KEEP,
            last_auto_backup: None,
            pool: None,
            backup_gfs: crate::retention::Gfs::default(),
            // Şablonluk ve bağlı klon ilişkisi yalnızca kendi işlemleriyle kurulur; istekle verilemez.
            hw: crate::hw::Hardware { template: false, base: None, suspended: false, disk_seq: 0, ..spec.hw.clone() },
        };
        let at = self.guests.partition_point(|g| g.vmid < vmid);
        self.guests.insert(at, guest);
        self.push_task(
            &spec.node,
            user,
            format!("{} create {vmid}", spec.kind),
            TaskStatus::Ok,
            now,
            Some(vmid),
            None,
        );
        Ok(vmid)
    }

    /// Durdurulmuş bir konuğu siler ve diskini depolamaya geri verir.
    pub fn delete_guest(&mut self, vmid: VmId, user: &str, now: u64) -> Result<(), Error> {
        // Silinebilirlik kuralları tek yerdedir; sürücü de diskleri silmeden önce aynı denetimi kullanır.
        let guest = self.check_deletable(vmid)?;
        let (kind, node, storage, disk_gb) =
            (guest.kind, guest.node.clone(), guest.storage.clone(), guest.total_disk_gb());
        if let Ok(si) = self.storage_index(&storage, &node) {
            self.storages[si].used_gb = self.storages[si].used_gb.saturating_sub(disk_gb);
        }
        self.guests.retain(|g| g.vmid != vmid);
        self.usage.retain(|u| u.vmid != vmid);
        self.push_task(&node, user, format!("{kind} destroy {vmid}"), TaskStatus::Ok, now, None, None);
        Ok(())
    }

    /// Eylemi doğrular, konuğu kilitler ve süren bir görev açar.
    /// Durum değişikliği `complete_task` çağrılınca uygulanır.
    pub fn begin_action(
        &mut self,
        vmid: VmId,
        action: Action,
        user: &str,
        now: u64,
    ) -> Result<TaskId, Error> {
        let guest = self.guest(vmid)?;
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if guest.hw.template {
            return Err(Error::InvalidSpec(format!("{vmid} bir şablon: başlatılamaz. Ondan bir klon oluşturun ya da şablonluktan çıkarın.")));
        }
        if action.target(guest.status).is_none() {
            return Err(Error::InvalidTransition {
                vmid,
                action,
                status: guest.status,
            });
        }
        let (kind, node) = (guest.kind, guest.node.clone());
        if self.node(&node)?.status != NodeStatus::Online {
            return Err(Error::NodeOffline(node));
        }
        if action == Action::Start {
            self.check_reserve(vmid, guest.hw.mem_reserve_mb)?;
        }
        // Sıfırdan açılış (askıdan dönüş değil): çalışırken eklenmiş bellek artık açılış belleğinin parçasıdır,
        // bellek yuvaları boşalır. Askıdaki konuk, askıya alındığı parçalarla dönmelidir.
        let fold = action == Action::Start && !guest.hw.suspended && !guest.hw.dimms.is_empty();
        // Yeni süreç kayıttaki kalıplarla kurulur.
        let remount = action == Action::Start && !guest.hw.suspended && guest.hw.media_changed;
        if fold {
            self.guest_mut(vmid)?.hw.dimms.clear();
        }
        if remount {
            self.guest_mut(vmid)?.hw.media_changed = false;
        }
        // Başka sunucuya taşınmış konuğun burada kalan kopyası elle başlatılıyorsa artık asıl konuk odur:
        // "geride kalan kopya" işareti kalkar (geri taşıma onu bayat sayıp yerine geçmesin).
        if action == Action::Start {
            self.guest_mut(vmid)?.tags.retain(|t| t != crate::MIGRATED_TAG);
        }
        let id = self.push_task(
            &node,
            user,
            format!("{kind} {action} {vmid}"),
            TaskStatus::Running,
            now,
            Some(vmid),
            Some(action),
        );
        self.guest_mut(vmid)?.lock = Some(id);
        Ok(id)
    }

    /// Süren görevi kapatır. `outcome` başarılıysa konuğun durumu değişir;
    /// değilse konuk olduğu gibi kalır ve hata görev kaydına yazılır.
    pub fn complete_task(
        &mut self,
        task_id: TaskId,
        outcome: Result<(), String>,
        now: u64,
    ) -> Result<(), Error> {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id && t.status == TaskStatus::Running)
            .ok_or(Error::TaskNotFound(task_id))?;
        task.ended = Some(now);
        let (vmid, action) = (task.vmid, task.action);
        match &outcome {
            Ok(()) => task.status = TaskStatus::Ok,
            Err(message) => {
                task.status = TaskStatus::Failed;
                task.message = Some(message.clone());
            }
        }

        let Some(vmid) = vmid else { return Ok(()) };
        // Konuk bu arada silinmiş olamaz (kilitliyken silinemez), yine de panik yerine geç.
        let Ok(guest) = self.guest_mut(vmid) else { return Ok(()) };
        if guest.lock == Some(task_id) {
            guest.lock = None;
        }
        let (Ok(()), Some(action)) = (outcome, action) else { return Ok(()) };
        let Some(next) = action.target(guest.status) else { return Ok(()) };
        guest.status = next;
        match action {
            Action::Start | Action::Reboot => {
                guest.started_at = Some(now);
                guest.cpu_pct = 8;
                guest.mem_used_mb = guest.mem_total_mb * 3 / 10;
            }
            Action::Stop => {
                guest.started_at = None;
                guest.cpu_pct = 0;
                guest.mem_used_mb = 0;
            }
            Action::Pause => guest.cpu_pct = 0,
            Action::Resume => guest.cpu_pct = 8,
        }
        Ok(())
    }
}

impl Cluster {
    /// Bakım kipinde konuk başlatılamaz.
    pub fn check_not_in_maintenance(&self) -> Result<(), Error> {
        if self.maintenance {
            return Err(Error::InvalidSpec("sunucu bakım kipinde: konuk başlatılamaz. Bakımı bitirin (redox maintenance off)".to_string()));
        }
        Ok(())
    }

    /// Bakım için kapatılacak konuklar: çalışanlar, açılış sırasının tersine (en son açılan en önce kapanır).
    /// Görevi süren (kilitli) konuklar listeye girmez.
    pub fn maintenance_drain_plan(&self) -> Vec<VmId> {
        let mut running: Vec<&Guest> = self.guests.iter().filter(|g| g.status != GuestStatus::Stopped && g.lock.is_none()).collect();
        running.sort_by_key(|g| (g.hw.boot_order == 0, g.hw.boot_order, g.vmid));
        running.iter().rev().map(|g| g.vmid).collect()
    }

    /// Bakım bitti: yeniden başlatılacak konuklar (hâlâ var olan ve kapalı duranlar), açılış sırasıyla.
    pub fn maintenance_finish(&mut self) -> Vec<VmId> {
        self.maintenance = false;
        let mut back: Vec<&Guest> = std::mem::take(&mut self.maintenance_resume).into_iter().filter_map(|vmid| self.guest(vmid).ok()).filter(|g| g.status == GuestStatus::Stopped && !g.hw.template).collect();
        back.sort_by_key(|g| (g.hw.boot_order == 0, g.hw.boot_order, g.vmid));
        back.iter().map(|g| g.vmid).collect()
    }
}
