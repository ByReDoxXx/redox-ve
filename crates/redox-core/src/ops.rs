//! Konuk üzerinde yaşam döngüsü dışındaki işlemler: düzenleme, anlık görüntü,
//! disk büyütme, klonlama ve yedekleme. Kurallar burada; dosya işleri sürücüde.

use crate::cluster::Cluster;
use crate::error::Error;
use crate::model::*;

impl Cluster {
    /// Konuğu kilitleyip süren bir görev açar (anlık görüntü, yedek, klon gibi uzun işler için).
    /// `need_stopped` doluysa konuk durdurulmuş olmalıdır; değer hata iletisinde işlemi adlandırır.
    /// Görev `complete_task` ile kapatılır.
    pub fn begin_op(
        &mut self,
        vmid: VmId,
        command: String,
        user: &str,
        now: u64,
        need_stopped: Option<&str>,
    ) -> Result<TaskId, Error> {
        let guest = self.guest(vmid)?;
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if let Some(what) = need_stopped {
            if guest.status != GuestStatus::Stopped {
                return Err(Error::RequiresStopped { vmid, what: what.to_string() });
            }
        }
        let node = guest.node.clone();
        if self.node(&node)?.status != NodeStatus::Online {
            return Err(Error::NodeOffline(node));
        }
        let id = self.push_task(&node, user, command, TaskStatus::Running, now, Some(vmid), None);
        self.guest_mut(vmid)?.lock = Some(id);
        Ok(id)
    }

    /// İkincil bir konuğu (klon hedefi) aynı görevle kilitler.
    pub fn lock_with(&mut self, vmid: VmId, task: TaskId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.lock = Some(task);
        }
    }

    /// `task` tarafından kilitlenmiş konuğun kilidini açar (klon hedefi gibi ikincil konuklar için).
    pub fn unlock(&mut self, vmid: VmId, task: TaskId) {
        if let Ok(g) = self.guest_mut(vmid) {
            if g.lock == Some(task) {
                g.lock = None;
            }
        }
    }

    /// Konuğun ayarlarını değiştirir. Donanımı etkileyen alanlar (çekirdek, bellek, kalıp,
    /// port yönlendirme) yalnızca konuk durmuşken; ad, etiket, açılış ayarı her zaman değişir.
    pub fn update_guest(&mut self, vmid: VmId, patch: &GuestPatch, user: &str, now: u64) -> Result<(), Error> {
        let guest = self.guest(vmid)?;
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        let node = self.node(&guest.node)?;
        let hardware = patch.cores.is_some_and(|c| c != guest.cores)
            || patch.memory_mb.is_some_and(|m| m != guest.mem_total_mb)
            || patch.iso.as_ref().is_some_and(|i| i.as_str() != guest.iso.as_deref().unwrap_or(""))
            || patch.forwards.as_ref().is_some_and(|f| *f != guest.forwards)
            || patch.bridge.as_ref().is_some_and(|b| *b != guest.bridge)
            || patch.firmware.is_some_and(|f| f != guest.hw.firmware)
            || patch.secure_boot.is_some_and(|s| s != guest.hw.secure_boot)
            || patch.tpm.is_some_and(|t| t != guest.hw.tpm)
            || patch.nics.as_ref().is_some_and(|n| *n != guest.hw.nics)
            || patch.usb.as_ref().is_some_and(|u| *u != guest.hw.usb)
            || patch.pci.as_ref().is_some_and(|p| *p != guest.hw.pci)
            || patch.disk_mbps.is_some_and(|v| v != guest.hw.disk_mbps)
            || patch.disk_iops.is_some_and(|v| v != guest.hw.disk_iops)
            || patch.net_mbps.is_some_and(|v| v != guest.hw.net_mbps)
            || patch.vlan.is_some_and(|v| v != guest.hw.vlan)
            || patch.shares.as_ref().is_some_and(|s| *s != guest.hw.shares)
            || patch.apply_cloud_init(&mut guest.hw.clone())
            || patch.cpu_type.as_ref().is_some_and(|c| *c != guest.hw.cpu_type)
            || patch.balloon.is_some_and(|b| b != guest.hw.balloon)
            || patch.nested.is_some_and(|n| n != guest.hw.nested);
        if hardware && guest.status != GuestStatus::Stopped {
            return Err(Error::RequiresStopped { vmid, what: "donanım ayarlarını değiştirmek".to_string() });
        }
        if hardware {
            // Askıdaki konuğun bellek durumu şimdiki donanımı bekler.
            self.not_suspended(vmid)?;
        }
        if let Some(name) = &patch.name {
            if !valid_name(name) {
                return Err(Error::InvalidName(name.clone()));
            }
        }
        if let Some(cores) = patch.cores {
            if cores == 0 || cores > node.cores {
                return Err(Error::InvalidSpec(format!("çekirdek sayısı 1 ile {} arasında olmalı", node.cores)));
            }
        }
        if let Some(mem) = patch.memory_mb {
            let min = guest.kind.min_memory_mb();
            if mem < min || mem > node.mem_total_mb {
                return Err(Error::InvalidSpec(format!("bellek {min} MB ile {} MB arasında olmalı", node.mem_total_mb)));
            }
        }
        if let Some(iso) = patch.iso.as_deref().filter(|i| !i.is_empty()) {
            if guest.kind != GuestKind::Vm {
                return Err(Error::InvalidSpec("ISO yalnızca sanal makinelere takılabilir".to_string()));
            }
            if !valid_iso_name(iso) {
                return Err(Error::InvalidSpec(format!("geçersiz ISO adı: {iso}")));
            }
        }
        if let Some(bridge) = patch.bridge.as_deref().filter(|b| !valid_bridge(b)) {
            return Err(Error::InvalidSpec(format!("geçersiz ağ ayarı: {bridge}")));
        }
        if let Some(time) = patch.backup_time.as_deref().filter(|t| !t.is_empty() && parse_time(t).is_none()) {
            return Err(Error::InvalidSpec(format!("geçersiz yedek saati \"{time}\"; SS:DD biçiminde yazın, örn. 03:30")));
        }
        if patch.rpo_hours.is_some_and(|h| h > crate::hw::MAX_RPO_HOURS) {
            return Err(Error::InvalidSpec(format!("yedek yaşı hedefi en fazla {} saat olabilir (0: genel ayar)", crate::hw::MAX_RPO_HOURS)));
        }
        if patch.backup_keep.is_some_and(|k| k == 0 || k > MAX_KEEP) {
            return Err(Error::InvalidSpec(format!("saklanacak yedek sayısı 1 ile {MAX_KEEP} arasında olmalı")));
        }
        if [patch.backup_weekly, patch.backup_monthly, patch.backup_yearly].iter().flatten().any(|n| *n > crate::retention::MAX_GFS) {
            return Err(Error::InvalidSpec(format!("haftalık, aylık ve yıllık yedek sayıları en fazla {} olabilir", crate::retention::MAX_GFS)));
        }
        if patch.test_port.is_some() || patch.test_cmd.is_some() || patch.nested.is_some() || patch.nic_guard.is_some() || patch.firewall.is_some() || patch.fw_rules.is_some() || patch.firmware.is_some() || patch.secure_boot.is_some() || patch.tpm.is_some() || patch.nics.is_some() || patch.usb.is_some() || patch.pci.is_some() || patch.boot_order.is_some() || patch.boot_delay.is_some() || patch.schedules.is_some() || patch.disk_mbps.is_some() || patch.disk_iops.is_some() || patch.net_mbps.is_some() || patch.vlan.is_some() || patch.shares.is_some() || patch.bridge.is_some() || patch.apply_cloud_init(&mut guest.hw.clone()) || patch.cpu_type.is_some() || patch.balloon.is_some() || patch.backup_pre.is_some() || patch.backup_post.is_some() {
            let mut hw = guest.hw.clone();
            hw.backup_pre = patch.backup_pre.clone().map(|c| c.trim().to_string()).unwrap_or(hw.backup_pre);
            hw.backup_post = patch.backup_post.clone().map(|c| c.trim().to_string()).unwrap_or(hw.backup_post);
            hw.cpu_type = patch.cpu_type.clone().unwrap_or(hw.cpu_type);
            hw.balloon = patch.balloon.unwrap_or(hw.balloon);
            hw.nic_guard = patch.nic_guard.unwrap_or(hw.nic_guard);
            hw.nested = patch.nested.unwrap_or(hw.nested);
            hw.test_port = patch.test_port.unwrap_or(hw.test_port);
            hw.test_cmd = patch.test_cmd.clone().map(|c| c.trim().to_string()).unwrap_or(hw.test_cmd);
            patch.apply_cloud_init(&mut hw);
            hw.nics = patch.nics.clone().unwrap_or(hw.nics);
            hw.usb = patch.usb.clone().unwrap_or(hw.usb);
            hw.pci = patch.pci.clone().unwrap_or(hw.pci);
            hw.firmware = patch.firmware.unwrap_or(hw.firmware);
            hw.secure_boot = patch.secure_boot.unwrap_or(hw.secure_boot);
            hw.tpm = patch.tpm.unwrap_or(hw.tpm);
            hw.firewall = patch.firewall.unwrap_or(hw.firewall);
            hw.fw_rules = patch.fw_rules.clone().unwrap_or(hw.fw_rules);
            hw.boot_order = patch.boot_order.unwrap_or(hw.boot_order);
            hw.boot_delay = patch.boot_delay.unwrap_or(hw.boot_delay);
            hw.schedules = patch.schedules.clone().unwrap_or(hw.schedules);
            hw.disk_mbps = patch.disk_mbps.unwrap_or(hw.disk_mbps);
            hw.disk_iops = patch.disk_iops.unwrap_or(hw.disk_iops);
            hw.net_mbps = patch.net_mbps.unwrap_or(hw.net_mbps);
            hw.vlan = patch.vlan.unwrap_or(hw.vlan);
            hw.shares = patch.shares.clone().unwrap_or(hw.shares);
            hw.validate(guest.kind).map_err(Error::InvalidSpec)?;
            if hw.vlan > 0 && patch.bridge.as_deref().unwrap_or(&guest.bridge) == NAT {
                return Err(Error::InvalidSpec("VLAN etiketi yalnızca köprüye bağlı kartta kullanılır; birinci kart NAT'ta. Etiketi 0 yapın ya da kartı köprüye bağlayın".to_string()));
            }
            if !hw.net_limit_applies(patch.bridge.as_deref().unwrap_or(&guest.bridge)) {
                return Err(Error::InvalidSpec("ağ hız sınırı yalnızca köprüye bağlı kartlarda uygulanır; bu konuğun bütün kartları NAT'ta. Sınırı 0 yapın ya da bir kartı köprüye bağlayın".to_string()));
            }
        }
        // Havuz kotası: konuk havuza giriyorsa ya da havuzdayken çekirdeği/belleği değişiyorsa denetlenir.
        let pool_after = match &patch.pool {
            Some(name) => Some(name.as_str()).filter(|n| !n.is_empty()),
            None => guest.pool.as_deref(),
        };
        if let Some(pool) = pool_after {
            if patch.pool.is_some() || patch.cores.is_some() || patch.memory_mb.is_some() {
                self.pool_allows(pool, Some(vmid), patch.cores.unwrap_or(guest.cores), patch.memory_mb.unwrap_or(guest.mem_total_mb), guest.total_disk_gb())?;
            }
        }
        if let Some(tags) = &patch.tags {
            if tags.len() > MAX_TAGS {
                return Err(Error::InvalidSpec(format!("en fazla {MAX_TAGS} etiket eklenebilir")));
            }
            if let Some(bad) = tags.iter().find(|t| !valid_tag(t)) {
                return Err(Error::InvalidSpec(format!(
                    "geçersiz etiket \"{bad}\": 1-24 karakter; küçük harf, rakam ve tire kullanın"
                )));
            }
        }
        if let Some(forwards) = &patch.forwards {
            if forwards.len() > MAX_FORWARDS {
                return Err(Error::InvalidSpec(format!("en fazla {MAX_FORWARDS} port yönlendirmesi eklenebilir")));
            }
            for (i, f) in forwards.iter().enumerate() {
                if f.host_port < 1024 || f.guest_port == 0 {
                    return Err(Error::InvalidSpec(format!(
                        "geçersiz yönlendirme {}:{}; sunucu portu 1024-65535, konuk portu 1-65535 olmalı",
                        f.host_port, f.guest_port
                    )));
                }
                let elsewhere = self.guests.iter().any(|g| g.vmid != vmid && g.forwards.iter().any(|o| o.host_port == f.host_port));
                if elsewhere || forwards[..i].iter().any(|o| o.host_port == f.host_port) {
                    return Err(Error::PortTaken(f.host_port));
                }
            }
        }

        let (kind, node_id) = (guest.kind, guest.node.clone());
        let g = self.guest_mut(vmid)?;
        if let Some(name) = &patch.name {
            g.name = name.clone();
        }
        if let Some(cores) = patch.cores {
            g.cores = cores;
        }
        if let Some(mem) = patch.memory_mb {
            g.mem_total_mb = mem;
            g.mem_used_mb = g.mem_used_mb.min(mem);
        }
        if let Some(os) = &patch.os {
            g.os = os.clone();
        }
        if let Some(iso) = &patch.iso {
            g.iso = Some(iso.clone()).filter(|i| !i.is_empty());
        }
        if let Some(onboot) = patch.onboot {
            g.onboot = onboot;
        }
        if let Some(tags) = &patch.tags {
            let mut tags = tags.clone();
            tags.sort();
            tags.dedup();
            g.tags = tags;
        }
        if let Some(forwards) = &patch.forwards {
            g.forwards = forwards.clone();
        }
        if let Some(bridge) = &patch.bridge {
            g.bridge = bridge.clone();
        }
        if let Some(time) = &patch.backup_time {
            g.backup_time = Some(time.clone()).filter(|t| !t.is_empty());
        }
        if let Some(keep) = patch.backup_keep {
            g.backup_keep = keep;
        }
        if let Some(hours) = patch.rpo_hours {
            g.hw.rpo_hours = hours;
        }
        if let Some(port) = patch.test_port {
            g.hw.test_port = port;
        }
        if let Some(command) = &patch.test_cmd {
            g.hw.test_cmd = command.trim().to_string();
        }
        if let Some(command) = &patch.backup_pre {
            g.hw.backup_pre = command.trim().to_string();
        }
        if let Some(command) = &patch.backup_post {
            g.hw.backup_post = command.trim().to_string();
        }
        g.backup_gfs.weekly = patch.backup_weekly.unwrap_or(g.backup_gfs.weekly);
        g.backup_gfs.monthly = patch.backup_monthly.unwrap_or(g.backup_gfs.monthly);
        g.backup_gfs.yearly = patch.backup_yearly.unwrap_or(g.backup_gfs.yearly);
        if let Some(firewall) = patch.firewall {
            g.hw.firewall = firewall;
        }
        if let Some(rules) = &patch.fw_rules {
            g.hw.fw_rules = rules.clone();
        }
        if let Some(firmware) = patch.firmware {
            g.hw.firmware = firmware;
        }
        if let Some(secure) = patch.secure_boot {
            g.hw.secure_boot = secure;
        }
        if let Some(tpm) = patch.tpm {
            g.hw.tpm = tpm;
        }
        if let Some(nics) = &patch.nics {
            g.hw.nics = nics.clone();
        }
        if let Some(usb) = &patch.usb {
            g.hw.usb = usb.clone();
        }
        if let Some(pci) = &patch.pci {
            g.hw.pci = pci.clone();
        }
        if let Some(v) = patch.disk_mbps {
            g.hw.disk_mbps = v;
        }
        if let Some(v) = patch.disk_iops {
            g.hw.disk_iops = v;
        }
        if let Some(v) = patch.net_mbps {
            g.hw.net_mbps = v;
        }
        if let Some(v) = patch.vlan {
            g.hw.vlan = v;
        }
        if let Some(v) = patch.restart {
            g.hw.restart = v;
        }
        patch.apply_cloud_init(&mut g.hw);
        if let Some(pool) = &patch.pool {
            g.pool = Some(pool.clone()).filter(|p| !p.is_empty());
        }
        if let Some(cpu) = &patch.cpu_type {
            g.hw.cpu_type = cpu.clone();
        }
        if let Some(balloon) = patch.balloon {
            g.hw.balloon = balloon;
        }
        if let Some(guard) = patch.nic_guard {
            g.hw.nic_guard = guard;
        }
        if let Some(nested) = patch.nested {
            g.hw.nested = nested;
        }
        // Balon kaldırıldıysa ya da bellek hedefin altına indiyse hedef sıfırlanır (konuk belleğin tamamını kullanır).
        if !g.hw.balloon || g.hw.balloon_mb >= g.mem_total_mb {
            g.hw.balloon_mb = 0;
        }
        // Dinamik bellek balonsuz çalışmaz; bellek alt sınırın altına indirildiyse de kapanır.
        if !g.hw.balloon || g.hw.mem_min_mb > g.mem_total_mb {
            g.hw.mem_min_mb = 0;
        }
        if let Some(shares) = &patch.shares {
            g.hw.shares = shares.clone();
        }
        if let Some(order) = patch.boot_order {
            g.hw.boot_order = order;
        }
        if let Some(delay) = patch.boot_delay {
            g.hw.boot_delay = delay;
        }
        if let Some(schedules) = &patch.schedules {
            // Değişmeyen satır bugün çalıştıysa kaydı korunur: liste kaydedilince aynı gün yeniden çalışmasın.
            let old = std::mem::take(&mut g.hw.schedules);
            g.hw.schedules = schedules
                .iter()
                .map(|s| {
                    let last_run = old.iter().find(|o| (o.action, &o.time, o.days) == (s.action, &s.time, s.days)).and_then(|o| o.last_run.clone());
                    crate::hw::Schedule { last_run, ..s.clone() }
                })
                .collect();
        }
        self.push_task(&node_id, user, format!("{kind} set {vmid}"), TaskStatus::Ok, now, Some(vmid), None);
        Ok(())
    }

    /// Yeni anlık görüntü alınabilir mi?
    pub fn check_snapshot_new(&self, vmid: VmId, name: &str) -> Result<(), Error> {
        let guest = self.guest(vmid)?;
        if !valid_snapshot_name(name) {
            return Err(Error::InvalidSpec(format!(
                "geçersiz anlık görüntü adı \"{name}\": 1-32 karakter; küçük harf, rakam, tire ve alt çizgi kullanın"
            )));
        }
        if guest.snapshots.iter().any(|s| s.name == name) {
            return Err(Error::SnapshotExists(name.to_string()));
        }
        if guest.snapshots.len() >= MAX_SNAPSHOTS {
            return Err(Error::InvalidSpec(format!(
                "bir konukta en fazla {MAX_SNAPSHOTS} anlık görüntü tutulabilir; önce eskilerden silin"
            )));
        }
        Ok(())
    }

    pub fn check_snapshot(&self, vmid: VmId, name: &str) -> Result<(), Error> {
        if self.guest(vmid)?.snapshots.iter().any(|s| s.name == name) {
            Ok(())
        } else {
            Err(Error::SnapshotNotFound(name.to_string()))
        }
    }

    pub fn add_snapshot(&mut self, vmid: VmId, name: &str, now: u64) {
        self.add_snapshot_kind(vmid, name, now, false, false);
    }

    /// `disk_only`: bellek durumu olmadan alınmış (üretim) denetim noktası; `quiesced`: dosya sistemleri dondurulmuştu.
    pub fn add_snapshot_kind(&mut self, vmid: VmId, name: &str, now: u64, disk_only: bool, quiesced: bool) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.snapshots.push(Snapshot { name: name.to_string(), created: now, disk_only, quiesced });
        }
    }

    /// Belleksiz görüntüye, konuk çalışırken dönülemez: QEMU'nun yükleyeceği bir bellek durumu yoktur.
    pub fn check_rollback_state(&self, vmid: VmId, name: &str) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        let disk_only = g.snapshots.iter().any(|s| s.name == name && s.disk_only);
        if disk_only && g.status != GuestStatus::Stopped {
            return Err(Error::RequiresStopped { vmid, what: "bellek durumu olmayan (üretim) denetim noktasına dönmek".to_string() });
        }
        Ok(())
    }

    pub fn remove_snapshot(&mut self, vmid: VmId, name: &str) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.snapshots.retain(|s| s.name != name);
        }
    }

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

    /// Disk ekleme ve çıkarma için ortak ön koşullar: sanal makine, kapalı, kilitsiz.
    fn disk_change_allowed(&self, vmid: VmId) -> Result<&Guest, Error> {
        self.device_change_allowed(vmid, false, "disk eklemek ya da çıkarmak")
    }

    /// Aygıt ekleme ve çıkarma kuralı. `hot`: sürücü çalışırken takabiliyorsa çalışan konuk da kabul edilir
    /// (duraklatılmış konuk edilmez: aygıtı konuğun işletim sistemi karşılamalıdır).
    fn device_change_allowed(&self, vmid: VmId, hot: bool, what: &str) -> Result<&Guest, Error> {
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

    fn add_disk_with(&mut self, vmid: VmId, size_gb: u64, hot: bool) -> Result<u8, Error> {
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

    /// Balonla hedeflenen belleği kaydeder; önceki hedefi döndürür (sürücü uygulayamazsa geri konur).
    /// 0 ya da belleğin tamamı "balonu boşalt" demektir. Konuk çalışırken de değişir.
    pub fn set_balloon_target(&mut self, vmid: VmId, target_mb: u64) -> Result<u64, Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm || !g.hw.balloon {
            return Err(Error::InvalidSpec("bu konukta bellek balonu yok; önce Donanım sekmesinden açın (konuk kapalıyken)".to_string()));
        }
        if g.hw.mem_min_mb > 0 {
            return Err(Error::InvalidSpec("dinamik bellek açık: hedef kendiliğinden ayarlanıyor. Elle hedef vermek için önce dinamik belleği kapatın (en az bellek 0)".to_string()));
        }
        let total = g.mem_total_mb;
        if target_mb != 0 && !(crate::hw::MIN_BALLOON_MB..=total).contains(&target_mb) {
            return Err(Error::InvalidSpec(format!("bellek hedefi {} MB ile {total} MB arasında olmalı (0: tamamı)", crate::hw::MIN_BALLOON_MB)));
        }
        let g = self.guest_mut(vmid)?;
        let before = g.hw.balloon_mb;
        g.hw.balloon_mb = if target_mb >= total { 0 } else { target_mb };
        Ok(before)
    }

    /// Dinamik belleği açar (`min_mb` alt sınır) ya da kapatır (0). Konuk çalışırken de değişir.
    pub fn set_dynamic_memory(&mut self, vmid: VmId, min_mb: u64) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm || !g.hw.balloon {
            return Err(Error::InvalidSpec("bu konukta bellek balonu yok; önce Donanım sekmesinden açın (konuk kapalıyken)".to_string()));
        }
        let total = g.mem_total_mb;
        if min_mb != 0 && !(crate::hw::MIN_BALLOON_MB..=total).contains(&min_mb) {
            return Err(Error::InvalidSpec(format!("en az bellek {} MB ile {total} MB arasında olmalı (0: dinamik bellek kapalı)", crate::hw::MIN_BALLOON_MB)));
        }
        self.guest_mut(vmid)?.hw.mem_min_mb = min_mb;
        Ok(())
    }

    /// Dinamik belleğin hesapladığı hedefi kaydeder (sürücü uyguladıktan sonra).
    pub fn set_dynamic_target(&mut self, vmid: VmId, target_mb: u64) {
        if let Ok(g) = self.guest_mut(vmid) {
            if g.hw.mem_min_mb > 0 {
                g.hw.balloon_mb = if target_mb >= g.mem_total_mb { 0 } else { target_mb };
            }
        }
    }

    /// Sınama konuğunun denetlenecek kapısına açılan sunucu tarafı kapıyı kaydeder (konuk başlatılmadan önce).
    pub fn set_test_forward(&mut self, vmid: VmId, host_port: u16) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.test_fwd = host_port;
        }
    }

    /// Uygulanamayan bellek hedefini eski değerine döndürür.
    pub fn restore_balloon_target(&mut self, vmid: VmId, target_mb: u64) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.balloon_mb = target_mb;
        }
    }

    /// Son eklenen ağ kartının kaydını geri alır (takılamayan kart için).
    pub fn forget_nic(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.nics.pop();
        }
    }

    /// Otomatik yedeği bugün henüz alınmamış ve saati gelmiş konuklar.
    /// Üzerinde işlem süren konuk bir sonraki denetimde yeniden değerlendirilir.
    pub fn auto_backups_due(&self, today: &str, minute_of_day: u16) -> Vec<VmId> {
        self.guests
            .iter()
            .filter(|g| g.lock.is_none() && g.last_auto_backup.as_deref() != Some(today))
            .filter(|g| g.backup_time.as_deref().and_then(parse_time).is_some_and(|at| at <= minute_of_day))
            .map(|g| g.vmid)
            .collect()
    }

    /// Saati gelen zamanlanmış güç işlemleri: `(konuk, satır, eylem)`. `weekday`: 0 Pazartesi … 6 Pazar.
    /// Saatinden en çok `SCHEDULE_GRACE_MIN` dakika sonra çalışır; sunucu o sırada kapalıysa o gün atlanır
    /// (kaçan bir "kapat" ile "başlat" saatler sonra art arda uygulanmasın). Kilitli konuk sonraki turu bekler.
    pub fn schedules_due(&self, today: &str, weekday: u8, minute_of_day: u16) -> Vec<(VmId, usize, Action)> {
        self.guests
            .iter()
            .filter(|g| g.lock.is_none() && !g.hw.template)
            .flat_map(|g| g.hw.schedules.iter().enumerate().map(move |(i, s)| (g.vmid, i, s)))
            .filter(|(_, _, s)| s.on_day(weekday) && s.last_run.as_deref() != Some(today))
            .filter(|(_, _, s)| parse_time(&s.time).is_some_and(|at| at <= minute_of_day && minute_of_day - at <= crate::hw::SCHEDULE_GRACE_MIN))
            .map(|(vmid, i, s)| (vmid, i, s.action))
            .collect()
    }

    /// Zamanlanmış işlemi bugün çalıştı olarak işaretler (başarısız olsa da gün içinde yinelenmez).
    pub fn mark_schedule(&mut self, vmid: VmId, index: usize, today: &str) {
        if let Some(s) = self.guest_mut(vmid).ok().and_then(|g| g.hw.schedules.get_mut(index)) {
            s.last_run = Some(today.to_string());
        }
    }

    /// Sunucu açılışında başlatılacak konuklar, sırasıyla: `(konuk, sonrakinden önce beklenecek saniye)`.
    /// Sıra numarası olanlar küçükten büyüğe önce, sırasızlar (0) kimlik sırasıyla sonra gelir.
    /// Zaten çalışan, kilitli ya da şablon olan konuklar listede yer almaz.
    pub fn boot_plan(&self) -> Vec<(VmId, u32)> {
        let mut due: Vec<&Guest> = self.guests.iter().filter(|g| g.onboot && !g.hw.template && g.lock.is_none() && g.status == GuestStatus::Stopped).collect();
        due.sort_by_key(|g| (g.hw.boot_order == 0, g.hw.boot_order, g.vmid));
        due.iter().map(|g| (g.vmid, g.hw.boot_delay)).collect()
    }

    /// Bugünün otomatik yedeği denendi olarak işaretler (başarısız olsa da gün içinde yinelenmez).
    pub fn mark_auto_backup(&mut self, vmid: VmId, today: &str) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.last_auto_backup = Some(today.to_string());
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

    /// Anında biten bir işlemi görev günlüğüne yazar.
    pub fn log(&mut self, node: &str, user: &str, command: String, outcome: &Result<(), String>, now: u64, vmid: Option<VmId>) {
        let status = if outcome.is_ok() { TaskStatus::Ok } else { TaskStatus::Failed };
        let id = self.push_task(node, user, command, status, now, vmid, None);
        if let (Err(message), Some(task)) = (outcome, self.tasks.iter_mut().find(|t| t.id == id)) {
            task.message = Some(message.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GuestSpec;

    const NOW: u64 = 1_800_000_000;

    fn cluster() -> Cluster {
        Cluster::demo(NOW)
    }

    fn patch() -> GuestPatch {
        GuestPatch::default()
    }

    fn fwd(host_port: u16, guest_port: u16) -> PortForward {
        PortForward { host_port, guest_port }
    }

    #[test]
    fn begin_op_locks_and_complete_unlocks() {
        let mut c = cluster();
        assert_eq!(
            c.begin_op(100, "vm snapshot 100".into(), "u", NOW, Some("anlık görüntü almak")),
            Err(Error::RequiresStopped { vmid: 100, what: "anlık görüntü almak".into() })
        );
        assert_eq!(c.begin_op(999, "x".into(), "u", NOW, None), Err(Error::GuestNotFound(999)));
        // Durdurma şartı olmayan işlem çalışan konukta da başlar.
        let live = c.begin_op(100, "vm backup 100".into(), "u", NOW, None).unwrap();
        c.complete_task(live, Ok(()), NOW).unwrap();

        let t = c.begin_op(103, "vm snapshot 103 temiz".into(), "ayse", NOW, Some("x")).unwrap();
        assert_eq!(c.guest(103).unwrap().lock, Some(t));
        assert_eq!(c.begin_op(103, "y".into(), "u", NOW, None), Err(Error::Locked { vmid: 103, task: t }));
        assert_eq!(c.begin_action(103, Action::Start, "u", NOW), Err(Error::Locked { vmid: 103, task: t }));

        c.complete_task(t, Err("disk dolu".into()), NOW + 3).unwrap();
        let g = c.guest(103).unwrap();
        assert_eq!((g.lock, g.status), (None, GuestStatus::Stopped), "işlem görevi durumu değiştirmez");
        let task = c.task(t).unwrap();
        assert_eq!((task.status, task.message.as_deref(), task.user.as_str()), (TaskStatus::Failed, Some("disk dolu"), "ayse"));
    }

    #[test]
    fn update_changes_fields_and_logs() {
        let mut c = cluster();
        let p = GuestPatch {
            name: Some("derleme".into()),
            cores: Some(4),
            memory_mb: Some(8192),
            os: Some("windows-2025".into()),
            iso: Some("win.iso".into()),
            onboot: Some(false),
            tags: Some(vec!["prod".into(), "ci".into(), "prod".into()]),
            forwards: Some(vec![fwd(8080, 80), fwd(2222, 22)]),
            bridge: None,
            backup_time: Some("03:30".into()),
            backup_keep: Some(3),
            ..Default::default()
        };
        c.update_guest(103, &p, "ayse", NOW).unwrap();
        let g = c.guest(103).unwrap();
        assert_eq!((g.name.as_str(), g.cores, g.mem_total_mb, g.os.as_str()), ("derleme", 4, 8192, "windows-2025"));
        assert_eq!((g.iso.as_deref(), g.onboot), (Some("win.iso"), false));
        assert_eq!(g.tags, ["ci", "prod"], "etiketler sıralı ve tekil");
        assert_eq!(g.forwards, [fwd(8080, 80), fwd(2222, 22)]);
        assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].user.as_str()), ("vm set 103", "ayse"));

        // Boş ISO kalıbı çıkarır; verilmeyen alanlar değişmez.
        c.update_guest(103, &GuestPatch { iso: Some(String::new()), ..patch() }, "u", NOW).unwrap();
        let g = c.guest(103).unwrap();
        assert_eq!((g.iso.as_deref(), g.cores, g.tags.len()), (None, 4, 2));
    }

    #[test]
    fn hardware_changes_need_a_stopped_guest() {
        let mut c = cluster();
        let before = c.clone();
        for p in [
            GuestPatch { cores: Some(2), ..patch() },
            GuestPatch { memory_mb: Some(4096), ..patch() },
            GuestPatch { iso: Some("x.iso".into()), ..patch() },
            GuestPatch { forwards: Some(vec![fwd(8080, 80)]), ..patch() },
        ] {
            assert!(matches!(c.update_guest(100, &p, "u", NOW), Err(Error::RequiresStopped { vmid: 100, .. })), "{p:?}");
        }
        assert_eq!(c, before);
        // Aynı değeri yeniden göndermek değişiklik sayılmaz; ad ve etiket çalışırken de değişir.
        let p = GuestPatch { cores: Some(4), name: Some("web-yeni".into()), tags: Some(vec!["web".into()]), onboot: Some(false), ..patch() };
        c.update_guest(100, &p, "u", NOW).unwrap();
        assert_eq!(c.guest(100).unwrap().name, "web-yeni");
    }

    #[test]
    fn update_validates_values() {
        let mut c = cluster();
        let before = c.clone();
        let bad: Vec<(GuestPatch, &str)> = vec![
            (GuestPatch { name: Some("Kötü".into()), ..patch() }, "geçersiz ad"),
            (GuestPatch { cores: Some(0), ..patch() }, "çekirdek"),
            (GuestPatch { cores: Some(999), ..patch() }, "çekirdek"),
            (GuestPatch { memory_mb: Some(1), ..patch() }, "bellek"),
            (GuestPatch { iso: Some("../x.iso".into()), ..patch() }, "geçersiz ISO"),
            (GuestPatch { tags: Some(vec!["Büyük".into()]), ..patch() }, "geçersiz etiket"),
            (GuestPatch { tags: Some((0..9).map(|i| format!("t{i}")).collect()), ..patch() }, "en fazla 8 etiket"),
            (GuestPatch { forwards: Some(vec![fwd(80, 80)]), ..patch() }, "geçersiz yönlendirme"),
            (GuestPatch { forwards: Some(vec![fwd(8080, 0)]), ..patch() }, "geçersiz yönlendirme"),
            (GuestPatch { forwards: Some((0..17).map(|i| fwd(9000 + i, 80)).collect()), ..patch() }, "en fazla 16"),
        ];
        for (p, needle) in bad {
            let err = c.update_guest(103, &p, "u", NOW).unwrap_err().to_string();
            assert!(err.contains(needle), "{p:?}: {err}");
        }
        assert!(matches!(c.update_guest(202, &GuestPatch { iso: Some("a.iso".into()), ..patch() }, "u", NOW), Err(Error::InvalidSpec(_))), "konteynere ISO takılmaz");
        assert_eq!(c.update_guest(999, &patch(), "u", NOW), Err(Error::GuestNotFound(999)));
        assert_eq!(c, before);
    }

    #[test]
    fn bridge_setting_and_mac() {
        let mut c = cluster();
        for ok in ["nat", "vmbr0", "br-lan", "eth0.100", "docker0"] {
            assert!(valid_bridge(ok), "{ok}");
        }
        for bad in ["", "-x", "..", ".", "a b", "br0;reboot", "çok-uzun-köprü-adı", "br0,script=/x", "abcdefghijklmnop"] {
            assert!(!valid_bridge(bad), "{bad}");
        }
        assert_eq!(mac_for(100), "52:54:00:00:00:64");
        assert_eq!(mac_for(0x00AB_CDEF), "52:54:00:ab:cd:ef");
        assert_ne!(mac_for(100), mac_for(101));

        c.update_guest(103, &GuestPatch { bridge: Some("br-lan".into()), ..patch() }, "u", NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().bridge, "br-lan");
        assert!(matches!(c.update_guest(103, &GuestPatch { bridge: Some("br0,x=1".into()), ..patch() }, "u", NOW), Err(Error::InvalidSpec(_))));
        assert!(matches!(c.update_guest(100, &GuestPatch { bridge: Some("nat".into()), ..patch() }, "u", NOW), Err(Error::RequiresStopped { .. })), "ağ ayarı çalışırken değişmez");

        assert!(c.observe_ip(100, Some("10.0.0.7".into())));
        assert!(!c.observe_ip(100, Some("10.0.0.7".into())), "aynı adres değişiklik sayılmaz");
        assert!(c.observe_ip(100, None));
        assert_eq!(c.guest(100).unwrap().ip, None);
        assert!(!c.observe_ip(999, None));
    }

    #[test]
    fn host_ports_are_unique_across_guests() {
        let mut c = cluster();
        c.update_guest(103, &GuestPatch { forwards: Some(vec![fwd(8080, 80)]), ..patch() }, "u", NOW).unwrap();
        assert_eq!(c.update_guest(202, &GuestPatch { forwards: Some(vec![fwd(8080, 8080)]), ..patch() }, "u", NOW), Err(Error::PortTaken(8080)));
        assert_eq!(c.update_guest(202, &GuestPatch { forwards: Some(vec![fwd(9000, 1), fwd(9000, 2)]), ..patch() }, "u", NOW), Err(Error::PortTaken(9000)));
        // Kendi portunu yeniden göndermek çakışma değildir.
        c.update_guest(103, &GuestPatch { forwards: Some(vec![fwd(8080, 80), fwd(8443, 443)]), ..patch() }, "u", NOW).unwrap();
    }

    #[test]
    fn snapshot_bookkeeping() {
        let mut c = cluster();
        assert!(matches!(c.check_snapshot_new(103, "Kötü Ad"), Err(Error::InvalidSpec(_))));
        assert!(matches!(c.check_snapshot_new(103, ""), Err(Error::InvalidSpec(_))));
        c.check_snapshot_new(103, "kurulum_oncesi").unwrap();
        c.add_snapshot(103, "kurulum_oncesi", NOW);
        assert_eq!(c.check_snapshot_new(103, "kurulum_oncesi"), Err(Error::SnapshotExists("kurulum_oncesi".into())));
        assert_eq!(c.check_snapshot(103, "yok"), Err(Error::SnapshotNotFound("yok".into())));
        c.check_snapshot(103, "kurulum_oncesi").unwrap();

        for i in 1..MAX_SNAPSHOTS {
            c.add_snapshot(103, &format!("s{i}"), NOW);
        }
        assert!(matches!(c.check_snapshot_new(103, "bir-daha"), Err(Error::InvalidSpec(m)) if m.contains("en fazla")));
        c.remove_snapshot(103, "s1");
        c.check_snapshot_new(103, "bir-daha").unwrap();
        c.after_restore(103, 0);
        assert!(c.guest(103).unwrap().snapshots.is_empty());
    }

    #[test]
    fn disk_can_only_grow_within_free_space() {
        let mut c = cluster();
        assert!(matches!(c.check_grow(103, 200), Err(Error::InvalidSpec(_))), "aynı boyut");
        assert!(matches!(c.check_grow(103, 100), Err(Error::InvalidSpec(_))), "küçültme");
        assert_eq!(
            c.check_grow(103, 1000),
            Err(Error::InsufficientStorage { storage: "local-zfs".into(), needed_gb: 800, free_gb: 410 })
        );
        c.check_grow(103, 300).unwrap();
        let used = c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb;
        c.apply_grow(103, 300);
        assert_eq!(c.guest(103).unwrap().disk_gb, 300);
        assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used + 100);
    }

    #[test]
    fn clone_copies_config_but_not_identity() {
        let mut c = cluster();
        c.update_guest(103, &GuestPatch { tags: Some(vec!["ci".into()]), forwards: Some(vec![fwd(8080, 80)]), ..patch() }, "u", NOW).unwrap();
        c.add_snapshot(103, "s", NOW);
        let used = c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb;

        assert_eq!(c.clone_guest(103, "Kötü"), Err(Error::InvalidName("Kötü".into())));
        assert_eq!(c.clone_guest(999, "x"), Err(Error::GuestNotFound(999)));
        let id = c.clone_guest(103, "win-build-2").unwrap();
        assert_eq!(id, 105);
        let (src, dst) = (c.guest(103).unwrap().clone(), c.guest(id).unwrap().clone());
        assert_eq!((dst.cores, dst.mem_total_mb, dst.disk_gb, &dst.os, &dst.tags), (src.cores, src.mem_total_mb, src.disk_gb, &src.os, &src.tags));
        assert_eq!((dst.status, dst.name.as_str()), (GuestStatus::Stopped, "win-build-2"));
        assert!(dst.forwards.is_empty() && dst.snapshots.is_empty(), "port ve anlık görüntü kopyalanmaz");
        assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used + 200);

        c.discard_clone(id);
        assert!(c.guest(id).is_err());
        assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used);

        let mut full = cluster();
        full.storages[1].used_gb = full.storages[1].total_gb - 10;
        assert!(matches!(full.clone_guest(103, "x"), Err(Error::InsufficientStorage { .. })));
    }

    #[test]
    fn extra_disks_reserve_and_release_storage() {
        let mut c = Cluster::demo(NOW);
        let stopped = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).unwrap().clone();
        let running = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
        let used = |c: &Cluster| c.storages.iter().find(|s| s.id == stopped.storage && s.available_on(&stopped.node)).unwrap().used_gb;
        let before = used(&c);

        assert_eq!(c.add_disk(stopped.vmid, 20).unwrap(), 1);
        assert_eq!(c.add_disk(stopped.vmid, 30).unwrap(), 2);
        assert_eq!(used(&c), before + 50);
        assert_eq!(c.guest(stopped.vmid).unwrap().total_disk_gb(), stopped.disk_gb + 50);
        // Aradaki disk silinince numarası boşalır ve yeni disk onu alır; diğerinin numarası değişmez.
        assert_eq!(c.remove_disk(stopped.vmid, 1).unwrap().size_gb, 20);
        assert_eq!(used(&c), before + 30);
        assert_eq!(c.add_disk(stopped.vmid, 5).unwrap(), 1);
        assert_eq!(c.guest(stopped.vmid).unwrap().hw.disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>(), [(1, 5), (2, 30)]);

        assert!(matches!(c.add_disk(running, 10), Err(Error::RequiresStopped { .. })));
        assert!(matches!(c.add_disk(ct, 10), Err(Error::InvalidSpec(m)) if m.contains("yalnızca sanal makinelere")));
        assert!(matches!(c.add_disk(stopped.vmid, 0), Err(Error::InvalidSpec(_))));
        assert!(matches!(c.add_disk(stopped.vmid, 10_000_000), Err(Error::InsufficientStorage { .. })));
        assert!(matches!(c.remove_disk(stopped.vmid, 7), Err(Error::InvalidSpec(m)) if m.contains("7 numaralı ek disk yok")));
        for _ in 0..6 {
            c.add_disk(stopped.vmid, 1).unwrap();
        }
        assert!(matches!(c.add_disk(stopped.vmid, 1), Err(Error::InvalidSpec(m)) if m.contains("en fazla 8")));

        // Klon ek diskleri de sayar; silme hepsini iade eder.
        let with_disks = used(&c);
        let total = c.guest(stopped.vmid).unwrap().total_disk_gb();
        let copy = c.clone_guest(stopped.vmid, "kopya").unwrap();
        assert_eq!(used(&c), with_disks + total);
        c.discard_clone(copy);
        assert_eq!(used(&c), with_disks);
        c.delete_guest(stopped.vmid, "u", NOW).unwrap();
        assert_eq!(used(&c), before - stopped.disk_gb);
    }

    #[test]
    fn backup_registry() {
        let mut c = cluster();
        let b = |id: &str, vmid| Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW, size_mb: 5, auto: false, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 };
        c.add_backup(b("vm-103-1", 103));
        c.add_backup(b("vm-100-1", 100));
        c.add_backup(b("vm-103-2", 103));
        assert_eq!(c.backups_of(103).map(|x| x.id.as_str()).collect::<Vec<_>>(), ["vm-103-2", "vm-103-1"], "en yeni başta");
        assert_eq!(c.backup("vm-100-1").unwrap().vmid, 100);
        assert_eq!(c.remove_backup("yok"), Err(Error::BackupNotFound("yok".into())));
        assert_eq!(c.remove_backup("vm-103-1").unwrap().id, "vm-103-1");
        assert_eq!(c.backups.len(), 2);
        // Yedekler durumla birlikte saklanır.
        let back: Cluster = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.backups, c.backups);
    }

    #[test]
    fn schedule_rules() {
        for (text, minutes) in [("00:00", Some(0)), ("03:30", Some(210)), ("23:59", Some(1439)), ("24:00", None), ("3:30", None), ("03:60", None), ("0330", None), ("ab:cd", None), ("", None), ("03:30:00", None), ("+3:30", None)] {
            assert_eq!(parse_time(text), minutes, "{text}");
        }
        let mut c = cluster();
        let set = |c: &mut Cluster, vmid, time: &str, keep| c.update_guest(vmid, &GuestPatch { backup_time: Some(time.into()), backup_keep: keep, ..patch() }, "u", NOW);
        set(&mut c, 103, "03:30", Some(2)).unwrap();
        set(&mut c, 100, "23:00", None).unwrap();
        assert_eq!((c.guest(103).unwrap().backup_keep, c.guest(100).unwrap().backup_keep), (2, DEFAULT_KEEP));
        assert!(matches!(set(&mut c, 103, "25:00", None), Err(Error::InvalidSpec(_))));
        assert!(matches!(set(&mut c, 103, "03:30", Some(0)), Err(Error::InvalidSpec(_))));
        assert!(matches!(set(&mut c, 103, "03:30", Some(366)), Err(Error::InvalidSpec(_))));

        assert!(c.auto_backups_due("2026-11-24", 209).is_empty(), "saati gelmeden alınmaz");
        assert_eq!(c.auto_backups_due("2026-11-24", 210), [103]);
        assert_eq!(c.auto_backups_due("2026-11-24", 1439), [100, 103], "sunucu kapalıyken kaçan saat gün içinde telafi edilir");
        c.mark_auto_backup(103, "2026-11-24");
        assert_eq!(c.auto_backups_due("2026-11-24", 1439), [100], "aynı gün ikinci kez alınmaz");
        assert_eq!(c.auto_backups_due("2026-11-25", 300), [103], "ertesi gün yeniden");

        let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        assert!(c.auto_backups_due("2026-11-25", 300).is_empty(), "kilitli konuk bekletilir");
        c.complete_task(t, Ok(()), NOW).unwrap();
        set(&mut c, 103, "", None).unwrap();
        assert_eq!(c.guest(103).unwrap().backup_time, None, "boş saat zamanlamayı kapatır");
        assert!(c.auto_backups_due("2026-11-25", 1439).iter().all(|v| *v != 103));
    }

    #[test]
    fn restore_covers_extra_disks_still_attached_and_unchanged() {
        use crate::hw::ExtraDisk;
        let mut c = cluster();
        let disk = |id, size_gb| ExtraDisk { id, size_gb, serial: id as u64 };
        let with = |c: &mut Cluster, disks: Vec<ExtraDisk>| c.guest_mut_for_import(103).unwrap().hw.disks = disks;
        with(&mut c, vec![disk(1, 10), disk(2, 20)]);
        c.add_backup(Backup { id: "b".into(), vmid: 103, guest_name: "x".into(), created: NOW, size_mb: 1, auto: false, offsite: false, new_mb: None, disks: vec![disk(1, 10), disk(2, 20)], disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        assert_eq!(c.restore_disks("b").unwrap(), [1, 2]);
        // Yedekten sonra kaldırılan disk geri getirilmez; sonradan eklenen (3) listeye girmez.
        with(&mut c, vec![disk(2, 20), disk(3, 5)]);
        assert_eq!(c.restore_disks("b").unwrap(), [2]);
        // Aynı numaraya sonradan takılmış disk (boyutu aynı olsa da) başka bir disktir: üzerine yazılmaz.
        with(&mut c, vec![ExtraDisk { id: 1, size_gb: 10, serial: 9 }, disk(2, 20)]);
        assert_eq!(c.restore_disks("b").unwrap(), [2]);
        // Sıra numarası tutmayan eski kayıtlar ayırt edilemez: boyut farklıysa açık hata.
        let old = |id, size_gb| ExtraDisk { id, size_gb, serial: 0 };
        c.add_backup(Backup { id: "eski".into(), vmid: 103, guest_name: "x".into(), created: NOW, size_mb: 1, auto: false, offsite: false, new_mb: None, disks: vec![old(1, 10)], disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        with(&mut c, vec![old(1, 10)]);
        assert_eq!(c.restore_disks("eski").unwrap(), [1]);
        with(&mut c, vec![old(1, 50)]);
        assert!(matches!(c.restore_disks("eski"), Err(Error::InvalidSpec(m)) if m.contains("ek disk 1") && m.contains("10 GB") && m.contains("50 GB")));
        assert_eq!(c.restore_disks("yok"), Err(Error::BackupNotFound("yok".into())));

        // Eklenen her disk yeni bir sıra numarası alır; aynı numara yeniden kullanılsa da.
        let mut c = cluster();
        let vm = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).unwrap().vmid;
        c.add_disk(vm, 1).unwrap();
        let first = c.guest(vm).unwrap().hw.disks[0].clone();
        c.remove_disk(vm, 1).unwrap();
        c.add_disk(vm, 1).unwrap();
        let second = c.guest(vm).unwrap().hw.disks[0].clone();
        assert_eq!((first.id, second.id), (1, 1));
        assert_ne!(first.serial, second.serial);

        // Yedekten sonra büyütülen asıl disk geri yüklenince kayıt da eski boyuta döner.
        let (was, storage) = (c.guest(vm).unwrap().disk_gb, c.guest(vm).unwrap().storage.clone());
        let used = |c: &Cluster| c.storages.iter().find(|s| s.id == storage).unwrap().used_gb;
        let before = used(&c);
        c.apply_grow(vm, was + 7);
        c.after_restore(vm, was);
        assert_eq!((c.guest(vm).unwrap().disk_gb, used(&c)), (was, before));
        c.after_restore(vm, 0);
        assert_eq!(c.guest(vm).unwrap().disk_gb, was, "boyutu bilinmeyen eski yedek kaydı değiştirmez");
        // Eski kayıtlarda alan yoktur: yalnızca asıl disk.
        let old: Backup = serde_json::from_str(r#"{"id":"e","vmid":103,"guest_name":"x","created":1,"size_mb":1}"#).unwrap();
        assert!(old.disks.is_empty());
    }

    #[test]
    fn suspended_guests_keep_their_disk_and_hardware() {
        let mut c = cluster();
        let vm = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
        c.after_suspend(vm);
        let g = c.guest(vm).unwrap();
        assert!(g.hw.suspended && g.status == GuestStatus::Stopped && g.started_at.is_none() && g.ip.is_none());
        assert!(c.not_suspended(vm).is_err());
        assert!(matches!(c.add_disk(vm, 1), Err(Error::InvalidSpec(m)) if m.contains("askıda")));
        assert!(matches!(c.set_template(vm, true), Err(Error::InvalidSpec(m)) if m.contains("askıda")));
        // Klon askıda başlamaz: kendi bellek durumu yoktur.
        let copy = c.clone_guest(vm, "kopya").unwrap();
        assert!(!c.guest(copy).unwrap().hw.suspended);
        // Başlatılabilir; devam edince işaret kalkar.
        assert!(c.begin_action(vm, Action::Start, "u", NOW).is_ok());
        c.clear_suspended(vm);
        assert!(c.not_suspended(vm).is_ok());
    }

    #[test]
    fn templates_cannot_change_and_linked_clones_protect_them() {
        let mut c = cluster();
        let vm = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).unwrap().vmid;
        let running = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
        assert!(matches!(c.clone_guest_as(vm, "bagli", true), Err(Error::InvalidSpec(m)) if m.contains("yalnızca şablondan")));
        assert!(matches!(c.set_template(running, true), Err(Error::RequiresStopped { .. })));
        assert!(matches!(c.set_template(ct, true), Err(Error::InvalidSpec(_))));

        c.update_guest(vm, &GuestPatch { backup_time: Some("03:30".into()), onboot: Some(true), ..patch() }, "u", NOW).unwrap();
        c.set_template(vm, true).unwrap();
        let t = c.guest(vm).unwrap();
        assert!(t.hw.template && !t.onboot && t.backup_time.is_none(), "şablon kendiliğinden açılmaz, yedeği zamanlanmaz");
        assert!(matches!(c.begin_action(vm, Action::Start, "u", NOW), Err(Error::InvalidSpec(m)) if m.contains("şablon")));
        assert!(c.not_template(vm).is_err() && c.not_template(running).is_ok());
        assert!(matches!(c.add_disk(vm, 1), Err(Error::InvalidSpec(m)) if m.contains("şablon")));

        let linked = c.clone_guest_as(vm, "bagli", true).unwrap();
        let full = c.clone_guest_as(vm, "tam", false).unwrap();
        assert_eq!((c.guest(linked).unwrap().hw.base, c.guest(linked).unwrap().hw.template), (Some(vm), false));
        assert_eq!((c.guest(full).unwrap().hw.base, c.guest(full).unwrap().hw.template), (None, false));
        // Bağlı klon varken şablon ne silinir ne şablonluktan çıkar; bağlı klon şablon olamaz.
        assert!(matches!(c.delete_guest(vm, "u", NOW), Err(Error::InvalidSpec(m)) if m.contains(&linked.to_string())));
        assert!(matches!(c.set_template(vm, false), Err(Error::InvalidSpec(m)) if m.contains("bağlı klonları")));
        assert!(matches!(c.set_template(linked, true), Err(Error::InvalidSpec(m)) if m.contains("bağlı klon")));
        c.delete_guest(linked, "u", NOW).unwrap();
        c.set_template(vm, false).unwrap();
        assert!(c.begin_action(vm, Action::Start, "u", NOW).is_ok());

        // İstekle şablon ya da bağlı klon ilişkisi kurulamaz.
        let like = c.guest(vm).unwrap().clone();
        let mut spec = GuestSpec {
            vmid: None, name: "sahte".into(), kind: like.kind, node: like.node, cores: 1, memory_mb: 512, disk_gb: 1,
            storage: like.storage, os: like.os, bridge: like.bridge, onboot: false, iso: None, hw: Default::default(),
        };
        spec.hw.template = true;
        spec.hw.base = Some(vm);
        let made = c.create_guest(&spec, "u", NOW).unwrap();
        assert_eq!((c.guest(made).unwrap().hw.template, c.guest(made).unwrap().hw.base), (false, None));
    }

    #[test]
    fn retention_prunes_only_old_automatic_backups() {
        let mut c = cluster();
        c.update_guest(103, &GuestPatch { backup_keep: Some(2), ..patch() }, "u", NOW).unwrap();
        let mut add = |id: &str, vmid, auto| c.add_backup(Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW, size_mb: 1, auto, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        add("a1", 103, true);
        add("elle", 103, false);
        add("a2", 103, true);
        add("a3", 103, true);
        add("a4", 103, true);
        add("baska", 100, true);
        add("yetim", 999, true);
        // En yeni iki otomatik yedek (a4, a3) kalır; elle alınan, başka konuğunki ve yetim olan korunur.
        assert_eq!(c.prunable_backups(NOW), ["a2", "a1"]);
        c.remove_backup("a2").unwrap();
        c.remove_backup("a1").unwrap();
        assert!(c.prunable_backups(NOW).is_empty());
    }

    #[test]
    fn gfs_and_locks_shape_what_is_pruned() {
        use crate::retention::{Gfs, DAY_SECS};
        let mut c = cluster();
        c.update_guest(103, &GuestPatch { backup_keep: Some(2), backup_weekly: Some(3), ..patch() }, "u", NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().backup_gfs, Gfs { weekly: 3, monthly: 0, yearly: 0 });
        assert!(c.update_guest(103, &GuestPatch { backup_yearly: Some(100_000), ..patch() }, "u", NOW).is_err());
        // 30 gün boyunca her gün bir otomatik yedek (g0 en yeni).
        for day in (0..30u64).rev() {
            c.add_backup(Backup { id: format!("g{day}"), vmid: 103, guest_name: "x".into(), created: NOW - day * DAY_SECS, size_mb: 1, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        }
        let kept = |c: &Cluster| {
            let doomed = c.prunable_backups(NOW);
            c.backups.iter().filter(|b| !doomed.contains(&b.id)).map(|b| b.id.clone()).collect::<Vec<_>>()
        };
        let survivors = kept(&c);
        // Son 2 yedek + yedeği olan son 3 haftanın en yenileri (en yeni hafta zaten ilk ikide).
        assert_eq!(survivors.len(), 4, "{survivors:?}");
        assert!(survivors.starts_with(&["g0".to_string(), "g1".to_string()]));
        let weekly: Vec<u64> = survivors[2..].iter().map(|id| id[1..].parse().unwrap()).collect();
        assert_eq!(weekly[1] - weekly[0], 7, "haftalık yedekler birer hafta arayla: {weekly:?}");
        // Kilitli yedek, fazlalık olsa da süresi dolana kadar budanmaz; kilit bitince budanır.
        assert!(c.prunable_backups(NOW).contains(&"g20".to_string()));
        let until = c.lock_backup("g20", 10, NOW).unwrap();
        assert_eq!(until, NOW + 10 * DAY_SECS);
        assert!(!c.prunable_backups(NOW).contains(&"g20".to_string()));
        assert!(c.prunable_backups(until + 1).contains(&"g20".to_string()));
        // Kilit yalnızca uzatılır; silme kilitliyken reddedilir.
        assert!(c.lock_backup("g20", 5, NOW).unwrap_err().to_string().contains("kısaltılamaz"));
        assert_eq!(c.lock_backup("g20", 20, NOW).unwrap(), NOW + 20 * DAY_SECS);
        assert!(c.lock_backup("g20", 0, NOW).is_err() && c.lock_backup("g20", 99_999, NOW).is_err() && c.lock_backup("yok", 1, NOW).is_err());
        assert!(c.backup_deletable("g20", NOW).unwrap_err().to_string().contains("tarihine kadar kilitli"));
        c.backup_deletable("g20", NOW + 21 * DAY_SECS).unwrap();
        c.backup_deletable("g0", NOW).unwrap();
        assert!(c.backup("g20").unwrap().is_locked(NOW) && !c.backup("g0").unwrap().is_locked(NOW));
        // İşin aldığı yedekler işin kurallarına uyar.
        c.put_job(crate::jobs::BackupJob::new("gece", "03:00", crate::hw::EVERY_DAY, Vec::new(), 1).with_retention(Gfs { weekly: 0, monthly: 2, yearly: 0 }, 0).unwrap()).unwrap();
        for day in [0u64, 1, 40, 41, 80] {
            c.add_backup(Backup { id: format!("j{day}"), vmid: 100, guest_name: "x".into(), created: NOW - day * DAY_SECS, size_mb: 1, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: Some("gece".into()), machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        }
        let doomed = c.prunable_backups(NOW);
        let job_kept: Vec<&str> = c.backups.iter().filter(|b| b.job.is_some() && !doomed.contains(&b.id)).map(|b| b.id.as_str()).collect();
        assert_eq!(job_kept.len(), 2, "en yeni yedek ve önceki ayın en yenisi: {job_kept:?}");
        assert!(job_kept.contains(&"j0"));
    }

    #[test]
    fn old_state_files_load_without_new_fields() {
        // v0.1 durum dosyasında tags/forwards/snapshots/backups alanları yoktu.
        let mut v = serde_json::to_value(cluster()).unwrap();
        v.as_object_mut().unwrap().remove("backups");
        for g in v["guests"].as_array_mut().unwrap() {
            for key in ["tags", "forwards", "snapshots", "iso", "backup_time", "backup_keep", "last_auto_backup"] {
                g.as_object_mut().unwrap().remove(key);
            }
        }
        let c: Cluster = serde_json::from_value(v).unwrap();
        assert!(c.backups.is_empty() && c.guests.iter().all(|g| g.tags.is_empty() && g.snapshots.is_empty() && g.backup_keep == DEFAULT_KEEP));
    }

    #[test]
    fn log_records_instant_operations() {
        let mut c = cluster();
        c.log("pve-ist-01", "u", "backup delete vm-1".into(), &Ok(()), NOW, None);
        c.log("pve-ist-01", "u", "vm clone 103".into(), &Err("yer yok".into()), NOW, Some(103));
        assert_eq!((c.tasks[1].status, c.tasks[1].ended), (TaskStatus::Ok, Some(NOW)));
        assert_eq!((c.tasks[0].status, c.tasks[0].message.as_deref()), (TaskStatus::Failed, Some("yer yok")));
    }

    #[test]
    fn text_parsers() {
        assert_eq!(parse_tags(" Prod, web  ci ,,"), ["prod", "web", "ci"]);
        assert!(parse_tags("").is_empty());
        assert_eq!(parse_forwards("8080:80, 2222:22"), Ok(vec![fwd(8080, 80), fwd(2222, 22)]));
        assert_eq!(parse_forwards("  "), Ok(vec![]));
        for bad in ["8080", "a:b", "8080:80:1", "99999:80", "8080-80"] {
            assert!(parse_forwards(bad).is_err(), "{bad}");
        }
        assert!(valid_tag("a-1") && !valid_tag("") && !valid_tag("a b") && !valid_tag(&"a".repeat(25)));
        assert!(valid_snapshot_name("a_b-1") && !valid_snapshot_name("a b") && !valid_snapshot_name("../x") && !valid_snapshot_name(&"a".repeat(33)));
    }

    #[test]
    fn boot_plan_orders_guests_and_skips_running_ones() {
        let mut c = cluster();
        let stopped: Vec<VmId> = c.guests.iter().filter(|g| g.status == GuestStatus::Stopped).map(|g| g.vmid).collect();
        assert!(stopped.contains(&103) && stopped.contains(&202), "{stopped:?}");
        for g in c.guests.iter_mut() {
            g.onboot = false;
        }
        assert!(c.boot_plan().is_empty());
        let set = |c: &mut Cluster, vmid, order, delay| c.update_guest(vmid, &GuestPatch { onboot: Some(true), boot_order: Some(order), boot_delay: Some(delay), ..patch() }, "u", NOW);
        set(&mut c, 202, 0, 0).unwrap();
        set(&mut c, 103, 2, 30).unwrap();
        set(&mut c, 100, 1, 5).unwrap();
        assert_eq!(c.boot_plan(), [(103, 30), (202, 0)], "çalışan 100 listede yok; sıralı olan sırasızdan önce");
        c.guest_mut(100).unwrap().status = GuestStatus::Stopped;
        assert_eq!(c.boot_plan(), [(100, 5), (103, 30), (202, 0)]);
        c.set_template(103, true).unwrap();
        assert_eq!(c.boot_plan(), [(100, 5), (202, 0)], "şablon başlatılmaz");
        let t = c.begin_action(202, Action::Start, "u", NOW).unwrap();
        assert_eq!(c.boot_plan(), [(100, 5)], "kilitli konuk atlanır");
        c.complete_task(t, Ok(()), NOW).unwrap();
        assert!(set(&mut c, 100, 1000, 0).is_err() && set(&mut c, 100, 1, 601).is_err());
        assert_eq!(c.guest(100).unwrap().hw.boot_order, 1, "geçersiz değer kaydı değiştirmez");
    }

    #[test]
    fn power_schedules_run_once_within_the_grace_window() {
        use crate::hw::{parse_schedules, SCHEDULE_GRACE_MIN};
        let mut c = cluster();
        let set = |c: &mut Cluster, vmid, text: &str| c.update_guest(vmid, &GuestPatch { schedules: Some(parse_schedules(text).unwrap()), ..patch() }, "u", NOW);
        set(&mut c, 103, "start@08:00@1-5, stop@20:00").unwrap();
        // 2026-11-24 Salı (1), 2026-11-28 Cumartesi (5).
        assert!(c.schedules_due("2026-11-24", 1, 479).is_empty(), "saati gelmeden çalışmaz");
        assert_eq!(c.schedules_due("2026-11-24", 1, 480), [(103, 0, Action::Start)]);
        assert_eq!(c.schedules_due("2026-11-24", 1, 480 + SCHEDULE_GRACE_MIN), [(103, 0, Action::Start)]);
        assert!(c.schedules_due("2026-11-24", 1, 481 + SCHEDULE_GRACE_MIN).is_empty(), "kaçan saat gün içinde telafi edilmez");
        assert!(c.schedules_due("2026-11-28", 5, 480).is_empty(), "hafta sonu çalışmaz");
        assert_eq!(c.schedules_due("2026-11-28", 5, 1200), [(103, 1, Action::Stop)]);

        c.mark_schedule(103, 0, "2026-11-24");
        assert!(c.schedules_due("2026-11-24", 1, 481).is_empty(), "aynı gün ikinci kez çalışmaz");
        assert_eq!(c.schedules_due("2026-11-25", 2, 481), [(103, 0, Action::Start)], "ertesi gün yeniden");
        // Listeyi yeniden kaydetmek değişmeyen satırın kaydını korur; değişen satır yenidir.
        set(&mut c, 103, "stop@20:00, start@08:00@1-5").unwrap();
        assert!(c.schedules_due("2026-11-24", 1, 481).is_empty());
        set(&mut c, 103, "start@08:01@1-5").unwrap();
        assert_eq!(c.schedules_due("2026-11-24", 1, 481), [(103, 0, Action::Start)]);

        let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        assert!(c.schedules_due("2026-11-24", 1, 481).is_empty(), "kilitli konuk bekletilir");
        c.complete_task(t, Ok(()), NOW).unwrap();
        c.mark_schedule(103, 9, "2026-11-24");
        c.mark_schedule(999, 0, "2026-11-24");
        assert!(set(&mut c, 103, "").is_ok() && c.guest(103).unwrap().hw.schedules.is_empty(), "boş liste zamanlamayı kaldırır");
        let many = vec![parse_schedules("stop@01:00").unwrap().remove(0); 9];
        assert!(c.update_guest(103, &GuestPatch { schedules: Some(many), ..patch() }, "u", NOW).is_err());
    }

    #[test]
    fn rate_limits_need_a_stopped_vm_and_a_bridged_nic() {
        let mut c = cluster();
        let limits = |disk, iops, net| GuestPatch { disk_mbps: Some(disk), disk_iops: Some(iops), net_mbps: Some(net), ..patch() };
        assert!(matches!(c.update_guest(100, &limits(50, 0, 0), "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
        assert!(c.update_guest(100, &limits(0, 0, 0), "u", NOW).is_ok(), "aynı değer değişiklik sayılmaz");
        c.update_guest(103, &limits(50, 2000, 100), "u", NOW).unwrap();
        let hw = c.guest(103).unwrap().hw.clone();
        assert_eq!((hw.disk_mbps, hw.disk_iops, hw.net_mbps), (50, 2000, 100));
        // Köprüden NAT'a geçmek, ağ sınırı dururken reddedilir; sınırla birlikte kaldırılırsa olur.
        let nat = || GuestPatch { bridge: Some("nat".into()), ..patch() };
        assert!(c.update_guest(103, &nat(), "u", NOW).unwrap_err().to_string().contains("köprüye bağlı"));
        assert_eq!(c.guest(103).unwrap().bridge, "vmbr0");
        c.update_guest(103, &GuestPatch { net_mbps: Some(0), ..nat() }, "u", NOW).unwrap();
        assert!(c.update_guest(103, &limits(50, 2000, 10), "u", NOW).is_err(), "NAT'ta ağ sınırı konamaz");
        assert!(c.update_guest(103, &GuestPatch { nics: Some(vec!["vmbr1".into()]), ..limits(50, 2000, 10) }, "u", NOW).is_ok(), "köprüdeki ek kart yeter");
        assert!(c.update_guest(103, &limits(10_001, 0, 0), "u", NOW).is_err());
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
        assert!(c.update_guest(ct, &limits(1, 0, 0), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    }

    #[test]
    fn devices_can_be_added_to_a_running_vm_only_when_hot() {
        let mut c = Cluster::demo(NOW);
        let find = |c: &Cluster, kind, status| c.guests.iter().find(|g| g.kind == kind && g.status == status).unwrap().vmid;
        let (running, stopped, paused) = (find(&c, GuestKind::Vm, GuestStatus::Running), find(&c, GuestKind::Vm, GuestStatus::Stopped), find(&c, GuestKind::Vm, GuestStatus::Paused));
        let ct = find(&c, GuestKind::Ct, GuestStatus::Running);

        assert!(matches!(c.add_disk(running, 5), Err(Error::RequiresStopped { .. })));
        assert_eq!(c.add_disk_hot(running, 5).unwrap(), 1);
        assert_eq!(c.add_disk_hot(stopped, 5).unwrap(), 1, "kapalı konukta da çalışır");
        assert!(matches!(c.add_disk_hot(paused, 5), Err(Error::RequiresStopped { .. })), "duraklatılmış konuğa takılmaz");
        assert!(c.add_disk_hot(ct, 5).is_err(), "konteynere takılmaz");
        // Çıkarmak her zaman kapalı konuk ister; takılamayan diskin kaydı ise durum denetimi olmadan geri alınır.
        assert!(matches!(c.remove_disk(running, 1), Err(Error::RequiresStopped { .. })));
        assert_eq!(c.forget_disk(running, 1).unwrap().size_gb, 5);
        assert!(c.guest(running).unwrap().hw.disks.is_empty() && c.forget_disk(running, 1).is_err());

        assert!(matches!(c.add_nic(running, "vmbr1", false), Err(Error::RequiresStopped { .. })));
        assert_eq!(c.add_nic(running, "vmbr1", true).unwrap(), 1);
        assert_eq!(c.add_nic(running, "nat", true).unwrap(), 2);
        assert!(c.add_nic(running, "br0,script=/x", true).is_err());
        assert_eq!(c.add_nic(running, "vmbr2", true).unwrap(), 3);
        assert!(c.add_nic(running, "vmbr3", true).unwrap_err().to_string().contains("en fazla 3"));
        assert_eq!(c.guest(running).unwrap().hw.nics, ["vmbr1", "nat", "vmbr2"]);
        c.forget_nic(running);
        assert_eq!(c.guest(running).unwrap().hw.nics, ["vmbr1", "nat"]);
        assert_eq!(c.add_nic(stopped, "nat", false).unwrap(), 1);
        assert!(c.add_nic(ct, "nat", true).is_err() && c.add_nic(paused, "nat", true).is_err());
        c.set_template(stopped, true).unwrap();
        assert!(c.add_nic(stopped, "nat", false).is_err(), "şablonun donanımı değişmez");
    }

    #[test]
    fn guests_move_between_storages_with_their_share() {
        use crate::model::{Storage, StorageKind};
        let mut c = cluster();
        let node = c.guest(103).unwrap().node.clone();
        c.storages.push(Storage { id: "hizli".into(), node: Some(node.clone()), kind: StorageKind::Dir, content: vec!["disk".into()], used_gb: 10, total_gb: 1000 });
        c.storages.push(Storage { id: "dar".into(), node: Some(node.clone()), kind: StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 50 });
        c.storages.push(Storage { id: "baska-dugum".into(), node: Some("yok".into()), kind: StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 1000 });
        c.add_disk(103, 20).unwrap();
        let size = c.guest(103).unwrap().total_disk_gb();
        let from = c.guest(103).unwrap().storage.clone();
        let used = |c: &Cluster, id: &str| c.storages.iter().find(|s| s.id == id && s.available_on(&node)).unwrap().used_gb;
        let before = used(&c, &from);

        assert!(c.check_storage_move(103, &from).unwrap_err().to_string().contains("zaten"));
        assert!(c.check_storage_move(103, "nfs-yedek").unwrap_err().to_string().contains("konuk diski tutmuyor"));
        assert!(matches!(c.check_storage_move(103, "dar"), Err(Error::InsufficientStorage { .. })));
        assert!(c.check_storage_move(103, "baska-dugum").is_err() && c.check_storage_move(103, "yok").is_err());
        c.check_storage_move(103, "hizli").unwrap();
        c.apply_storage_move(103, "hizli");
        assert_eq!((c.guest(103).unwrap().storage.as_str(), used(&c, "hizli"), used(&c, &from)), ("hizli", 10 + size, before - size), "ek diskler dahil bütün pay taşınır");

        // Şablon ve bağlı klon: disk yolları birbirine bağlı olduğu için taşınmaz; askıdaki konuk da taşınmaz.
        c.set_template(103, true).unwrap();
        let linked = c.clone_guest_as(103, "bagli", true).unwrap();
        assert!(c.check_storage_move(103, &from).unwrap_err().to_string().contains("bağlı klonları olan şablon"));
        assert!(c.check_storage_move(linked, &from).unwrap_err().to_string().contains("bağlı klon taşınamaz"));
        let full = c.clone_guest_as(103, "tam", false).unwrap();
        c.check_storage_move(full, &from).unwrap();
        c.guest_mut_for_import(full).unwrap().hw.suspended = true;
        assert!(c.check_storage_move(full, &from).is_err(), "askıdaki konuğun bellek durumu şimdiki diskine aittir");
    }

    #[test]
    fn guests_without_a_recent_backup_are_unprotected() {
        use crate::retention::DAY_SECS;
        let mut c = cluster();
        let all = c.guests.len();
        assert_eq!(c.unprotected(NOW, 7).len(), all, "hiç yedeği olmayan her konuk korumasızdır");
        let add = |c: &mut Cluster, id: &str, vmid, age_days: u64| c.add_backup(Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW - age_days * DAY_SECS, size_mb: 1, auto: false, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        add(&mut c, "yeni", 100, 1);
        add(&mut c, "eski", 101, 30);
        add(&mut c, "daha-eski", 101, 60);
        let list = c.unprotected(NOW, 7);
        assert!(!list.iter().any(|(vmid, _, _)| *vmid == 100), "bir gün önce yedeklenen konuk korunuyor");
        assert_eq!(list.iter().find(|(vmid, _, _)| *vmid == 101).unwrap().2, Some(NOW - 30 * DAY_SECS), "en yeni yedeğin anı döner");
        assert!(!c.unprotected(NOW, 45).iter().any(|(vmid, _, _)| *vmid == 101), "süre uzayınca korunuyor sayılır");
        c.set_template(103, true).unwrap();
        assert!(!c.unprotected(NOW, 7).iter().any(|(vmid, _, _)| *vmid == 103), "şablonlar sayılmaz");
    }

    #[test]
    fn archived_backups_return_as_manual_backups() {
        let mut c = cluster();
        let b = |id: &str, created| Backup { id: id.into(), vmid: 103, guest_name: "x".into(), created, size_mb: 5, auto: true, offsite: true, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: true, job: Some("gece".into()), machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 1 };
        c.add_backup(b("yeni", NOW));
        c.archive_backup(b("eski", NOW - 100));
        c.archive_backup(b("daha-eski", NOW - 200));
        c.archive_backup(b("eski", NOW - 100));
        assert_eq!(c.archive.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), ["eski", "daha-eski"], "yinelenmez, en yeni başta");
        c.unarchive_backup("daha-eski").unwrap();
        let back = c.backup("daha-eski").unwrap();
        assert_eq!((back.auto, back.offsite, back.job.clone(), back.copies, back.offsite_bound), (false, false, None, 1, true));
        assert_eq!(c.backups.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), ["yeni", "daha-eski"], "tarih sırası korunur");
        assert!(matches!(c.unarchive_backup("yok"), Err(Error::BackupNotFound(_))));
        c.drop_archived("eski");
        assert!(c.archive.is_empty());
    }

    #[test]
    fn boot_test_checks_are_validated_and_change_while_running() {
        let mut c = cluster();
        c.update_guest(100, &GuestPatch { test_port: Some(22), test_cmd: Some(" /usr/local/bin/saglik.sh --hizli ".into()), ..patch() }, "u", NOW).unwrap();
        let hw = c.guest(100).unwrap().hw.clone();
        assert_eq!((hw.test_port, hw.test_cmd.as_str(), hw.test_fwd), (22, "/usr/local/bin/saglik.sh --hizli", 0));
        assert!(c.update_guest(100, &GuestPatch { test_cmd: Some("saglik.sh".into()), ..patch() }, "u", NOW).unwrap_err().to_string().contains("mutlak bir program yoluyla"));
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
        assert!(c.update_guest(ct, &GuestPatch { test_port: Some(22), ..patch() }, "u", NOW).is_err());
        c.set_test_forward(100, 40123);
        assert_eq!(c.guest(100).unwrap().hw.test_fwd, 40123);
        c.update_guest(100, &GuestPatch { test_port: Some(0), test_cmd: Some(String::new()), ..patch() }, "u", NOW).unwrap();
    }

    #[test]
    fn rpo_overrides_the_general_backup_age_and_anomalies_are_flagged() {
        use crate::retention::DAY_SECS;
        let mut c = cluster();
        let total = c.guests.iter().filter(|g| !g.hw.template).count();
        assert_eq!(c.unprotected(NOW, 0).len(), 0, "genel ayar kapalı, hedefi olan konuk yok");
        // 100'ün hedefi 4 saat: genel ayar kapalıyken de denetlenir.
        c.update_guest(100, &GuestPatch { rpo_hours: Some(4), ..patch() }, "u", NOW).unwrap();
        assert_eq!(c.unprotected(NOW, 0).iter().map(|u| u.0).collect::<Vec<_>>(), [100]);
        assert!(c.update_guest(100, &GuestPatch { rpo_hours: Some(9000), ..patch() }, "u", NOW).is_err());
        let add = |c: &mut Cluster, id: &str, vmid, age_secs: u64, new_mb: Option<u64>| {
            c.add_backup(Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW - age_secs, size_mb: 20_000, auto: false, offsite: false, new_mb, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        };
        // 5 saatlik yedek 4 saatlik hedefi kaçırır, 7 günlük genel ayarı kaçırmaz.
        add(&mut c, "a", 100, 5 * 3600, None);
        add(&mut c, "b", 103, 5 * 3600, None);
        assert!(c.unprotected(NOW, 7).iter().any(|u| u.0 == 100) && !c.unprotected(NOW, 7).iter().any(|u| u.0 == 103));
        assert_eq!(c.unprotected(NOW, 7).len(), total - 1);
        add(&mut c, "c", 100, 3600, None);
        assert!(!c.unprotected(NOW, 0).iter().any(|u| u.0 == 100));

        // Olağan dışı değişim: üç olağan artımlı yedekten sonra diskin yarısı değişir.
        for (i, mb) in [100u64, 140, 90].iter().enumerate() {
            add(&mut c, &format!("n{i}"), 103, 4000 - i as u64 * DAY_SECS / 100, Some(*mb));
        }
        assert!(c.backups.iter().all(|b| b.suspicious.is_none()));
        add(&mut c, "sifreli", 103, 10, Some(10_000));
        let flagged = c.backups.iter().find(|b| b.id == "sifreli").unwrap();
        assert!(flagged.suspicious.as_deref().is_some_and(|n| n.contains("fidye yazılımı")), "{:?}", flagged.suspicious);
        add(&mut c, "baska", 100, 5, Some(10_000));
        assert!(c.backups[0].suspicious.is_none(), "başka konuğun geçmişi sayılmaz");
    }

    #[test]
    fn nested_virtualization_is_a_hardware_change() {
        let mut c = cluster();
        let on = GuestPatch { nested: Some(true), ..patch() };
        assert!(matches!(c.update_guest(100, &on, "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
        c.update_guest(103, &on, "u", NOW).unwrap();
        assert!(c.guest(103).unwrap().hw.nested);
        assert!(c.update_guest(103, &GuestPatch { cpu_type: Some("qemu64".into()), ..patch() }, "u", NOW).is_err(), "uzantıyı taşıyamayan model reddedilir");
        c.update_guest(103, &GuestPatch { cpu_type: Some("qemu64".into()), nested: Some(false), ..patch() }, "u", NOW).unwrap();
    }

    #[test]
    fn nic_guard_is_a_vm_option_that_changes_while_running() {
        let mut c = cluster();
        // 100 çalışıyor: koruma donanım değişikliği değildir, kapatmadan açılır.
        c.update_guest(100, &GuestPatch { nic_guard: Some(true), ..patch() }, "u", NOW).unwrap();
        assert!(c.guest(100).unwrap().hw.nic_guard);
        c.update_guest(100, &GuestPatch { nic_guard: Some(false), ..patch() }, "u", NOW).unwrap();
        assert!(!c.guest(100).unwrap().hw.nic_guard);
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
        assert!(c.update_guest(ct, &GuestPatch { nic_guard: Some(true), ..patch() }, "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    }

    #[test]
    fn dynamic_memory_needs_a_balloon_and_sane_bounds() {
        let mut c = cluster();
        assert!(c.set_dynamic_memory(103, 1024).unwrap_err().to_string().contains("bellek balonu yok"));
        c.update_guest(103, &GuestPatch { balloon: Some(true), ..patch() }, "u", NOW).unwrap();
        let total = c.guest(103).unwrap().mem_total_mb;
        assert!(c.set_dynamic_memory(103, 100).is_err() && c.set_dynamic_memory(103, total + 1).is_err());
        c.set_dynamic_memory(103, 1024).unwrap();
        assert!(c.set_balloon_target(103, 2048).unwrap_err().to_string().contains("dinamik bellek açık"));
        c.set_dynamic_target(103, 1536);
        assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 1536);
        c.set_dynamic_target(103, total);
        assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0, "tamamı 0 olarak saklanır");
        // Kapatınca elle hedef yeniden verilebilir; kapalıyken hesaplanan hedef kaydedilmez.
        c.set_dynamic_memory(103, 0).unwrap();
        c.set_dynamic_target(103, 1536);
        assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0);
        c.set_balloon_target(103, 2048).unwrap();
        // Balon çıkarılınca dinamik bellek de kapanır.
        c.set_balloon_target(103, 0).unwrap();
        c.set_dynamic_memory(103, 1024).unwrap();
        c.update_guest(103, &GuestPatch { balloon: Some(false), ..patch() }, "u", NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().hw.mem_min_mb, 0);
    }

    #[test]
    fn production_checkpoints_need_a_stopped_guest_to_roll_back() {
        let mut c = cluster();
        c.add_snapshot(100, "bellekli", NOW);
        c.add_snapshot_kind(100, "uretim", NOW, true, true);
        let snaps = c.guest(100).unwrap().snapshots.clone();
        assert_eq!(snaps.iter().map(|s| (s.name.as_str(), s.disk_only, s.quiesced)).collect::<Vec<_>>(), [("bellekli", false, false), ("uretim", true, true)]);
        // 100 çalışıyor: bellekli görüntüye dönülebilir, belleksize dönmek için kapatmak gerekir.
        c.check_rollback_state(100, "bellekli").unwrap();
        assert!(matches!(c.check_rollback_state(100, "uretim"), Err(Error::RequiresStopped { .. })));
        c.add_snapshot_kind(103, "uretim", NOW, true, false);
        c.check_rollback_state(103, "uretim").unwrap();
        // Eski kayıtlar (alanlar yok) bellekli sayılır.
        let old: Snapshot = serde_json::from_str(r#"{"name":"eski","created":1}"#).unwrap();
        assert!(!old.disk_only && !old.quiesced);
    }

    #[test]
    fn instantly_recovered_guests_pin_their_backup() {
        let mut c = cluster();
        c.update_guest(103, &GuestPatch { backup_keep: Some(1), ..patch() }, "u", NOW).unwrap();
        for (i, id) in ["eski", "yeni"].iter().enumerate() {
            c.add_backup(Backup { id: id.to_string(), vmid: 103, guest_name: "x".into(), created: NOW + i as u64, size_mb: 1, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        }
        assert_eq!(c.prunable_backups(NOW), ["eski"]);
        assert!(c.check_detach(100).unwrap_err().to_string().contains("üzerinde çalışmıyor"));
        // 100 numaralı konuk "eski" yedeğinin üzerinde çalışıyor: yedek silinemez ve budanmaz.
        c.guest_mut_for_import(100).unwrap().hw.instant = Some("eski".into());
        assert!(c.backup_deletable("eski", NOW).unwrap_err().to_string().contains("bu yedeğin üzerinde çalışıyor"));
        assert!(c.prunable_backups(NOW).is_empty());
        c.backup_deletable("yeni", NOW).unwrap();
        c.check_detach(100).unwrap();
        c.apply_detach(100);
        assert_eq!(c.guest(100).unwrap().hw.instant, None);
        c.backup_deletable("eski", NOW).unwrap();
        assert_eq!(c.prunable_backups(NOW), ["eski"]);
    }

    #[test]
    fn imported_disks_keep_their_numbers_and_serials() {
        use crate::hw::ExtraDisk;
        let mut c = cluster();
        let used = |c: &Cluster| c.storages.iter().map(|s| s.used_gb).sum::<u64>();
        let before = used(&c);
        c.add_disk_exact(103, &ExtraDisk { id: 5, size_gb: 10, serial: 9 }).unwrap();
        c.add_disk_exact(103, &ExtraDisk { id: 2, size_gb: 4, serial: 3 }).unwrap();
        let hw = c.guest(103).unwrap().hw.clone();
        assert_eq!(hw.disks.iter().map(|d| (d.id, d.serial)).collect::<Vec<_>>(), [(2, 3), (5, 9)], "numara ve seri korunur, sıralı durur");
        assert_eq!((hw.disk_seq, used(&c) - before), (9, 14), "sonraki disk çakışmayan bir seri alır; yer ayrılır");
        assert_eq!(c.add_disk(103, 1).unwrap(), 1);
        assert_eq!(c.guest(103).unwrap().hw.disks.iter().find(|d| d.id == 1).unwrap().serial, 10);
        for bad in [ExtraDisk { id: 5, size_gb: 1, serial: 1 }, ExtraDisk { id: 0, size_gb: 1, serial: 1 }, ExtraDisk { id: 99, size_gb: 1, serial: 1 }, ExtraDisk { id: 3, size_gb: 0, serial: 1 }] {
            assert!(c.add_disk_exact(103, &bad).is_err(), "{bad:?}");
        }
        assert!(matches!(c.add_disk_exact(103, &ExtraDisk { id: 3, size_gb: 1_000_000, serial: 1 }), Err(Error::InsufficientStorage { .. })));
    }

    #[test]
    fn cpu_type_and_balloon_follow_hardware_rules() {
        let mut c = cluster();
        let set = |cpu: &str, balloon: bool| GuestPatch { cpu_type: Some(cpu.into()), balloon: Some(balloon), ..patch() };
        assert!(matches!(c.update_guest(100, &set("Haswell", true), "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
        c.update_guest(100, &set("", false), "u", NOW).unwrap();
        assert!(c.update_guest(103, &set("Pentium9", false), "u", NOW).unwrap_err().to_string().contains("bilinmeyen işlemci türü"));
        assert!(c.set_balloon_target(103, 4096).unwrap_err().to_string().contains("bellek balonu yok"));
        c.update_guest(103, &set("Haswell", true), "u", NOW).unwrap();
        let total = c.guest(103).unwrap().mem_total_mb;
        assert_eq!((c.guest(103).unwrap().hw.cpu_type.as_str(), c.guest(103).unwrap().hw.balloon), ("Haswell", true));
        // Hedef: alt sınır ile bellek arasında; 0 ya da belleğin tamamı balonu boşaltır.
        assert_eq!(c.set_balloon_target(103, 4096).unwrap(), 0);
        assert_eq!(c.set_balloon_target(103, 8192).unwrap(), 4096, "önceki hedef döner");
        assert!(c.set_balloon_target(103, 100).is_err() && c.set_balloon_target(103, total + 1).is_err());
        c.set_balloon_target(103, total).unwrap();
        assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0);
        c.set_balloon_target(103, 4096).unwrap();
        c.restore_balloon_target(103, 8192);
        assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 8192);
        // Bellek hedefin altına inerse ya da balon kaldırılırsa hedef sıfırlanır.
        c.update_guest(103, &GuestPatch { memory_mb: Some(4096), ..patch() }, "u", NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0);
        c.set_balloon_target(103, 2048).unwrap();
        c.update_guest(103, &set("", false), "u", NOW).unwrap();
        let hw = c.guest(103).unwrap().hw.clone();
        assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("", false, 0));
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
        assert!(c.update_guest(ct, &set("Haswell", false), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
        assert!(c.set_balloon_target(ct, 512).is_err());
    }

    #[test]
    fn cloud_init_is_edited_on_a_stopped_vm() {
        const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGtMF0qv2yBAZ8lfmTtk1mKkZHjPl7Yj8d0Lr0oQ1X2w ali@dizustu";
        let mut c = cluster();
        let ci = |key: &str, ip: &str| GuestPatch { ci_user: Some("ali".into()), ci_ssh_key: Some(key.into()), ci_ip: Some(ip.into()), ci_gw: Some(String::new()), ci_dns: Some(String::new()), ..patch() };
        assert!(matches!(c.update_guest(100, &ci(KEY, ""), "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
        c.update_guest(103, &ci(KEY, "10.0.0.5/24"), "u", NOW).unwrap();
        let hw = c.guest(103).unwrap().hw.clone();
        assert_eq!((hw.ci_user.as_deref(), hw.ci_ip.as_deref(), hw.ci_gen, hw.cloud_init()), (Some("ali"), Some("10.0.0.5/24"), 1, true));
        // Aynı değerleri yeniden kaydetmek sürümü artırmaz; başka bir ayar değişikliği de artırmaz.
        c.update_guest(103, &ci(KEY, "10.0.0.5/24"), "u", NOW).unwrap();
        c.update_guest(103, &GuestPatch { cores: Some(2), ..patch() }, "u", NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().hw.ci_gen, 1);
        assert!(c.update_guest(103, &ci(KEY, "10.0.0.500/24"), "u", NOW).unwrap_err().to_string().contains("geçersiz cloud-init adresi"));
        assert!(c.update_guest(103, &ci("ssh-rsa kisa", ""), "u", NOW).unwrap_err().to_string().contains("geçersiz SSH"));
        assert_eq!(c.guest(103).unwrap().hw.ci_gen, 1, "reddedilen değişiklik kaydı bozmaz");
        // Anahtarı boşaltmak cloud-init'i kapatır.
        c.update_guest(103, &ci("", "10.0.0.5/24"), "u", NOW).unwrap();
        let hw = c.guest(103).unwrap().hw.clone();
        assert_eq!((hw.cloud_init(), hw.ci_user, hw.ci_ip, hw.ci_gen), (false, None, None, 2));
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
        assert!(c.update_guest(ct, &ci(KEY, ""), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    }

    #[test]
    fn vlan_tags_need_a_stopped_vm_on_a_bridge() {
        let mut c = cluster();
        let vlan = |v| GuestPatch { vlan: Some(v), ..patch() };
        assert!(matches!(c.update_guest(100, &vlan(20), "u", NOW), Err(Error::RequiresStopped { .. })));
        c.update_guest(103, &vlan(20), "u", NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().hw.vlan, 20);
        assert!(c.update_guest(103, &vlan(4095), "u", NOW).is_err());
        // NAT'a geçmek etiket dururken reddedilir; etiketle birlikte kaldırılırsa olur.
        let nat = || GuestPatch { bridge: Some("nat".into()), ..patch() };
        assert!(c.update_guest(103, &nat(), "u", NOW).unwrap_err().to_string().contains("VLAN etiketi"));
        c.update_guest(103, &GuestPatch { vlan: Some(0), ..nat() }, "u", NOW).unwrap();
        assert!(c.update_guest(103, &vlan(5), "u", NOW).is_err(), "NAT'taki karta etiket konmaz");
        // Ek kartlar: etiket tanımın içindedir.
        c.update_guest(103, &GuestPatch { nics: Some(vec!["vmbr1:30".into(), "nat".into()]), ..patch() }, "u", NOW).unwrap();
        assert!(c.update_guest(103, &GuestPatch { nics: Some(vec!["nat:30".into()]), ..patch() }, "u", NOW).is_err());
        assert_eq!(c.add_nic(103, "vmbr2:4094", false).unwrap(), 3);
        c.forget_nic(103);
        assert!(c.add_nic(103, "vmbr2:0", false).is_err());
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
        assert!(c.update_guest(ct, &vlan(5), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    }

    #[test]
    fn shares_and_other_hardware_cannot_change_while_suspended() {
        use crate::hw::parse_shares;
        let mut c = cluster();
        let shares = |text: &str| GuestPatch { shares: Some(parse_shares(text).unwrap()), ..patch() };
        assert!(matches!(c.update_guest(100, &shares("ortak"), "u", NOW), Err(Error::RequiresStopped { .. })));
        c.update_guest(103, &shares("ortak, veri:ro"), "u", NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().hw.shares.iter().map(ToString::to_string).collect::<Vec<_>>(), ["ortak", "veri:ro"]);
        assert!(c.update_guest(103, &shares("ortak, ortak"), "u", NOW).is_err());
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
        assert!(c.update_guest(ct, &shares("ortak"), "u", NOW).is_err());

        // Askıdaki konuk: donanım değişmez (bellek durumu şimdiki donanımı bekler); ad ve etiket değişir.
        c.after_suspend(103);
        assert!(c.update_guest(103, &shares(""), "u", NOW).unwrap_err().to_string().contains("askıda"));
        assert!(c.update_guest(103, &GuestPatch { cores: Some(2), ..patch() }, "u", NOW).unwrap_err().to_string().contains("askıda"));
        assert!(c.update_guest(103, &shares("ortak, veri:ro"), "u", NOW).is_ok(), "aynı liste değişiklik sayılmaz");
        assert!(c.update_guest(103, &GuestPatch { tags: Some(vec!["x".into()]), ..patch() }, "u", NOW).is_ok());
        c.clear_suspended(103);
        assert!(c.update_guest(103, &shares(""), "u", NOW).is_ok());
    }
}
