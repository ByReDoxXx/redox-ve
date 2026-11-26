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
fn boot_test_port_is_forwarded_only_on_loopback_of_isolated_guests() {
    let mut g = guest(None);
    (g.hw.test_port, g.hw.test_fwd) = (22, 40123);
    assert!(!args(&cfg(), &g).join(" ").contains("hostfwd"), "olağan konukta sınama kapısı açılmaz");
    g.hw.isolated = true;
    assert!(args(&cfg(), &g).join(" ").contains("user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:40123-:22 "));
    g.hw.test_fwd = 0;
    assert!(!args(&cfg(), &g).join(" ").contains("hostfwd"));
}

#[test]
fn isolated_guests_have_no_way_out_of_their_network() {
    let mut g = guest(None);
    g.hw.nics = vec!["nat".into()];
    let open = args(&cfg(), &g).join(" ");
    assert!(open.contains("-netdev user,id=net0 ") && open.contains("-netdev user,id=net1 ") && !open.contains("restrict"), "{open}");
    g.hw.isolated = true;
    let closed = args(&cfg(), &g).join(" ");
    assert!(closed.contains("-netdev user,id=net0,restrict=on ") && closed.contains("-netdev user,id=net1,restrict=on "), "{closed}");
}

#[test]
fn cpu_model_and_balloon_device_follow_the_guest() {
    let mut g = guest(None);
    let plain = args(&cfg(), &g).join(" ");
    assert!(plain.contains("-cpu host") && !plain.contains("balloon"), "{plain}");
    let mut soft = cfg();
    soft.kvm = false;
    assert!(args(&soft, &g).join(" ").contains("accel=tcg -cpu max"));
    g.hw.nested = true;
    assert!(args(&cfg(), &g).join(" ").contains("-cpu host "), "host uzantıyı zaten taşır");
    g.hw.cpu_type = "EPYC".into();
    assert!(args(&cfg(), &g).join(" ").contains("-cpu EPYC,+svm "));
    g.hw.cpu_type = "Haswell".into();
    assert!(args(&cfg(), &g).join(" ").contains("-cpu Haswell,+vmx "));
    g.hw.nested = false;
    let sys = std::env::temp_dir().join(format!("redox-nested-{}", std::process::id()));
    assert!(!nested_enabled(&sys));
    std::fs::create_dir_all(sys.join("kvm_amd/parameters")).unwrap();
    std::fs::write(sys.join("kvm_amd/parameters/nested"), "0\n").unwrap();
    assert!(!nested_enabled(&sys));
    std::fs::write(sys.join("kvm_amd/parameters/nested"), "1\n").unwrap();
    assert!(nested_enabled(&sys));
    std::fs::remove_dir_all(&sys).unwrap();
    g.hw.cpu_type = "Skylake-Server".into();
    g.hw.balloon = true;
    let a = args(&cfg(), &g);
    assert!(a.join(" ").contains("accel=kvm -cpu Skylake-Server"), "{a:?}");
    assert!(args(&soft, &g).join(" ").contains("accel=tcg -cpu Skylake-Server"), "seçilen model öykünmede de kullanılır");
    assert_eq!(&a[a.len() - 2..], ["-device", "virtio-balloon-pci,id=balloon0"], "balon en son aygıttır: öbürlerinin yeri değişmez");
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
    // Sabit adres: ağ ayarı dosyası yazılır ve kimlik değişir (cloud-init yeniden uygular); adres silinince dosya kalkar.
    assert!(!c.seed_dir(150).join("network-config").exists(), "adres yokken ağ dosyası yazılmaz");
    assert!(g.hw.apply_cloud_init(None, None, Some("10.0.0.5/24"), Some("10.0.0.1"), Some("1.1.1.1")));
    write_seed(&c, &g).unwrap();
    let network = std::fs::read_to_string(c.seed_dir(150).join("network-config")).unwrap();
    assert!(network.contains("macaddress: \"52:54:00:00:00:96\"") && network.contains("- 10.0.0.5/24") && network.contains("gateway4: 10.0.0.1"), "{network}");
    assert!(std::fs::read_to_string(c.seed_dir(150).join("meta-data")).unwrap().starts_with("instance-id: redox-150-1
"));
    g.hw.apply_cloud_init(None, None, Some(""), Some(""), Some(""));
    write_seed(&c, &g).unwrap();
    assert!(!c.seed_dir(150).join("network-config").exists() && c.seed_dir(150).join("user-data").exists());
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
    g.hw.pci = vec!["0000:01:00.0".into(), "0000:01:00.0,x-vga=on".into(), "83b8f4f2-509f-382f-3c1e-e6bfe0fa1001".into()];
    let a = args(&cfg(), &g).join(" ");
    assert_eq!(a.matches("-device qemu-xhci,id=xhci").count(), 1, "tek denetleyici");
    assert!(a.contains("-device usb-host,bus=xhci.0,vendorid=0x046d,productid=0xc52b") && a.contains("vendorid=0x0951,productid=0x1666"), "{a}");
    assert!(a.contains("-device vfio-pci,host=0000:01:00.0"), "{a}");
    assert!(a.contains("-device vfio-pci,sysfsdev=/sys/bus/mdev/devices/83b8f4f2-509f-382f-3c1e-e6bfe0fa1001"), "{a}");
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

#[test]
fn guest_traffic_is_read_from_the_tap_counters_with_directions_swapped() {
    let sys = std::env::temp_dir().join(format!("redox-tapstat-{}", std::process::id()));
    for (tap, rx, tx) in [("tap0", "100\n", "7000\n"), ("rx5n1", "20\n", "300\n")] {
        let dir = sys.join(tap).join("statistics");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rx_bytes"), rx).unwrap();
        std::fs::write(dir.join("tx_bytes"), tx).unwrap();
    }
    let taps = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    // Konuğun aldığı = arayüzlerin gönderdiği (7000 + 300); gönderdiği = arayüzlerin aldığı (100 + 20).
    assert_eq!(tap_counters(&sys, &taps(&["tap0", "rx5n1"])), Some((7300, 120)));
    assert_eq!(tap_counters(&sys, &taps(&["tap0", "yok"])), Some((7000, 100)), "kaybolan arayüz atlanır");
    assert_eq!(tap_counters(&sys, &taps(&["yok"])), None);
    assert_eq!(tap_counters(&sys, &[]), None, "NAT'taki konukta arayüz yoktur");
    std::fs::remove_dir_all(sys).unwrap();
}

#[test]
fn free_core_slots_are_declared_only_when_a_limit_is_set() {
    let mut g = guest(None);
    assert!(args(&cfg(), &g).join(" ").contains(" -smp 2 -m "), "sınır yokken eski komut satırı aynen kalır");
    g.hw.max_cores = 6;
    let with_slots = args(&cfg(), &g).join(" ");
    assert!(with_slots.contains(" -smp 2,maxcpus=6 -m "));
    // Disk kuyrukları çekirdek sayısına değil üst sınıra bağlıdır: çekirdek eklenince aygıt değişmez.
    assert!(with_slots.contains(" -global virtio-blk-pci.num-queues=6 "), "{with_slots}");
    assert!(!args(&cfg(), &guest(None)).join(" ").contains("num-queues"));
    g.cores = 4;
    assert!(args(&cfg(), &g).join(" ").contains(" -global virtio-blk-pci.num-queues=6 "));
    g.hw.max_cores = 64;
    assert!(args(&cfg(), &g).join(" ").contains(" -global virtio-blk-pci.num-queues=16 "));
    g.hw.max_cores = 6;
    // Sınıra ulaşmış konuk da aynı düzenle açılır (askıdan dönüş aynı makineyi bulsun).
    g.cores = 6;
    assert!(args(&cfg(), &g).join(" ").contains(" -smp 6,maxcpus=6 -m "));
    // Sınırın altında kalmış tutarsız kayıt QEMU'ya geçersiz değer olarak gitmez.
    g.hw.max_cores = 4;
    assert!(args(&cfg(), &g).join(" ").contains(" -smp 6 -m "));
}

#[test]
fn free_memory_slots_are_declared_only_when_a_limit_is_set() {
    let mut g = guest(None);
    let total = g.mem_total_mb;
    let plain = args(&cfg(), &g).join(" ");
    assert!(plain.contains(&format!(" -m {total}M ")) && !plain.contains("maxmem") && !plain.contains("-numa"), "sınır yokken eski komut satırı aynen kalır: {plain}");
    g.hw.max_mem_mb = total + 2048;
    let with = args(&cfg(), &g).join(" ");
    assert!(with.contains(&format!(" -m {total}M,slots=8,maxmem={}M -object memory-backend-ram,id=ram0,size={total}M -numa node,nodeid=0,memdev=ram0 ", total + 2048)), "{with}");
    assert!(!with.contains("pc-dimm"));
    // Çalışırken eklenmiş parçalar toplam belleğe dahildir: açılış belleği aynı kalır, parçalar sırayla takılır.
    (g.mem_total_mb, g.hw.dimms) = (total + 640, vec![512, 128]);
    let with = args(&cfg(), &g).join(" ");
    assert!(with.contains(&format!(" -m {total}M,slots=8,maxmem={}M ", total + 2048)), "{with}");
    assert!(with.contains(" -object memory-backend-ram,id=xmem0,size=512M -device pc-dimm,id=xdimm0,memdev=xmem0,slot=0 -object memory-backend-ram,id=xmem1,size=128M -device pc-dimm,id=xdimm1,memdev=xmem1,slot=1 "), "{with}");
    // Sınıra parçalarla ulaşmış konuk aynı düzenle açılır; sınıra açılış belleğiyle ulaşmış konukta yuva gerekmez.
    (g.mem_total_mb, g.hw.dimms) = (total + 2048, vec![2048]);
    assert!(args(&cfg(), &g).join(" ").contains(&format!(" -m {total}M,slots=8,maxmem={}M ", total + 2048)));
    g.hw.dimms.clear();
    let at_limit = args(&cfg(), &g).join(" ");
    assert!(at_limit.contains(&format!(" -m {}M ", total + 2048)) && !at_limit.contains("maxmem"), "{at_limit}");
}
