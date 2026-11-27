//! QEMU/KVM sürücüsü: her sanal makine ayrı bir `qemu-system-x86_64` sürecidir.
//!
//! Süreç `-daemonize` ile arka plana geçer; durumu PID dosyası ve QMP soketi
//! (JSON satırları) üzerinden izlenir ve yönetilir. Sunucu yeniden başlasa da
//! konuklar çalışmaya devam eder ve yeniden sahiplenilir.
//!
//! Komut satırı ve QMP ayrıştırması saf işlevlerdir (her platformda test edilir);
//! süreç ve soket işlemleri yalnızca Unix'te derlenir.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use redox_core::{mac_for, valid_bridge, valid_iso_name, valid_snapshot_name, Action, BackupSize, Guest, GuestKind, Hypervisor, Probe, SnapshotOp, VmId, NAT};
use serde_json::Value;

mod backup;
mod cbt;
mod machine;

mod args;
mod config;
mod qmp;

pub use args::*;
pub use config::*;
pub use qmp::*;

/// Değişen blokları kaydeden kalıcı haritanın (dirty bitmap) adı.
pub const CBT_BITMAP: &str = "redox-cbt";
/// Arka arkaya en çok bu kadar yedek değişen blok takibiyle alınır; sonraki disk baştan sona okunur.
/// Harita herhangi bir nedenle gerçeği yansıtmıyorsa hata bu sayıda yedekle sınırlı kalır.
pub const CBT_MAX_STREAK: u32 = 6;

/// Canlı yedekte değişen blok haritasının kullanımı.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cbt {
    /// Harita kullanılmaz.
    Off,
    /// Disk baştan sona kopyalanır ve aynı anda harita sıfırlanır (yoksa oluşturulur): sonraki yedeğin temeli.
    Arm,
    /// Yalnızca haritadaki (son yedekten beri yazılan) bloklar kopyalanır.
    Delta,
}

/// `qemu-img map --output=json` çıktısından bu imajda gerçekten yazılmış bölgeler: (başlangıç, uzunluk).
/// Çıktıda `present` alanı yoksa (eski QEMU) yazılmamış bölge ile sıfır yazılmış bölge ayırt edilemez: `None`.
pub fn present_ranges(map: &Value) -> Option<Vec<(u64, u64)>> {
    let mut ranges = Vec::new();
    for entry in map.as_array()? {
        let (start, length, present) = (entry["start"].as_u64()?, entry["length"].as_u64()?, entry["present"].as_bool()?);
        if present && length > 0 {
            ranges.push((start, length));
        }
    }
    Some(ranges)
}

/// Konuğun değişen blok takibi durumu: harita hangi yedekten beri yazılanları gösteriyor?
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct CbtState {
    /// Haritanın sıfırlandığı andaki yedek.
    base: String,
    disks: Vec<u8>,
    /// Bu temelden önce arka arkaya kaç yedek haritayla alındı.
    streak: u32,
}

/// TAP arayüzlerinin çekirdek sayaçlarından konuğun (aldığı, gönderdiği) bayt toplamı; arayüz yoksa `None`.
/// Sunucu tarafında yön terstir: arayüzün "aldığı" konuğun gönderdiğidir.
pub fn tap_counters(sys_net: &Path, taps: &[String]) -> Option<(u64, u64)> {
    let read = |tap: &str, file: &str| std::fs::read_to_string(sys_net.join(tap).join("statistics").join(file)).ok()?.trim().parse::<u64>().ok();
    let seen: Vec<(u64, u64)> = taps.iter().filter_map(|tap| Some((read(tap, "tx_bytes")?, read(tap, "rx_bytes")?))).collect();
    (!seen.is_empty()).then(|| seen.iter().fold((0, 0), |sum, c| (sum.0 + c.0, sum.1 + c.1)))
}

impl QemuDriver {
    /// Çalışan konuğun QEMU sürecinin açık tuttuğu TAP arayüzleri (köprüdeki kartları); kapalıysa boş.
    pub fn guest_taps(&self, vmid: VmId) -> Vec<String> {
        #[cfg(unix)]
        {
            imp::taps(self, vmid)
        }
        #[cfg(not(unix))]
        {
            let _ = vmid;
            Vec::new()
        }
    }
}

pub struct QemuDriver {
    cfg: QemuConfig,
    /// Başlangıçta saptanır: OVMF dosyaları ve swtpm kurulu mu?
    has_uefi: bool,
    has_tpm: bool,
    /// Bu QEMU derlemesinde 9p (paylaşılan klasör) aygıtı var mı?
    has_9p: bool,
    /// Artımlı yedeklerin parça deposu.
    store: crate::chunkstore::Store,
    /// Konuk ajanının son görüldüğü durum: (an, yanıt verdi mi). Yanıt vermeyen ajan her turda yeniden
    /// sorulmaz; yoksa ajansız her konuk durum okumasını geciktirirdi.
    #[cfg_attr(not(unix), allow(dead_code))]
    agents: Mutex<HashMap<VmId, (Instant, bool)>>,
    /// Son CPU örneği: (tık, an).
    samples: Mutex<HashMap<VmId, (u64, Instant)>>,
}

impl QemuDriver {
    /// Dizinleri oluşturur ve soket yolu sınırını denetler.
    pub fn new(cfg: QemuConfig) -> Result<Self, String> {
        for dir in [cfg.images_dir(), cfg.iso_dir(), cfg.cloud_dir(), cfg.run_dir()] {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
        }
        // Unix soket yolu 108 baytı aşamaz; en uzun kimlikle dene.
        let longest = cfg.qmp_path(redox_core::VMID_MAX);
        if longest.as_os_str().len() > 100 {
            return Err(format!(
                "veri dizini yolu çok uzun: QMP soketi {} 100 baytı aşıyor; daha kısa bir --data-dir verin",
                longest.display()
            ));
        }
        let has_uefi = cfg.ovmf_files(false).0.is_file();
        let has_tpm = std::process::Command::new(&cfg.swtpm_bin).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        let has_9p = std::process::Command::new(&cfg.qemu_bin)
            .args(["-device", "help"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("virtio-9p-pci") || String::from_utf8_lossy(&o.stderr).contains("virtio-9p-pci"))
            .unwrap_or(false);
        // Önceki çalışmadan yarım kalmış geçici yedek dosyaları temizlenir.
        for entry in std::fs::read_dir(cfg.backups_dir()).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".export-") {
                let _ = std::fs::remove_dir_all(entry.path());
            } else if (name.starts_with('.') && name.ends_with(".tmp")) || name.ends_with(".indiriliyor") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        // Yarıda kalmış kalıp ve imaj yüklemeleri.
        for dir in [cfg.iso_dir(), cfg.cloud_dir()] {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') && name.ends_with(".upload.tmp") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        // Yarıda kesilmiş bir geri yüklemenin (asıl diske dokunmamış) hazırlık dosyaları.
        for dir in std::iter::once(cfg.images_dir()).chain(cfg.storages.iter().map(|(_, dir)| dir.clone())) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.file_name().to_string_lossy().ends_with(".restore") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let store = crate::chunkstore::Store::new(cfg.backups_dir());
        let store = match &cfg.backup_passphrase {
            Some(_) if !cfg.incremental_backups => return Err("yerel yedek şifrelemesi artımlı yedekle çalışır (--incremental-backups)".to_string()),
            Some(passphrase) => store.sealed(passphrase)?,
            // Şifrelenmiş depo parolasız açılırsa yeni yedekler sessizce şifresiz yazılırdı.
            None if crate::chunkstore::seal_marked(&cfg.backups_dir()) => return Err(format!("{} şifreli bir yedek deposu; sunucuyu --backup-passphrase-file ile başlatın", cfg.backups_dir().display())),
            None => store,
        };
        Ok(QemuDriver { cfg, has_uefi, has_tpm, has_9p, store, agents: Mutex::new(HashMap::new()), samples: Mutex::new(HashMap::new()) })
    }

    pub fn config(&self) -> &QemuConfig {
        &self.cfg
    }
}

impl Hypervisor for QemuDriver {
    fn name(&self) -> &'static str {
        if self.cfg.kvm {
            "qemu-kvm"
        } else {
            "qemu-tcg"
        }
    }

    fn is_real(&self) -> bool {
        true
    }

    fn supports(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm
    }

    fn isos(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.cfg.iso_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| valid_iso_name(n))
            .collect();
        names.sort();
        names
    }

    fn images(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.cfg.cloud_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| redox_core::hw::valid_image_name(n))
            .collect();
        names.sort();
        names
    }

    fn backup_file(&self, id: &str) -> Option<PathBuf> {
        self.backup_path(id).ok()
    }

    fn backup_present(&self, id: &str) -> bool {
        self.backup_path(id).is_ok_and(|path| path.exists() || self.store.has(id))
    }

    fn backup_disk_file(&self, id: &str, disk: u8) -> Option<PathBuf> {
        self.backup_part(id, disk).ok()
    }

    /// Artımlı yedek tek dosya değildir: indirme için istenen disk sıkıştırılmış qcow2'ye dökülür.
    fn export_backup(&self, id: &str, disk: u8) -> Result<Option<(PathBuf, bool)>, String> {
        let path = self.backup_part(id, disk)?;
        if !self.store.has(id) {
            return Ok(Some((path, false)));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let dir = self.cfg.backups_dir().join(format!(".export-{}-{}", std::process::id(), EXPORTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
        let (raw, out) = (dir.join(format!("{id}.raw")), dir.join(name));
        let built = self.unpack(id, disk, &raw).and_then(|()| self.raw_to_qcow2(&raw, &out, true, "yedek dışa aktarılamadı"));
        let _ = std::fs::remove_file(&raw);
        if let Err(e) = built {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        Ok(Some((out, true)))
    }

    fn push_backup(&self, id: &str, known: &[String], remote: &dyn redox_core::Remote) -> Result<Option<u64>, String> {
        self.backup_path(id)?;
        if !self.store.has(id) {
            return Ok(None);
        }
        self.store.push(id, known, remote).map(|(_, bytes)| Some(bytes))
    }

    fn pull_backup(&self, id: &str, remote: &dyn redox_core::Remote) -> Result<bool, String> {
        self.backup_path(id)?;
        self.store.pull(id, remote)
    }

    fn unpush_backup(&self, id: &str, others: &[String], remote: &dyn redox_core::Remote) -> Result<bool, String> {
        self.backup_path(id)?;
        self.store.unpush(id, others, remote)
    }

    fn verify_backup(&self, id: &str, disks: &[u8]) -> Result<String, String> {
        self.backup_path(id)?;
        if self.store.has(id) {
            let present: Vec<u8> = self.store.disks(id)?.into_iter().map(|(n, _)| n).collect();
            if let Some(missing) = std::iter::once(&0).chain(disks).find(|n| !present.contains(n)) {
                return Err(format!("yedekte {} yok", if *missing == 0 { "asıl disk".to_string() } else { format!("ek disk {missing}") }));
            }
            return self.store.verify(id).map(|chunks| format!("{chunks} parça doğrulandı"));
        }
        let mut checked = 0;
        for disk in std::iter::once(0).chain(disks.iter().copied()) {
            let path = self.backup_part(id, disk)?;
            if !path.is_file() {
                return Err(format!("yedek dosyası bulunamadı: {}", path.display()));
            }
            let out = std::process::Command::new(&self.cfg.qemu_img_bin)
                .args(["check", "-f", "qcow2"])
                .arg(&path)
                .output()
                .map_err(|e| format!("{} çalıştırılamadı: {e}", self.cfg.qemu_img_bin))?;
            // 0: temiz; 3: yalnızca kullanılmayan alan sızıntısı (veri sağlam).
            if !matches!(out.status.code(), Some(0) | Some(3)) {
                return Err(format!("yedek bozuk: {}", last_line(if out.stderr.is_empty() { &out.stdout } else { &out.stderr })));
            }
            checked += 1;
        }
        Ok(if checked == 1 { "imaj tutarlı".to_string() } else { format!("{checked} imaj tutarlı") })
    }

    fn export_disks(&self, guest: &Guest) -> Result<Option<(PathBuf, Vec<(String, PathBuf, u64)>)>, String> {
        const GB: u64 = 1024 * 1024 * 1024;
        #[cfg(unix)]
        if self.alive(guest.vmid) {
            return Err("konuk çalışıyor; dışa aktarmak için önce kapatın".to_string());
        }
        let dir = self.cfg.backups_dir().join(format!(".export-{}-{}", std::process::id(), EXPORTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
        let mut disks = Vec::new();
        for (i, n) in disk_ids(guest).into_iter().enumerate() {
            let name = format!("{}-disk{}.vmdk", guest.name, i + 1);
            let (source, target) = (self.guest_disk(guest, n), dir.join(&name));
            // streamOptimized: sıkıştırılmış, tek dosyalık vmdk; OVA içinde beklenen biçim.
            let made = self.img(
                &["convert".as_ref(), "-O".as_ref(), "vmdk".as_ref(), "-o".as_ref(), "subformat=streamOptimized".as_ref(), source.as_os_str(), target.as_os_str()],
                "disk dışa aktarılamadı",
            );
            if let Err(e) = made {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
            let capacity = if n == 0 { guest.disk_gb } else { guest.hw.disks.iter().find(|d| d.id == n).map(|d| d.size_gb).unwrap_or(0) } * GB;
            disks.push((name, target, capacity));
        }
        Ok(Some((dir, disks)))
    }

    fn iso_file(&self, name: &str) -> Option<PathBuf> {
        valid_iso_name(name).then(|| self.cfg.iso_dir().join(name))
    }

    fn image_file(&self, name: &str) -> Option<PathBuf> {
        redox_core::hw::valid_image_name(name).then(|| self.cfg.cloud_dir().join(name))
    }

    fn uefi(&self) -> bool {
        self.has_uefi
    }

    fn tpm(&self) -> bool {
        self.has_tpm
    }

    fn shared_folders(&self) -> bool {
        self.has_9p
    }

    fn share_dir(&self, name: &str) -> Option<PathBuf> {
        Some(self.cfg.shares_dir().join(name))
    }

    fn bridges(&self) -> Vec<String> {
        crate::net::bridges()
    }

    /// Konuk ajanı varsa adresi o bildirir (NAT ağındaki konuk için tek yol); yoksa köprülü konuğun
    /// adresi sunucunun komşu tablosundan (MAC ile) bulunur.
    fn guest_ip(&self, guest: &Guest) -> Option<String> {
        #[cfg(unix)]
        if let Some(ip) = self.agent_call(guest.vmid, "guest-network-get-interfaces", Duration::from_secs(2)).and_then(|v| crate::qga::first_ip(&v)) {
            return Some(ip);
        }
        if is_bridged(guest) { crate::net::ip_of_mac(&mac_for(guest.vmid)) } else { None }
    }

    fn guest_agent(&self, guest: &Guest) -> Option<String> {
        #[cfg(unix)]
        {
            let info = self.agent_call(guest.vmid, "guest-get-osinfo", Duration::from_secs(2))?;
            Some(crate::qga::os_name(&info).unwrap_or_else(|| "bilinmeyen işletim sistemi".to_string()))
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            None
        }
    }

    fn create(&self, guest: &Guest) -> Result<(), String> {
        let disk = self.cfg.disk_of(guest);
        if disk.exists() {
            return Err(format!("{} zaten var; başka bir kimlik seçin ya da dosyayı kaldırın", disk.display()));
        }
        if let Some(image) = guest.hw.image.as_deref() {
            // Hazır imajdan: kopyala, sonra istenen boyuta büyüt.
            if !redox_core::hw::valid_image_name(image) {
                return Err(format!("geçersiz bulut imajı adı: {image}"));
            }
            let mut source = self.cfg.cloud_dir().join(image);
            if !source.is_file() {
                return Err(format!("bulut imajı bulunamadı: {}", source.display()));
            }
            // OVA bir arşivdir: içindeki disk önce geçici bir dosyaya çıkarılır.
            let unpacked = image.to_ascii_lowercase().ends_with(".ova").then(|| self.cfg.cloud_dir().join(format!(".vm-{}.ova-disk.upload.tmp", guest.vmid)));
            if let Some(temp) = &unpacked {
                crate::ova::extract_disk(&source, temp).inspect_err(|_| {
                    let _ = std::fs::remove_file(temp);
                })?;
                source = temp.clone();
            }
            let converted = self.convert(&source, &disk, false, "bulut imajı kopyalanamadı");
            if let Some(temp) = &unpacked {
                let _ = std::fs::remove_file(temp);
            }
            let built = converted.and_then(|()| {
                let size = format!("{}G", guest.disk_gb);
                self.img(&["resize".as_ref(), disk.as_os_str(), size.as_ref()], "disk istenen boyuta getirilemedi (boyut imajdan küçük olamaz)")
            });
            if built.is_err() {
                let _ = std::fs::remove_file(&disk);
            }
            return built;
        }
        self.create_image(&disk, guest.disk_gb)
    }

    fn add_disk(&self, guest: &Guest, id: u8, size_gb: u64) -> Result<(), String> {
        self.cbt_invalidate(guest.vmid);
        let disk = self.cfg.extra_disk_of(guest, id);
        if disk.exists() {
            return Err(format!("{} zaten var; dosyayı kaldırın ya da yeniden deneyin", disk.display()));
        }
        self.create_image(&disk, size_gb)
    }

    fn remove_disk(&self, guest: &Guest, id: u8) -> Result<(), String> {
        self.cbt_invalidate(guest.vmid);
        let disk = self.cfg.extra_disk_of(guest, id);
        match std::fs::remove_file(&disk) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{} silinemedi: {e}", disk.display())),
        }
    }

    fn destroy(&self, guest: &Guest) -> Result<(), String> {
        self.cbt_invalidate(guest.vmid);
        let _ = std::fs::remove_file(self.cfg.state_path(guest.vmid));
        let _ = std::fs::remove_file(self.cfg.state_path(guest.vmid).with_extension("state.running"));
        let _ = std::fs::remove_file(self.cfg.state_path(guest.vmid).with_extension("state.hp"));
        let _ = std::fs::remove_dir_all(self.cfg.tpm_dir(guest.vmid));
        // Ek disk dosyaları kayıttan bağımsız aranır: yarım kalmış bir silmenin artığı da temizlenir.
        for id in 1..=redox_core::hw::MAX_EXTRA_DISKS as u8 {
            let _ = std::fs::remove_file(self.cfg.extra_disk_of(guest, id));
        }
        for path in [
            self.cfg.disk_of(guest),
            self.cfg.efivars_path(guest.vmid, false),
            self.cfg.efivars_path(guest.vmid, true),
            self.cfg.pid_path(guest.vmid),
            self.cfg.qmp_path(guest.vmid),
        ] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{} silinemedi: {e}", path.display())),
            }
        }
        let _ = std::fs::remove_dir_all(self.cfg.seed_dir(guest.vmid));
        lock(&self.samples).remove(&guest.vmid);
        Ok(())
    }

    fn apply(&self, guest: &Guest, action: Action) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::apply(self, guest, action)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, action);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn probe(&self, guest: &Guest) -> Option<Probe> {
        #[cfg(unix)]
        {
            Some(imp::probe(self, guest))
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            None
        }
    }

    fn live_disk_ops(&self, kind: GuestKind) -> bool {
        cfg!(unix) && kind == GuestKind::Vm
    }

    fn vnc_socket(&self, guest: &Guest) -> Option<PathBuf> {
        let path = self.cfg.vnc_path(guest.vmid);
        #[cfg(unix)]
        {
            (imp::is_alive(self, guest.vmid) && path.exists()).then_some(path)
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }

    fn hotplug(&self, kind: GuestKind) -> bool {
        cfg!(unix) && kind == GuestKind::Vm
    }

    fn hot_add_disk(&self, guest: &Guest, id: u8) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::hot_add_disk(self, guest, id)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, id);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn hot_add_nic(&self, guest: &Guest, index: u8, network: &str) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::hot_add_nic(self, guest, index, network)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, index, network);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn hot_add_core(&self, guest: &Guest) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::hot_add_core(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn online_cores(&self, guest: &Guest) -> bool {
        #[cfg(unix)]
        {
            imp::online_cores(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            false
        }
    }

    fn live_migration(&self) -> bool {
        cfg!(unix)
    }

    /// Çalışırken değişebilen ayarlar (işlemci payı, bellek garantisi) çalışan konuğa hemen uygulanır.
    fn configure(&self, guest: &Guest) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::apply_limits(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            Ok(())
        }
    }

    fn migrate_layout(&self, guest: &Guest) -> Option<u8> {
        #[cfg(unix)]
        {
            imp::migrate_layout(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            None
        }
    }

    fn migrate_listen(&self, guest: &Guest, layout: u8, bind: &str, nbd_port: u16, ram_port: u16, key: &str) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::migrate_listen(self, guest, layout, bind, nbd_port, ram_port, key)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, layout, bind, nbd_port, ram_port, key);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn migrate_send(&self, guest: &Guest, host: &str, nbd_port: u16, ram_port: u16, key: &str) -> Result<u64, String> {
        #[cfg(unix)]
        {
            imp::migrate_send(self, guest, host, nbd_port, ram_port, key)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, host, nbd_port, ram_port, key);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn migrate_finish(&self, guest: &Guest) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::migrate_finish(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn migrate_abort(&self, guest: &Guest) {
        #[cfg(unix)]
        imp::migrate_abort(self, guest);
        #[cfg(not(unix))]
        let _ = guest;
    }

    fn hot_add_memory(&self, guest: &Guest, mb: u64) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::hot_add_memory(self, guest, mb)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, mb);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn online_memory(&self, guest: &Guest) -> bool {
        #[cfg(unix)]
        {
            imp::online_memory(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            false
        }
    }

    fn snapshot(&self, guest: &Guest, op: SnapshotOp<'_>) -> Result<(), String> {
        if matches!(op, SnapshotOp::Rollback(_)) {
            // Geri dönüş diskin içeriğini haritaya işlenmeden değiştirir: harita artık güvenilmez.
            self.cbt_invalidate(guest.vmid);
        }
        let name = match op {
            SnapshotOp::Create(n) | SnapshotOp::Rollback(n) | SnapshotOp::Delete(n) => n,
        };
        if !valid_snapshot_name(name) {
            return Err(format!("geçersiz anlık görüntü adı: {name}"));
        }
        // Çalışan konukta disk QEMU'nun elindedir: işlem QEMU'ya yaptırılır.
        #[cfg(unix)]
        if self.alive(guest.vmid) {
            if matches!(op, SnapshotOp::Rollback(_)) {
                imp::drop_bitmaps(self, guest);
            }
            return imp::live_snapshot(self, guest, op);
        }
        if matches!(op, SnapshotOp::Rollback(_)) {
            // QEMU, etkin değişen blok haritası olan imajda geri dönüşü reddeder. Harita zaten geçersiz
            // kılındı; imajdan da silinir (yoksa komut zararsızca başarısız olur).
            for n in disk_ids(guest) {
                let disk = self.guest_disk(guest, n);
                let _ = self.img(&["bitmap".as_ref(), "--remove".as_ref(), disk.as_os_str(), CBT_BITMAP.as_ref()], "harita");
            }
        }
        let (flag, name) = match op {
            SnapshotOp::Create(n) => ("-c", n),
            SnapshotOp::Rollback(n) => ("-a", n),
            SnapshotOp::Delete(n) => ("-d", n),
        };
        if !valid_snapshot_name(name) {
            return Err(format!("geçersiz anlık görüntü adı: {name}"));
        }
        let disk = self.cfg.disk_of(guest);
        self.img(&["snapshot".as_ref(), flag.as_ref(), name.as_ref(), disk.as_os_str()], "anlık görüntü işlemi başarısız")?;
        // UEFI değişken deposu da aynı adla işlenir: çalışırken alınan görüntüler onu da kapsar ve
        // QEMU geri dönerken görüntüyü bütün yazılabilir imajlarda arar.
        let mut others = vec![self.cfg.efivars_path(guest.vmid, false), self.cfg.efivars_path(guest.vmid, true)];
        others.extend(guest.hw.disks.iter().map(|d| self.cfg.extra_disk_of(guest, d.id)));
        for vars in others {
            if vars.is_file() {
                let done = self.img(&["snapshot".as_ref(), flag.as_ref(), name.as_ref(), vars.as_os_str()], "ek imajda anlık görüntü işlemi başarısız");
                // Depo sonradan oluşmuşsa eski görüntü onda yoktur; yalnızca oluşturma hatası önemlidir.
                if matches!(op, SnapshotOp::Create(_)) {
                    done?;
                }
            }
        }
        Ok(())
    }

    fn snapshot_disk_only(&self, guest: &Guest, name: &str) -> Result<bool, String> {
        if !valid_snapshot_name(name) {
            return Err(format!("geçersiz anlık görüntü adı: {name}"));
        }
        #[cfg(unix)]
        if self.alive(guest.vmid) {
            let disks: Vec<PathBuf> = disk_ids(guest).iter().map(|n| self.guest_disk(guest, *n)).collect();
            return imp::disk_snapshot(self, guest, name, &disks);
        }
        // Kapalı konukta anlık görüntü zaten yalnızca disklerdir.
        self.snapshot(guest, SnapshotOp::Create(name)).map(|()| false)
    }

    fn resize_disk(&self, guest: &Guest, new_gb: u64) -> Result<(), String> {
        self.cbt_invalidate(guest.vmid);
        let disk = self.cfg.disk_of(guest);
        let size = format!("{new_gb}G");
        self.img(&["resize".as_ref(), disk.as_os_str(), size.as_ref()], "disk büyütülemedi")
    }

    fn clone_disk(&self, source: &Guest, target: &Guest) -> Result<(), String> {
        let (from, to) = (self.cfg.disk_of(source), self.cfg.disk_of(target));
        if to.exists() {
            return Err(format!("{} zaten var", to.display()));
        }
        self.convert(&from, &to, false, "disk kopyalanamadı")?;
        for disk in &source.hw.disks {
            self.convert(&self.cfg.extra_disk_of(source, disk.id), &self.cfg.extra_disk_of(target, disk.id), false, "ek disk kopyalanamadı")?;
        }
        Ok(())
    }

    fn suspend(&self, guest: &Guest) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::suspend(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn suspend_state(&self, guest: &Guest) -> Option<(PathBuf, bool, u8)> {
        let state = self.cfg.state_path(guest.vmid);
        if !state.is_file() {
            return None;
        }
        let running = state.with_extension("state.running").exists();
        let layout = match std::fs::read_to_string(state.with_extension("state.hp")) {
            Err(_) => LAYOUT_LEGACY,
            Ok(text) => text.trim().parse().unwrap_or(LAYOUT_PORTS),
        };
        Some((state, running, layout))
    }

    fn adopt_suspend(&self, guest: &Guest, file: &Path, running: bool, layout: u8) -> Result<(), String> {
        if layout > LAYOUT_CURRENT {
            return Err("bellek durumu bu sunucudan daha yeni bir sürümle kaydedilmiş; önce bu sunucuyu güncelleyin".to_string());
        }
        let state = self.cfg.state_path(guest.vmid);
        for marker in ["state.running", "state.hp"] {
            let _ = std::fs::remove_file(state.with_extension(marker));
        }
        if running {
            std::fs::write(state.with_extension("state.running"), b"").map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))?;
        }
        if layout > LAYOUT_LEGACY {
            std::fs::write(state.with_extension("state.hp"), layout.to_string()).map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))?;
        }
        // Durum dosyası en son yerine konur: yarım kayıttan dönülmez.
        std::fs::rename(file, &state).map_err(|e| format!("bellek durumu yerine konamadı: {e}"))
    }

    fn discard_suspend(&self, guest: &Guest) -> Result<(), String> {
        let _ = std::fs::remove_file(self.cfg.state_path(guest.vmid).with_extension("state.running"));
        let _ = std::fs::remove_file(self.cfg.state_path(guest.vmid).with_extension("state.hp"));
        match std::fs::remove_file(self.cfg.state_path(guest.vmid)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("bellek durumu silinemedi: {e}")),
        }
    }

    /// Bağlı klon: her disk, şablonun aynı numaralı diskini arkalık alan boş bir qcow2'dir.
    fn link_disk(&self, source: &Guest, target: &Guest) -> Result<(), String> {
        let mut made: Vec<PathBuf> = Vec::new();
        for n in disk_ids(source) {
            let (base, disk) = (self.guest_disk(source, n), self.guest_disk(target, n));
            let linked = if disk.exists() {
                Err(format!("{} zaten var", disk.display()))
            } else if !base.is_file() {
                Err(format!("şablonun diski bulunamadı: {}", base.display()))
            } else {
                self.img(
                    &["create".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-F".as_ref(), "qcow2".as_ref(), "-b".as_ref(), base.as_os_str(), disk.as_os_str()],
                    "bağlı klon oluşturulamadı",
                )
            };
            if let Err(e) = linked {
                for path in &made {
                    let _ = std::fs::remove_file(path);
                }
                return Err(e);
            }
            made.push(disk);
        }
        Ok(())
    }

    fn backup(&self, guest: &Guest, id: &str) -> Result<BackupSize, String> {
        let size = self.backup_disks(guest, id)?;
        // Diskler tamam; makine durumu eklenemezse yedek eksik sayılır ve geri alınır.
        if let Err(e) = self.save_machine(guest, id) {
            let _ = Hypervisor::delete_backup(self, id);
            return Err(e);
        }
        Ok(size)
    }

    fn backup_machine_file(&self, id: &str) -> Option<PathBuf> {
        self.machine_path(id).ok()
    }

    fn restore(&self, guest: &Guest, id: &str, disks: &[u8]) -> Result<(), String> {
        self.backup_path(id)?;
        self.cbt_invalidate(guest.vmid);
        let incremental = self.store.has(id);
        // Önce bütün diskler yanlarına yazılır; hepsi başarılıysa yerlerine konur. Yarım kalan ya da
        // bir diski eksik olan geri yükleme hiçbir diske dokunmaz.
        let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut failure = None;
        for n in std::iter::once(0).chain(disks.iter().copied()) {
            let disk = self.guest_disk(guest, n);
            let staging = disk.with_extension("qcow2.restore");
            let _ = std::fs::remove_file(&staging);
            staged.push((staging.clone(), disk.clone()));
            let built = if incremental {
                // Parçalar özetleri doğrulanarak ham diske dizilir, sonra qcow2'ye çevrilir.
                let raw = disk.with_extension("raw.restore");
                let built = self.unpack(id, n, &raw).and_then(|()| self.raw_to_qcow2(&raw, &staging, false, "yedek geri yüklenemedi"));
                let _ = std::fs::remove_file(&raw);
                built
            } else {
                self.backup_part(id, n).and_then(|source| {
                    if source.is_file() {
                        // Dosya dış hedeften gelmiş olabilir: biçimi tahmine bırakılmaz ve başka bir
                        // dosyaya bağlanan (arkalıklı) imaj kabul edilmez.
                        self.check_standalone(&source)?;
                        self.img(
                            &["convert".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-O".as_ref(), "qcow2".as_ref(), source.as_os_str(), staging.as_os_str()],
                            "yedek geri yüklenemedi",
                        )
                    } else {
                        Err(format!("yedek dosyası bulunamadı: {}", source.display()))
                    }
                })
            };
            if let Err(e) = built {
                failure = Some(e);
                break;
            }
        }
        if let Some(e) = failure {
            for (staging, _) in &staged {
                let _ = std::fs::remove_file(staging);
            }
            return Err(e);
        }
        for (staging, disk) in &staged {
            if let Err(e) = std::fs::rename(staging, disk) {
                for (leftover, _) in &staged {
                    let _ = std::fs::remove_file(leftover);
                }
                return Err(format!("disk değiştirilemedi: {e}"));
            }
        }
        self.load_machine(guest, id).map_err(|e| format!("diskler geri yüklendi ama makine durumu (UEFI/TPM) geri yüklenemedi: {e}"))
    }

    fn delete_backup(&self, id: &str) -> Result<(), String> {
        self.backup_path(id)?;
        match std::fs::remove_file(self.machine_path(id)?) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("makine durumu arşivi silinemedi: {e}")),
        }
        for n in 0..=redox_core::hw::MAX_EXTRA_DISKS as u8 {
            let path = self.backup_part(id, n)?;
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{} silinemedi: {e}", path.display())),
            }
        }
        // Artımlı yedekse dizini silinir ve başka yedeğin anmadığı parçalar toplanır.
        self.store.delete(id).map(|_| ())
    }

    fn instant_from_backup(&self, guest: &Guest, backup: &str, disks: &[u8]) -> Result<(), String> {
        if self.store.has(backup) {
            return Err("artımlı yedek parça parça durur, bir diskin arkalığı olamaz; yeni konuk olarak geri yükleyin".to_string());
        }
        self.cbt_invalidate(guest.vmid);
        let mut made: Vec<PathBuf> = Vec::new();
        let built = std::iter::once(0).chain(disks.iter().copied()).try_for_each(|n| {
            let (base, disk) = (self.backup_part(backup, n)?, self.guest_disk(guest, n));
            if !base.is_file() {
                return Err(format!("yedek dosyası yerelde yok: {}", base.display()));
            }
            // Arkalık olarak kullanılacak dosya zincirsiz olmalı (dış hedeften gelmiş olabilir).
            self.check_standalone(&base)?;
            // Konuk oluşturulurken açılan boş disk kaldırılır; yerine yedeğe dayanan katman kurulur.
            let _ = std::fs::remove_file(&disk);
            made.push(disk.clone());
            self.img(
                &["create".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-F".as_ref(), "qcow2".as_ref(), "-b".as_ref(), base.as_os_str(), disk.as_os_str()],
                "yedeğe dayanan disk oluşturulamadı",
            )
        });
        if let Err(e) = built {
            for path in &made {
                let _ = std::fs::remove_file(path);
            }
            return Err(e);
        }
        self.load_machine(guest, backup)
    }

    fn detach_backing(&self, guest: &Guest) -> Result<(), String> {
        let disks: Vec<PathBuf> = disk_ids(guest).iter().map(|n| self.guest_disk(guest, *n)).collect();
        #[cfg(unix)]
        let live = self.alive(guest.vmid);
        #[cfg(not(unix))]
        let live = false;
        if live {
            #[cfg(unix)]
            imp::stream_disks(self, guest, &disks)?;
        } else {
            for disk in &disks {
                // Arkalıktaki veri diske kopyalanır ve bağ kaldırılır.
                self.img(&["rebase".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-b".as_ref(), "".as_ref(), disk.as_os_str()], "disk yedekten ayrılamadı")?;
            }
        }
        // Hâlâ arkalığı olan disk varsa işlem tamamlanmamıştır: yedek serbest bırakılmaz.
        match disks.iter().find(|disk| self.has_backing(disk).unwrap_or(true)) {
            Some(disk) => Err(format!("{} hâlâ yedeğe dayanıyor", disk.display())),
            None => Ok(()),
        }
    }

    /// Diskler önce hedefe geçici adla kopyalanır; hepsi tamamlanınca yerlerine konur ve ancak ondan sonra
    /// kaynaklar silinir. Herhangi bir adım başarısız olursa hedefte dosya bırakılmaz, konuk eski yerinde kalır.
    fn move_storage(&self, guest: &Guest, target: &str) -> Result<(), String> {
        let dest_dir = self.cfg.storage_dir(target);
        if dest_dir == self.cfg.storage_dir(&guest.storage) {
            return Err(format!("{target} alanı konuğun şimdiki alanıyla aynı dizinde"));
        }
        std::fs::create_dir_all(&dest_dir).map_err(|e| format!("{} oluşturulamadı: {e}", dest_dir.display()))?;
        self.cbt_invalidate(guest.vmid);
        let sources: Vec<PathBuf> = disk_ids(guest).iter().map(|n| self.guest_disk(guest, *n)).collect();
        let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
        let copied = sources.iter().try_for_each(|source| {
            let name = source.file_name().and_then(|n| n.to_str()).ok_or("disk adı okunamadı")?.to_string();
            let dest = dest_dir.join(&name);
            if dest.exists() {
                return Err(format!("{} zaten var; önce onu kaldırın", dest.display()));
            }
            let temp = dest_dir.join(format!(".{name}.tasiniyor"));
            let _ = std::fs::remove_file(&temp);
            staged.push((temp.clone(), dest));
            // Bayt bayt kopya: anlık görüntüler ve değişiklik haritası dosyayla birlikte gelir.
            std::fs::copy(source, &temp).map(|_| ()).map_err(|e| format!("{} kopyalanamadı: {e}", source.display()))
        });
        let placed = copied.and_then(|()| staged.iter().try_for_each(|(temp, dest)| std::fs::rename(temp, dest).map_err(|e| format!("{} yerine konamadı: {e}", dest.display()))));
        if let Err(e) = placed {
            for (temp, dest) in &staged {
                let _ = std::fs::remove_file(temp);
                let _ = std::fs::remove_file(dest);
            }
            return Err(e);
        }
        for source in &sources {
            let _ = std::fs::remove_file(source);
        }
        Ok(())
    }

    fn import_disk(&self, guest: &Guest, id: u8, source: &Path) -> Result<(), String> {
        self.cbt_invalidate(guest.vmid);
        let disk = self.cfg.extra_disk_of(guest, id);
        if disk.exists() {
            return Err(format!("{} zaten var", disk.display()));
        }
        // Dosya başka sunucudan geldi: biçimi tahmine bırakılmaz, başka dosyaya bağlanan imaj kabul edilmez.
        self.check_standalone(source)?;
        self.img(&["convert".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-O".as_ref(), "qcow2".as_ref(), source.as_os_str(), disk.as_os_str()], "ek disk içe aktarılamadı")
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&disk);
            })
    }

    fn restore_machine(&self, guest: &Guest, archive: &Path) -> Result<(), String> {
        self.load_machine_from(guest, archive)
    }

    fn host_cpu_vendor(&self) -> Option<&'static str> {
        std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|text| redox_core::hw::parse_cpu_vendor(&text))
    }

    fn guest_run(&self, guest: &Guest, command: &str) -> Result<(), String> {
        let (path, args) = redox_core::hw::split_hook(command).ok_or("komut boş")?;
        #[cfg(unix)]
        {
            match crate::qga::exec(&self.cfg.qga_path(guest.vmid), &path, &args, Duration::from_secs(120)) {
                Ok((0, _)) => Ok(()),
                Ok((code, last)) => Err(format!("{path} {code} koduyla bitti ({last})")),
                Err(e) => Err(format!("{path} çalıştırılamadı: {e}")),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, path, args);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn guest_put(&self, guest: &Guest, target: &str, tar: bool, data: &mut dyn std::io::Read) -> Result<u64, String> {
        #[cfg(unix)]
        {
            let sock = self.cfg.qga_path(guest.vmid);
            if !tar {
                return crate::qga::write_file(&sock, target, data);
            }
            // Arşiv önce hedef dizine yazılır, konuğun kendi `tar` programıyla açılır, sonra silinir.
            let backslash = target.contains('\\');
            let windows = backslash || target.as_bytes().get(1) == Some(&b':');
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) ^ std::process::id();
            let archive = format!("{}{}redox-geri-yukleme-{stamp:08x}.tar", target.trim_end_matches(['/', '\\']), if backslash { '\\' } else { '/' });
            let written = crate::qga::write_file(&sock, &archive, data).map_err(|e| format!("{e} (hedef dizin konukta var mı?)"))?;
            let run = |program: &str, args: &[&str], limit: u64| crate::qga::exec(&sock, program, &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(), Duration::from_secs(limit));
            let extracted = run("tar", &["-xpf", &archive, "-C", target], 1800);
            let _ = if windows { run("cmd.exe", &["/c", "del", "/q", &archive], 60) } else { run("rm", &["-f", &archive], 60) };
            match extracted {
                Ok((0, _)) => Ok(written),
                Ok((code, last)) => Err(format!("arşiv konukta açılamadı: tar {code} koduyla bitti ({last})")),
                Err(e) => Err(format!("arşiv konukta açılamadı: {e}")),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, target, tar, data);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    // Yazılım öykünmesinde (KVM yokken) uzantıyı QEMU kendisi sağlar.
    fn nested_virt(&self) -> bool {
        !self.cfg.kvm || nested_enabled(&self.cfg.sys_module)
    }

    fn io_counters(&self, guest: &Guest) -> Option<(u64, u64)> {
        #[cfg(unix)]
        {
            imp::io_counters(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            None
        }
    }

    fn backups_sealed(&self) -> bool {
        self.store.is_sealed()
    }

    fn backup_raw(&self, id: &str, disk: u8, dest: &Path) -> Result<(), String> {
        if !valid_backup_id(id) {
            return Err(format!("geçersiz yedek kimliği: {id}"));
        }
        if self.store.has(id) {
            return self.unpack(id, disk, dest);
        }
        let source = self.backup_part(id, disk)?;
        self.img(&["convert".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-O".as_ref(), "raw".as_ref(), source.as_os_str(), dest.as_os_str()], "yedeğin ham içeriği çıkarılamadı")
    }

    fn disk_raw(&self, guest: &Guest, disk: u8, dest: &Path) -> Result<(), String> {
        let source = self.guest_disk(guest, disk);
        self.img(&["convert".as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-O".as_ref(), "raw".as_ref(), source.as_os_str(), dest.as_os_str()], "diskin ham içeriği çıkarılamadı")
    }

    fn raw_to_image(&self, raw: &Path, dest: &Path) -> Result<(), String> {
        self.raw_to_qcow2(raw, dest, false, "ham içerikten imaj üretilemedi")
    }

    fn net_counters(&self, guest: &Guest) -> Option<(u64, u64)> {
        tap_counters(&self.cfg.sys_net, &self.guest_taps(guest.vmid))
    }

    fn guest_filesystems(&self, guest: &Guest) -> Option<Vec<redox_core::FsUsage>> {
        #[cfg(unix)]
        {
            let info = self.agent_call(guest.vmid, "guest-get-fsinfo", Duration::from_secs(5))?;
            Some(crate::qga::filesystems(&info))
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            None
        }
    }

    fn balloon_stats(&self, guest: &Guest) -> Option<redox_core::BalloonStats> {
        #[cfg(unix)]
        {
            imp::balloon_stats(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            None
        }
    }

    fn set_balloon(&self, guest: &Guest, target_mb: u64) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::set_balloon(self, guest, target_mb)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, target_mb);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn screenshot(&self, guest: &Guest) -> Result<Vec<u8>, String> {
        #[cfg(unix)]
        {
            imp::screenshot(self, guest)
        }
        #[cfg(not(unix))]
        {
            let _ = guest;
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }

    fn send_keys(&self, guest: &Guest, chords: &[Vec<String>]) -> Result<(), String> {
        #[cfg(unix)]
        {
            imp::send_keys(self, guest, chords)
        }
        #[cfg(not(unix))]
        {
            let _ = (guest, chords);
            Err("QEMU sürücüsü yalnızca Linux'ta çalışır".to_string())
        }
    }
}

impl QemuDriver {
    /// Konuğun QEMU süreci yaşıyor mu?
    #[cfg(unix)]
    fn alive(&self, vmid: VmId) -> bool {
        imp::is_alive(self, vmid)
    }

    /// `qemu-img` çalıştırır; başarısızsa çıktısının son satırını `what` ile birlikte döndürür.
    fn create_image(&self, path: &Path, size_gb: u64) -> Result<(), String> {
        let size = format!("{size_gb}G");
        self.img(&["create".as_ref(), "-f".as_ref(), "qcow2".as_ref(), path.as_os_str(), size.as_ref()], "disk imajı oluşturulamadı")
    }

    fn img(&self, args: &[&std::ffi::OsStr], what: &str) -> Result<(), String> {
        let out = std::process::Command::new(&self.cfg.qemu_img_bin)
            .args(args)
            .output()
            .map_err(|e| format!("{} çalıştırılamadı: {e}", self.cfg.qemu_img_bin))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("{what}: {}", last_line(&out.stderr)))
        }
    }

    fn convert(&self, from: &Path, to: &Path, compress: bool, what: &str) -> Result<(), String> {
        let mut args: Vec<&std::ffi::OsStr> = vec!["convert".as_ref(), "-O".as_ref(), "qcow2".as_ref()];
        if compress {
            args.push("-c".as_ref());
        }
        args.extend([from.as_os_str(), to.as_os_str()]);
        self.img(&args, what)
    }

    /// Çalışan konuğun ajanına komut gönderir. Ajan yanıt vermiyorsa `None`; yanıtsız ajan 30 saniye
    /// boyunca yeniden denenmez.
    #[cfg(unix)]
    fn agent_call(&self, vmid: VmId, command: &str, timeout: Duration) -> Option<Value> {
        const RETRY_ABSENT: Duration = Duration::from_secs(30);
        if !self.alive(vmid) {
            lock(&self.agents).remove(&vmid);
            return None;
        }
        if lock(&self.agents).get(&vmid).is_some_and(|(seen, present)| !present && seen.elapsed() < RETRY_ABSENT) {
            return None;
        }
        let reply = crate::qga::call(&self.cfg.qga_path(vmid), command, timeout).ok();
        lock(&self.agents).insert(vmid, (Instant::now(), reply.is_some()));
        reply
    }

    /// Yedek dosyası kendi başına duran bir qcow2 mu? Arkalık (backing file) ya da ayrı veri dosyası
    /// gösteren bir imaj, çevrilirken sunucudaki başka bir dosyanın içeriğini konuğun diskine taşırdı.
    /// İmajın bir arkalığı var mı? Konuk çalışırken de sorulabilsin diye kilit zorlanmaz (`-U`).
    fn has_backing(&self, path: &Path) -> Result<bool, String> {
        let out = std::process::Command::new(&self.cfg.qemu_img_bin)
            .args(["info", "-U", "--output=json", "-f", "qcow2"])
            .arg(path)
            .output()
            .map_err(|e| format!("{} çalıştırılamadı: {e}", self.cfg.qemu_img_bin))?;
        if !out.status.success() {
            return Err(format!("disk bilgisi okunamadı: {}", last_line(&out.stderr)));
        }
        let info: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("disk bilgisi okunamadı: {e}"))?;
        Ok(["backing-filename", "full-backing-filename"].iter().any(|key| info.get(*key).is_some()))
    }

    fn check_standalone(&self, path: &Path) -> Result<(), String> {
        let out = std::process::Command::new(&self.cfg.qemu_img_bin)
            .args(["info", "--output=json", "-f", "qcow2"])
            .arg(path)
            .output()
            .map_err(|e| format!("{} çalıştırılamadı: {e}", self.cfg.qemu_img_bin))?;
        if !out.status.success() {
            return Err(format!("yedek dosyası okunamadı: {}", last_line(&out.stderr)));
        }
        let info: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("yedek dosyası bilgisi okunamadı: {e}"))?;
        let linked = ["backing-filename", "full-backing-filename", "backing-filename-format"].iter().any(|key| info.get(*key).is_some())
            || info["format-specific"]["data"].get("data-file").is_some();
        if linked {
            return Err("yedek dosyası başka bir dosyaya bağlı (arkalıklı imaj); güvenlik nedeniyle geri yüklenmedi".to_string());
        }
        Ok(())
    }

    /// Konuğun `n` numaralı diski (0: asıl disk).
    fn guest_disk(&self, g: &Guest, n: u8) -> PathBuf {
        if n == 0 { self.cfg.disk_of(g) } else { self.cfg.extra_disk_of(g, n) }
    }

    /// `qemu-img` alt komutunu çalıştırıp JSON çıktısını döndürür.
    #[cfg(unix)]
    fn img_json(&self, args: &[&std::ffi::OsStr], what: &str) -> Result<Value, String> {
        let out = std::process::Command::new(&self.cfg.qemu_img_bin).args(args).output().map_err(|e| format!("{} çalıştırılamadı: {e}", self.cfg.qemu_img_bin))?;
        if !out.status.success() {
            return Err(format!("{what}: {}", last_line(&out.stderr)));
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("{what}: çıktı okunamadı ({e})"))
    }
}

/// Yedeğe giren diskler: 0 asıl disk, ardından konuğun ek diskleri.
fn disk_ids(guest: &Guest) -> Vec<u8> {
    std::iter::once(0).chain(guest.hw.disks.iter().map(|d| d.id)).collect()
}

/// Aynı anda hazırlanan dışa aktarma kopyaları ayrı dizinlerde durur.
static EXPORTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("(çıktı yok)").trim().to_string()
}

#[cfg(test)]
mod tests;

#[cfg(unix)]
mod imp;
