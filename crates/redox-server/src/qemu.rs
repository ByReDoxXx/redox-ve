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

#[derive(Debug, Clone)]
pub struct QemuConfig {
    /// `images/`, `iso/` ve `run/` alt dizinlerinin kökü.
    pub data_dir: PathBuf,
    pub qemu_bin: String,
    pub qemu_img_bin: String,
    /// "Kapat" isteğinde konuğun kendi kapanması için beklenen süre; dolunca zorla kapatılır.
    pub shutdown_timeout: Duration,
    /// KVM hızlandırması var mı? Yoksa yazılım öykünmesi (TCG) kullanılır.
    pub kvm: bool,
    /// UEFI donanım yazılımı dosyalarının (OVMF) dizini.
    pub ovmf_dir: PathBuf,
    /// Sanal TPM öykünücüsü.
    pub swtpm_bin: String,
    /// Ağ hız sınırını uygulayan `tc` (iproute2).
    pub tc_bin: String,
    /// VLAN'lı kartların TAP arayüzünü kuran `ip` ve `bridge` (iproute2).
    pub ip_bin: String,
    pub bridge_bin: String,
    /// Ağ arayüzlerinin sysfs dizini (köprünün VLAN süzmesi buradan okunur).
    pub sys_net: PathBuf,
    /// Ek depolama alanları: `(ad, dizin)`. Konuğun diskleri, kaydındaki depolamanın dizininde durur;
    /// `local` (ve burada tanımlı olmayan her ad) veri dizinindeki `images/` klasörüdür.
    pub storages: Vec<(String, PathBuf)>,
    /// Yeni yedekler parça deposuna mı yazılsın? (Yalnızca değişen kısım yer kaplar.)
    pub incremental_backups: bool,
    /// Değişen blok takibi: çalışan konuğun artımlı yedeğinde yalnızca son yedekten beri yazılan bloklar okunur.
    pub cbt: bool,
}

impl QemuConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        QemuConfig {
            data_dir: data_dir.into(),
            qemu_bin: "qemu-system-x86_64".to_string(),
            qemu_img_bin: "qemu-img".to_string(),
            shutdown_timeout: Duration::from_secs(60),
            kvm: true,
            ovmf_dir: PathBuf::from("/usr/share/OVMF"),
            swtpm_bin: "swtpm".to_string(),
            tc_bin: "tc".to_string(),
            ip_bin: "ip".to_string(),
            bridge_bin: "bridge".to_string(),
            sys_net: PathBuf::from("/sys/class/net"),
            storages: Vec::new(),
            incremental_backups: false,
            cbt: false,
        }
    }

    /// UEFI kod imajı ve değişken deposu kalıbı. Güvenli önyüklemede Microsoft anahtarları kayıtlı kalıp kullanılır.
    pub fn ovmf_files(&self, secure: bool) -> (PathBuf, PathBuf) {
        let (code, vars) = if secure { ("OVMF_CODE_4M.secboot.fd", "OVMF_VARS_4M.ms.fd") } else { ("OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd") };
        (self.ovmf_dir.join(code), self.ovmf_dir.join(vars))
    }

    /// Konuğun UEFI değişken deposu (önyükleme kayıtları, güvenli önyükleme anahtarları).
    pub fn efivars_path(&self, vmid: VmId, secure: bool) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}-efivars{}.qcow2", if secure { "-secure" } else { "" }))
    }

    /// Sanal TPM'in kalıcı durumu.
    pub fn tpm_dir(&self, vmid: VmId) -> PathBuf {
        self.data_dir.join("tpm").join(vmid.to_string())
    }

    pub fn tpm_sock(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.tpm"))
    }

    pub fn tpm_pid(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.tpm.pid"))
    }

    pub fn images_dir(&self) -> PathBuf {
        self.data_dir.join("images")
    }

    pub fn iso_dir(&self) -> PathBuf {
        self.data_dir.join("iso")
    }

    /// Hazır bulut imajlarının durduğu dizin.
    pub fn cloud_dir(&self) -> PathBuf {
        self.data_dir.join("cloud")
    }

    /// Konuğun cloud-init dosyaları (`user-data`, `meta-data`); konuğa `cidata` etiketli sürücü olarak takılır.
    pub fn seed_dir(&self, vmid: VmId) -> PathBuf {
        self.data_dir.join("cidata").join(vmid.to_string())
    }

    pub fn run_dir(&self) -> PathBuf {
        self.data_dir.join("run")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    /// Bir depolama alanının disk dizini.
    pub fn storage_dir(&self, id: &str) -> PathBuf {
        self.storages.iter().find(|(name, _)| name == id).map(|(_, dir)| dir.clone()).unwrap_or_else(|| self.images_dir())
    }

    /// Konuğun asıl diski, kendi depolama alanında.
    pub fn disk_of(&self, g: &Guest) -> PathBuf {
        self.storage_dir(&g.storage).join(format!("vm-{}-disk-0.qcow2", g.vmid))
    }

    /// Konuğun ek diski, kendi depolama alanında.
    pub fn extra_disk_of(&self, g: &Guest, id: u8) -> PathBuf {
        self.storage_dir(&g.storage).join(format!("vm-{}-disk-{id}.qcow2", g.vmid))
    }

    /// Yerel depolamadaki (`local`) asıl disk yolu.
    pub fn disk_path(&self, vmid: VmId) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}-disk-0.qcow2"))
    }

    /// Ek veri diskinin imajı (`id` 1'den başlar).
    pub fn extra_disk_path(&self, vmid: VmId, id: u8) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}-disk-{id}.qcow2"))
    }

    /// Askıya alınan konuğun bellek durumu.
    pub fn state_path(&self, vmid: VmId) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}.state"))
    }

    /// Paylaşılan klasörlerin kökü: her paylaşım `shares/<ad>` dizinidir.
    pub fn shares_dir(&self) -> PathBuf {
        self.data_dir.join("shares")
    }

    /// Canlı konsolun bağlandığı VNC soketi (parolasız; yalnızca hizmet kullanıcısının girebildiği dizinde durur).
    pub fn vnc_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.vnc"))
    }

    /// Konuk ajanının (qemu-guest-agent) kanalı.
    pub fn qga_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.qga"))
    }

    pub fn qmp_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.qmp"))
    }

    pub fn pid_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.pid"))
    }
}

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

/// QEMU seçenek değerlerinde virgül ayırıcıdır; yol içindeki virgül ikilenerek kaçırılır.
fn opt(path: &Path) -> String {
    path.display().to_string().replace(',', ",,")
}

/// Çalışırken takılabilen aygıtların bağlantı noktaları: her ek diskin ve her ek ağ kartının kendi sabit
/// noktası vardır (`hp0`…`hp7` diskler, `hp8`…`hp10` kartlar).
pub const HOTPLUG_PORTS: usize = redox_core::hw::MAX_EXTRA_DISKS + redox_core::hw::MAX_EXTRA_NICS;

/// Ek diskin bağlantı noktası.
pub fn disk_port(id: u8) -> String {
    format!("hp{}", id.saturating_sub(1))
}

/// Ek ağ kartının bağlantı noktası (`index`: 1 ilk ek kart).
pub fn nic_port(index: u8) -> String {
    format!("hp{}", redox_core::hw::MAX_EXTRA_DISKS as u8 + index.saturating_sub(1))
}

/// VLAN'lı kartın sunucudaki TAP arayüzünün adı (`index`: 0 birinci kart). Ad bizim verdiğimiz addır;
/// böylece hangi arayüzün hangi karta ait olduğu kesindir ve etiket yanlış karta uygulanamaz.
pub fn tap_name(vmid: VmId, index: u8) -> String {
    format!("rx{vmid}n{index}")
}

/// VLAN etiketi taşıyan kartlar: `(kart sırası, köprü, etiket)`; 0 birinci karttır.
pub fn vlan_nics(g: &Guest) -> Vec<(u8, String, u16)> {
    let mut list = Vec::new();
    if g.hw.vlan > 0 && is_bridged(g) {
        list.push((0, g.bridge.clone(), g.hw.vlan));
    }
    for (i, spec) in g.hw.nics.iter().enumerate() {
        if let Some((bridge, vlan)) = redox_core::hw::parse_nic(spec).filter(|(_, vlan)| *vlan > 0) {
            list.push((i as u8 + 1, bridge.to_string(), vlan));
        }
    }
    list
}

/// Hazır TAP arayüzüne bağlanan ağ ucu.
fn tap_backend(vmid: VmId, index: u8) -> String {
    format!("tap,id=net{index},ifname={},script=no,downscript=no", tap_name(vmid, index))
}

/// Disk hız sınırının `-drive` ekleri.
fn disk_throttle(g: &Guest) -> String {
    let mut throttle = String::new();
    if g.hw.disk_mbps > 0 {
        throttle.push_str(&format!(",throttling.bps-total={}", u64::from(g.hw.disk_mbps) * 1024 * 1024));
    }
    if g.hw.disk_iops > 0 {
        throttle.push_str(&format!(",throttling.iops-total={}", g.hw.disk_iops));
    }
    throttle
}

/// Donanım yerleşimleri. Askıya alınan konuk, askıya alındığı yerleşimle dönmelidir; bu yüzden eskileri korunur.
pub const LAYOUT_LEGACY: u8 = 0;
/// Çalışırken takma bağlantı noktaları.
pub const LAYOUT_PORTS: u8 = 1;
/// Bağlantı noktaları ve canlı konsol için işaretçi aygıtı (şimdiki yerleşim).
pub const LAYOUT_CURRENT: u8 = 2;

/// Konuğu başlatan QEMU argümanları. Değerler kabuk üzerinden geçmez (argv).
pub fn build_args(cfg: &QemuConfig, g: &Guest) -> Vec<String> {
    build_args_layout(cfg, g, LAYOUT_CURRENT)
}

/// `ports`: çalışırken takma düzeni. Ek diskler ve ek kartlar kendi sabit bağlantı noktalarına oturur; böylece
/// çalışırken takılan aygıt, konuk yeniden açıldığında aynı yerde durur ve askıya alınan konuk aynı donanımla
/// döner. `false`: bu düzenden önce askıya alınmış konukların döndürülmesi için eski yerleşim.
pub fn build_args_layout(cfg: &QemuConfig, g: &Guest, layout: u8) -> Vec<String> {
    let ports = layout >= LAYOUT_PORTS;
    let tablet = layout >= LAYOUT_CURRENT;
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    push(&["-name", &g.name]);
    let uefi = g.hw.firmware == redox_core::hw::Firmware::Uefi;
    let secure = uefi && g.hw.secure_boot;
    // Güvenli önyükleme, değişken deposunu yalnızca sistem yönetim kipinden yazılabilir kılar (SMM).
    let smm = if secure { ",smm=on" } else { "" };
    if cfg.kvm {
        push(&["-machine", &format!("q35,accel=kvm{smm}"), "-cpu", "host"]);
    } else {
        push(&["-machine", &format!("q35,accel=tcg{smm}"), "-cpu", "max"]);
    }
    if uefi {
        let (code, _) = cfg.ovmf_files(secure);
        push(&["-drive", &format!("if=pflash,unit=0,format=raw,readonly=on,file={}", opt(&code))]);
        // Değişken deposu qcow2'dir: çalışan konukta anlık görüntü alınabilsin diye.
        push(&["-drive", &format!("if=pflash,unit=1,format=qcow2,file={}", opt(&cfg.efivars_path(g.vmid, secure)))]);
        if secure {
            push(&["-global", "driver=cfi.pflash01,property=secure,value=on"]);
        }
    }
    if g.hw.tpm {
        push(&["-chardev", &format!("socket,id=chrtpm,path={}", opt(&cfg.tpm_sock(g.vmid)))]);
        push(&["-tpmdev", "emulator,id=tpm0,chardev=chrtpm", "-device", "tpm-crb,tpmdev=tpm0"]);
    }
    push(&["-smp", &g.cores.to_string(), "-m", &format!("{}M", g.mem_total_mb)]);
    // Disk hız sınırı her diske ayrı uygulanır (okuma ve yazma toplamı).
    let throttle = disk_throttle(g);
    push(&["-drive", &format!("file={},if=virtio,format=qcow2{throttle}", opt(&cfg.disk_of(g)))]);
    if ports {
        for i in 0..HOTPLUG_PORTS {
            push(&["-device", &format!("pcie-root-port,id=hp{i},chassis={},slot={}", i + 1, i + 1)]);
        }
    }
    for disk in &g.hw.disks {
        let path = opt(&cfg.extra_disk_of(g, disk.id));
        if ports && (disk.id as usize) <= redox_core::hw::MAX_EXTRA_DISKS && disk.id > 0 {
            push(&["-drive", &format!("file={path},if=none,id=xd{},format=qcow2{throttle}", disk.id)]);
            push(&["-device", &format!("virtio-blk-pci,drive=xd{0},id=xdev{0},bus={1}", disk.id, disk_port(disk.id))]);
        } else {
            push(&["-drive", &format!("file={path},if=virtio,format=qcow2{throttle}")]);
        }
    }
    // Geçersiz ad buraya ulaşmamalı (çekirdek doğrular); yine de yol kaçışına karşı yoksayılır.
    if let Some(iso) = g.iso.as_deref().filter(|n| valid_iso_name(n)) {
        push(&["-drive", &format!("file={},media=cdrom,readonly=on", opt(&cfg.iso_dir().join(iso)))]);
        // İlk açılışta kalıptan, sonraki yeniden başlatmalarda diskten.
        push(&["-boot", "order=c,once=d"]);
    }
    if g.hw.cloud_init() {
        // cloud-init (NoCloud): QEMU dizini salt okunur bir FAT sürücüsü olarak sunar; konuk onu
        // `cidata` etiketinden tanır. Ayrı bir ISO üretmeye gerek kalmaz.
        push(&["-drive", &format!("file.driver=vvfat,file.dir={},file.label=cidata,file.rw=off,format=raw,if=virtio,readonly=on", opt(&cfg.seed_dir(g.vmid)))]);
    }
    // Kullanıcı kipinde ağ (NAT): köprü ve kök yetkisi gerektirmez.
    let nic = format!("virtio-net-pci,netdev=net0,mac={}", mac_for(g.vmid));
    if is_bridged(g) {
        // Köprü: konuk sunucunun ağına doğrudan bağlanır. QEMU'nun setuid yardımcısı
        // (qemu-bridge-helper) TAP arayüzünü açar; köprü /etc/qemu/bridge.conf içinde izinli olmalıdır.
        // VLAN etiketli kart, sunucunun hazırladığı adı belli TAP arayüzüne bağlanır (etiket orada uygulanır).
        let backend = if g.hw.vlan > 0 { tap_backend(g.vmid, 0) } else { format!("bridge,id=net0,br={}", g.bridge) };
        push(&["-netdev", &backend, "-device", &nic]);
    } else {
        // Kullanıcı kipinde NAT: köprü ve kök yetkisi gerektirmez. Port yönlendirmeleri sunucunun
        // bütün arayüzlerinde dinler (güvenlik duvarı ayrıca açılmalıdır).
        let mut net = "user,id=net0".to_string();
        for f in &g.forwards {
            net.push_str(&format!(",hostfwd=tcp::{}-:{}", f.host_port, f.guest_port));
        }
        push(&["-netdev", &net, "-device", &nic]);
    }
    // Sunucudaki fiziksel aygıtlar. Kimlikler çekirdekte doğrulanır; yine de yalnızca geçerli olanlar argümana girer.
    let usb: Vec<&String> = g.hw.usb.iter().filter(|u| redox_core::hw::valid_usb_id(u)).collect();
    if !usb.is_empty() || tablet {
        push(&["-device", "qemu-xhci,id=xhci"]);
    }
    if tablet {
        // Mutlak konumlu işaretçi: canlı konsolda konuğun imleci tarayıcıdaki imleçle üst üste kalır.
        push(&["-device", "usb-tablet,bus=xhci.0"]);
    }
    for id in usb {
        let (vendor, product) = id.split_once(':').unwrap_or(("", ""));
        push(&["-device", &format!("usb-host,bus=xhci.0,vendorid=0x{vendor},productid=0x{product}")]);
    }
    for addr in g.hw.pci.iter().filter(|p| redox_core::hw::valid_pci_addr(p)) {
        push(&["-device", &format!("vfio-pci,host={addr}")]);
    }
    // Ek ağ kartları: her biri kendi ağına, kimlikten türetilen ayrı bir adresle.
    for (i, spec) in g.hw.nics.iter().enumerate() {
        let Some((network, vlan)) = redox_core::hw::parse_nic(spec) else { continue };
        let n = i as u8 + 1;
        let backend = if network == NAT {
            format!("user,id=net{n}")
        } else if vlan > 0 {
            tap_backend(g.vmid, n)
        } else {
            format!("bridge,id=net{n},br={network}")
        };
        let place = if ports && (n as usize) <= redox_core::hw::MAX_EXTRA_NICS { format!(",id=xnic{n},bus={}", nic_port(n)) } else { String::new() };
        push(&["-netdev", &backend, "-device", &format!("virtio-net-pci,netdev=net{n},mac={}{place}", redox_core::mac_for_nic(g.vmid, n))]);
    }
    // Paylaşılan klasörler: konukta `mount -t 9p -o trans=virtio <ad> <dizin>` ile bağlanır. Dosyalar sunucuda
    // hizmet kullanıcısının olur (security_model=none); adlar çekirdekte doğrulanır, yine de burada süzülür.
    for (i, share) in g.hw.shares.iter().enumerate().filter(|(_, s)| s.valid()) {
        let ro = if share.readonly { ",readonly=on" } else { "" };
        push(&["-fsdev", &format!("local,id=fs{i},path={},security_model=none,multidevs=remap{ro}", opt(&cfg.shares_dir().join(&share.name)))]);
        push(&["-device", &format!("virtio-9p-pci,fsdev=fs{i},mount_tag={}", share.name)]);
    }
    // Konuk ajanı kanalı: konukta qemu-guest-agent çalışıyorsa adresini ve işletim sistemini bildirir,
    // yedekten önce dosya sistemini dondurur. Ajan yoksa kanal boşta durur.
    push(&["-device", "virtio-serial-pci,id=agent0"]);
    push(&["-chardev", &format!("socket,id=qga0,path={},server=on,wait=off", opt(&cfg.qga_path(g.vmid)))]);
    push(&["-device", "virtserialport,chardev=qga0,name=org.qemu.guest_agent.0"]);
    // Ekran penceresi açılmaz; görüntü konsol için QMP ekran dökümüyle alınır.
    push(&["-vga", "std", "-display", "none"]);
    // Canlı konsol: ekran ve giriş, hizmetin köprülediği bir Unix soketinden (VNC) sunulur. Ağa açılmaz.
    push(&["-vnc", &format!("unix:{}", opt(&cfg.vnc_path(g.vmid)))]);
    push(&["-qmp", &format!("unix:{},server=on,wait=off", opt(&cfg.qmp_path(g.vmid)))]);
    push(&["-pidfile", &cfg.pid_path(g.vmid).display().to_string(), "-daemonize"]);
    a
}

/// cloud-init dosyalarını yazar (her başlatmadan önce: ad değişikliği konuğa yansır).
/// cloud-init kullanılmıyorsa dizini kaldırır.
pub fn write_seed(cfg: &QemuConfig, g: &Guest) -> Result<(), String> {
    let dir = cfg.seed_dir(g.vmid);
    let Some(user_data) = g.hw.user_data(&g.name) else {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(());
    };
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).map_err(|e| format!("cloud-init dosyası yazılamadı ({name}): {e}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    write("user-data", &user_data)?;
    write("meta-data", &redox_core::hw::meta_data(g.vmid, &g.name))
}

/// Başlatmadan önce UEFI değişken deposunu (yoksa kalıptan) hazırlar.
#[cfg(unix)]
fn prepare_efivars(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    if g.hw.firmware != redox_core::hw::Firmware::Uefi {
        return Ok(());
    }
    let (code, template) = d.cfg.ovmf_files(g.hw.secure_boot);
    if !code.is_file() || !template.is_file() {
        return Err(format!("UEFI donanım yazılımı bulunamadı: {} (sudo apt install ovmf)", code.display()));
    }
    let vars = d.cfg.efivars_path(g.vmid, g.hw.secure_boot);
    if vars.is_file() {
        return Ok(());
    }
    d.convert(&template, &vars, false, "UEFI değişken deposu oluşturulamadı")
}

/// Yedek kimliği dosya adına dönüşür; yol kaçışına karşı yalnızca güvenli karakterler.
pub fn valid_backup_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Konuk bir köprüye mi bağlı? (`nat` ya da geçersiz ad kullanıcı kipine düşer.)
pub fn is_bridged(g: &Guest) -> bool {
    g.bridge != NAT && valid_bridge(&g.bridge)
}

pub fn qmp_command(name: &str) -> String {
    format!("{}\n", serde_json::json!({ "execute": name }))
}

#[derive(Debug, Clone, PartialEq)]
pub enum QmpLine {
    Greeting,
    Return(Value),
    Error(String),
    /// Kendiliğinden gelen olay (SHUTDOWN, STOP …); yanıt beklerken atlanır.
    Event(String),
    Other,
}

pub fn parse_qmp_line(line: &str) -> QmpLine {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return QmpLine::Other;
    };
    if v.get("QMP").is_some() {
        QmpLine::Greeting
    } else if let Some(r) = v.get("return") {
        QmpLine::Return(r.clone())
    } else if let Some(e) = v.get("error") {
        QmpLine::Error(e["desc"].as_str().unwrap_or("bilinmeyen QMP hatası").to_string())
    } else if let Some(e) = v.get("event") {
        QmpLine::Event(e.as_str().unwrap_or_default().to_string())
    } else {
        QmpLine::Other
    }
}

/// `/proc/<pid>/stat` içinden toplam CPU tıkları (utime + stime).
pub fn parse_stat_ticks(stat: &str) -> Option<u64> {
    // Süreç adı parantez içinde boşluk içerebilir; son ')' sonrasından say.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

/// `/proc/<pid>/status` içinden bellek kullanımı (kB).
pub fn parse_vmrss_kb(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// İki örnek arasındaki tık farkından, konuğun çekirdeklerine göre CPU yüzdesi.
/// Linux'ta saniyede 100 tık (USER_HZ) varsayılır.
pub fn cpu_pct(delta_ticks: u64, elapsed: Duration, cores: u32) -> u8 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 || cores == 0 {
        return 0;
    }
    let used = delta_ticks as f64 / 100.0;
    (used / secs / cores as f64 * 100.0).round().clamp(0.0, 100.0) as u8
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
        if self.cfg.incremental_backups {
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
        Ok(())
    }

    fn delete_backup(&self, id: &str) -> Result<(), String> {
        self.backup_path(id)?;
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

    /// Tam yedekte `n` numaralı diskin dosyası.
    fn backup_part(&self, id: &str, n: u8) -> Result<PathBuf, String> {
        let main = self.backup_path(id)?;
        Ok(if n == 0 { main } else { self.cfg.backups_dir().join(format!("{id}.disk{n}.qcow2")) })
    }

    /// Diskleri hedef dosyalara döker: çalışan konukta hepsi aynı andaki tutarlı görüntüdür
    /// (tek QMP işlemi), kapalı konukta `qemu-img` ile. `raw`: sıkıştırılmış qcow2 yerine ham imaj.
    /// Dönen değer: konuk çalışıyorsa dosya sistemi donduruldu mu (kapalı konukta `None`).
    fn dump(&self, guest: &Guest, parts: &[(PathBuf, PathBuf)], live: bool, raw: bool, mode: Cbt) -> Result<Option<bool>, String> {
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
        for (disk, target) in parts {
            if raw {
                self.img(&["convert".as_ref(), "-O".as_ref(), "raw".as_ref(), disk.as_os_str(), target.as_os_str()], "yedek alınamadı")?;
            } else {
                self.convert(disk, target, true, "yedek alınamadı")?;
            }
        }
        Ok(None)
    }

    /// Artımlı yedek: diskler önce ham dosyalara dökülür, sonra birlikte parça deposuna işlenir.
    /// Ham dosyalar geçicidir.
    fn backup_chunks(&self, guest: &Guest, id: &str, live: bool, ids: &[u8]) -> Result<BackupSize, String> {
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

    fn cbt_path(&self, vmid: VmId) -> PathBuf {
        self.cfg.images_dir().join(format!("vm-{vmid}.cbt.json"))
    }

    fn cbt_state(&self, vmid: VmId) -> Option<CbtState> {
        serde_json::from_slice(&std::fs::read(self.cbt_path(vmid)).ok()?).ok()
    }

    /// Diskin içeriği haritaya işlenmeden değişecekse (geri yükleme, geri dönüş, boyut değişikliği) çağrılır.
    fn cbt_invalidate(&self, vmid: VmId) {
        let _ = std::fs::remove_file(self.cbt_path(vmid));
    }

    fn cbt_record(&self, vmid: VmId, state: &CbtState) {
        if let Ok(text) = serde_json::to_vec(state) {
            let _ = std::fs::write(self.cbt_path(vmid), text);
        }
    }

    /// Kapalı konuğun imajında haritayı sıfırlar (yoksa oluşturur). Başarısızsa (eski qemu-img) `false`.
    fn arm_offline(&self, disk: &Path) -> bool {
        let run = |flag: &str| self.img(&["bitmap".as_ref(), flag.as_ref(), disk.as_os_str(), CBT_BITMAP.as_ref()], "harita").is_ok();
        run("--add") || run("--clear") || (run("--remove") && run("--add"))
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

    /// Çalışan konuğun yalnızca son yedekten beri yazılan bloklarını okuyup `base` yedeğinin üzerine işler.
    #[cfg(unix)]
    fn backup_delta(&self, guest: &Guest, id: &str, base: &str, ids: &[u8]) -> Result<BackupSize, String> {
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
    fn unpack(&self, id: &str, disk: u8, raw: &Path) -> Result<(), String> {
        let mut file = std::fs::File::create(raw).map_err(|e| format!("{} oluşturulamadı: {e}", raw.display()))?;
        self.store.read(id, disk, &mut file).map(|_| ())
    }

    /// Ham diski qcow2'ye çevirir. Biçim açıkça verilir: ham içerik konuğun elindedir ve
    /// biçim tahminine bırakılırsa başka bir imaj gibi yorumlanabilir.
    fn raw_to_qcow2(&self, raw: &Path, to: &Path, compress: bool, what: &str) -> Result<(), String> {
        let mut args: Vec<&std::ffi::OsStr> = vec!["convert".as_ref(), "-f".as_ref(), "raw".as_ref(), "-O".as_ref(), "qcow2".as_ref()];
        if compress {
            args.push("-c".as_ref());
        }
        args.extend([raw.as_os_str(), to.as_os_str()]);
        self.img(&args, what)
    }

    /// Yedek kimliği dosya adına dönüşür; yol kaçışına karşı yalnızca güvenli karakterler.
    fn backup_path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_backup_id(id) {
            return Err(format!("geçersiz yedek kimliği: {id}"));
        }
        Ok(self.cfg.backups_dir().join(format!("{id}.qcow2")))
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

#[cfg(unix)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use redox_core::{Action, Guest, GuestStatus, Probe, SnapshotOp, VmId};
    use serde_json::Value;

    use super::{cpu_pct, last_line, lock, parse_qmp_line, parse_stat_ticks, parse_vmrss_kb, QemuDriver, QmpLine};

    const QMP_TIMEOUT: Duration = Duration::from_secs(3);
    const POLL: Duration = Duration::from_millis(150);

    /// PID dosyasındaki süreç yaşıyor ve gerçekten bu konuğun QEMU'su mu?
    /// (PID yeniden kullanılmış olabilir; komut satırında soket yolu aranır.)
    fn live_pid(d: &QemuDriver, vmid: VmId) -> Option<u32> {
        let pid: u32 = std::fs::read_to_string(d.cfg.pid_path(vmid)).ok()?.trim().parse().ok()?;
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let sock = d.cfg.qmp_path(vmid).display().to_string();
        String::from_utf8_lossy(&cmdline).contains(&sock).then_some(pid)
    }

    /// QMP oturumu açar, yetenek el sıkışmasını yapar ve komutları sırayla çalıştırır.
    fn qmp(sock: &Path, commands: &[&str]) -> Result<Vec<Value>, String> {
        let values: Vec<Value> = commands.iter().map(|c| serde_json::json!({ "execute": c })).collect();
        qmp_values(sock, &values)
    }

    /// Argümanlı komutlar için: her öge eksiksiz bir QMP isteğidir.
    fn qmp_values(sock: &Path, commands: &[Value]) -> Result<Vec<Value>, String> {
        qmp_run(sock, commands, QMP_TIMEOUT)
    }

    pub fn is_alive(d: &QemuDriver, vmid: VmId) -> bool {
        live_pid(d, vmid).is_some()
    }

    /// Yanıt için `timeout` kadar bekler (uzun süren izleyici komutları için).
    fn qmp_run(sock: &Path, commands: &[Value], timeout: Duration) -> Result<Vec<Value>, String> {
        let err = |e: std::io::Error| format!("QMP ({}): {e}", sock.display());
        let stream = UnixStream::connect(sock).map_err(err)?;
        stream.set_read_timeout(Some(timeout)).map_err(err)?;
        stream.set_write_timeout(Some(QMP_TIMEOUT)).map_err(err)?;
        let mut writer = stream.try_clone().map_err(err)?;
        let mut reader = BufReader::new(stream);

        let read = |reader: &mut BufReader<UnixStream>| -> Result<QmpLine, String> {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => Err("QMP bağlantısı kapandı".to_string()),
                Ok(_) => Ok(parse_qmp_line(&line)),
                Err(e) => Err(err(e)),
            }
        };
        if read(&mut reader)? != QmpLine::Greeting {
            return Err("QMP karşılaması alınamadı".to_string());
        }
        let mut results = Vec::new();
        let hello = serde_json::json!({ "execute": "qmp_capabilities" });
        for (i, command) in std::iter::once(&hello).chain(commands).enumerate() {
            let name = command["execute"].as_str().unwrap_or("?");
            writer.write_all(format!("{command}\n").as_bytes()).map_err(err)?;
            loop {
                match read(&mut reader)? {
                    QmpLine::Return(v) => {
                        if i > 0 {
                            results.push(v);
                        }
                        break;
                    }
                    QmpLine::Error(desc) => return Err(format!("QMP {name}: {desc}")),
                    // Olaylar ve tanınmayan satırlar yanıt değildir.
                    QmpLine::Event(_) | QmpLine::Other | QmpLine::Greeting => continue,
                }
            }
        }
        Ok(results)
    }

    fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            if done() {
                return true;
            }
            if start.elapsed() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Konuğun sanal TPM öykünücüsünü başlatır ve soketinin açılmasını bekler.
    fn start_tpm(d: &QemuDriver, g: &Guest) -> Result<(), String> {
        if !g.hw.tpm {
            return Ok(());
        }
        let (dir, sock, pid) = (d.cfg.tpm_dir(g.vmid), d.cfg.tpm_sock(g.vmid), d.cfg.tpm_pid(g.vmid));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
        // --terminate: QEMU bağlantıyı kapatınca (konuk durunca) öykünücü de çıkar.
        let out = std::process::Command::new(&d.cfg.swtpm_bin)
            .args(["socket", "--tpm2", "--terminate", "--daemon"])
            .arg("--tpmstate")
            .arg(format!("dir={}", dir.display()))
            .arg("--ctrl")
            .arg(format!("type=unixio,path={}", sock.display()))
            .arg("--pid")
            .arg(format!("file={}", pid.display()))
            .output()
            .map_err(|e| format!("{} çalıştırılamadı: {e} (sudo apt install swtpm)", d.cfg.swtpm_bin))?;
        if !out.status.success() {
            return Err(format!("sanal TPM başlatılamadı: {}", last_line(&out.stderr)));
        }
        if !wait_until(Duration::from_secs(3), || sock.exists()) {
            stop_tpm(d, g.vmid);
            return Err("sanal TPM başladı ama soketi açılmadı".to_string());
        }
        Ok(())
    }

    /// Artakalan TPM öykünücüsünü durdurur. Kimlik dosyasındaki süreç gerçekten bu konuğun öykünücüsüyse öldürülür.
    fn stop_tpm(d: &QemuDriver, vmid: VmId) {
        let (sock, pid_file) = (d.cfg.tpm_sock(vmid), d.cfg.tpm_pid(vmid));
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if String::from_utf8_lossy(&cmdline).contains(&sock.display().to_string()) {
                let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
            }
        }
        let _ = std::fs::remove_file(pid_file);
        let _ = std::fs::remove_file(sock);
    }

    fn tool(bin: &str, args: &[&str]) -> Result<(), String> {
        let out = std::process::Command::new(bin).args(args).output().map_err(|e| format!("{bin} çalıştırılamadı: {e} (iproute2 kurulu mu?)"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("{bin} {}: {}", args[..args.len().min(3)].join(" "), last_line(&out.stderr)))
        }
    }

    /// Bir VLAN kartının TAP arayüzünü kurar: arayüz bu kullanıcıya ait açılır, köprüye bağlanır ve yalnızca
    /// kendi VLAN'ına (etiketsiz, varsayılan olarak) üye yapılır.
    fn vlan_tap(d: &QemuDriver, name: &str, bridge: &str, vlan: u16) -> Result<(), String> {
        let filtering = std::fs::read_to_string(d.cfg.sys_net.join(bridge).join("bridge/vlan_filtering")).map(|s| s.trim() == "1").unwrap_or(false);
        if !filtering {
            return Err(format!("{bridge} köprüsünde VLAN süzme kapalı (ya da köprü yok); açmak için: ip link set {bridge} type bridge vlan_filtering 1"));
        }
        let (ip, br) = (d.cfg.ip_bin.as_str(), d.cfg.bridge_bin.as_str());
        if d.cfg.sys_net.join(name).exists() {
            let _ = tool(ip, &["link", "del", name]);
        }
        let uid = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata("/proc/self").map(|m| m.uid().to_string()).unwrap_or_else(|_| "0".to_string())
        };
        tool(ip, &["tuntap", "add", "dev", name, "mode", "tap", "user", &uid])?;
        tool(ip, &["link", "set", "dev", name, "master", bridge])?;
        // Köprünün varsayılan VLAN'ı (1) kaldırılır; yoksa hata önemsizdir.
        let _ = tool(br, &["vlan", "del", "dev", name, "vid", "1"]);
        tool(br, &["vlan", "add", "dev", name, "vid", &vlan.to_string(), "pvid", "untagged"])?;
        tool(ip, &["link", "set", "dev", name, "up"])
    }

    const VLAN_HINT: &str = "etiketi kaldırın ya da hizmete CAP_NET_ADMIN yetkisi verin (iproute2 kurulu ve köprüde VLAN süzme açık olmalı)";

    /// Konuğun VLAN etiketli bütün kartlarının arayüzlerini kurar.
    fn prepare_vlan_taps(d: &QemuDriver, g: &Guest) -> Result<(), String> {
        for (index, bridge, vlan) in super::vlan_nics(g) {
            vlan_tap(d, &super::tap_name(g.vmid, index), &bridge, vlan).map_err(|e| format!("VLAN arayüzü hazırlanamadı: {e}; {VLAN_HINT}"))?;
        }
        Ok(())
    }

    /// Konuğun bizim kurduğumuz TAP arayüzlerini kaldırır (konuk kapanınca ya da açılamayınca).
    fn remove_vlan_taps(d: &QemuDriver, vmid: VmId) {
        for index in 0..=redox_core::hw::MAX_EXTRA_NICS as u8 {
            let name = super::tap_name(vmid, index);
            if d.cfg.sys_net.join(&name).exists() {
                let _ = tool(&d.cfg.ip_bin, &["link", "del", &name]);
            }
        }
    }

    fn cleanup(d: &QemuDriver, vmid: VmId) {
        stop_tpm(d, vmid);
        remove_vlan_taps(d, vmid);
        let _ = std::fs::remove_file(d.cfg.pid_path(vmid));
        let _ = std::fs::remove_file(d.cfg.qmp_path(vmid));
        let _ = std::fs::remove_file(d.cfg.qga_path(vmid));
        let _ = std::fs::remove_file(d.cfg.vnc_path(vmid));
        lock(&d.agents).remove(&vmid);
        lock(&d.samples).remove(&vmid);
    }

    /// Konuk, çalışırken takma bağlantı noktalarıyla mı açılmış?
    fn has_ports(d: &QemuDriver, vmid: VmId) -> bool {
        live_pid(d, vmid).and_then(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok()).is_some_and(|c| String::from_utf8_lossy(&c).contains("pcie-root-port,id=hp0"))
    }

    /// Çalışan konuğun açıldığı donanım yerleşimi (komut satırından okunur).
    fn running_layout(d: &QemuDriver, vmid: VmId) -> u8 {
        let cmdline = live_pid(d, vmid).and_then(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok()).map(|c| String::from_utf8_lossy(&c).into_owned()).unwrap_or_default();
        if !cmdline.contains("pcie-root-port,id=hp0") {
            super::LAYOUT_LEGACY
        } else if cmdline.contains("usb-tablet") {
            super::LAYOUT_CURRENT
        } else {
            super::LAYOUT_PORTS
        }
    }

    const NO_PORTS: &str = "bu konuk çalışırken takmayı desteklemeyen eski yerleşimle açılmış; bir kez kapatıp açın, sonra yeniden deneyin";

    /// İzleyici komutunun çıktısını döndürür.
    fn hmp_text(d: &QemuDriver, g: &Guest, line: &str) -> Result<String, String> {
        let command = serde_json::json!({ "execute": "human-monitor-command", "arguments": { "command-line": line } });
        let out = qmp_values(&d.cfg.qmp_path(g.vmid), &[command])?;
        Ok(out.first().and_then(Value::as_str).unwrap_or_default().trim().to_string())
    }

    pub fn hot_add_disk(d: &QemuDriver, g: &Guest, id: u8) -> Result<(), String> {
        if live_pid(d, g.vmid).is_none() {
            return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
        }
        if !has_ports(d, g.vmid) {
            return Err(NO_PORTS.to_string());
        }
        let sock = d.cfg.qmp_path(g.vmid);
        let drive = format!("xd{id}");
        let added = hmp_text(d, g, &format!("drive_add 0 file={},if=none,id={drive},format=qcow2{}", super::opt(&d.cfg.extra_disk_of(g, id)), super::disk_throttle(g)))?;
        if added != "OK" {
            return Err(format!("disk konuğa tanıtılamadı: {}", added.lines().last().unwrap_or("bilinmeyen hata")));
        }
        let device = serde_json::json!({ "execute": "device_add", "arguments": { "driver": "virtio-blk-pci", "drive": drive, "id": format!("xdev{id}"), "bus": super::disk_port(id) } });
        qmp_values(&sock, &[device]).map(|_| ()).map_err(|e| {
            let _ = hmp_text(d, g, &format!("drive_del {drive}"));
            format!("disk konuğa takılamadı: {e}")
        })
    }

    pub fn hot_add_nic(d: &QemuDriver, g: &Guest, index: u8, network: &str) -> Result<(), String> {
        if live_pid(d, g.vmid).is_none() {
            return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
        }
        if !has_ports(d, g.vmid) {
            return Err(NO_PORTS.to_string());
        }
        let Some((network, vlan)) = redox_core::hw::parse_nic(network) else {
            return Err(format!("geçersiz ağ ayarı: {network}"));
        };
        let sock = d.cfg.qmp_path(g.vmid);
        let fdinfo = live_pid(d, g.vmid).map(|pid| format!("/proc/{pid}/fdinfo")).unwrap_or_default();
        let before = crate::netlimit::taps_of(Path::new(&fdinfo));
        let netdev = format!("net{index}");
        let tap = super::tap_name(g.vmid, index);
        let backend = if network == super::NAT {
            serde_json::json!({ "execute": "netdev_add", "arguments": { "type": "user", "id": netdev } })
        } else if vlan > 0 {
            vlan_tap(d, &tap, network, vlan).map_err(|e| format!("VLAN arayüzü hazırlanamadı: {e}; {VLAN_HINT}"))?;
            serde_json::json!({ "execute": "netdev_add", "arguments": { "type": "tap", "id": netdev, "ifname": tap, "script": "no", "downscript": "no" } })
        } else {
            serde_json::json!({ "execute": "netdev_add", "arguments": { "type": "bridge", "id": netdev, "br": network } })
        };
        let drop_tap = || {
            if vlan > 0 {
                let _ = tool(&d.cfg.ip_bin, &["link", "del", &tap]);
            }
        };
        qmp_values(&sock, &[backend]).map_err(|e| {
            drop_tap();
            format!("ağ kartı konuğa tanıtılamadı: {e}")
        })?;
        let remove = || {
            let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "netdev_del", "arguments": { "id": netdev } })]);
            drop_tap();
        };
        let device = serde_json::json!({ "execute": "device_add", "arguments": {
            "driver": "virtio-net-pci", "netdev": netdev, "id": format!("xnic{index}"), "mac": redox_core::mac_for_nic(g.vmid, index), "bus": super::nic_port(index) } });
        if let Err(e) = qmp_values(&sock, &[device]) {
            remove();
            return Err(format!("ağ kartı konuğa takılamadı: {e}"));
        }
        // Ağ hız sınırı yeni kartın arayüzüne de uygulanır; uygulanamazsa kart sınırsız bırakılmaz.
        if g.hw.net_mbps > 0 && network != super::NAT {
            let fresh: Vec<String> = crate::netlimit::taps_of(Path::new(&fdinfo)).into_iter().filter(|t| !before.contains(t)).collect();
            let limited = if fresh.is_empty() { Err("yeni kartın köprü arayüzü (TAP) bulunamadı".to_string()) } else { crate::netlimit::apply(&d.cfg.tc_bin, &fresh, g.hw.net_mbps) };
            if let Err(e) = limited {
                let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "device_del", "arguments": { "id": format!("xnic{index}") } })]);
                remove();
                return Err(format!("ağ hız sınırı uygulanamadı: {e}; kart takılmadı"));
            }
        }
        Ok(())
    }

    fn limit_network(d: &QemuDriver, g: &Guest) -> Result<(), String> {
        if g.hw.net_mbps == 0 {
            return Ok(());
        }
        let hint = "sınırı 0 yapın ya da hizmete CAP_NET_ADMIN yetkisi verin (iproute2 kurulu olmalı)";
        let taps = live_pid(d, g.vmid).map(|pid| crate::netlimit::taps_of(Path::new(&format!("/proc/{pid}/fdinfo")))).unwrap_or_default();
        if taps.is_empty() {
            return Err(format!("ağ hız sınırı uygulanamadı: konuğun köprü arayüzü (TAP) bulunamadı; {hint}"));
        }
        crate::netlimit::apply(&d.cfg.tc_bin, &taps, g.hw.net_mbps).map_err(|e| format!("ağ hız sınırı uygulanamadı: {e}; {hint}"))
    }

    pub fn apply(d: &QemuDriver, g: &Guest, action: Action) -> Result<(), String> {
        let sock = d.cfg.qmp_path(g.vmid);
        let alive = || live_pid(d, g.vmid).is_some();
        match action {
            Action::Start => {
                if alive() {
                    return Err("konuk zaten çalışıyor".to_string());
                }
                if !d.cfg.disk_of(g).exists() {
                    return Err(format!("disk imajı bulunamadı: {}", d.cfg.disk_of(g).display()));
                }
                cleanup(d, g.vmid);
                super::write_seed(&d.cfg, g)?;
                super::prepare_efivars(d, g)?;
                // Paylaşılan klasörlerin dizinleri yoksa oluşturulur (yalnızca hizmet kullanıcısına açık).
                for share in g.hw.shares.iter().filter(|s| s.valid()) {
                    let dir = d.cfg.shares_dir().join(&share.name);
                    if !dir.is_dir() {
                        use std::os::unix::fs::DirBuilderExt;
                        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("paylaşılan klasör {} oluşturulamadı: {e}", dir.display()))?;
                    }
                }
                start_tpm(d, g)?;
                if let Err(e) = prepare_vlan_taps(d, g) {
                    stop_tpm(d, g.vmid);
                    remove_vlan_taps(d, g.vmid);
                    return Err(e);
                }
                // Askıdan dönüş: kayıtlı bellek durumu varsa QEMU onu yükleyerek başlar ve konuk kaldığı yerden sürer.
                let state = d.cfg.state_path(g.vmid);
                let resuming = state.is_file();
                // Askıdaki konuk, askıya alındığı donanım yerleşimiyle döner (eski kayıtlarda bağlantı noktası yoktur).
                let layout = if !resuming {
                    super::LAYOUT_CURRENT
                } else {
                    // İşaret dosyası yoksa en eski yerleşim; içeriği yerleşim numarasıdır (boşsa 1).
                    match std::fs::read_to_string(state.with_extension("state.hp")) {
                        Err(_) => super::LAYOUT_LEGACY,
                        Ok(text) => text.trim().parse().unwrap_or(super::LAYOUT_PORTS),
                    }
                };
                let mut args = super::build_args_layout(&d.cfg, g, layout);
                if resuming {
                    args.extend(["-incoming".to_string(), format!("exec:cat {}", shell_quote(&state))]);
                }
                let started = std::process::Command::new(&d.cfg.qemu_bin)
                    .args(args)
                    .output()
                    .map_err(|e| format!("{} çalıştırılamadı: {e}", d.cfg.qemu_bin))
                    .and_then(|out| if out.status.success() { Ok(()) } else { Err(format!("QEMU başlatılamadı: {}", last_line(&out.stderr))) })
                    .and_then(|()| {
                        if wait_until(Duration::from_secs(5), alive) { Ok(()) } else { Err("QEMU başladı ama süreç bulunamadı (hemen çıkmış olabilir)".to_string()) }
                    });
                if started.is_err() {
                    stop_tpm(d, g.vmid);
                    remove_vlan_taps(d, g.vmid);
                    return started;
                }
                // Ağ hız sınırı konuk başladıktan sonra TAP arayüzüne uygulanır. Uygulanamıyorsa konuk
                // sınırsız çalışmaya bırakılmaz: kapatılır ve hata bildirilir.
                if let Err(e) = limit_network(d, g) {
                    let _ = qmp(&sock, &["quit"]);
                    wait_until(Duration::from_secs(5), || !alive());
                    cleanup(d, g.vmid);
                    return Err(e);
                }
                if resuming {
                    // Durum yüklenene kadar konuk "inmigrate" görünür; yüklenince kendiliğinden çalışır.
                    let running = || qmp(&sock, &["query-status"]).ok().and_then(|r| r.first().and_then(|s| s["status"].as_str().map(|s| s == "running" || s == "paused"))).unwrap_or(false);
                    if !wait_until(Duration::from_secs(600), || running() || !alive()) || !alive() {
                        let _ = qmp(&sock, &["quit"]);
                        cleanup(d, g.vmid);
                        return Err("konuk askıdan döndürülemedi (bellek durumu bu donanımla uyuşmuyor olabilir); askı durumunu atıp yeniden başlatın".to_string());
                    }
                    // Durum, konuk durdurulmuşken kaydedilir ve QEMU onu durdurulmuş olarak geri yükler:
                    // askıya alınırken çalışıyorduysa yeniden çalıştırılır.
                    let marker = state.with_extension("state.running");
                    if marker.exists() {
                        let _ = qmp(&sock, &["cont"]);
                    }
                    let _ = std::fs::remove_file(&marker);
                    let _ = std::fs::remove_file(state.with_extension("state.hp"));
                    // Durum kullanıldı: aynı andan ikinci kez devam edilmemeli.
                    let _ = std::fs::remove_file(&state);
                }
                Ok(())
            }
            Action::Stop => {
                if !alive() {
                    cleanup(d, g.vmid);
                    return Ok(());
                }
                // Önce konuktan kapanmasını iste (ACPI); süre dolarsa zorla kapat.
                let asked = qmp(&sock, &["system_powerdown"]).is_ok();
                if !(asked && wait_until(d.cfg.shutdown_timeout, || !alive())) {
                    let _ = qmp(&sock, &["quit"]);
                    if !wait_until(Duration::from_secs(5), || !alive()) {
                        if let Some(pid) = live_pid(d, g.vmid) {
                            let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
                        }
                        if !wait_until(Duration::from_secs(3), || !alive()) {
                            return Err("QEMU süreci sonlandırılamadı".to_string());
                        }
                    }
                }
                cleanup(d, g.vmid);
                Ok(())
            }
            Action::Reboot | Action::Pause | Action::Resume => {
                if !alive() {
                    return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
                }
                let command = match action {
                    Action::Reboot => "system_reset",
                    Action::Pause => "stop",
                    _ => "cont",
                };
                qmp(&sock, &[command]).map(|_| ())
            }
        }
    }

    /// Kabuk için tek tırnakla güvenli alıntı (QEMU `exec:` adresini kabukta çalıştırır).
    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    /// Askıya alma: konuk durdurulur, bellek ve aygıt durumu dosyaya yazılır, QEMU kapatılır.
    pub fn suspend(d: &QemuDriver, g: &Guest) -> Result<(), String> {
        let sock = d.cfg.qmp_path(g.vmid);
        let alive = || live_pid(d, g.vmid).is_some();
        if !alive() {
            return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
        }
        let state = d.cfg.state_path(g.vmid);
        let temp = state.with_extension("state.tmp");
        let marker = state.with_extension("state.running");
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_file(&marker);
        // Konuk elle duraklatılmış olabilir: geri dönünce de öyle kalmalı.
        let was_running = qmp(&sock, &["query-status"]).ok().and_then(|r| r.first().map(|s| s["status"] == "running")).unwrap_or(true);
        // Konuk hangi yerleşimle açıldıysa onunla dönmelidir.
        let layout = running_layout(d, g.vmid);
        let _ = std::fs::remove_file(state.with_extension("state.hp"));
        let migrate = serde_json::json!({ "execute": "migrate", "arguments": { "uri": format!("exec:cat > {}", shell_quote(&temp)) } });
        let saved = qmp_values(&sock, &[serde_json::json!({ "execute": "stop" }), migrate]).and_then(|_| {
            let mut outcome = Err("bellek durumu yazılamadı: zaman aşımı".to_string());
            wait_until(Duration::from_secs(1800), || {
                let status = qmp(&sock, &["query-migrate"]).ok().and_then(|r| r.first().and_then(|m| m["status"].as_str().map(str::to_string)));
                match status.as_deref() {
                    Some("completed") => outcome = Ok(()),
                    Some("failed") | Some("cancelled") => {
                        outcome = Err(if g.hw.shares.is_empty() {
                            "bellek durumu yazılamadı (disk dolu olabilir)".to_string()
                        } else {
                            "bellek durumu yazılamadı: paylaşılan klasör konukta bağlıyken konuk askıya alınamaz; önce konukta ayırın (umount) ya da disk dolu olabilir".to_string()
                        })
                    }
                    None if !alive() => outcome = Err("QEMU askıya alma sırasında kapandı".to_string()),
                    _ => return false,
                }
                true
            });
            outcome
        });
        if let Err(e) = saved {
            // Başarısızsa konuk çalışmaya devam eder; yarım dosya bırakılmaz.
            let _ = std::fs::remove_file(&temp);
            let _ = qmp(&sock, &["cont"]);
            return Err(e);
        }
        let _ = qmp(&sock, &["quit"]);
        if !wait_until(Duration::from_secs(10), || !alive()) {
            if let Some(pid) = live_pid(d, g.vmid) {
                let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
            }
            wait_until(Duration::from_secs(3), || !alive());
        }
        cleanup(d, g.vmid);
        if was_running {
            std::fs::write(&marker, b"").map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))?;
        }
        if layout > super::LAYOUT_LEGACY {
            std::fs::write(state.with_extension("state.hp"), layout.to_string()).map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))?;
        }
        // Durum dosyası ancak QEMU kapandıktan sonra geçerli adını alır: yarım ya da hâlâ yazılan dosyadan dönülmez.
        std::fs::rename(&temp, &state).map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))
    }

    /// QEMU izleyici (HMP) komutu çalıştırır. Başarıda çıktı boştur; hata metin olarak döner.
    fn hmp(d: &QemuDriver, g: &Guest, line: &str) -> Result<(), String> {
        let command = serde_json::json!({ "execute": "human-monitor-command", "arguments": { "command-line": line } });
        // Bellek durumu diske yazılırken yanıt gecikebilir.
        let out = qmp_run(&d.cfg.qmp_path(g.vmid), &[command], Duration::from_secs(900))?;
        let text = out.first().and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if text.is_empty() {
            Ok(())
        } else {
            Err(text.lines().last().unwrap_or_default().trim_start_matches("Error: ").to_string())
        }
    }

    /// Çalışan konuğun disklerindeki değişen blok haritalarını siler (geri dönüşten önce; hata yok sayılır).
    pub fn drop_bitmaps(d: &QemuDriver, g: &Guest) {
        let sock = d.cfg.qmp_path(g.vmid);
        let Ok(blocks) = qmp(&sock, &["query-block"]) else { return };
        for block in blocks.first().and_then(Value::as_array).into_iter().flatten() {
            let has = [&block["dirty-bitmaps"], &block["inserted"]["dirty-bitmaps"]].into_iter().filter_map(Value::as_array).flatten().any(|m| m["name"] == super::CBT_BITMAP);
            if let (true, Some(device)) = (has, block["device"].as_str()) {
                let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "block-dirty-bitmap-remove", "arguments": { "node": device, "name": super::CBT_BITMAP } })]);
            }
        }
    }

    /// Çalışan konukta anlık görüntü: disk ve bellek durumu birlikte kaydedilir (savevm).
    pub fn live_snapshot(d: &QemuDriver, g: &Guest, op: SnapshotOp<'_>) -> Result<(), String> {
        match op {
            SnapshotOp::Create(name) => hmp(d, g, &format!("savevm {name}")),
            SnapshotOp::Rollback(name) => hmp(d, g, &format!("loadvm {name}")).map_err(|e| {
                format!("{e}; konuk kapalıyken alınmış bir görüntüye dönmek için önce konuğu kapatın")
            }),
            SnapshotOp::Delete(name) => hmp(d, g, &format!("delvm {name}")),
        }
    }

    /// Çalışan konuğun disklerini, konuğu durdurmadan hedef dosyalara kopyalar. Bütün kopyalar tek bir
    /// QMP işlemiyle (transaction) başlar: hepsi aynı andaki tutarlı disk görüntüsüdür.
    /// `parts`: (konuğun disk dosyası, hedef dosya). `raw`: sıkıştırılmış qcow2 yerine ham imaj.
    ///
    /// Konukta ajan çalışıyorsa kopyalar başlamadan önce dosya sistemleri dondurulur ve kopyalar başlar
    /// başlamaz çözülür: yedek, uygulamaların yarım yazdığı bir anı değil temiz bir anı içerir. Dönen değer
    /// dondurmanın yapılıp yapılmadığıdır. Ajan yoksa yedek yine alınır (elektrik kesilmiş gibi tutarlıdır).
    pub fn live_backup(d: &QemuDriver, g: &Guest, parts: &[(std::path::PathBuf, std::path::PathBuf)], raw: bool, mode: super::Cbt) -> Result<bool, String> {
        let frozen = d.agent_call(g.vmid, "guest-fsfreeze-freeze", Duration::from_secs(60)).is_some();
        let mut thawed = !frozen;
        // Çözme başarısız olursa konuk donmuş kalır: birkaç kez denenir ve sonuç mutlaka bildirilir.
        let thaw = |d: &QemuDriver| -> Result<(), String> {
            let sock = d.cfg.qga_path(g.vmid);
            let mut last = String::new();
            for _ in 0..5 {
                match crate::qga::call(&sock, "guest-fsfreeze-thaw", Duration::from_secs(30)) {
                    Ok(_) => return Ok(()),
                    Err(e) => last = e,
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(format!("konuğun dosya sistemi yedekten sonra çözülemedi; konukta 'fsfreeze -u' çalıştırın ya da konuğu yeniden başlatın ({last})"))
        };
        let outcome = live_copy(d, g, parts, raw, mode, &mut || {
            if !thawed {
                thawed = true;
                return thaw(d);
            }
            Ok(())
        });
        // Kopyalar başlayamadıysa da (hata) konuk donmuş bırakılmaz.
        let unfrozen = if thawed { Ok(()) } else { thaw(d) };
        outcome.and(unfrozen).map(|()| frozen)
    }

    /// `started`: kopyalar başladığı (anlık görüntü alındığı) an bir kez çağrılır.
    fn live_copy(
        d: &QemuDriver,
        g: &Guest,
        parts: &[(std::path::PathBuf, std::path::PathBuf)],
        raw: bool,
        mode: super::Cbt,
        started: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        use super::{Cbt, CBT_BITMAP};
        let sock = d.cfg.qmp_path(g.vmid);
        // Sürücü adı QEMU sürümüne göre değişebilir: diskler dosya yollarından bulunur.
        let blocks = qmp(&sock, &["query-block"])?;
        let devices = blocks.first().and_then(Value::as_array).ok_or("konuğun diski QEMU'da bulunamadı")?;
        let mut nonce = [0u8; 4];
        getrandom::getrandom(&mut nonce).map_err(|e| format!("rastgele sayı üretilemedi: {e}"))?;
        let attempt = u32::from_le_bytes(nonce);
        // Harita işlemleri kopyalarla aynı QMP işlemindedir: harita tam olarak kopyanın alındığı anda sıfırlanır.
        let (mut bitmaps, mut actions, mut pending) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (source, target)) in parts.iter().enumerate() {
            let file = source.display().to_string();
            let block = devices.iter().find(|b| b["inserted"]["file"] == file.as_str());
            let device = block
                .and_then(|b| b["device"].as_str())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| if i == 0 { "konuğun diski QEMU'da bulunamadı".to_string() } else { format!("konuğun ek diski QEMU'da bulunamadı: {file}") })?;
            // Haritalar QEMU sürümüne göre aygıtın ya da takılı imajın altında listelenir.
            let bitmap = block.and_then(|b| {
                [&b["dirty-bitmaps"], &b["inserted"]["dirty-bitmaps"]].into_iter().filter_map(Value::as_array).flatten().find(|m| m["name"] == CBT_BITMAP)
            });
            let on_bitmap = |kind: &str, extra: Value| {
                let mut data = serde_json::json!({ "node": device, "name": CBT_BITMAP });
                if let (Some(data), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
                    data.extend(extra.clone());
                }
                serde_json::json!({ "type": kind, "data": data })
            };
            match (mode, bitmap) {
                (Cbt::Off, _) => {}
                // Düzgün kapanmamış konuğun haritası tutarsızdır: silinip yeniden oluşturulur.
                (Cbt::Arm, Some(b)) if b["inconsistent"] == true => {
                    bitmaps.push(on_bitmap("block-dirty-bitmap-remove", Value::Null));
                    bitmaps.push(on_bitmap("block-dirty-bitmap-add", serde_json::json!({ "persistent": true })));
                }
                (Cbt::Arm, Some(_)) => bitmaps.push(on_bitmap("block-dirty-bitmap-clear", Value::Null)),
                (Cbt::Arm, None) => bitmaps.push(on_bitmap("block-dirty-bitmap-add", serde_json::json!({ "persistent": true }))),
                (Cbt::Delta, Some(b)) if b["inconsistent"] != true && b["recording"] != false && b["persistent"] != false && b["busy"] != true => {}
                (Cbt::Delta, _) => return Err("değişen blok haritası yok ya da tutarsız (konuk düzgün kapanmamış olabilir)".to_string()),
            }
            let delta = mode == Cbt::Delta;
            // İş adı her denemede farklıdır: önceki bir denemeden kalmış işin olayları bizimkilerle karışamaz.
            let job = format!("redox-backup-{}-{i}-{attempt}", g.vmid);
            let mut data = serde_json::json!({
                "job-id": job, "device": device, "target": target.display().to_string(),
                "format": if raw && !delta { "raw" } else { "qcow2" }, "sync": if delta { "incremental" } else { "full" },
            });
            if delta {
                // İş başarıyla biterse QEMU kopyalanan bitleri haritadan siler; başarısız olursa harita olduğu gibi kalır.
                data["bitmap"] = CBT_BITMAP.into();
            } else if !raw {
                data["compress"] = true.into();
            }
            actions.push(serde_json::json!({ "type": "drive-backup", "data": data }));
            pending.push(job);
        }
        let actions: Vec<Value> = bitmaps.into_iter().chain(actions).collect();
        let start = serde_json::json!({ "execute": "transaction", "arguments": { "actions": actions } });
        let cancel = |job: &str| serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": job } });
        // Önceki denemeden kalmış (ör. sunucu yedek sırasında yeniden başlamış) işler diskleri meşgul tutar: durdurulur.
        let prefix = format!("redox-backup-{}-", g.vmid);
        if let Ok(reply) = qmp(&sock, &["query-block-jobs"]) {
            let stale: Vec<String> = reply
                .first()
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|job| job["device"].as_str())
                .filter(|name| name.starts_with(&prefix))
                .map(str::to_string)
                .collect();
            for job in &stale {
                let _ = qmp_values(&sock, &[cancel(job)]);
            }
            if !stale.is_empty() {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        let all_jobs = pending.clone();

        let err = |e: std::io::Error| format!("QMP ({}): {e}", sock.display());
        let stream = UnixStream::connect(&sock).map_err(err)?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(err)?;
        let mut writer = stream.try_clone().map_err(err)?;
        let mut reader = BufReader::new(stream);
        let deadline = Instant::now() + Duration::from_secs(6 * 3600);
        // Okuma zaman aşımı yalnızca "henüz satır yok" demektir; iş bitene kadar beklenir.
        let next = |reader: &mut BufReader<UnixStream>| -> Result<Value, String> {
            let mut line = String::new();
            loop {
                match reader.read_line(&mut line) {
                    Ok(0) => return Err("QMP bağlantısı kapandı (konuk kapanmış olabilir)".to_string()),
                    Ok(_) => return serde_json::from_str(line.trim()).map_err(|e| format!("QMP yanıtı okunamadı: {e}")),
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                        if Instant::now() >= deadline {
                            return Err("yedekleme zaman aşımına uğradı".to_string());
                        }
                    }
                    Err(e) => return Err(err(e)),
                }
            }
        };
        let mut send = |command: &Value| writer.write_all(format!("{command}\n").as_bytes()).map_err(err);
        let outcome = (|| -> Result<(), String> {
            next(&mut reader)?; // karşılama
            send(&serde_json::json!({ "execute": "qmp_capabilities" }))?;
            // 0: yetenek yanıtı bekleniyor; 1: işlemin başlama yanıtı bekleniyor; 2: işler sürüyor.
            let mut stage = 0;
            let mut failure: Option<String> = None;
            loop {
                let message = next(&mut reader)?;
                if stage < 2 {
                    if let Some(error) = message.get("error") {
                        return Err(format!("yedekleme başlatılamadı: {}", error["desc"].as_str().unwrap_or("bilinmeyen hata")));
                    }
                    if message.get("return").is_some() {
                        if stage == 0 {
                            send(&start)?;
                        } else {
                            // Kopyalar başladı: bu andaki disk görüntüsü alındı, konuk yazmaya devam edebilir.
                            started()?;
                        }
                        stage += 1;
                        // Kopyalanacak bir şey yoksa işler, başlama yanıtı gelmeden bitmiş olabilir.
                        if stage == 2 && pending.is_empty() {
                            return failure.map_or(Ok(()), Err);
                        }
                        continue;
                    }
                    if stage == 0 {
                        continue;
                    }
                    // Başlama isteği gönderildi: QEMU, hızlı biten işin "bitti" olayını yanıttan önce yollayabilir.
                }
                // Bu aşamadaki yanıtlar (durdurma isteklerine) önemsizdir; yalnızca kendi işlerimizin olayları izlenir.
                let (event, data) = (message["event"].as_str().unwrap_or_default(), &message["data"]);
                let Some(job) = data["device"].as_str().filter(|job| pending.iter().any(|p| p == job)).map(str::to_string) else {
                    continue;
                };
                let failed = match event {
                    "BLOCK_JOB_COMPLETED" => {
                        pending.retain(|p| *p != job);
                        data["error"].as_str().map(|e| format!("yedekleme başarısız: {e}"))
                    }
                    "BLOCK_JOB_CANCELLED" => {
                        pending.retain(|p| *p != job);
                        Some("yedekleme yarıda kesildi".to_string())
                    }
                    "BLOCK_JOB_ERROR" => Some("yedekleme yarıda kesildi".to_string()),
                    _ => None,
                };
                if let (Some(reason), None) = (failed, &failure) {
                    failure = Some(reason);
                    // Bir disk kopyalanamadıysa yedek geçersizdir: süren diğer kopyalar da durdurulur.
                    for other in &pending {
                        send(&cancel(other))?;
                    }
                }
                // Başlama yanıtı (ve konuğun çözülmesi) beklenmeden çıkılmaz.
                if pending.is_empty() && stage == 2 {
                    return failure.map_or(Ok(()), Err);
                }
            }
        })();
        // QMP aynı anda tek istemciye hizmet verir: yeni bağlantı açmadan önce bu bağlantı kapatılır.
        drop(reader);
        drop(writer);
        if outcome.is_err() {
            // Zaman aşımı, kopan bağlantı ya da okunamayan yanıtla çıkılıyorsa işler QEMU'da sürüyor olabilir:
            // hedef dosyalar silinmeden önce durdurulurlar (bitmiş iş için istek zararsızca reddedilir).
            for job in &all_jobs {
                let _ = qmp_values(&sock, &[cancel(job)]);
            }
        }
        outcome
    }

    pub fn screenshot(d: &QemuDriver, g: &Guest) -> Result<Vec<u8>, String> {
        if live_pid(d, g.vmid).is_none() {
            return Err("konuk çalışmıyor; ekranı görmek için başlatın".to_string());
        }
        let sock = d.cfg.qmp_path(g.vmid);
        let file = d.cfg.run_dir().join(format!("{}.screen", g.vmid));
        let dump = |format: Option<&str>| {
            let mut arguments = serde_json::json!({ "filename": file.display().to_string() });
            if let Some(f) = format {
                arguments["format"] = f.into();
            }
            qmp_values(&sock, &[serde_json::json!({ "execute": "screendump", "arguments": arguments })])
        };
        // Yeni QEMU doğrudan PNG yazar; eskisi yalnızca PPM bilir.
        let png = dump(Some("png")).is_ok();
        if !png {
            dump(None)?;
        }
        let bytes = std::fs::read(&file).map_err(|e| format!("ekran dökümü okunamadı: {e}"));
        let _ = std::fs::remove_file(&file);
        let bytes = bytes?;
        if png && bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            Ok(bytes)
        } else {
            crate::console::ppm_to_png(&bytes)
        }
    }

    pub fn send_keys(d: &QemuDriver, g: &Guest, chords: &[Vec<String>]) -> Result<(), String> {
        if live_pid(d, g.vmid).is_none() {
            return Err("konuk çalışmıyor".to_string());
        }
        if chords.len() > crate::console::MAX_CHORDS {
            return Err(format!("tek seferde en fazla {} tuş gönderilebilir", crate::console::MAX_CHORDS));
        }
        let commands: Vec<Value> = chords
            .iter()
            .map(|keys| {
                let keys: Vec<Value> = keys.iter().map(|k| serde_json::json!({ "type": "qcode", "data": k })).collect();
                serde_json::json!({ "execute": "send-key", "arguments": { "keys": keys } })
            })
            .collect();
        qmp_values(&d.cfg.qmp_path(g.vmid), &commands).map(|_| ())
    }

    pub fn probe(d: &QemuDriver, g: &Guest) -> Probe {
        let Some(pid) = live_pid(d, g.vmid) else {
            lock(&d.samples).remove(&g.vmid);
            return Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None };
        };
        // QMP yanıt vermezse (meşgul) süreç yaşadığı için çalışıyor sayılır.
        let paused = qmp(&d.cfg.qmp_path(g.vmid), &["query-status"])
            .ok()
            .and_then(|r| r.first().map(|v| v["status"] == "paused" || v["running"] == false))
            .unwrap_or(false);

        let now = Instant::now();
        let ticks = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| parse_stat_ticks(&s));
        let cpu = ticks.and_then(|t| {
            let previous = lock(&d.samples).insert(g.vmid, (t, now));
            previous.map(|(old, at)| cpu_pct(t.saturating_sub(old), now.duration_since(at), g.cores))
        });
        let mem = std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|s| parse_vmrss_kb(&s))
            .map(|kb| kb / 1024);
        Probe {
            status: if paused { GuestStatus::Paused } else { GuestStatus::Running },
            cpu_pct: cpu,
            mem_used_mb: mem,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::{Cluster, GuestSpec};

    fn guest(iso: Option<&str>) -> Guest {
        let mut c = Cluster::demo(0);
        let spec = GuestSpec {
            vmid: Some(150),
            name: "deneme-vm".into(),
            kind: GuestKind::Vm,
            node: "pve-ist-01".into(),
            cores: 2,
            memory_mb: 2048,
            disk_gb: 10,
            storage: "local-zfs".into(),
            os: "debian-12".into(),
            bridge: "nat".into(),
            onboot: false,
            iso: iso.map(str::to_string),
            hw: Default::default(),
        };
        c.create_guest(&spec, "u", 0).unwrap();
        c.guest(150).unwrap().clone()
    }

    /// Yol ayırıcısı platforma göre değişir; karşılaştırma için '/' biçimine çevrilir.
    fn args(cfg: &QemuConfig, g: &Guest) -> Vec<String> {
        build_args(cfg, g).into_iter().map(|a| a.replace('\\', "/")).collect()
    }

    fn cfg() -> QemuConfig {
        QemuConfig::new("/var/lib/redox")
    }

    #[test]
    fn args_for_kvm_guest() {
        let a = args(&cfg(), &guest(None));
        let joined = a.join(" ");
        assert!(joined.starts_with("-name deneme-vm -machine q35,accel=kvm -cpu host -smp 2 -m 2048M"), "{joined}");
        assert!(a.contains(&"file=/var/lib/redox/images/vm-150-disk-0.qcow2,if=virtio,format=qcow2".to_string()));
        assert!(a.contains(&"unix:/var/lib/redox/run/150.qmp,server=on,wait=off".to_string()));
        assert!(joined.ends_with("-pidfile /var/lib/redox/run/150.pid -daemonize"));
        assert!(joined.contains("-display none") && joined.contains("-netdev user,id=net0"));
        assert!(!joined.contains("cdrom") && !joined.contains("-boot"), "ISO yokken kalıp sürücüsü eklenmez");
    }

    #[test]
    fn extra_devices_sit_on_fixed_hotplug_ports() {
        let mut g = guest(None);
        let bare = args(&cfg(), &g).join(" ");
        assert_eq!(bare.matches("-device pcie-root-port,id=hp").count(), HOTPLUG_PORTS, "boş bağlantı noktaları her konukta hazırdır");
        assert!(bare.contains("-device pcie-root-port,id=hp0,chassis=1,slot=1") && bare.contains("-device pcie-root-port,id=hp10,chassis=11,slot=11"));
        g.hw.disks = vec![redox_core::hw::ExtraDisk { id: 1, size_gb: 5, serial: 1 }, redox_core::hw::ExtraDisk { id: 8, size_gb: 5, serial: 2 }];
        g.hw.nics = vec!["vmbr1".into(), "nat".into(), "vmbr2".into()];
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-drive file=/var/lib/redox/images/vm-150-disk-1.qcow2,if=none,id=xd1,format=qcow2 -device virtio-blk-pci,drive=xd1,id=xdev1,bus=hp0"), "{a}");
        assert!(a.contains("-device virtio-blk-pci,drive=xd8,id=xdev8,bus=hp7"), "{a}");
        assert!(a.contains("netdev=net1,mac=52:54:01:00:00:96,id=xnic1,bus=hp8") && a.contains("netdev=net3,mac=52:54:03:00:00:96,id=xnic3,bus=hp10"), "{a}");
        assert!(a.find("id=hp10").unwrap() < a.find("id=xd1").unwrap(), "bağlantı noktaları aygıtlardan önce tanımlanır");
        assert!(a.contains("vm-150-disk-0.qcow2,if=virtio,format=qcow2"), "asıl diskin yeri değişmez");
        assert_eq!((disk_port(1), disk_port(8), nic_port(1), nic_port(3)), ("hp0".to_string(), "hp7".to_string(), "hp8".to_string(), "hp10".to_string()));

        // Eski yerleşim (bu düzenden önce askıya alınmış konukları döndürmek için): bağlantı noktası yok.
        let old = build_args_layout(&cfg(), &g, LAYOUT_LEGACY).join(" ").replace('\\', "/");
        assert!(!old.contains("pcie-root-port") && !old.contains("bus=hp") && !old.contains("if=none"), "{old}");
        assert!(old.contains("-drive file=/var/lib/redox/images/vm-150-disk-1.qcow2,if=virtio,format=qcow2"), "{old}");
        assert!(old.contains("-netdev bridge,id=net1,br=vmbr1 -device virtio-net-pci,netdev=net1,mac=52:54:01:00:00:96 "), "{old}");
    }

    #[test]
    fn shared_folders_become_9p_devices() {
        let mut g = guest(None);
        assert!(!args(&cfg(), &g).join(" ").contains("fsdev"), "paylaşım yokken aygıt eklenmez");
        g.hw.shares = redox_core::hw::parse_shares("ortak, veri:ro").unwrap();
        g.hw.shares.push(redox_core::hw::Share { name: "../kacis".into(), readonly: false });
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-fsdev local,id=fs0,path=/var/lib/redox/shares/ortak,security_model=none,multidevs=remap -device virtio-9p-pci,fsdev=fs0,mount_tag=ortak"), "{a}");
        assert!(a.contains("-fsdev local,id=fs1,path=/var/lib/redox/shares/veri,security_model=none,multidevs=remap,readonly=on -device virtio-9p-pci,fsdev=fs1,mount_tag=veri"), "{a}");
        assert!(!a.contains("kacis") && !a.contains("fs2"), "geçersiz ad argümana girmez");
    }

    #[test]
    fn live_console_gets_a_vnc_socket_and_a_tablet() {
        let mut g = guest(None);
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-vnc unix:/var/lib/redox/run/150.vnc"), "{a}");
        assert!(a.contains("-device qemu-xhci,id=xhci -device usb-tablet,bus=xhci.0"), "{a}");
        assert!(!a.contains("-vnc :") && !a.contains("-vnc 0.0.0.0") && !a.contains("websocket"), "VNC ağa açılmaz");
        // USB aygıtı aktarılan konukta denetleyici bir kez eklenir.
        g.hw.usb = vec!["046d:c52b".into()];
        assert_eq!(args(&cfg(), &g).join(" ").matches("qemu-xhci").count(), 1);
        // Önceki yerleşimlerde işaretçi aygıtı yoktur (askıdan dönüşte donanım değişmesin); VNC soketi her zaman vardır.
        for layout in [LAYOUT_LEGACY, LAYOUT_PORTS] {
            let old = build_args_layout(&cfg(), &guest(None), layout).join(" ").replace('\\', "/");
            assert!(!old.contains("usb-tablet") && !old.contains("xhci") && old.contains("-vnc unix:/var/lib/redox/run/150.vnc"), "{layout}: {old}");
        }
        assert!(build_args_layout(&cfg(), &guest(None), LAYOUT_PORTS).join(" ").contains("pcie-root-port,id=hp0"));
    }

    #[test]
    fn vlan_cards_use_a_named_tap_instead_of_the_bridge_helper() {
        let mut g = guest(None);
        g.bridge = "vmbr0".into();
        g.hw.nics = vec!["vmbr1:30".into(), "vmbr1".into(), "nat".into()];
        let plain = args(&cfg(), &g).join(" ");
        assert!(plain.contains("-netdev bridge,id=net0,br=vmbr0 ") && plain.contains("-netdev tap,id=net1,ifname=rx150n1,script=no,downscript=no -device virtio-net-pci,netdev=net1,mac=52:54:01:00:00:96,id=xnic1,bus=hp8"), "{plain}");
        assert!(plain.contains("-netdev bridge,id=net2,br=vmbr1 ") && plain.contains("-netdev user,id=net3 "), "{plain}");
        g.hw.vlan = 20;
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-netdev tap,id=net0,ifname=rx150n0,script=no,downscript=no -device virtio-net-pci,netdev=net0,mac=52:54:00:00:00:96"), "{a}");
        assert!(!a.contains("br=vmbr0"), "etiketli kart köprü yardımcısını kullanmaz");
        assert_eq!(vlan_nics(&g), [(0, "vmbr0".to_string(), 20), (1, "vmbr1".to_string(), 30)]);
        // NAT'taki birinci kartta etiket yok sayılır (çekirdek zaten reddeder).
        g.bridge = "nat".into();
        assert_eq!(vlan_nics(&g), [(1, "vmbr1".to_string(), 30)]);
        assert!(args(&cfg(), &g).join(" ").contains("-netdev user,id=net0"));
        assert_eq!(tap_name(4_294_967_295, 3).len(), 14, "arayüz adı 15 karakter sınırını aşmaz");
    }

    #[test]
    fn disk_limits_are_applied_to_every_disk() {
        let mut g = guest(None);
        g.hw.disks = vec![redox_core::hw::ExtraDisk { id: 1, size_gb: 5, serial: 1 }];
        let plain = args(&cfg(), &g).join(" ");
        assert!(!plain.contains("throttling"), "sınır yokken seçenek eklenmez");
        g.hw.disk_mbps = 50;
        let a = args(&cfg(), &g).join(" ");
        assert_eq!(a.matches(",throttling.bps-total=52428800").count(), 2, "{a}");
        assert!(!a.contains("iops-total"));
        g.hw.disk_iops = 2000;
        let a = args(&cfg(), &g);
        assert!(a.contains(&"file=/var/lib/redox/images/vm-150-disk-0.qcow2,if=virtio,format=qcow2,throttling.bps-total=52428800,throttling.iops-total=2000".to_string()), "{a:?}");
        assert!(a.contains(&"file=/var/lib/redox/images/vm-150-disk-1.qcow2,if=none,id=xd1,format=qcow2,throttling.bps-total=52428800,throttling.iops-total=2000".to_string()), "{a:?}");
        assert_eq!(a.join(" ").matches("throttling.iops-total=2000").count(), 2);
        g.hw.net_mbps = 100;
        assert_eq!(args(&cfg(), &g), a, "ağ sınırı QEMU satırını değiştirmez; TAP arayüzünde uygulanır");
    }

    #[test]
    fn args_fall_back_to_tcg_and_attach_iso() {
        let a = args(&QemuConfig { kvm: false, ..cfg() }, &guest(Some("debian-12.iso"))).join(" ");
        assert!(a.contains("-machine q35,accel=tcg -cpu max"), "{a}");
        assert!(a.contains("-drive file=/var/lib/redox/iso/debian-12.iso,media=cdrom,readonly=on -boot order=c,once=d"), "{a}");
    }

    #[test]
    fn hostile_values_cannot_inject_options() {
        // Çekirdek bu adı reddeder; sürücü yine de bozuk kayda karşı savunur.
        let mut g = guest(None);
        g.iso = Some("../../etc/shadow".into());
        assert!(!args(&cfg(), &g).join(" ").contains("shadow"));
        g.iso = Some("x.iso,readonly=off".into());
        assert!(!args(&cfg(), &g).join(" ").contains("readonly=off"));

        // Veri dizinindeki virgül QEMU seçenek ayırıcısı olarak yorumlanmamalı.
        let a = args(&QemuConfig::new("/srv/a,b"), &guest(None));
        assert!(a.contains(&"file=/srv/a,,b/images/vm-150-disk-0.qcow2,if=virtio,format=qcow2".to_string()), "{a:?}");
        // Her değer ayrı bir argümandır; ad içinde boşluk olsa bile bölünmez.
        g.name = "a -snapshot".into();
        assert_eq!(args(&cfg(), &g)[1], "a -snapshot");
    }

    #[test]
    fn args_include_port_forwards_and_a_display_adapter() {
        let mut g = guest(None);
        g.forwards = vec![redox_core::PortForward { host_port: 8080, guest_port: 80 }, redox_core::PortForward { host_port: 2222, guest_port: 22 }];
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-netdev user,id=net0,hostfwd=tcp::8080-:80,hostfwd=tcp::2222-:22 -device virtio-net-pci,netdev=net0"), "{a}");
        assert!(a.contains("-vga std -display none"), "konsol için ekran kartı gerekir: {a}");
    }

    #[test]
    fn bridged_guests_use_the_bridge_helper_and_a_stable_mac() {
        let mut g = guest(None);
        let nat = args(&cfg(), &g).join(" ");
        assert!(nat.contains("-netdev user,id=net0 -device virtio-net-pci,netdev=net0,mac=52:54:00:00:00:96"), "{nat}");
        g.bridge = "vmbr0".into();
        g.forwards = vec![redox_core::PortForward { host_port: 8080, guest_port: 80 }];
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-netdev bridge,id=net0,br=vmbr0 -device virtio-net-pci,netdev=net0,mac=52:54:00:00:00:96"), "{a}");
        assert!(!a.contains("hostfwd") && !a.contains("user,id=net0"), "köprüde port yönlendirme uygulanmaz");
        // Geçersiz köprü adı seçenek enjekte edemez: kullanıcı kipine düşer.
        g.bridge = "br0,script=/tmp/x".into();
        let a = args(&cfg(), &g).join(" ");
        assert!(!a.contains("script") && a.contains("-netdev user,id=net0"), "{a}");
        assert!(!is_bridged(&g));
    }

    #[test]
    fn cloud_init_guests_get_a_cidata_drive_and_seed_files() {
        const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxvcmVtaXBzdW1kb2xvcnNpdGFtZXRjb25zZWN0 ben@dizustu";
        let mut g = guest(None);
        assert!(!args(&cfg(), &g).join(" ").contains("vvfat"), "cloud-init yokken sürücü takılmaz");
        g.hw.ci_ssh_key = Some(KEY.into());
        g.hw.ci_user = Some("ali".into());
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-drive file.driver=vvfat,file.dir=/var/lib/redox/cidata/150,file.label=cidata,file.rw=off,format=raw,if=virtio,readonly=on"), "{a}");
        assert!(a.find("vm-150-disk-0.qcow2").unwrap() < a.find("vvfat").unwrap(), "asıl disk ilk sürücü kalır");

        let dir = std::env::temp_dir().join(format!("redox-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let c = QemuConfig::new(&dir);
        write_seed(&c, &g).unwrap();
        let user_data = std::fs::read_to_string(c.seed_dir(150).join("user-data")).unwrap();
        assert!(user_data.starts_with("#cloud-config\nhostname: deneme-vm\n") && user_data.contains("- name: ali") && user_data.contains(KEY));
        assert_eq!(std::fs::read_to_string(c.seed_dir(150).join("meta-data")).unwrap(), "instance-id: redox-150\nlocal-hostname: deneme-vm\n");
        // Ad değişince bir sonraki başlatmada dosyalar yenilenir; cloud-init kapatılınca dizin silinir.
        g.name = "yeni-ad".into();
        write_seed(&c, &g).unwrap();
        assert!(std::fs::read_to_string(c.seed_dir(150).join("user-data")).unwrap().contains("hostname: yeni-ad"));
        g.hw = Default::default();
        write_seed(&c, &g).unwrap();
        assert!(!c.seed_dir(150).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uefi_secure_boot_and_tpm_arguments() {
        use redox_core::hw::Firmware;
        let mut g = guest(None);
        let plain = args(&cfg(), &g).join(" ");
        assert!(!plain.contains("pflash") && !plain.contains("tpm") && !plain.contains("smm"), "BIOS konuğunda hiçbiri yok");

        g.hw.firmware = Firmware::Uefi;
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-machine q35,accel=kvm -cpu host"), "{a}");
        assert!(a.contains("-drive if=pflash,unit=0,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd"), "{a}");
        assert!(a.contains("-drive if=pflash,unit=1,format=qcow2,file=/var/lib/redox/images/vm-150-efivars.qcow2"), "{a}");
        assert!(!a.contains("cfi.pflash01"));

        g.hw.secure_boot = true;
        g.hw.tpm = true;
        let a = args(&cfg(), &g).join(" ");
        assert!(a.contains("-machine q35,accel=kvm,smm=on"), "{a}");
        assert!(a.contains("file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd") && a.contains("vm-150-efivars-secure.qcow2"), "{a}");
        assert!(a.contains("-global driver=cfi.pflash01,property=secure,value=on"));
        assert!(a.contains("-chardev socket,id=chrtpm,path=/var/lib/redox/run/150.tpm -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-crb,tpmdev=tpm0"), "{a}");
        // Değişken deposu ayrı pflash sürücüsüdür; asıl disk ilk virtio sürücüsü kalır.
        assert!(a.contains("vm-150-disk-0.qcow2,if=virtio,format=qcow2"));
    }

    #[test]
    fn extra_disks_and_nics_become_devices() {
        let mut g = guest(None);
        g.hw.disks = vec![redox_core::hw::ExtraDisk { id: 1, size_gb: 10, serial: 1 }, redox_core::hw::ExtraDisk { id: 4, size_gb: 20, serial: 2 }];
        g.hw.nics = vec!["vmbr1".into(), "nat".into(), "kötü,script=/x".into()];
        let a = args(&cfg(), &g).join(" ");
        let at = |needle: &str| a.find(needle).unwrap_or_else(|| panic!("{needle} yok: {a}"));
        assert!(at("vm-150-disk-0.qcow2") < at("vm-150-disk-1.qcow2,if=none,id=xd1,format=qcow2") && at("vm-150-disk-1.qcow2") < at("vm-150-disk-4.qcow2"), "asıl disk önce");
        assert!(a.contains("-netdev bridge,id=net1,br=vmbr1 -device virtio-net-pci,netdev=net1,mac=52:54:01:00:00:96,id=xnic1,bus=hp8"), "{a}");
        assert!(a.contains("-netdev user,id=net2 -device virtio-net-pci,netdev=net2,mac=52:54:02:00:00:96,id=xnic2,bus=hp9"), "{a}");
        assert!(!a.contains("script=") && !a.contains("net3"), "geçersiz ağ adı argümana girmez");
        assert!(a.contains("netdev=net0,mac=52:54:00:00:00:96"), "birinci kartın adresi değişmez");
    }

    #[test]
    fn passthrough_devices_are_attached() {
        let mut g = guest(None);
        assert!(!args(&cfg(), &g).join(" ").contains("usb-host"), "aktarılan aygıt yokken yalnızca işaretçi aygıtı vardır");
        g.hw.usb = vec!["046d:c52b".into(), "0951:1666".into(), "bozuk,x=1".into()];
        g.hw.pci = vec!["0000:01:00.0".into(), "0000:01:00.0,x-vga=on".into()];
        let a = args(&cfg(), &g).join(" ");
        assert_eq!(a.matches("-device qemu-xhci,id=xhci").count(), 1, "tek denetleyici");
        assert!(a.contains("-device usb-host,bus=xhci.0,vendorid=0x046d,productid=0xc52b") && a.contains("vendorid=0x0951,productid=0x1666"), "{a}");
        assert!(a.contains("-device vfio-pci,host=0000:01:00.0"), "{a}");
        assert!(!a.contains("x-vga") && !a.contains("x=1"), "geçersiz kimlik argümana girmez");
    }

    #[test]
    fn written_regions_are_read_from_the_image_map() {
        use serde_json::json;
        let map = json!([
            {"start": 0, "length": 65536, "depth": 0, "present": false, "zero": true, "data": false},
            {"start": 65536, "length": 131072, "depth": 0, "present": true, "zero": false, "data": true, "offset": 327680},
            {"start": 196608, "length": 65536, "depth": 0, "present": true, "zero": true, "data": false},
            {"start": 262144, "length": 0, "depth": 0, "present": true, "zero": false, "data": true}
        ]);
        // Sıfır yazılmış bölge de "yazılmış"tır (eski veri sıfırlanmış olabilir); hiç yazılmamış bölge değildir.
        assert_eq!(present_ranges(&map), Some(vec![(65536, 131072), (196608, 65536)]));
        assert_eq!(present_ranges(&json!([])), Some(vec![]));
        // Eski qemu-img `present` bildirmez: ayırt edilemediği için harita kullanılmaz.
        assert_eq!(present_ranges(&json!([{"start": 0, "length": 65536, "zero": true, "data": false}])), None);
        assert_eq!(present_ranges(&json!({"hata": 1})), None);
    }

    #[test]
    fn backup_ids_cannot_escape_the_backup_directory() {
        let dir = std::env::temp_dir().join(format!("rdx-b-{}", std::process::id()));
        let d = QemuDriver::new(QemuConfig::new(&dir)).unwrap();
        assert!(d.backup_path("vm-103-1800000000").unwrap().ends_with("backups/vm-103-1800000000.qcow2"));
        for bad in ["", "../x", "a/b", "a.b", "A", "a b", &"a".repeat(65)] {
            assert!(d.backup_path(bad).is_err(), "{bad}");
            assert!(d.delete_backup(bad).is_err() && d.restore(&guest(None), bad, &[]).is_err());
        }
        assert!(d.restore(&guest(None), "yok", &[]).unwrap_err().contains("bulunamadı"));
        d.delete_backup("yok").unwrap();
        assert!(d.snapshot(&guest(None), SnapshotOp::Create("../kacis")).unwrap_err().contains("geçersiz"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn qmp_lines() {
        assert_eq!(qmp_command("query-status"), "{\"execute\":\"query-status\"}\n");
        assert_eq!(parse_qmp_line(r#"{"QMP": {"version": {}, "capabilities": []}}"#), QmpLine::Greeting);
        assert_eq!(parse_qmp_line(r#"{"return": {}}"#), QmpLine::Return(serde_json::json!({})));
        assert_eq!(
            parse_qmp_line(r#"{"return": {"status": "paused", "running": false}}"#),
            QmpLine::Return(serde_json::json!({"status": "paused", "running": false}))
        );
        assert_eq!(
            parse_qmp_line(r#"{"error": {"class": "CommandNotFound", "desc": "The command foo has not been found"}}"#),
            QmpLine::Error("The command foo has not been found".into())
        );
        assert_eq!(parse_qmp_line(r#"{"timestamp": {"seconds": 1}, "event": "SHUTDOWN", "data": {}}"#), QmpLine::Event("SHUTDOWN".into()));
        assert_eq!(parse_qmp_line("bozuk"), QmpLine::Other);
        assert_eq!(parse_qmp_line(""), QmpLine::Other);
    }

    #[test]
    fn proc_parsers() {
        // Süreç adı boşluk ve parantez içerebilir.
        let stat = "4242 (qemu-system-x86 (x)) S 1 4242 4242 0 -1 4194560 100 0 0 0 1500 250 0 0 20 0 5 0 100 1000000 5000 18446744073709551615";
        assert_eq!(parse_stat_ticks(stat), Some(1750));
        assert_eq!(parse_stat_ticks("bozuk"), None);
        assert_eq!(parse_stat_ticks("1 (x) S 1 2"), None);

        let status = "Name:\tqemu\nVmPeak:\t  900000 kB\nVmRSS:\t  524288 kB\nThreads:\t5\n";
        assert_eq!(parse_vmrss_kb(status), Some(524_288));
        assert_eq!(parse_vmrss_kb("Name:\tx\n"), None);
    }

    #[test]
    fn cpu_percent_is_relative_to_guest_cores() {
        let five = Duration::from_secs(5);
        assert_eq!(cpu_pct(500, five, 1), 100, "5 sn'de 5 CPU-saniye, 1 çekirdek = %100");
        assert_eq!(cpu_pct(500, five, 2), 50);
        assert_eq!(cpu_pct(250, five, 2), 25);
        assert_eq!(cpu_pct(9999, five, 1), 100, "üst sınır");
        assert_eq!(cpu_pct(0, five, 4), 0);
        assert_eq!(cpu_pct(100, Duration::ZERO, 1), 0);
        assert_eq!(cpu_pct(100, five, 0), 0);
    }

    #[test]
    fn driver_setup_lists_isos_and_rejects_long_paths() {
        let dir = std::env::temp_dir().join(format!("rdx-q-{}", std::process::id()));
        let d = QemuDriver::new(QemuConfig::new(&dir)).unwrap();
        assert!(d.config().images_dir().is_dir() && d.config().run_dir().is_dir());
        assert!(d.is_real() && d.supports(GuestKind::Vm) && !d.supports(GuestKind::Ct));
        assert_eq!(d.name(), "qemu-kvm");

        for name in ["b.iso", "a.iso", "not.txt", ".gizli.iso", "kötü ad.iso"] {
            std::fs::write(d.config().iso_dir().join(name), b"").unwrap();
        }
        std::fs::create_dir(d.config().iso_dir().join("dizin.iso")).unwrap();
        assert_eq!(d.isos(), ["a.iso", "b.iso"], "yalnızca geçerli adlı düz dosyalar, sıralı");
        std::fs::remove_dir_all(&dir).unwrap();

        let long = std::env::temp_dir().join("x".repeat(120));
        let err = QemuDriver::new(QemuConfig::new(&long)).err().expect("uzun yol reddedilmeli");
        assert!(err.contains("çok uzun"), "{err}");
        let _ = std::fs::remove_dir_all(&long);
    }
}
