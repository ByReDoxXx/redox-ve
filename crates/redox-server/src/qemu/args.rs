//! QEMU komut satırı: aygıt yerleşimi, ağ arka uçları, disk sınırları, cloud-init ve UEFI hazırlığı.

use super::*;

/// QEMU seçenek değerlerinde virgül ayırıcıdır; yol içindeki virgül ikilenerek kaçırılır.
pub(super) fn opt(path: &Path) -> String {
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

/// KVM modülünde iç içe sanallaştırma açık mı? (`kvm_intel` ya da `kvm_amd` modülünün `nested` parametresi.)
pub fn nested_enabled(sys_module: &Path) -> bool {
    ["kvm_intel", "kvm_amd"].iter().any(|module| std::fs::read_to_string(sys_module.join(module).join("parameters/nested")).is_ok_and(|v| matches!(v.trim(), "Y" | "1")))
}

pub const NESTED_HINT: &str = "sunucuda iç içe sanallaştırma kapalı. Açmak için /etc/modprobe.d/kvm.conf dosyasına \"options kvm_intel nested=1\" (AMD'de kvm_amd) yazıp modülü yeniden yükleyin ya da sunucuyu yeniden başlatın";

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
pub(super) fn tap_backend(vmid: VmId, index: u8) -> String {
    format!("tap,id=net{index},ifname={},script=no,downscript=no", tap_name(vmid, index))
}

/// Disk hız sınırının `-drive` ekleri.
pub(super) fn disk_throttle(g: &Guest) -> String {
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
    // İşlemci modeli: seçilmemişse KVM'de sunucunun kendi işlemcisi, yazılım öykünmesinde en geniş model.
    let cpu = match g.hw.cpu_type.as_str() {
        "" if cfg.kvm => "host",
        "" => "max",
        model => model,
    };
    let cpu = redox_core::hw::cpu_arg(cpu, g.hw.nested);
    if cfg.kvm {
        push(&["-machine", &format!("q35,accel=kvm{smm}"), "-cpu", &cpu]);
    } else {
        push(&["-machine", &format!("q35,accel=tcg{smm}"), "-cpu", &cpu]);
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
    // Çalışırken çekirdek ekleme: konuk boş çekirdek yuvalarıyla açılır. Yuva sayısı konuğun kaydındadır;
    // böylece askıya alınan ya da başka sunucuya geçen konuk aynı düzenle döner.
    let slots = g.hw.max_cores >= g.cores && g.hw.max_cores > 0;
    let smp = if slots { format!("{},maxcpus={}", g.cores, g.hw.max_cores) } else { g.cores.to_string() };
    push(&["-smp", &smp, "-m", &format!("{}M", g.mem_total_mb)]);
    if slots {
        // QEMU, disk aygıtının kuyruk sayısını açılıştaki çekirdek sayısından türetir. Çekirdek eklenmiş konuk
        // askıdan yeni sayıyla döndüğünde aygıt değişmiş olur ve bellek durumu yüklenmez; bu yüzden sayı,
        // çalışırken değişmeyen üst sınıra bağlanır (çalışırken takılan diskler de aynı değeri alır).
        push(&["-global", &format!("virtio-blk-pci.num-queues={}", core_queues(g.hw.max_cores))]);
    }
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
        // Yalıtılmış konukta ağ dışarıya kapalıdır (yedek sınaması: özgün konukla aynı adresle ağa çıkmasın).
        let mut net = if g.hw.isolated { "user,id=net0,restrict=on".to_string() } else { "user,id=net0".to_string() };
        for f in &g.forwards {
            net.push_str(&format!(",hostfwd=tcp::{}-:{}", f.host_port, f.guest_port));
        }
        // Yedek sınaması: denetlenecek kapı yalnızca sunucunun kendisinden (127.0.0.1) erişilir. Yalıtım
        // (restrict=on) açıkça tanımlanan yönlendirmeleri etkilemez.
        if g.hw.isolated && g.hw.test_port > 0 && g.hw.test_fwd > 0 {
            net.push_str(&format!(",hostfwd=tcp:127.0.0.1:{}-:{}", g.hw.test_fwd, g.hw.test_port));
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
    // Aracılı aygıt (paylaşılan ekran kartının dilimi): aynı kart birkaç konuğa bölünür.
    for uuid in g.hw.pci.iter().filter(|p| redox_core::hw::valid_mdev_uuid(p)) {
        push(&["-device", &format!("vfio-pci,sysfsdev=/sys/bus/mdev/devices/{uuid}")]);
    }
    // Ek ağ kartları: her biri kendi ağına, kimlikten türetilen ayrı bir adresle.
    for (i, spec) in g.hw.nics.iter().enumerate() {
        let Some((network, vlan)) = redox_core::hw::parse_nic(spec) else { continue };
        let n = i as u8 + 1;
        let backend = if network == NAT {
            format!("user,id=net{n}{}", if g.hw.isolated { ",restrict=on" } else { "" })
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
    // Bellek balonu en sona eklenir: öbür aygıtların veri yolundaki yeri değişmez.
    if g.hw.balloon {
        push(&["-device", "virtio-balloon-pci,id=balloon0"]);
    }
    a
}

/// Çekirdek yuvalı konukta disk aygıtlarının kuyruk sayısı.
pub fn core_queues(max_cores: u32) -> u32 {
    max_cores.clamp(1, 16)
}

/// Makine durumu arşivindeki TPM dosyasının adı: yol ayırıcısız, gizli olmayan, sade bir dosya adı.
pub(super) fn machine_member_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.starts_with('.') && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
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
    // Sabit adres verilmişse ağ ayarı da yazılır; verilmemişse eski dosya kaldırılır (konuk DHCP kullanır).
    match g.hw.network_config(g.vmid) {
        Some(network) => write("network-config", &network)?,
        None => {
            let _ = std::fs::remove_file(dir.join("network-config"));
        }
    }
    write("meta-data", &redox_core::hw::meta_data(g.vmid, &g.name, g.hw.ci_gen))
}

/// Başlatmadan önce UEFI değişken deposunu (yoksa kalıptan) hazırlar.
#[cfg(unix)]
pub(super) fn prepare_efivars(d: &QemuDriver, g: &Guest) -> Result<(), String> {
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
