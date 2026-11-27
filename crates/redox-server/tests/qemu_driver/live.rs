//! Çalışan konukta anlık görüntü ve yedek; cloud imajları.

use super::*;

#[test]
pub(super) fn snapshots_and_backups_work_on_a_running_guest() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    assert!(d.live_disk_ops(GuestKind::Vm) && !d.live_disk_ops(GuestKind::Ct));
    d.create(&g).unwrap();
    let disk = cfg.disk_path(g.vmid);
    let snaps = || std::fs::read_to_string(format!("{}.snaps", disk.display())).unwrap_or_default();

    // Kapalıyken alınan görüntü qemu-img ile, çalışırken alınan QEMU izleyicisiyle (savevm) yazılır.
    d.snapshot(&g, SnapshotOp::Create("kapali")).unwrap();
    d.apply(&g, Action::Start).unwrap();
    d.snapshot(&g, SnapshotOp::Create("canli")).unwrap();
    assert_eq!(snaps(), "kapali\ncanli\n");
    d.snapshot(&g, SnapshotOp::Rollback("canli")).unwrap();
    let err = d.snapshot(&g, SnapshotOp::Rollback("yok")).unwrap_err();
    assert!(err.starts_with("Snapshot 'yok' does not exist") && err.contains("önce konuğu kapatın"), "{err}");
    d.snapshot(&g, SnapshotOp::Delete("kapali")).unwrap();
    assert_eq!(snaps(), "canli\n");
    assert!(d.snapshot(&g, SnapshotOp::Create("../x")).unwrap_err().contains("geçersiz"), "ad izleyici komutuna ulaşmadan reddedilir");
    assert!(d.snapshot(&g, SnapshotOp::Create("a b")).is_err());

    // Üretim denetim noktası: bellek kaydedilmez, diskler tek bir QMP işlemiyle görüntülenir. Konuk ajanı
    // yokken dondurma yapılmaz ve bu açıkça bildirilir.
    assert!(!d.snapshot_disk_only(&g, "uretim").unwrap(), "ajan yok: dondurulmadı");
    assert_eq!(snaps(), "canli\nuretim\n");
    assert!(std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(g.vmid).display())).unwrap_or_default().contains("disk-snapshot uretim x1"), "savevm değil, blok işlemi kullanılır");
    assert!(d.snapshot_disk_only(&g, "../x").unwrap_err().contains("geçersiz"));
    d.snapshot(&g, SnapshotOp::Delete("uretim")).unwrap();

    // Canlı yedek: disk QEMU üzerinden kopyalanır; ilgisiz iş olayları atlanır.
    assert_eq!(d.backup(&g, "vm-100-canli").unwrap(), redox_core::BackupSize { quiesced: Some(false), ..redox_core::BackupSize::full(1) });
    let copy = cfg.backups_dir().join("vm-100-canli.qcow2");
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&disk).unwrap());
    assert_eq!(status(&d, &g), GuestStatus::Running, "yedek sırasında konuk çalışmaya devam eder");

    // QEMU işi hatayla bitirirse: ileti aktarılır ve yarım dosya bırakılmaz.
    let failing = cfg.backups_dir().join("vm-100-dolu.qcow2");
    std::fs::write(cfg.backups_dir().join(".vm-100-dolu.qcow2.tmp.fail"), b"").unwrap();
    assert_eq!(d.backup(&g, "vm-100-dolu").unwrap_err(), "yedekleme başarısız: No space left on device");
    assert!(!failing.exists());

    d.apply(&g, Action::Stop).unwrap();
    // Kapandıktan sonra yeniden qemu-img yolu kullanılır.
    d.snapshot(&g, SnapshotOp::Rollback("canli")).unwrap();
    assert_eq!(d.backup(&g, "vm-100-kapali").unwrap().size_mb, 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn live_operations_through_server_state() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, auth);
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    state.refresh(None, None);

    state.snapshot_create(id, "canli", "admin").unwrap();
    let backup = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    assert_eq!((c.guest(id).unwrap().status, c.guest(id).unwrap().snapshots.len(), c.backups.len()), (GuestStatus::Running, 1, 1));
    assert!(c.tasks.iter().take(2).all(|t| t.status == TaskStatus::Ok), "{:?}", c.tasks);
    // Geri yükleme ve disk büyütme hâlâ kapalı konuk ister.
    assert!(matches!(state.backup_restore(&backup, "admin"), Err(Error::RequiresStopped { .. })));
    assert!(matches!(state.resize_disk(id, 5, "admin"), Err(Error::RequiresStopped { .. })));

    // Zamanlanmış yedek artık çalışan konukta da alınır.
    state.update_guest(id, &redox_core::GuestPatch { backup_time: Some("00:00".into()), ..Default::default() }, "admin").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(state.scheduler_tick("2026-11-24", 5), 1);
    let c = state.snapshot();
    assert!(c.backups[0].auto && c.tasks[0].status == TaskStatus::Ok, "{:?}", c.tasks[0]);

    state.run_action(id, Action::Stop, "admin").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn cloud_image_guests_are_copied_resized_and_seeded() {
    if !python_available() {
        return;
    }
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxvcmVtaXBzdW1kb2xvcnNpdGFtZXRjb25zZWN0 ben@dizustu";
    let (cfg, dir) = config();
    let driver = QemuDriver::new(cfg.clone()).unwrap();
    std::fs::write(cfg.cloud_dir().join("debian-12.qcow2"), "bulut-imaji").unwrap();
    std::fs::write(cfg.cloud_dir().join("notlar.txt"), "x").unwrap();
    assert_eq!(driver.images(), ["debian-12.qcow2"], "yalnızca imaj dosyaları listelenir");

    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, auth);
    let cloud = |image: &str| GuestSpec {
        disk_gb: 20,
        hw: redox_core::Hardware { image: Some(image.into()), ci_user: Some("ali".into()), ci_ssh_key: Some(KEY.into()), ..Default::default() },
        ..spec("bulut", &node)
    };
    assert!(matches!(state.create_guest(&cloud("yok.qcow2"), "admin"), Err(Error::InvalidSpec(m)) if m.contains("bulut imajı bulunamadı")));
    assert!(state.snapshot().guests.is_empty());

    // Başka bir dosyaya bağlanan imaj (arkalığı sunucudaki bir dosyayı gösteren qcow2) dönüştürülmez: yoksa o
    // dosyanın içeriği konuğun diskine kopyalanırdı. Konuk da yarım disk de kalmaz.
    std::fs::write(cfg.cloud_dir().join("tuzak.qcow2"), "arkalikli").unwrap();
    let trapped = state.create_guest(&cloud("tuzak.qcow2"), "admin");
    let after = state.snapshot();
    let why = format!("{trapped:?} {:?}", after.tasks.first().map(|t| &t.message));
    assert!(why.contains("başka bir dosyaya bağlı"), "{why}");
    assert!(after.guests.is_empty(), "konuk oluşmadı");
    assert!(std::fs::read_dir(cfg.disk_path(100).parent().unwrap()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".qcow2")), "disk kalmadı");
    std::fs::remove_file(cfg.cloud_dir().join("tuzak.qcow2")).unwrap();

    let id = state.create_guest(&cloud("debian-12.qcow2"), "admin").unwrap();
    assert_eq!(std::fs::read_to_string(cfg.disk_path(id)).unwrap(), "bulut-imaji", "disk imajdan kopyalanır");
    assert_eq!(std::fs::read_to_string(format!("{}.size", cfg.disk_path(id).display())).unwrap().trim(), "20G", "ve istenen boyuta büyütülür");
    assert!(!cfg.seed_dir(id).exists(), "cloud-init dosyaları başlatılırken yazılır");

    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    let user_data = std::fs::read_to_string(cfg.seed_dir(id).join("user-data")).unwrap();
    assert!(user_data.contains("hostname: bulut") && user_data.contains("- name: ali") && user_data.contains(KEY));
    assert!(std::fs::read_to_string(cfg.seed_dir(id).join("meta-data")).unwrap().contains(&format!("instance-id: redox-{id}")));

    // Klon kendi kimliğiyle ayrı bir cloud-init kimliği alır.
    state.run_action(id, Action::Stop, "admin").unwrap();
    let copy = state.clone_guest(id, "kopya", "admin").unwrap();
    state.run_action(copy, Action::Start, "admin").unwrap();
    let meta = std::fs::read_to_string(cfg.seed_dir(copy).join("meta-data")).unwrap();
    assert_eq!(meta, format!("instance-id: redox-{copy}\nlocal-hostname: kopya\n"));
    state.run_action(copy, Action::Stop, "admin").unwrap();

    state.delete_guest(id, "admin").unwrap();
    assert!(!cfg.seed_dir(id).exists() && !cfg.disk_path(id).exists(), "silme cloud-init dosyalarını da kaldırır");
    state.delete_guest(copy, "admin").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
