//! Yaşam döngüsü, hata iletileri, disk işlemleri, ekran görüntüsü.

use super::*;

#[test]
pub(super) fn full_lifecycle_against_fake_qemu() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");

    assert_eq!(status(&d, &g), GuestStatus::Stopped, "hiç başlamamış konuk durmuş görünür");
    assert!(d.apply(&g, Action::Start).unwrap_err().contains("disk imajı bulunamadı"));
    assert!(d.apply(&g, Action::Pause).unwrap_err().contains("konuk çalışmıyor"));

    d.create(&g).unwrap();
    assert!(cfg.disk_path(g.vmid).is_file());
    assert!(d.create(&g).unwrap_err().contains("zaten var"), "var olan diskin üzerine yazılmaz");

    d.apply(&g, Action::Start).unwrap();
    assert!(cfg.pid_path(g.vmid).is_file() && cfg.qmp_path(g.vmid).exists());
    let first = d.probe(&g).unwrap();
    assert_eq!(first.status, GuestStatus::Running);
    assert!(first.mem_used_mb.is_some(), "bellek /proc üzerinden okunmalı");
    assert_eq!(first.cpu_pct, None, "ilk örnekte fark hesaplanamaz");
    assert!(d.probe(&g).unwrap().cpu_pct.is_some(), "ikinci örnekte CPU yüzdesi gelir");
    assert!(d.apply(&g, Action::Start).unwrap_err().contains("zaten çalışıyor"));

    d.apply(&g, Action::Pause).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Paused, "QMP olayları atlanıp yanıt okunmalı");
    d.apply(&g, Action::Resume).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Running);
    d.apply(&g, Action::Reboot).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Running);

    d.apply(&g, Action::Stop).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    assert!(!cfg.pid_path(g.vmid).exists() && !cfg.qmp_path(g.vmid).exists(), "kalıntı dosya bırakılmaz");
    d.apply(&g, Action::Stop).unwrap();

    // Yeniden başlatılabilir, ardından silinir.
    d.apply(&g, Action::Start).unwrap();
    d.apply(&g, Action::Stop).unwrap();
    d.destroy(&g).unwrap();
    assert!(!cfg.disk_path(g.vmid).exists());
    d.destroy(&g).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn stop_forces_quit_when_guest_ignores_powerdown() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    cfg.shutdown_timeout = Duration::from_millis(600);
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "inatci");
    d.create(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();

    let started = std::time::Instant::now();
    d.apply(&g, Action::Stop).unwrap();
    let took = started.elapsed();
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    assert!(took >= Duration::from_millis(600), "önce nazikçe beklenmeli: {took:?}");
    assert!(took < Duration::from_secs(8), "süre dolunca zorla kapatılmalı: {took:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn failures_surface_the_tool_output() {
    let (mut cfg, dir) = config();
    cfg.qemu_bin = script(&dir, "bozuk-qemu", "echo 'ilk satır' >&2; echo 'qemu: KVM yok' >&2; exit 1");
    cfg.qemu_img_bin = script(&dir, "bozuk-img", "echo 'qemu-img: disk dolu' >&2; exit 1");
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");

    assert_eq!(d.create(&g).unwrap_err(), "disk imajı oluşturulamadı: qemu-img: disk dolu");
    std::fs::write(cfg.disk_path(g.vmid), b"").unwrap();
    assert_eq!(d.apply(&g, Action::Start).unwrap_err(), "QEMU başlatılamadı: qemu: KVM yok");
    assert_eq!(status(&d, &g), GuestStatus::Stopped);

    cfg.qemu_bin = "/yok/qemu".into();
    let missing = QemuDriver::new(cfg).unwrap();
    assert!(missing.apply(&g, Action::Start).unwrap_err().contains("çalıştırılamadı"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn stale_pidfile_is_not_mistaken_for_a_running_guest() {
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    // Yaşayan ama bu konuğa ait olmayan bir süreç (testin kendisi): PID yeniden kullanımı.
    std::fs::write(cfg.pid_path(g.vmid), std::process::id().to_string()).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    for junk in ["", "abc", "999999999"] {
        std::fs::write(cfg.pid_path(g.vmid), junk).unwrap();
        assert_eq!(status(&d, &g), GuestStatus::Stopped, "{junk:?}");
    }
    assert!(d.apply(&g, Action::Reboot).unwrap_err().contains("konuk çalışmıyor"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn server_state_drives_real_guests_end_to_end() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, Some(cfg.data_dir.join("state.json")), auth);

    let ct = GuestSpec { kind: GuestKind::Ct, ..spec("kutu", &node) };
    assert!(matches!(state.create_guest(&ct, "admin"), Err(Error::InvalidSpec(_))), "konteyner bu sürücüde yok");

    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    assert_eq!(id, 100);
    assert!(cfg.disk_path(id).is_file(), "oluşturma disk imajını da yaratmalı");

    let task = state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(state.snapshot().task(task).unwrap().status, TaskStatus::Ok);
    state.refresh(None, None);
    let g = state.snapshot().guest(id).unwrap().clone();
    assert_eq!(g.status, GuestStatus::Running);
    assert!(g.mem_used_mb > 0 && g.mem_used_mb <= g.mem_total_mb, "bellek gerçek süreçten: {}", g.mem_used_mb);
    assert_eq!(state.delete_guest(id, "admin"), Err(Error::NotStopped(id)));

    // Konuk "kendi kendine" kapanırsa (süreç ölür) bir sonraki yenilemede fark edilir.
    let pid = std::fs::read_to_string(cfg.pid_path(id)).unwrap();
    std::process::Command::new("kill").args(["-9", pid.trim()]).status().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    state.refresh(None, None);
    assert_eq!(state.snapshot().guest(id).unwrap().status, GuestStatus::Stopped);

    // Durum diske yazıldı; sunucu yeniden başlasa da konuk kaydı durur.
    let saved = redox_server::state::load_state(&cfg.data_dir.join("state.json")).unwrap().unwrap();
    assert_eq!(saved.guest(id).unwrap().status, GuestStatus::Stopped);

    state.run_action(id, Action::Start, "admin").unwrap();
    state.run_action(id, Action::Stop, "admin").unwrap();
    state.delete_guest(id, "admin").unwrap();
    assert!(!cfg.disk_path(id).exists() && state.snapshot().guests.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn failed_start_is_reported_as_a_failed_task() {
    let (mut cfg, dir) = config();
    cfg.qemu_bin = script(&dir, "bozuk-qemu", "echo 'qemu: bellek ayrılamadı' >&2; exit 1");
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg).unwrap()), Duration::ZERO, None, auth);
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    let task = state.run_action(id, Action::Start, "admin").unwrap();
    let c = state.snapshot();
    let t = c.task(task).unwrap();
    assert_eq!((t.status, t.message.as_deref()), (TaskStatus::Failed, Some("QEMU başlatılamadı: qemu: bellek ayrılamadı")));
    assert_eq!((c.guest(id).unwrap().status, c.guest(id).unwrap().lock), (GuestStatus::Stopped, None));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn disk_operations_run_qemu_img() {
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    d.create(&g).unwrap();
    let disk = cfg.disk_path(g.vmid);
    let snaps = || std::fs::read_to_string(format!("{}.snaps", disk.display())).unwrap_or_default();

    d.snapshot(&g, SnapshotOp::Create("temiz")).unwrap();
    d.snapshot(&g, SnapshotOp::Create("ikinci")).unwrap();
    assert_eq!(snaps(), "temiz\nikinci\n");
    d.snapshot(&g, SnapshotOp::Rollback("temiz")).unwrap();
    assert_eq!(d.snapshot(&g, SnapshotOp::Rollback("yok")).unwrap_err(), "anlık görüntü işlemi başarısız: qemu-img: snapshot 'yok' yok");
    d.snapshot(&g, SnapshotOp::Delete("temiz")).unwrap();
    assert_eq!(snaps(), "ikinci\n");

    d.resize_disk(&g, 20).unwrap();
    assert_eq!(std::fs::read_to_string(format!("{}.size", disk.display())).unwrap().trim(), "20G");

    // Yedek: disk içeriği kopyalanır; geri yükleme diski yedekteki haline döndürür.
    assert_eq!(d.backup(&g, "vm-100-1").unwrap().size_mb, 1, "boyut MB'a yukarı yuvarlanır");
    assert!(cfg.backups_dir().join("vm-100-1.qcow2").is_file());
    assert!(d.backup(&g, "vm-100-1").unwrap_err().contains("zaten var"), "var olan yedeğin üzerine yazılmaz");
    std::fs::write(&disk, b"bozuldu").unwrap();
    d.restore(&g, "vm-100-1", &[]).unwrap();
    assert_eq!(std::fs::read(&disk).unwrap(), b"disk");
    assert!(!disk.with_extension("qcow2.restore").exists(), "ara dosya bırakılmaz");
    d.delete_backup("vm-100-1").unwrap();
    assert!(!cfg.backups_dir().join("vm-100-1.qcow2").exists());

    // Klon: hedef diski kaynak içeriğiyle oluşturur, var olanın üzerine yazmaz.
    let mut copy = g.clone();
    copy.vmid = 101;
    d.clone_disk(&g, &copy).unwrap();
    assert_eq!(std::fs::read(cfg.disk_path(101)).unwrap(), b"disk");
    assert!(d.clone_disk(&g, &copy).unwrap_err().contains("zaten var"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn console_screenshot_and_keys() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    d.create(&g).unwrap();
    assert!(d.screenshot(&g).unwrap_err().contains("konuk çalışmıyor"));
    assert!(d.send_keys(&g, &[vec!["a".into()]]).unwrap_err().contains("konuk çalışmıyor"));

    d.apply(&g, Action::Start).unwrap();
    // Sahte QEMU "format" parametresini reddeder: sürücü PPM'e düşüp PNG'ye çevirmeli.
    let png = d.screenshot(&g).unwrap();
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    assert_eq!(&png[16..24], &[0, 0, 0, 4, 0, 0, 0, 2], "4x2 ekran");
    assert!(std::fs::read_dir(cfg.run_dir()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".screen")), "döküm dosyası silinir");

    let chords = redox_server::console::text_to_chords("Ab!\n").unwrap();
    d.send_keys(&g, &chords).unwrap();
    d.send_keys(&g, &[redox_server::console::special_key("ctrl-alt-del").unwrap()]).unwrap();
    let sent = std::fs::read_to_string(format!("{}.keys", cfg.qmp_path(g.vmid).display())).unwrap();
    assert_eq!(sent, "shift+a\nb\nshift+1\nret\nctrl+alt+delete\n");
    let too_many = vec![vec!["a".to_string()]; redox_server::console::MAX_CHORDS + 1];
    assert!(d.send_keys(&g, &too_many).unwrap_err().contains("en fazla"));

    d.apply(&g, Action::Stop).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
