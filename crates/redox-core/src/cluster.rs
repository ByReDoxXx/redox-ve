use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::fmt::pct;
use crate::model::*;

/// Görev günlüğünde tutulan en fazla kayıt.
const MAX_TASKS: usize = 200;

/// Bir kümenin bütün durumu. Değişiklikler yalnızca buradaki metotlarla yapılır;
/// her biri kuralları doğrular ve görev günlüğüne kayıt düşer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    pub name: String,
    pub nodes: Vec<Node>,
    /// Kimliğe göre sıralı tutulur.
    pub guests: Vec<Guest>,
    pub storages: Vec<Storage>,
    /// En yeni görev başta.
    pub tasks: Vec<Task>,
    /// En yeni yedek başta.
    #[serde(default)]
    pub backups: Vec<Backup>,
    next_task_id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClusterSummary {
    pub name: String,
    pub nodes_online: usize,
    pub nodes_total: usize,
    pub quorate: bool,
    pub cores: u32,
    pub cpu_pct: u8,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub mem_pct: u8,
    pub storage_used_gb: u64,
    pub storage_total_gb: u64,
    pub storage_pct: u8,
    pub guests_running: usize,
    pub guests_total: usize,
}

impl Cluster {
    pub fn new(name: impl Into<String>) -> Self {
        Cluster {
            name: name.into(),
            nodes: Vec::new(),
            guests: Vec::new(),
            storages: Vec::new(),
            tasks: Vec::new(),
            backups: Vec::new(),
            next_task_id: 1,
        }
    }

    pub fn node(&self, id: &str) -> Result<&Node, Error> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or_else(|| Error::NodeNotFound(id.to_string()))
    }

    pub fn guest(&self, vmid: VmId) -> Result<&Guest, Error> {
        self.guests
            .iter()
            .find(|g| g.vmid == vmid)
            .ok_or(Error::GuestNotFound(vmid))
    }

    pub(crate) fn guest_mut(&mut self, vmid: VmId) -> Result<&mut Guest, Error> {
        self.guests
            .iter_mut()
            .find(|g| g.vmid == vmid)
            .ok_or(Error::GuestNotFound(vmid))
    }

    pub fn task(&self, id: TaskId) -> Result<&Task, Error> {
        self.tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or(Error::TaskNotFound(id))
    }

    pub fn guests_on<'a>(&'a self, node: &'a str) -> impl Iterator<Item = &'a Guest> {
        self.guests.iter().filter(move |g| g.node == node)
    }

    /// `node` üzerinde kullanılabilen `id` kimlikli depolamanın dizini.
    pub(crate) fn storage_index(&self, id: &str, node: &str) -> Result<usize, Error> {
        if let Some(i) = self
            .storages
            .iter()
            .position(|s| s.id == id && s.available_on(node))
        {
            return Ok(i);
        }
        if self.storages.iter().any(|s| s.id == id) {
            Err(Error::StorageNotOnNode {
                storage: id.to_string(),
                node: node.to_string(),
            })
        } else {
            Err(Error::StorageNotFound(id.to_string()))
        }
    }

    pub fn storage_for(&self, id: &str, node: &str) -> Result<&Storage, Error> {
        self.storage_index(id, node).map(|i| &self.storages[i])
    }

    /// Tek düğümlü, konuksuz küme: gerçek sürücüyle bu makineyi yönetmek için.
    pub fn local(node: Node, storage: Storage) -> Self {
        let mut c = Cluster::new(node.id.clone());
        c.nodes.push(node);
        c.storages.push(storage);
        c
    }

    /// Silinebilir mi? (durdurulmuş ve kilitsiz). Sürücü kaynakları silmeden önce sorulur.
    pub fn check_deletable(&self, vmid: VmId) -> Result<&Guest, Error> {
        let guest = self.guest(vmid)?;
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if guest.status != GuestStatus::Stopped {
            return Err(Error::NotStopped(vmid));
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
        let suffix = format!(" create {vmid}");
        if let Some(task) = self.tasks.iter_mut().find(|t| t.vmid == Some(vmid) && t.command.ends_with(&suffix)) {
            task.status = TaskStatus::Failed;
            task.message = Some(message.to_string());
            task.ended = Some(now);
            task.vmid = None;
        }
    }

    /// Sürücünün gözlediği gerçek durumu işler (ör. konuk kendi kendine kapandıysa).
    /// Süren bir görev varken dokunmaz: sonucu o görev belirler. Durum değiştiyse `true`.
    pub fn observe_guest(&mut self, vmid: VmId, probe: &crate::driver::Probe, now: u64) -> bool {
        let Ok(g) = self.guest_mut(vmid) else { return false };
        if g.lock.is_some() {
            return false;
        }
        let changed = g.status != probe.status;
        g.status = probe.status;
        match probe.status {
            GuestStatus::Stopped => {
                g.cpu_pct = 0;
                g.mem_used_mb = 0;
                g.started_at = None;
            }
            GuestStatus::Running | GuestStatus::Paused => {
                if g.started_at.is_none() {
                    g.started_at = Some(now);
                }
                if let Some(cpu) = probe.cpu_pct {
                    g.cpu_pct = if probe.status == GuestStatus::Paused { 0 } else { cpu.min(100) };
                }
                if let Some(mem) = probe.mem_used_mb {
                    g.mem_used_mb = mem.min(g.mem_total_mb);
                }
            }
        }
        changed
    }

    /// Sürücünün bulduğu ağ adresini işler (bulunamadıysa `None`). Değiştiyse `true`.
    /// Konuk ajanının bildirdiği işletim sistemini (ya da ajanın yokluğunu) kaydeder; değiştiyse `true`.
    pub fn observe_agent(&mut self, vmid: VmId, agent: Option<String>) -> bool {
        match self.guest_mut(vmid) {
            Ok(g) if g.agent != agent => {
                g.agent = agent;
                true
            }
            _ => false,
        }
    }

    pub fn observe_ip(&mut self, vmid: VmId, ip: Option<String>) -> bool {
        match self.guest_mut(vmid) {
            Ok(g) if g.ip != ip => {
                g.ip = ip;
                true
            }
            _ => false,
        }
    }

    /// Düğümün ölçülen kullanımını işler.
    pub fn observe_node(&mut self, id: &str, cpu_pct: u8, mem_used_mb: u64, mem_total_mb: u64, uptime_secs: u64) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.cpu_pct = cpu_pct.min(100);
            n.mem_total_mb = mem_total_mb;
            n.mem_used_mb = mem_used_mb.min(mem_total_mb);
            n.uptime_secs = uptime_secs;
        }
    }

    /// Depolamanın ölçülen doluluğunu işler.
    pub fn observe_storage(&mut self, id: &str, node: &str, used_gb: u64, total_gb: u64) {
        if let Ok(si) = self.storage_index(id, node) {
            self.storages[si].total_gb = total_gb;
            self.storages[si].used_gb = used_gb.min(total_gb);
        }
    }

    pub fn summary(&self) -> ClusterSummary {
        let online: Vec<&Node> = self
            .nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Online)
            .collect();
        let cores: u32 = online.iter().map(|n| n.cores).sum();
        let weighted: u64 = online.iter().map(|n| n.cpu_pct as u64 * n.cores as u64).sum();
        let mem_used_mb: u64 = online.iter().map(|n| n.mem_used_mb).sum();
        let mem_total_mb: u64 = online.iter().map(|n| n.mem_total_mb).sum();
        let storage_used_gb: u64 = self.storages.iter().map(|s| s.used_gb).sum();
        let storage_total_gb: u64 = self.storages.iter().map(|s| s.total_gb).sum();
        ClusterSummary {
            name: self.name.clone(),
            nodes_online: online.len(),
            nodes_total: self.nodes.len(),
            quorate: online.len() * 2 > self.nodes.len(),
            cores,
            cpu_pct: if cores == 0 { 0 } else { (weighted as f64 / cores as f64).round() as u8 },
            mem_used_mb,
            mem_total_mb,
            mem_pct: pct(mem_used_mb, mem_total_mb),
            storage_used_gb,
            storage_total_gb,
            storage_pct: pct(storage_used_gb, storage_total_gb),
            guests_running: self
                .guests
                .iter()
                .filter(|g| g.status == GuestStatus::Running)
                .count(),
            guests_total: self.guests.len(),
        }
    }

    /// Kullanılmayan en küçük geçerli kimlik.
    pub fn next_vmid(&self) -> VmId {
        let mut candidate = VMID_MIN;
        for g in &self.guests {
            if g.vmid == candidate {
                candidate += 1;
            } else if g.vmid > candidate {
                break;
            }
        }
        candidate
    }

    pub fn has_running_tasks(&self) -> bool {
        self.tasks.iter().any(|t| t.status == TaskStatus::Running)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_task(
        &mut self,
        node: &str,
        user: &str,
        command: String,
        status: TaskStatus,
        now: u64,
        vmid: Option<VmId>,
        action: Option<Action>,
    ) -> TaskId {
        let id = self.next_task_id;
        self.next_task_id += 1;
        self.tasks.insert(
            0,
            Task {
                id,
                started: now,
                ended: (status != TaskStatus::Running).then_some(now),
                node: node.to_string(),
                user: user.to_string(),
                command,
                status,
                message: None,
                vmid,
                action,
            },
        );
        // Süren görevler asla atılmaz; yalnızca bitmiş en eski kayıtlar düşer.
        while self.tasks.len() > MAX_TASKS {
            match self.tasks.iter().rposition(|t| t.status != TaskStatus::Running) {
                Some(i) => {
                    self.tasks.remove(i);
                }
                None => break,
            }
        }
        id
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

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn cluster() -> Cluster {
        Cluster::demo(NOW)
    }

    fn spec(name: &str) -> GuestSpec {
        GuestSpec {
            vmid: None,
            name: name.to_string(),
            kind: GuestKind::Vm,
            node: "pve-ist-01".to_string(),
            cores: 2,
            memory_mb: 2048,
            disk_gb: 32,
            storage: "local-zfs".to_string(),
            os: "debian-12".to_string(),
            bridge: "vmbr0".to_string(),
            onboot: false,
            iso: None,
            hw: Default::default(),
        }
    }

    fn run(c: &mut Cluster, vmid: VmId, action: Action) -> Result<TaskId, Error> {
        let id = c.begin_action(vmid, action, "root@pam", NOW)?;
        c.complete_task(id, Ok(()), NOW + 2)?;
        Ok(id)
    }

    #[test]
    fn demo_summary_adds_up() {
        let s = cluster().summary();
        assert_eq!((s.nodes_online, s.nodes_total), (3, 3));
        assert!(s.quorate);
        assert_eq!(s.cores, 80);
        // (34*32 + 61*32 + 12*16) / 80 = 40.4
        assert_eq!(s.cpu_pct, 40);
        assert_eq!(s.mem_total_mb, 320 * 1024);
        assert_eq!(s.mem_pct, 63);
        assert_eq!((s.storage_used_gb, s.storage_total_gb), (9612, 12_500));
        assert_eq!(s.storage_pct, 77);
        assert_eq!((s.guests_running, s.guests_total), (5, 8));
    }

    #[test]
    fn quorum_needs_strict_majority() {
        let mut c = cluster();
        c.nodes[0].status = NodeStatus::Offline;
        assert!(c.summary().quorate, "2/3 çoğunluktur");
        c.nodes[1].status = NodeStatus::Offline;
        assert!(!c.summary().quorate, "1/3 çoğunluk değildir");
        assert_eq!(c.summary().cores, 16, "çevrimdışı düğümler toplamdan düşer");
    }

    #[test]
    fn start_stop_cycle_updates_state_and_uptime() {
        let mut c = cluster();
        assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);

        run(&mut c, 103, Action::Start).unwrap();
        let g = c.guest(103).unwrap();
        assert_eq!(g.status, GuestStatus::Running);
        assert_eq!(g.started_at, Some(NOW + 2));
        assert_eq!(g.uptime_secs(NOW + 62), Some(60));
        assert!(g.mem_used_mb > 0 && g.lock.is_none());

        run(&mut c, 103, Action::Stop).unwrap();
        let g = c.guest(103).unwrap();
        assert_eq!(g.status, GuestStatus::Stopped);
        assert_eq!((g.cpu_pct, g.mem_used_mb, g.started_at), (0, 0, None));
    }

    #[test]
    fn pause_resume_and_reboot() {
        let mut c = cluster();
        run(&mut c, 100, Action::Pause).unwrap();
        assert_eq!(c.guest(100).unwrap().status, GuestStatus::Paused);
        run(&mut c, 100, Action::Resume).unwrap();
        assert_eq!(c.guest(100).unwrap().status, GuestStatus::Running);

        let before = c.guest(100).unwrap().started_at;
        run(&mut c, 100, Action::Reboot).unwrap();
        let g = c.guest(100).unwrap();
        assert_eq!(g.status, GuestStatus::Running);
        assert_ne!(g.started_at, before, "yeniden başlatma çalışma süresini sıfırlar");
    }

    #[test]
    fn invalid_transitions_are_rejected_without_side_effects() {
        let mut c = cluster();
        let before = c.clone();
        assert_eq!(
            c.begin_action(100, Action::Start, "u", NOW),
            Err(Error::InvalidTransition {
                vmid: 100,
                action: Action::Start,
                status: GuestStatus::Running
            })
        );
        assert_eq!(
            c.begin_action(103, Action::Reboot, "u", NOW).unwrap_err().kind(),
            crate::ErrorKind::Conflict
        );
        assert_eq!(c.begin_action(999, Action::Start, "u", NOW), Err(Error::GuestNotFound(999)));
        assert_eq!(c, before, "reddedilen eylem durum ya da görev bırakmamalı");
    }

    #[test]
    fn guest_is_locked_while_a_task_runs() {
        let mut c = cluster();
        let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        assert!(c.has_running_tasks());
        assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped, "bitene kadar durum değişmez");
        assert_eq!(
            c.begin_action(103, Action::Start, "u", NOW),
            Err(Error::Locked { vmid: 103, task: t })
        );
        assert_eq!(c.delete_guest(103, "u", NOW), Err(Error::Locked { vmid: 103, task: t }));

        c.complete_task(t, Ok(()), NOW + 1).unwrap();
        assert!(!c.has_running_tasks());
        assert_eq!(c.complete_task(t, Ok(()), NOW + 1), Err(Error::TaskNotFound(t)), "görev iki kez kapanmaz");
    }

    #[test]
    fn failed_task_leaves_guest_unchanged_and_unlocked() {
        let mut c = cluster();
        let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        c.complete_task(t, Err("depolama kilidi alınamadı".into()), NOW + 1).unwrap();
        let g = c.guest(103).unwrap();
        assert_eq!((g.status, g.lock), (GuestStatus::Stopped, None));
        let task = c.task(t).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.message.as_deref(), Some("depolama kilidi alınamadı"));
        assert_eq!(task.ended, Some(NOW + 1));
        // Başarısızlıktan sonra yeniden denenebilir.
        assert!(c.begin_action(103, Action::Start, "u", NOW + 2).is_ok());
    }

    #[test]
    fn action_refused_on_offline_node() {
        let mut c = cluster();
        c.nodes[1].status = NodeStatus::Offline;
        assert_eq!(
            c.begin_action(103, Action::Start, "u", NOW),
            Err(Error::NodeOffline("pve-ist-02".into()))
        );
    }

    #[test]
    fn next_vmid_fills_first_gap() {
        let mut c = cluster();
        assert_eq!(c.next_vmid(), 105, "100-104 dolu, 200+ ayrı blok");
        c.guests.retain(|g| g.vmid != 101);
        assert_eq!(c.next_vmid(), 101);
        c.guests.clear();
        assert_eq!(c.next_vmid(), 100);
    }

    #[test]
    fn create_inserts_sorted_reserves_disk_and_logs() {
        let mut c = cluster();
        let free_before = c.storage_for("local-zfs", "pve-ist-01").unwrap().free_gb();
        let vmid = c.create_guest(&spec("yeni-vm"), "ayse@pve", NOW).unwrap();
        assert_eq!(vmid, 105);
        let ids: Vec<VmId> = c.guests.iter().map(|g| g.vmid).collect();
        assert_eq!(ids, [100, 101, 102, 103, 104, 105, 200, 201, 202]);
        let g = c.guest(105).unwrap();
        assert_eq!((g.status, g.mem_total_mb, g.ip.as_deref()), (GuestStatus::Stopped, 2048, None));
        assert_eq!(c.storage_for("local-zfs", "pve-ist-01").unwrap().free_gb(), free_before - 32);
        // Aynı adlı depolama başka düğümde etkilenmez.
        assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, 1390);
        let t = &c.tasks[0];
        assert_eq!((t.command.as_str(), t.user.as_str(), t.status), ("vm create 105", "ayse@pve", TaskStatus::Ok));
    }

    #[test]
    fn create_validates_every_field() {
        let mut c = cluster();
        let before = c.clone();
        let cases: Vec<(GuestSpec, Error)> = vec![
            (GuestSpec { vmid: Some(100), ..spec("a") }, Error::VmidTaken(100)),
            (GuestSpec { vmid: Some(99), ..spec("a") }, Error::InvalidVmid(99)),
            (GuestSpec { vmid: Some(1_000_000_000), ..spec("a") }, Error::InvalidVmid(1_000_000_000)),
            (spec("Kötü Ad"), Error::InvalidName("Kötü Ad".into())),
            (GuestSpec { node: "yok".into(), ..spec("a") }, Error::NodeNotFound("yok".into())),
            (GuestSpec { storage: "yok".into(), ..spec("a") }, Error::StorageNotFound("yok".into())),
            (
                GuestSpec { storage: "local-lvm".into(), ..spec("a") },
                Error::StorageNotOnNode { storage: "local-lvm".into(), node: "pve-ist-01".into() },
            ),
            (
                GuestSpec { disk_gb: 5000, ..spec("a") },
                Error::InsufficientStorage { storage: "local-zfs".into(), needed_gb: 5000, free_gb: 1188 },
            ),
        ];
        for (s, expected) in cases {
            assert_eq!(c.create_guest(&s, "u", NOW), Err(expected));
        }
        for s in [
            GuestSpec { cores: 0, ..spec("a") },
            GuestSpec { cores: 33, ..spec("a") },
            GuestSpec { memory_mb: 32, ..spec("a") },
            GuestSpec { memory_mb: 999_999_999, ..spec("a") },
            GuestSpec { disk_gb: 0, ..spec("a") },
        ] {
            assert!(matches!(c.create_guest(&s, "u", NOW), Err(Error::InvalidSpec(_))), "{s:?}");
        }
        assert_eq!(c, before, "reddedilen istek hiçbir şeyi değiştirmemeli");
    }

    #[test]
    fn container_may_use_less_memory_than_vm_and_shared_storage() {
        let mut c = cluster();
        let s = GuestSpec { kind: GuestKind::Ct, memory_mb: 32, storage: "nfs-yedek".into(), ..spec("kucuk") };
        let id = c.create_guest(&s, "u", NOW).unwrap();
        assert_eq!(c.guest(id).unwrap().kind, GuestKind::Ct);
        assert_eq!(c.tasks[0].command, format!("ct create {id}"));
    }

    #[test]
    fn create_refused_on_offline_node() {
        let mut c = cluster();
        c.nodes[0].status = NodeStatus::Offline;
        assert_eq!(c.create_guest(&spec("a"), "u", NOW), Err(Error::NodeOffline("pve-ist-01".into())));
    }

    #[test]
    fn delete_requires_stopped_and_frees_disk() {
        let mut c = cluster();
        assert_eq!(c.delete_guest(100, "u", NOW), Err(Error::NotStopped(100)));
        assert_eq!(c.delete_guest(104, "u", NOW), Err(Error::NotStopped(104)), "duraklatılmış da silinemez");
        assert_eq!(c.delete_guest(999, "u", NOW), Err(Error::GuestNotFound(999)));

        let used = c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb;
        c.delete_guest(103, "u", NOW).unwrap();
        assert_eq!(c.guest(103), Err(Error::GuestNotFound(103)));
        assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used - 200);
        assert_eq!(c.tasks[0].command, "vm destroy 103");
    }

    #[test]
    fn create_then_delete_restores_storage() {
        let mut c = cluster();
        let before = c.storages.clone();
        let id = c.create_guest(&spec("gecici"), "u", NOW).unwrap();
        c.delete_guest(id, "u", NOW).unwrap();
        assert_eq!(c.storages, before);
    }

    #[test]
    fn task_log_is_capped_but_keeps_running_tasks() {
        let mut c = cluster();
        let running = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        for i in 0..(MAX_TASKS as u64 + 50) {
            run(&mut c, 100, if i % 2 == 0 { Action::Pause } else { Action::Resume }).unwrap();
        }
        assert_eq!(c.tasks.len(), MAX_TASKS);
        assert_eq!(c.task(running).unwrap().status, TaskStatus::Running);
        let ids: Vec<TaskId> = c.tasks.iter().map(|t| t.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(ids, sorted, "günlük en yeniden eskiye sıralı kalır");
    }

    #[test]
    fn iso_is_validated_and_only_for_vms() {
        let mut c = cluster();
        let with_iso = |iso: &str| GuestSpec { iso: Some(iso.to_string()), ..spec("a") };
        assert!(matches!(c.create_guest(&with_iso("../x.iso"), "u", NOW), Err(Error::InvalidSpec(_))));
        assert!(matches!(c.create_guest(&with_iso("x.img"), "u", NOW), Err(Error::InvalidSpec(_))));
        let ct = GuestSpec { kind: GuestKind::Ct, ..with_iso("debian.iso") };
        assert!(matches!(c.create_guest(&ct, "u", NOW), Err(Error::InvalidSpec(_))), "konteynere ISO takılmaz");
        let id = c.create_guest(&with_iso("debian.iso"), "u", NOW).unwrap();
        assert_eq!(c.guest(id).unwrap().iso.as_deref(), Some("debian.iso"));
    }

    #[test]
    fn abort_create_rolls_everything_back() {
        let mut c = cluster();
        let before = c.storages.clone();
        let id = c.create_guest(&spec("yarim"), "u", NOW).unwrap();
        c.abort_create(id, "qemu-img bulunamadı", NOW + 1);
        assert_eq!(c.guest(id), Err(Error::GuestNotFound(id)));
        assert_eq!(c.storages, before, "disk payı iade edilmeli");
        let t = &c.tasks[0];
        assert_eq!((t.status, t.message.as_deref()), (TaskStatus::Failed, Some("qemu-img bulunamadı")));
        assert_eq!(c.next_vmid(), id, "kimlik yeniden kullanılabilir");
        c.abort_create(999, "yok", NOW); // bilinmeyen kimlik sessizce geçilir
    }

    #[test]
    fn check_deletable_matches_delete_rules() {
        let mut c = cluster();
        assert!(c.check_deletable(103).is_ok());
        assert_eq!(c.check_deletable(100).unwrap_err(), Error::NotStopped(100));
        assert_eq!(c.check_deletable(999).unwrap_err(), Error::GuestNotFound(999));
        let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        assert_eq!(c.check_deletable(103).unwrap_err(), Error::Locked { vmid: 103, task: t });
    }

    #[test]
    fn observed_state_reconciles_guest() {
        use crate::driver::Probe;
        let mut c = cluster();

        // Çalışan konuk kendi kendine kapanmış.
        let stopped = Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None };
        assert!(c.observe_guest(100, &stopped, NOW));
        let g = c.guest(100).unwrap();
        assert_eq!((g.status, g.cpu_pct, g.mem_used_mb, g.started_at), (GuestStatus::Stopped, 0, 0, None));
        assert!(!c.observe_guest(100, &stopped, NOW), "aynı gözlem değişiklik sayılmaz");

        // Durmuş görünen konuk aslında çalışıyor (sunucu yeniden başlamış).
        let running = Probe { status: GuestStatus::Running, cpu_pct: Some(250), mem_used_mb: Some(999_999) };
        assert!(c.observe_guest(103, &running, NOW + 5));
        let g = c.guest(103).unwrap();
        assert_eq!((g.status, g.cpu_pct, g.mem_used_mb, g.started_at), (GuestStatus::Running, 100, g.mem_total_mb, Some(NOW + 5)));

        // Ölçüm yoksa eldeki değerler korunur; duraklatılmış konuğun CPU'su sıfırdır.
        let paused = Probe { status: GuestStatus::Paused, cpu_pct: Some(40), mem_used_mb: None };
        c.observe_guest(103, &paused, NOW + 9);
        let g = c.guest(103).unwrap();
        assert_eq!((g.cpu_pct, g.mem_used_mb, g.started_at), (0, g.mem_total_mb, Some(NOW + 5)));

        assert!(!c.observe_guest(999, &running, NOW));
    }

    #[test]
    fn observation_does_not_override_a_running_task() {
        use crate::driver::Probe;
        let mut c = cluster();
        let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        let running = Probe { status: GuestStatus::Running, cpu_pct: Some(5), mem_used_mb: Some(64) };
        assert!(!c.observe_guest(103, &running, NOW));
        assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);
        c.complete_task(t, Ok(()), NOW).unwrap();
        assert_eq!(c.guest(103).unwrap().status, GuestStatus::Running);
    }

    #[test]
    fn local_cluster_and_host_observations() {
        let node = Node { id: "ev".into(), status: NodeStatus::Online, cores: 8, cpu_pct: 0, mem_used_mb: 0, mem_total_mb: 1, uptime_secs: 0, kernel: "6".into(), ip: "10.0.0.2".into() };
        let storage = Storage { id: "local".into(), node: Some("ev".into()), kind: StorageKind::Dir, content: vec![], used_gb: 0, total_gb: 0 };
        let mut c = Cluster::local(node, storage);
        assert_eq!((c.name.as_str(), c.guests.len(), c.next_vmid()), ("ev", 0, 100));

        c.observe_node("ev", 140, 9000, 8000, 77);
        let n = c.node("ev").unwrap();
        assert_eq!((n.cpu_pct, n.mem_used_mb, n.mem_total_mb, n.uptime_secs), (100, 8000, 8000, 77));
        c.observe_storage("local", "ev", 30, 100);
        assert_eq!(c.storage_for("local", "ev").unwrap().free_gb(), 70);
        c.observe_node("yok", 1, 1, 1, 1);
        c.observe_storage("yok", "ev", 1, 1);

        let s = GuestSpec { node: "ev".into(), storage: "local".into(), ..spec("ilk") };
        assert_eq!(c.create_guest(&s, "u", NOW), Ok(100));
    }

    #[test]
    fn state_survives_json_round_trip() {
        let mut c = cluster();
        c.create_guest(&spec("kalici"), "u", NOW).unwrap();
        let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
        let json = serde_json::to_string(&c).unwrap();
        let mut back: Cluster = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        // Görev sayacı da korunur: yeni görev eski kimlikle çakışmaz.
        back.complete_task(t, Ok(()), NOW).unwrap();
        let next = back.begin_action(100, Action::Pause, "u", NOW).unwrap();
        assert!(next > t);
    }
}
