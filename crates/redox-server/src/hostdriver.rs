//! Bu sunucudaki gerçek konukların sürücüsü: sanal makineler QEMU'ya, konteynerler LXC'ye gider.

use redox_core::{Action, BackupSize, Guest, GuestKind, Hypervisor, Probe, SnapshotOp};

use crate::lxc::LxcDriver;
use crate::qemu::QemuDriver;

pub struct HostDriver {
    vm: QemuDriver,
    /// LXC kurulu değilse `None`: konteyner oluşturulamaz.
    ct: Option<LxcDriver>,
    /// Güvenlik duvarı için `nft` ikilisi; `None` ise güvenlik duvarı kapalıdır.
    nft: Option<String>,
    /// En son yüklenen kural betiği: korumalar yalnızca betik değiştiyse yeniden yüklenir.
    loaded: std::sync::Mutex<Option<String>>,
}

impl HostDriver {
    pub fn new(vm: QemuDriver, ct: Option<LxcDriver>) -> Self {
        HostDriver { vm, ct, nft: None, loaded: std::sync::Mutex::new(None) }
    }

    /// Konuk güvenlik duvarını açar (hizmetin CAP_NET_ADMIN yetkisi olmalıdır).
    pub fn with_firewall(mut self, nft_bin: impl Into<String>) -> Self {
        self.nft = Some(nft_bin.into());
        self
    }

    pub fn qemu(&self) -> &QemuDriver {
        &self.vm
    }

    fn of(&self, guest: &Guest) -> Result<&dyn Hypervisor, String> {
        match guest.kind {
            GuestKind::Vm => Ok(&self.vm),
            GuestKind::Ct => self.ct.as_ref().map(|d| d as &dyn Hypervisor).ok_or_else(|| "bu sunucuda LXC kurulu değil".to_string()),
        }
    }
}

impl HostDriver {
    /// Güvenlik duvarı ve ağ kartı korumalarının tam betiği. Korunan konukların arayüzleri o an QEMU
    /// süreçlerinden okunur.
    fn rules(&self, guests: &[Guest]) -> String {
        let taps: Vec<_> = guests.iter().filter(|g| crate::firewall::guarded(g)).map(|g| (g.vmid, self.vm.guest_taps(g.vmid))).collect();
        crate::firewall::ruleset_with(guests, &taps)
    }
}

impl Hypervisor for HostDriver {
    fn name(&self) -> &'static str {
        match (self.vm.config().kvm, self.ct.is_some()) {
            (true, true) => "qemu-kvm+lxc",
            (true, false) => "qemu-kvm",
            (false, true) => "qemu-tcg+lxc",
            (false, false) => "qemu-tcg",
        }
    }

    fn is_real(&self) -> bool {
        true
    }

    fn supports(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm || self.ct.is_some()
    }

    fn live_disk_ops(&self, kind: GuestKind) -> bool {
        match kind {
            GuestKind::Vm => self.vm.live_disk_ops(kind),
            GuestKind::Ct => self.ct.as_ref().is_some_and(|d| d.live_disk_ops(kind)),
        }
    }

    fn live_backup(&self, kind: GuestKind) -> bool {
        match kind {
            GuestKind::Vm => self.vm.live_backup(kind),
            GuestKind::Ct => self.ct.as_ref().is_some_and(|d| d.live_backup(kind)),
        }
    }

    fn isos(&self) -> Vec<String> {
        self.vm.isos()
    }

    fn backup_file(&self, id: &str) -> Option<std::path::PathBuf> {
        // Kimlik türle başlar: `vm-…` ya da `ct-…`.
        if id.starts_with("ct-") { self.ct.as_ref()?.backup_file(id) } else { self.vm.backup_file(id) }
    }

    fn add_disk(&self, guest: &Guest, id: u8, size_gb: u64) -> Result<(), String> {
        self.of(guest)?.add_disk(guest, id, size_gb)
    }

    fn remove_disk(&self, guest: &Guest, id: u8) -> Result<(), String> {
        self.of(guest)?.remove_disk(guest, id)
    }

    /// Konteynerlerin kök dosya sistemi LXC dizinindedir; ek depolama alanları yalnızca sanal makine diski tutar.
    fn storage_supports(&self, storage: &str, kind: GuestKind) -> bool {
        kind == GuestKind::Vm || !self.vm.config().storages.iter().any(|(id, _)| id == storage)
    }

    fn shared_folders(&self) -> bool {
        self.vm.shared_folders()
    }

    fn share_dir(&self, name: &str) -> Option<std::path::PathBuf> {
        self.vm.share_dir(name)
    }

    fn vnc_socket(&self, guest: &Guest) -> Option<std::path::PathBuf> {
        self.of(guest).ok()?.vnc_socket(guest)
    }

    fn hotplug(&self, kind: GuestKind) -> bool {
        match kind {
            GuestKind::Vm => self.vm.hotplug(kind),
            GuestKind::Ct => self.ct.as_ref().is_some_and(|d| d.hotplug(kind)),
        }
    }

    fn hot_add_disk(&self, guest: &Guest, id: u8) -> Result<(), String> {
        self.of(guest)?.hot_add_disk(guest, id)
    }

    fn instant_from_backup(&self, guest: &Guest, backup: &str, disks: &[u8]) -> Result<(), String> {
        self.of(guest)?.instant_from_backup(guest, backup, disks)
    }

    fn detach_backing(&self, guest: &Guest) -> Result<(), String> {
        self.of(guest)?.detach_backing(guest)
    }

    fn move_storage(&self, guest: &Guest, target: &str) -> Result<(), String> {
        self.of(guest)?.move_storage(guest, target)
    }

    fn import_disk(&self, guest: &Guest, id: u8, source: &std::path::Path) -> Result<(), String> {
        self.of(guest)?.import_disk(guest, id, source)
    }

    fn restore_machine(&self, guest: &Guest, archive: &std::path::Path) -> Result<(), String> {
        self.of(guest)?.restore_machine(guest, archive)
    }

    fn host_cpu_vendor(&self) -> Option<&'static str> {
        self.vm.host_cpu_vendor()
    }

    fn nested_virt(&self) -> bool {
        self.vm.nested_virt()
    }

    fn guest_run(&self, guest: &Guest, command: &str) -> Result<(), String> {
        self.of(guest)?.guest_run(guest, command)
    }

    fn snapshot_disk_only(&self, guest: &Guest, name: &str) -> Result<bool, String> {
        self.of(guest)?.snapshot_disk_only(guest, name)
    }

    fn io_counters(&self, guest: &Guest) -> Option<(u64, u64)> {
        self.of(guest).ok()?.io_counters(guest)
    }

    fn backup_raw(&self, id: &str, disk: u8, dest: &std::path::Path) -> Result<(), String> {
        if id.starts_with("ct-") { Err("konteyner yedeği blok blok aktarılmaz".to_string()) } else { self.vm.backup_raw(id, disk, dest) }
    }

    fn disk_raw(&self, guest: &Guest, disk: u8, dest: &std::path::Path) -> Result<(), String> {
        self.of(guest)?.disk_raw(guest, disk, dest)
    }

    fn raw_to_image(&self, raw: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
        self.vm.raw_to_image(raw, dest)
    }

    fn net_counters(&self, guest: &Guest) -> Option<(u64, u64)> {
        self.of(guest).ok()?.net_counters(guest)
    }

    fn guest_filesystems(&self, guest: &Guest) -> Option<Vec<redox_core::FsUsage>> {
        self.of(guest).ok()?.guest_filesystems(guest)
    }

    fn balloon_stats(&self, guest: &Guest) -> Option<redox_core::BalloonStats> {
        self.of(guest).ok()?.balloon_stats(guest)
    }

    fn set_balloon(&self, guest: &Guest, target_mb: u64) -> Result<(), String> {
        self.of(guest)?.set_balloon(guest, target_mb)
    }

    fn hot_add_nic(&self, guest: &Guest, index: u8, network: &str) -> Result<(), String> {
        self.of(guest)?.hot_add_nic(guest, index, network)
    }

    fn uefi(&self) -> bool {
        self.vm.uefi()
    }

    fn tpm(&self) -> bool {
        self.vm.tpm()
    }

    fn firewall(&self) -> bool {
        self.nft.is_some()
    }

    fn apply_firewall(&self, guests: &[Guest]) -> Result<(), String> {
        let Some(nft) = &self.nft else { return Ok(()) };
        let script = self.rules(guests);
        crate::firewall::apply(nft, &script)?;
        *self.loaded.lock().unwrap_or_else(|e| e.into_inner()) = Some(script);
        Ok(())
    }

    fn refresh_guards(&self, guests: &[Guest]) -> Result<(), String> {
        let Some(nft) = &self.nft else { return Ok(()) };
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if loaded.is_none() && !guests.iter().any(|g| g.hw.nic_guard) {
            return Ok(());
        }
        let script = self.rules(guests);
        if loaded.as_deref() == Some(script.as_str()) {
            return Ok(());
        }
        crate::firewall::apply(nft, &script)?;
        *loaded = Some(script);
        Ok(())
    }

    fn export_disks(&self, guest: &Guest) -> Result<Option<(std::path::PathBuf, Vec<(String, std::path::PathBuf, u64)>)>, String> {
        match guest.kind {
            GuestKind::Vm => self.vm.export_disks(guest),
            GuestKind::Ct => Ok(None),
        }
    }

    fn iso_file(&self, name: &str) -> Option<std::path::PathBuf> {
        self.vm.iso_file(name)
    }

    fn image_file(&self, name: &str) -> Option<std::path::PathBuf> {
        self.vm.image_file(name)
    }

    fn images(&self) -> Vec<String> {
        self.vm.images()
    }

    fn bridges(&self) -> Vec<String> {
        self.vm.bridges()
    }

    fn templates(&self) -> Vec<String> {
        self.ct.as_ref().map(|d| d.templates()).unwrap_or_default()
    }

    fn create(&self, guest: &Guest) -> Result<(), String> {
        self.of(guest)?.create(guest)
    }

    fn configure(&self, guest: &Guest) -> Result<(), String> {
        self.of(guest)?.configure(guest)
    }

    fn destroy(&self, guest: &Guest) -> Result<(), String> {
        self.of(guest)?.destroy(guest)
    }

    fn apply(&self, guest: &Guest, action: Action) -> Result<(), String> {
        self.of(guest)?.apply(guest, action)
    }

    fn probe(&self, guest: &Guest) -> Option<Probe> {
        self.of(guest).ok()?.probe(guest)
    }

    fn guest_agent(&self, guest: &Guest) -> Option<String> {
        self.of(guest).ok()?.guest_agent(guest)
    }

    fn guest_ip(&self, guest: &Guest) -> Option<String> {
        self.of(guest).ok()?.guest_ip(guest)
    }

    fn exec(&self, guest: &Guest, command: &str) -> Result<String, String> {
        self.of(guest)?.exec(guest, command)
    }

    fn snapshot(&self, guest: &Guest, op: SnapshotOp<'_>) -> Result<(), String> {
        self.of(guest)?.snapshot(guest, op)
    }

    fn resize_disk(&self, guest: &Guest, new_gb: u64) -> Result<(), String> {
        self.of(guest)?.resize_disk(guest, new_gb)
    }

    fn clone_disk(&self, source: &Guest, target: &Guest) -> Result<(), String> {
        self.of(source)?.clone_disk(source, target)
    }

    fn suspend(&self, guest: &Guest) -> Result<(), String> {
        self.of(guest)?.suspend(guest)
    }

    fn suspend_state(&self, guest: &Guest) -> Option<(std::path::PathBuf, bool, u8)> {
        self.of(guest).ok()?.suspend_state(guest)
    }

    fn adopt_suspend(&self, guest: &Guest, file: &std::path::Path, running: bool, layout: u8) -> Result<(), String> {
        self.of(guest)?.adopt_suspend(guest, file, running, layout)
    }

    fn discard_suspend(&self, guest: &Guest) -> Result<(), String> {
        self.of(guest)?.discard_suspend(guest)
    }

    fn link_disk(&self, source: &Guest, target: &Guest) -> Result<(), String> {
        self.of(source)?.link_disk(source, target)
    }

    fn backup(&self, guest: &Guest, id: &str) -> Result<BackupSize, String> {
        self.of(guest)?.backup(guest, id)
    }

    fn backup_present(&self, id: &str) -> bool {
        if id.starts_with("ct-") { self.ct.as_ref().is_some_and(|d| d.backup_present(id)) } else { self.vm.backup_present(id) }
    }

    fn export_backup(&self, id: &str, disk: u8) -> Result<Option<(std::path::PathBuf, bool)>, String> {
        match (id.starts_with("ct-"), &self.ct) {
            (true, Some(ct)) => ct.export_backup(id, disk),
            (true, None) => Ok(None),
            (false, _) => self.vm.export_backup(id, disk),
        }
    }

    fn backup_machine_file(&self, id: &str) -> Option<std::path::PathBuf> {
        if id.starts_with("ct-") { None } else { self.vm.backup_machine_file(id) }
    }

    fn backup_disk_file(&self, id: &str, disk: u8) -> Option<std::path::PathBuf> {
        if id.starts_with("ct-") { self.ct.as_ref()?.backup_disk_file(id, disk) } else { self.vm.backup_disk_file(id, disk) }
    }

    // Parçalı (artımlı) yedek yalnızca sanal makinelerde vardır; konteyner yedekleri tek dosyadır.
    fn push_backup(&self, id: &str, known: &[String], remote: &dyn redox_core::Remote) -> Result<Option<u64>, String> {
        if id.starts_with("ct-") { Ok(None) } else { self.vm.push_backup(id, known, remote) }
    }

    fn pull_backup(&self, id: &str, remote: &dyn redox_core::Remote) -> Result<bool, String> {
        if id.starts_with("ct-") { Ok(false) } else { self.vm.pull_backup(id, remote) }
    }

    fn unpush_backup(&self, id: &str, others: &[String], remote: &dyn redox_core::Remote) -> Result<bool, String> {
        if id.starts_with("ct-") { Ok(false) } else { self.vm.unpush_backup(id, others, remote) }
    }

    fn verify_backup(&self, id: &str, disks: &[u8]) -> Result<String, String> {
        match (id.starts_with("ct-"), &self.ct) {
            (true, Some(ct)) => ct.verify_backup(id, disks),
            (true, None) => Err("bu sunucuda LXC kurulu değil".to_string()),
            (false, _) => self.vm.verify_backup(id, disks),
        }
    }

    fn restore(&self, guest: &Guest, id: &str, disks: &[u8]) -> Result<(), String> {
        self.of(guest)?.restore(guest, id, disks)
    }

    fn delete_backup(&self, id: &str) -> Result<(), String> {
        // Kimlik hangi türe aitse o dosya vardır; olmayanı silmek hata değildir.
        self.vm.delete_backup(id)?;
        match &self.ct {
            Some(ct) => ct.delete_backup(id),
            None => Ok(()),
        }
    }

    fn screenshot(&self, guest: &Guest) -> Result<Vec<u8>, String> {
        self.of(guest)?.screenshot(guest)
    }

    fn send_keys(&self, guest: &Guest, chords: &[Vec<String>]) -> Result<(), String> {
        self.of(guest)?.send_keys(guest, chords)
    }
}
