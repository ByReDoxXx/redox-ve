//! Konuk ajanı, bağlı klonlar, değişen blok takibi.

use super::*;

#[test]
pub(super) fn guest_agent_reports_address_and_freezes_filesystems_for_live_backups() {
    if !python_available() {
        return;
    }
    for incremental in [false, true] {
        let (mut cfg, dir) = config();
        cfg.incremental_backups = incremental;
        let d = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
        let g = guest(&cfg, "ajanli");
        let log = || std::fs::read_to_string(format!("{}.log", cfg.qga_path(g.vmid).display())).unwrap_or_default();
        d.create(&g).unwrap();
        // Kapalı konuk: ajan sorulmaz, yedek "dondurma" bilgisi taşımaz.
        assert_eq!((d.guest_agent(&g), d.guest_ip(&g)), (None, None));
        assert_eq!(d.backup(&g, "vm-100-kapali").unwrap().quiesced, None);

        d.apply(&g, Action::Start).unwrap();
        assert_eq!(d.guest_agent(&g).as_deref(), Some("Debian GNU/Linux 12 (bookworm)"));
        assert_eq!(d.guest_ip(&g).as_deref(), Some("10.0.2.15"), "NAT ağındaki konuğun adresi ajandan gelir");
        std::fs::remove_file(format!("{}.log", cfg.qga_path(g.vmid).display())).unwrap();

        // Canlı yedek: önce dondur, kopyalar başlayınca çöz.
        assert_eq!(d.backup(&g, "vm-100-canli").unwrap().quiesced, Some(true));
        assert_eq!(log(), "guest-fsfreeze-freeze
guest-fsfreeze-thaw
");
        // Yedek başarısız olsa da konuk donmuş bırakılmaz.
        let marker = cfg.backups_dir().join(if incremental { ".vm-100-dolu.d0.raw.tmp.fail" } else { ".vm-100-dolu.qcow2.tmp.fail" });
        std::fs::write(&marker, b"").unwrap();
        assert!(d.backup(&g, "vm-100-dolu").is_err());
        assert_eq!(log(), "guest-fsfreeze-freeze
guest-fsfreeze-thaw
".repeat(2));
        std::fs::remove_file(&marker).unwrap();
        // Ajan dondurmayı reddederse yedek yine alınır, ama "donduruldu" denmez ve çözme gönderilmez.
        std::fs::write(format!("{}.nofreeze", cfg.qga_path(g.vmid).display()), b"").unwrap();
        assert_eq!(d.backup(&g, "vm-100-red").unwrap().quiesced, Some(false));
        assert!(log().ends_with("guest-fsfreeze-thaw
guest-fsfreeze-freeze
"), "{}", log());

        d.apply(&g, Action::Stop).unwrap();
        assert!(!cfg.qga_path(g.vmid).exists(), "ajan soketi kapanışta temizlenir");
        assert_eq!(d.guest_agent(&g), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Ajansız konuk: her şey eskisi gibi çalışır, yanıt beklemek durum okumasını uzatmaz.
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    d.create(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();
    let began = std::time::Instant::now();
    assert_eq!((d.guest_agent(&g), d.guest_ip(&g)), (None, None));
    for _ in 0..20 {
        assert_eq!(d.guest_agent(&g), None);
    }
    assert!(began.elapsed() < Duration::from_secs(2), "yanıtsız ajan her seferinde yeniden beklenmez: {:?}", began.elapsed());
    assert_eq!(d.backup(&g, "vm-100-ajansiz").unwrap().quiesced, Some(false));
    d.apply(&g, Action::Stop).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn linked_clones_share_the_template_disk() {
    let (cfg, dir) = config();
    let d = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let mut template = guest_with_disks(&cfg, d.qemu(), &[1]);
    template.hw.template = true;
    let mut clone = template.clone();
    clone.vmid = 105;
    d.link_disk(&template, &clone).unwrap();
    // Sahte qemu-img, arkalık olarak verilen yolu dosyaya yazar: her disk şablonun aynı numaralı diskine dayanır.
    assert_eq!(std::fs::read_to_string(cfg.disk_path(105)).unwrap(), format!("bagli:{}", cfg.disk_path(template.vmid).display()));
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_path(105, 1)).unwrap(), format!("bagli:{}", cfg.extra_disk_path(template.vmid, 1).display()));
    assert!(d.link_disk(&template, &clone).unwrap_err().contains("zaten var"), "var olan diskin üzerine yazılmaz");
    // Şablonun bir diski eksikse yarım klon bırakılmaz.
    clone.vmid = 106;
    std::fs::remove_file(cfg.extra_disk_path(template.vmid, 1)).unwrap();
    assert!(d.link_disk(&template, &clone).unwrap_err().contains("şablonun diski bulunamadı"));
    assert!(!cfg.disk_path(106).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn refused_template_deletion_keeps_its_disks() {
    // Bağlı klonu olan şablonun silinmesi reddedilir ve diskleri yerinde kalır.
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, auth);
    let kalip = state.create_guest(&spec("kalip", &node), "admin").unwrap();
    std::fs::write(cfg.disk_path(kalip), "kalip-verisi").unwrap();
    state.set_template(kalip, true, "admin").unwrap();
    let bagli = state.clone_guest_as(kalip, "bagli", true, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0]);
    assert!(std::fs::read_to_string(cfg.disk_path(bagli)).unwrap().starts_with("bagli:"));
    assert!(matches!(state.delete_guest(kalip, "admin"), Err(Error::InvalidSpec(m)) if m.contains("bağlı klonları")));
    assert_eq!(std::fs::read_to_string(cfg.disk_path(kalip)).unwrap(), "kalip-verisi", "reddedilen silme şablonun diskine dokunmaz");
    assert!(matches!(state.run_action(kalip, Action::Start, "admin"), Err(Error::InvalidSpec(_))));
    state.delete_guest(bagli, "admin").unwrap();
    state.delete_guest(kalip, "admin").unwrap();
    assert!(!cfg.disk_path(kalip).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn changed_block_tracking_falls_back_to_a_full_read_when_unsupported() {
    if !python_available() {
        return;
    }
    // Sahte QEMU ve qemu-img harita işlemlerini bilmez (eski sürüm gibi): yedekler yine alınmalı.
    let (mut cfg, dir) = config();
    cfg.incremental_backups = true;
    cfg.cbt = true;
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    d.create(&g).unwrap();
    let disk = cfg.disk_path(g.vmid);
    let state = cfg.images_dir().join(format!("vm-{}.cbt.json", g.vmid));
    std::fs::write(&disk, patterned(2)).unwrap();
    assert!(!d.backup(&g, "vm-100-kapali").unwrap().cbt);
    assert!(!state.exists(), "harita kurulamadıysa durum yazılmaz: sonraki yedek de baştan sona okur");
    d.apply(&g, Action::Start).unwrap();
    for id in ["vm-100-a", "vm-100-b"] {
        let size = d.backup(&g, id).unwrap();
        assert!(!size.cbt && size.new_mb.is_some());
    }
    assert!(!state.exists() && temporaries(&cfg).is_empty());
    // Elle bırakılmış (sahte) bir durum dosyası da veri bozmaz: harita yoksa tam okumaya dönülür.
    std::fs::write(&state, r#"{"base":"vm-100-b","disks":[0],"streak":0}"#).unwrap();
    let mut changed = patterned(2);
    changed[10..15].copy_from_slice(b"YENI!");
    std::fs::write(&disk, &changed).unwrap();
    assert!(!d.backup(&g, "vm-100-c").unwrap().cbt);
    assert!(!state.exists());
    d.apply(&g, Action::Stop).unwrap();
    std::fs::write(&disk, b"bozuk").unwrap();
    d.restore(&g, "vm-100-c", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == changed, "yedek, diskin o anki tam içeriğidir");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn live_backup_finishes_when_jobs_complete_before_the_start_reply() {
    if !python_available() {
        return;
    }
    // "hizli" adlı sahte konuk, gerçek QEMU'nun az verili işlerde yaptığı gibi "bitti" olayını başlama
    // yanıtından önce yollar. Yedek beklemede kalmamalı.
    for incremental in [false, true] {
        let (mut cfg, dir) = config();
        cfg.incremental_backups = incremental;
        let d = QemuDriver::new(cfg.clone()).unwrap();
        let g = guest_with_disks(&cfg, &d, &[1]);
        let mut g = g;
        g.name = "hizli".into();
        d.apply(&g, Action::Start).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (driver, guest) = (std::sync::Arc::new(d), g.clone());
        let worker = driver.clone();
        std::thread::spawn(move || sender.send(worker.backup(&guest, "vm-100-hizli")).ok());
        let outcome = receiver.recv_timeout(Duration::from_secs(20)).expect("yedek takıldı: erken gelen bitti olayı kaçırıldı");
        assert_eq!(outcome.unwrap().quiesced, Some(false));
        assert!(driver.verify_backup("vm-100-hizli", &[1]).is_ok());
        driver.apply(&g, Action::Stop).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
