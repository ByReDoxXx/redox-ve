use super::*;

const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxvcmVtaXBzdW1kb2xvcnNpdGFtZXRjb25zZWN0 ben@dizustu";

fn hw(image: Option<&str>, user: Option<&str>, key: Option<&str>) -> Hardware {
    Hardware { image: image.map(Into::into), ci_user: user.map(Into::into), ci_ssh_key: key.map(Into::into), ..Default::default() }
}

#[test]
fn names_and_keys_are_validated() {
    for ok in ["debian-12-genericcloud-amd64.qcow2", "noble-server-cloudimg-amd64.img", "a.raw"] {
        assert!(valid_image_name(ok), "{ok}");
    }
    for ok in ["eski-sunucu.vmdk", "win.vhdx", "kutu.vdi", "a.ova"] {
        assert!(valid_image_name(ok), "{ok}");
    }
    for bad in ["", ".qcow2", ".gizli.qcow2", "../a.qcow2", "a/b.img", "a.iso", "a b.img", "a.qcow2;rm", "a.ovf", ".ova"] {
        assert!(!valid_image_name(bad), "{bad}");
    }
    for ok in ["redox", "_svc", "ali-2", "a"] {
        assert!(valid_ci_user(ok), "{ok}");
    }
    for bad in ["", "root", "Ali", "2ali", "a b", "a:b", &"a".repeat(33)] {
        assert!(!valid_ci_user(bad), "{bad}");
    }
    assert!(valid_ssh_key(KEY));
    assert!(valid_ssh_key(KEY.rsplit_once(' ').unwrap().0), "açıklama isteğe bağlı");
    for bad in [
        "",
        "ssh-ed25519",
        "ssh-dss AAAAB3NzaC1kc3MAAACBAJ0000000000000000000000000000 eski",
        "ssh-ed25519 kısa x",
        &format!("{KEY}\nssh-ed25519 AAAA ikinci"),
        &format!("{KEY} \"tırnak"),
        &format!("{KEY} ters\\bolu"),
        "-----BEGIN OPENSSH PRIVATE KEY-----",
    ] {
        assert!(!valid_ssh_key(bad), "{bad}");
    }
}

#[test]
fn hardware_rules_depend_on_kind_and_each_other() {
    assert!(Hardware::default().validate(GuestKind::Ct).is_ok());
    assert!(hw(Some("d.qcow2"), Some("ali"), Some(KEY)).validate(GuestKind::Vm).is_ok());
    assert!(hw(None, None, Some(KEY)).validate(GuestKind::Vm).is_ok(), "ISO kurulumunda da kullanılabilir");
    for (h, kind, needle) in [
        (hw(None, None, Some(KEY)), GuestKind::Ct, "yalnızca sanal makinelerde"),
        (hw(None, Some("ali"), None), GuestKind::Vm, "SSH açık anahtarı gerekli"),
        (hw(Some("../d.qcow2"), None, Some(KEY)), GuestKind::Vm, "geçersiz bulut imajı"),
        (hw(None, Some("root"), Some(KEY)), GuestKind::Vm, "geçersiz cloud-init kullanıcı"),
        (hw(None, None, Some("parola123")), GuestKind::Vm, "geçersiz SSH açık anahtarı"),
    ] {
        let err = h.validate(kind).unwrap_err();
        assert!(err.contains(needle), "{err}");
    }
}

#[test]
fn renders_nocloud_files() {
    assert_eq!(Hardware::default().user_data("web"), None);
    let data = hw(None, Some("ali"), Some(KEY)).user_data("web-01").unwrap();
    assert!(data.starts_with("#cloud-config\nhostname: web-01\n"), "{data}");
    assert!(data.contains("  - name: ali\n") && data.contains(&format!("      - \"{KEY}\"\n")) && data.contains("ssh_pwauth: false"));
    assert!(hw(None, None, Some(KEY)).user_data("x").unwrap().contains("  - name: redox\n"));
    // Doğrulamadan kaçmış bozuk anahtar YAML'a yazılmaz.
    assert_eq!(hw(None, None, Some("ssh-ed25519 AAAA\"\nruncmd: [reboot]")).user_data("x"), None);
    assert_eq!(meta_data(103, "web-01", 0), "instance-id: redox-103\nlocal-hostname: web-01\n");
    assert_eq!(meta_data(103, "web-01", 2), "instance-id: redox-103-2\nlocal-hostname: web-01\n");
}

#[test]
fn backup_hooks_are_validated_and_split() {
    for ok in ["", "/usr/local/bin/once.sh", "/usr/bin/pg_ctl -D /var/lib/pg checkpoint", "C:\\betik\\once.cmd /sessiz"] {
        assert!(valid_hook(ok), "{ok}");
    }
    for bad in ["once.sh", "sh -c reboot", "/bin/x\nrm -rf /", "; reboot"] {
        assert!(!valid_hook(bad), "{bad:?}");
    }
    assert!(!valid_hook(&format!("/{}", "a".repeat(300))));
    assert_eq!(split_hook("/usr/bin/pg_ctl  -D /veri checkpoint"), Some(("/usr/bin/pg_ctl".to_string(), vec!["-D".to_string(), "/veri".to_string(), "checkpoint".to_string()])));
    assert_eq!(split_hook("   "), None);
    let with = Hardware { backup_pre: "/usr/local/bin/once.sh".into(), ..Default::default() };
    with.validate(GuestKind::Vm).unwrap();
    assert!(with.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    assert!(Hardware { backup_post: "sonra.sh".into(), ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("mutlak bir program yoluyla"));
}

#[test]
fn nested_virtualization_adds_the_vendor_extension() {
    assert_eq!(cpu_arg("host", true), "host");
    assert_eq!(cpu_arg("max", true), "max");
    assert_eq!(cpu_arg("Haswell", true), "Haswell,+vmx");
    assert_eq!(cpu_arg("EPYC-Milan", true), "EPYC-Milan,+svm");
    assert_eq!(cpu_arg("Haswell", false), "Haswell");
    let hw = |cpu: &str| Hardware { cpu_type: cpu.into(), nested: true, ..Default::default() };
    hw("").validate(GuestKind::Vm).unwrap();
    hw("EPYC").validate(GuestKind::Vm).unwrap();
    assert!(hw("kvm64").validate(GuestKind::Vm).unwrap_err().contains("kvm64 modeliyle kullanılamaz"));
    assert!(hw("").validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
}

#[test]
fn dynamic_memory_follows_guest_usage_within_bounds() {
    use crate::driver::BalloonStats;
    let stats = |total_mb, available_mb| BalloonStats { total_mb, available_mb };
    // 8 GB'lık konuk 1 GB kullanıyor: hedef kullanım + pay (en az 256 MB), 64'e yuvarlanır.
    assert_eq!(dynamic_memory_target(8192, 1024, 0, stats(8192, 7168)), Some(1280));
    // Alt sınırın altına inilmez.
    assert_eq!(dynamic_memory_target(8192, 2048, 0, stats(8192, 7168)), Some(2048));
    // Kullanım artınca hedef hemen büyür (pay %20: 3000 + 600 = 3600 → 3648).
    assert_eq!(dynamic_memory_target(8192, 1024, 1280, stats(1280, 0)), Some(1536));
    assert_eq!(dynamic_memory_target(8192, 1024, 3072, stats(3072, 72)), Some(3648));
    // Belleğin tamamı aşılmaz.
    assert_eq!(dynamic_memory_target(4096, 1024, 2048, stats(4096, 96)), Some(4096));
    // Küçük farklar için küçültme yapılmaz; hedef zaten yerindeyse değişmez.
    assert_eq!(dynamic_memory_target(8192, 1024, 1536, stats(1536, 400)), None);
    assert_eq!(dynamic_memory_target(8192, 1024, 1280, stats(1280, 256)), None);
    // Kapalıyken, tutarsız sınırla ya da ölçüm yokken dokunulmaz.
    assert_eq!(dynamic_memory_target(8192, 0, 0, stats(8192, 7168)), None);
    assert_eq!(dynamic_memory_target(1024, 2048, 0, stats(1024, 900)), None);
    assert_eq!(dynamic_memory_target(8192, 1024, 0, stats(0, 0)), None);
}

#[test]
fn cpu_type_and_balloon_are_validated() {
    let ok = Hardware { cpu_type: "Haswell".into(), balloon: true, ..Default::default() };
    ok.validate(GuestKind::Vm).unwrap();
    assert!(ok.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    Hardware::default().validate(GuestKind::Ct).unwrap();
    for bad in ["haswell", "Pentium9", "host,+vmx", "Haswell -device x"] {
        assert!(Hardware { cpu_type: bad.into(), ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("bilinmeyen işlemci türü"), "{bad}");
    }
    assert!(valid_cpu_type("") && valid_cpu_type("max") && !valid_cpu_type("host"), "host boş değerle seçilir");
    // Üretici: başka üreticinin modeli reddedilir; bağımsız modeller ve bilinmeyen sunucu engellenmez.
    assert_eq!((cpu_vendor_of("Haswell"), cpu_vendor_of("EPYC-Milan"), cpu_vendor_of("kvm64"), cpu_vendor_of("")), (Some("intel"), Some("amd"), None, None));
    assert_eq!(parse_cpu_vendor("processor\t: 0\nvendor_id\t: GenuineIntel\ncpu family\t: 6\n"), Some("intel"));
    assert_eq!((parse_cpu_vendor("vendor_id\t: AuthenticAMD\n"), parse_cpu_vendor("vendor_id\t: Baska\n"), parse_cpu_vendor("")), (Some("amd"), None, None));
    assert!(check_cpu_vendor("EPYC", Some("intel")).unwrap_err().contains("bir AMD modeli, bu sunucunun işlemcisi Intel"));
    assert!(check_cpu_vendor("Haswell", Some("amd")).is_err());
    for (model, host) in [("Haswell", Some("intel")), ("EPYC", Some("amd")), ("max", Some("intel")), ("", Some("amd")), ("EPYC", None)] {
        check_cpu_vendor(model, host).unwrap();
    }
}

#[test]
fn cloud_init_settings_are_edited_validated_and_versioned() {
    let mut h = Hardware::default();
    assert!(!h.apply_cloud_init(Some(""), Some(""), Some(""), Some(""), Some("")), "boş alanlar değişiklik sayılmaz");
    assert_eq!(h.ci_gen, 0);
    assert!(h.apply_cloud_init(Some("ali"), Some(&format!("  {KEY}\n")), Some("10.0.0.5/24"), Some("10.0.0.1"), Some("1.1.1.1, 9.9.9.9")));
    assert_eq!((h.ci_user.as_deref(), h.ci_ssh_key.as_deref(), h.ci_ip.as_deref(), h.ci_gw.as_deref(), h.ci_dns.len(), h.ci_gen), (Some("ali"), Some(KEY), Some("10.0.0.5/24"), Some("10.0.0.1"), 2, 1));
    h.validate(GuestKind::Vm).unwrap();
    assert!(!h.apply_cloud_init(Some("ali"), None, Some("10.0.0.5/24"), None, None), "aynı değerler sürümü artırmaz");
    assert!(h.apply_cloud_init(None, None, Some("10.0.0.6/24"), None, None));
    assert_eq!(h.ci_gen, 2);
    assert_eq!(
        h.network_config(103).unwrap(),
        "version: 2\nethernets:\n  nic0:\n    match:\n      macaddress: \"52:54:00:00:00:67\"\n    dhcp4: false\n    addresses:\n      - 10.0.0.6/24\n    gateway4: 10.0.0.1\n    nameservers:\n      addresses: [1.1.1.1, 9.9.9.9]\n"
    );
    // Geçersiz değerler.
    for (ip, gw, dns, want) in [
        ("10.0.0.5", "", "", "geçersiz cloud-init adresi"),
        ("10.0.0.5/33", "", "", "geçersiz cloud-init adresi"),
        ("10.0.0.5/24\n    x: y", "", "", "geçersiz cloud-init adresi"),
        ("10.0.0.5/24", "10.0.0", "", "geçersiz ağ geçidi"),
        ("10.0.0.5/24", "", "1.1.1.1, dns.example", "geçersiz ad sunucusu"),
        ("10.0.0.5/24", "", "1.1.1.1 2.2.2.2 3.3.3.3 4.4.4.4", "en fazla 3"),
        ("", "10.0.0.1", "", "yalnızca sabit adresle"),
    ] {
        let mut bad = h.clone();
        bad.apply_cloud_init(None, None, Some(ip), Some(gw), Some(dns));
        assert!(bad.validate(GuestKind::Vm).unwrap_err().contains(want), "{ip} {gw} {dns}");
    }
    let mut no_key = Hardware::default();
    no_key.apply_cloud_init(None, None, Some("10.0.0.5/24"), None, None);
    assert!(no_key.validate(GuestKind::Vm).unwrap_err().contains("SSH açık anahtarı gerekli"));
    assert!(no_key.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    // Adres boşaltılınca DHCP'ye dönülür; anahtar boşaltılınca cloud-init tümüyle kapanır.
    let mut dhcp = h.clone();
    dhcp.apply_cloud_init(None, None, Some(""), Some(""), Some(""));
    assert_eq!((dhcp.network_config(103), dhcp.cloud_init()), (None, true));
    assert!(h.apply_cloud_init(None, Some(""), None, None, None));
    assert_eq!((h.cloud_init(), h.ci_user.clone(), h.ci_ip.clone(), h.ci_dns.len(), h.ci_gen), (false, None, None, 0, 3));
    assert_eq!(h.user_data("x"), None);
}

#[test]
fn firewall_rules_parse_print_and_validate() {
    let rules = parse_fw_rules("tcp/22, tcp/80-443 from 10.0.0.0/8\nudp/53 from 192.168.1.10 ,icmp,, udp").unwrap();
    let text: Vec<String> = rules.iter().map(ToString::to_string).collect();
    assert_eq!(text, ["tcp/22", "tcp/80-443 from 10.0.0.0/8", "udp/53 from 192.168.1.10", "icmp", "udp"]);
    assert_eq!(parse_fw_rules(&text.join(", ")).unwrap(), rules, "yazdırılan biçim yeniden okunur");
    assert!(parse_fw_rules("  \n ").unwrap().is_empty());
    for bad in ["sctp/22", "tcp/0", "tcp/443-80", "tcp/70000", "tcp/22 from evim", "tcp/22 from 10.0.0.0/33", "tcp/22 from 300.1.1.1",
        "icmp/8", "tcp/22; drop", "tcp/22 from 1.2.3.4 }", "tcp/a-b"] {
        assert!(parse_fw_rules(bad).unwrap_err().contains("geçersiz güvenlik duvarı kuralı"), "{bad}");
    }
    for ok in ["0.0.0.0/0", "10.1.2.3", "192.168.0.0/16"] {
        assert!(valid_cidr(ok), "{ok}");
    }
    for bad in ["", "1.2.3", "1.2.3.4.5", "1.2.3.4/", "1.2.3.4/x", "a.b.c.d", "1.2.3.4 ", "::1"] {
        assert!(!valid_cidr(bad), "{bad}");
    }

    // Güvenlik duvarı konteynerlerde de geçerlidir; doğrulamadan kaçan bozuk kural yakalanır.
    let fw = Hardware { firewall: true, fw_rules: rules, ..Default::default() };
    assert!(fw.validate(GuestKind::Ct).is_ok() && fw.validate(GuestKind::Vm).is_ok());
    let forged = Hardware { fw_rules: vec![FwRule { proto: FwProto::Tcp, ports: Some((22, 22)), source: Some("1.2.3.4 } ; flush ruleset".into()) }], ..Default::default() };
    assert!(forged.validate(GuestKind::Vm).unwrap_err().contains("geçersiz güvenlik duvarı kuralı"));
    let many = Hardware { fw_rules: vec![FwRule { proto: FwProto::Icmp, ports: None, source: None }; MAX_FW_RULES + 1], ..Default::default() };
    assert!(many.validate(GuestKind::Vm).unwrap_err().contains("en fazla"));
}

#[test]
fn firmware_rules() {
    let uefi = Hardware { firmware: Firmware::Uefi, secure_boot: true, tpm: true, ..Default::default() };
    assert!(uefi.validate(GuestKind::Vm).is_ok());
    assert_eq!((uefi.firmware_choice(), uefi.firmware_label()), ("uefi-secure", "UEFI + güvenli önyükleme"));
    assert!(uefi.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    let tpm_only = Hardware { tpm: true, ..Default::default() };
    assert!(tpm_only.validate(GuestKind::Vm).is_ok(), "TPM BIOS ile de kullanılabilir");
    assert!(tpm_only.validate(GuestKind::Ct).is_err());
    let bad = Hardware { secure_boot: true, ..Default::default() };
    assert!(bad.validate(GuestKind::Vm).unwrap_err().contains("UEFI donanım yazılımı gerektirir"));
    for choice in ["bios", "uefi", "uefi-secure"] {
        let (firmware, secure_boot) = parse_firmware_choice(choice).unwrap();
        assert_eq!(Hardware { firmware, secure_boot, ..Default::default() }.firmware_choice(), choice);
    }
    assert_eq!(parse_firmware_choice("coreboot"), None);
    assert_eq!(serde_json::to_value(Firmware::Uefi).unwrap(), "uefi");
}

#[test]
fn extra_disks_and_nics_are_bounded() {
    let d = |id, size_gb| ExtraDisk { id, size_gb, serial: 0 };
    let ok = Hardware { disks: vec![d(1, 10), d(3, 500)], nics: vec!["vmbr1".into(), "nat".into()], ..Default::default() };
    assert!(ok.validate(GuestKind::Vm).is_ok());
    assert!(ok.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelere"));
    assert!(Hardware { nics: vec!["vmbr1".into()], ..Default::default() }.validate(GuestKind::Ct).is_ok(), "konteynere ek kart takılabilir");
    for disks in [vec![d(1, 10), d(1, 20)], vec![d(0, 10)], vec![d(9, 10)], vec![d(1, 0)]] {
        assert!(Hardware { disks, ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("geçersiz ek disk"));
    }
    assert!(Hardware { nics: vec!["nat".into(); 4], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("en fazla 3"));
    assert!(Hardware { nics: vec!["br0,script=/x".into()], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("geçersiz ağ"));
}

#[test]
fn passthrough_ids_are_strict() {
    for ok in ["046d:c52b", "0000:ffff"] {
        assert!(valid_usb_id(ok), "{ok}");
    }
    for bad in ["", "046d", "046D:C52B", "46d:c52b", "046d:c52b:1", "046d-c52b", "046g:c52b", "046d:c52b,hostbus=1"] {
        assert!(!valid_usb_id(bad), "{bad}");
    }
    for ok in ["0000:01:00.0", "0000:3b:1f.7"] {
        assert!(valid_pci_addr(ok), "{ok}");
    }
    for bad in ["", "01:00.0", "0000:01:00", "0000:01:00.8", "0000:01:00.0,x-vga=on", "0000:1:00.0", "0000.01.00.0"] {
        assert!(!valid_pci_addr(bad), "{bad}");
    }
    let hw = Hardware { usb: vec!["046d:c52b".into()], pci: vec!["0000:01:00.0".into()], ..Default::default() };
    assert!(hw.validate(GuestKind::Vm).is_ok());
    assert!(hw.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    assert!(Hardware { usb: vec!["046d:c52b".into(); 5], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("en fazla 4"));
    assert!(Hardware { pci: vec!["gpu".into()], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("geçersiz PCI adresi"));
    // Bulut imajı artık anahtarsız da seçilebilir: başka sanallaştırıcıdan aktarılan diskte cloud-init olmayabilir.
    assert!(Hardware { image: Some("eski.vmdk".into()), ..Default::default() }.validate(GuestKind::Vm).is_ok());
}

#[test]
fn old_records_without_hardware_still_load() {
    let h: Hardware = serde_json::from_str("{}").unwrap();
    assert_eq!(h, Hardware::default());
}

#[test]
fn schedules_parse_format_and_validate() {
    use crate::model::Action;
    let list = parse_schedules("start@08:00@1-5, stop@20:00\nreboot@03:15@67").unwrap();
    assert_eq!(list.iter().map(|s| (s.action, s.time.as_str(), s.days)).collect::<Vec<_>>(), [(Action::Start, "08:00", 0b001_1111), (Action::Stop, "20:00", EVERY_DAY), (Action::Reboot, "03:15", 0b110_0000)]);
    assert_eq!(list.iter().map(ToString::to_string).collect::<Vec<_>>(), ["start@08:00@12345", "stop@20:00", "reboot@03:15@67"]);
    assert_eq!(list.iter().map(Schedule::days_label).collect::<Vec<_>>(), ["hafta içi", "her gün", "hafta sonu"]);
    assert_eq!(parse_schedules("start@08:00@135").unwrap()[0].days_label(), "Pzt, Çar, Cum");
    assert!(list[0].on_day(0) && list[0].on_day(4) && !list[0].on_day(5) && !list[0].on_day(9));
    assert_eq!(parse_schedules(" , ").unwrap(), []);
    // Biçimi kendi çıktısından geri okunur.
    assert_eq!(parse_schedules(&list.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).unwrap(), list);
    for bad in ["start", "start@8:00", "start@25:00", "pause@08:00", "uç@08:00", "start@08:00@", "start@08:00@8", "start@08:00@5-1", "start@08:00@1-5@x", "start@08:00@0"] {
        assert!(parse_schedules(bad).is_err(), "{bad}");
    }
    assert_eq!((parse_days("1-57"), parse_days("7"), parse_days("1-"), parse_days("")), (Some(0b101_1111), Some(0b100_0000), None, None));

    let hw = |f: &dyn Fn(&mut Hardware)| {
        let mut hw = Hardware::default();
        f(&mut hw);
        hw.validate(GuestKind::Ct)
    };
    assert!(hw(&|h| h.schedules = list.clone()).is_ok(), "konteynerde de zamanlanır");
    assert!(hw(&|h| h.schedules = vec![list[0].clone(); MAX_SCHEDULES + 1]).unwrap_err().contains("en fazla"));
    assert!(hw(&|h| h.schedules = vec![Schedule { action: Action::Pause, time: "08:00".into(), days: EVERY_DAY, last_run: None }]).is_err());
    assert!(hw(&|h| h.schedules = vec![Schedule { action: Action::Stop, time: "08:00".into(), days: 0, last_run: None }]).is_err());
    assert!(hw(&|h| h.boot_order = MAX_BOOT_ORDER + 1).is_err() && hw(&|h| h.boot_delay = MAX_BOOT_DELAY + 1).is_err());
    assert!(hw(&|h| (h.boot_order, h.boot_delay) = (MAX_BOOT_ORDER, MAX_BOOT_DELAY)).is_ok());
}

#[test]
fn rate_limits_are_bounded_and_vm_only() {
    let hw = |f: &dyn Fn(&mut Hardware), kind| {
        let mut hw = Hardware::default();
        f(&mut hw);
        hw.validate(kind)
    };
    assert!(hw(&|h| (h.disk_mbps, h.disk_iops, h.net_mbps) = (MAX_DISK_MBPS, MAX_DISK_IOPS, MAX_NET_MBPS), GuestKind::Vm).is_ok());
    assert!(hw(&|h| h.disk_mbps = MAX_DISK_MBPS + 1, GuestKind::Vm).is_err());
    assert!(hw(&|h| h.disk_iops = MAX_DISK_IOPS + 1, GuestKind::Vm).is_err());
    assert!(hw(&|h| h.net_mbps = MAX_NET_MBPS + 1, GuestKind::Vm).is_err());
    for set in [(|h: &mut Hardware| h.disk_mbps = 1) as fn(&mut Hardware), |h| h.disk_iops = 1, |h| h.net_mbps = 1] {
        assert!(hw(&set, GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    }
    let mut limited = Hardware { net_mbps: 100, ..Default::default() };
    assert!(limited.net_limit_applies("vmbr0") && !limited.net_limit_applies("nat"));
    limited.nics = vec!["nat".into(), "vmbr1".into()];
    assert!(limited.net_limit_applies("nat"), "köprüdeki ek kart yeter");
    assert!(Hardware::default().net_limit_applies("nat"), "sınır yoksa koşul da yok");
}

#[test]
fn nic_specs_carry_an_optional_vlan_tag() {
    assert_eq!(parse_nic("vmbr1"), Some(("vmbr1", 0)));
    assert_eq!([normalize_network(" NAT "), normalize_network("Nat"), normalize_network("vmbrT"), normalize_network("vmbrT:20")], ["nat", "nat", "vmbrT", "vmbrT:20"]);
    assert_eq!(parse_nic("nat"), Some(("nat", 0)));
    assert_eq!(parse_nic("vmbr1:20"), Some(("vmbr1", 20)));
    assert_eq!(parse_nic("br.lan:4094"), Some(("br.lan", 4094)));
    for bad in ["", "vmbr1:", "vmbr1:0", "vmbr1:4095", "vmbr1:+5", "vmbr1:2 0", "nat:20", ":20", "vmbr1:20:30", "br0,script=/x:20", "vmbr1:yirmi"] {
        assert_eq!(parse_nic(bad), None, "{bad}");
    }
    let hw = |f: &dyn Fn(&mut Hardware), kind| {
        let mut hw = Hardware::default();
        f(&mut hw);
        hw.validate(kind)
    };
    assert!(hw(&|h| (h.vlan, h.nics) = (4094, vec!["vmbr1:20".into(), "nat".into()]), GuestKind::Vm).is_ok());
    assert!(hw(&|h| h.vlan = 4095, GuestKind::Vm).is_err());
    assert!(hw(&|h| h.nics = vec!["nat:5".into()], GuestKind::Vm).unwrap_err().contains("geçersiz ağ ayarı"));
    assert!(hw(&|h| h.vlan = 5, GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    assert!(hw(&|h| h.nics = vec!["vmbr1:5".into()], GuestKind::Ct).is_err());
    assert!(hw(&|h| h.nics = vec!["vmbr1".into()], GuestKind::Ct).is_ok(), "etiketsiz ek kart konteynerde de olur");
    // Hız sınırı için köprüdeki kart sayılır; etiketli tanım da köprüdür.
    let limited = Hardware { net_mbps: 10, nics: vec!["vmbr1:20".into()], ..Default::default() };
    assert!(limited.net_limit_applies("nat"));
}

#[test]
fn shares_parse_format_and_validate() {
    let list = parse_shares("ortak, veri:ro\nyedek-2:rw").unwrap();
    assert_eq!(list.iter().map(|s| (s.name.as_str(), s.readonly)).collect::<Vec<_>>(), [("ortak", false), ("veri", true), ("yedek-2", false)]);
    assert_eq!(list.iter().map(ToString::to_string).collect::<Vec<_>>(), ["ortak", "veri:ro", "yedek-2"]);
    assert_eq!(parse_shares(" , ").unwrap(), []);
    for bad in ["../etc", "a/b", "Ortak", "-x", "a.b", "veri:rx", "a:ro:ro", "çok", &"a".repeat(25)] {
        assert!(parse_shares(bad).is_err(), "{bad}");
    }
    let hw = |shares: Vec<Share>, kind| Hardware { shares, ..Default::default() }.validate(kind);
    assert!(hw(list.clone(), GuestKind::Vm).is_ok());
    assert!(hw(list.clone(), GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    assert!(hw(vec![list[0].clone(), list[0].clone()], GuestKind::Vm).unwrap_err().contains("iki kez"));
    assert!(hw(vec![Share { name: "..".into(), readonly: false }], GuestKind::Vm).is_err(), "API'den gelen geçersiz ad da reddedilir");
    let many: Vec<Share> = (0..=MAX_SHARES).map(|i| Share { name: format!("p{i}"), readonly: false }).collect();
    assert!(hw(many, GuestKind::Vm).unwrap_err().contains("en fazla"));
}
