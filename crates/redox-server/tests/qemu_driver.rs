//! QEMU sürücüsünün Linux üzerindeki gerçek süreç ve soket davranışı.
//! Gerçek QEMU yerine `fixtures/fake-qemu.py` kullanılır (python3 gerekir).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use redox_core::{Action, Cluster, Error, Guest, GuestKind, GuestSpec, GuestStatus, Hypervisor, SnapshotOp, TaskStatus};
use redox_server::auth::{Auth, Cost, UserFile};
use redox_server::host;
use redox_server::qemu::{QemuConfig, QemuDriver};
use redox_server::AppState;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Sahte qemu-img: dosyaları gerçekten oluşturur/kopyalar, anlık görüntüleri yan dosyada tutar.
///   create -f qcow2 YOL BOYUT | snapshot -c|-a|-d AD YOL | resize YOL BOYUT | convert [...] KAYNAK HEDEF | check [...] YOL
const FAKE_IMG: &str = r#"case "$1" in
  create) if [ "$4" = "-F" ]; then printf 'bagli:%s' "$7" > "$8"; else printf 'disk' > "$4"; fi ;;
  snapshot)
    case "$2" in
      -c) echo "$3" >> "$4.snaps" ;;
      -a) grep -qx "$3" "$4.snaps" || { echo "qemu-img: snapshot '$3' yok" >&2; exit 1; } ;;
      -d) grep -vx "$3" "$4.snaps" > "$4.tmp"; mv "$4.tmp" "$4.snaps" ;;
    esac ;;
  resize) echo "$3" > "$2.size" ;;
  convert) for a; do src=$dst; dst=$a; done; cp "$src" "$dst" ;;
  check) for a; do f=$a; done; if grep -q bozuk "$f"; then echo "qemu-img: ERROR küme bozuk" >&2; exit 2; fi ;;
  info) for a; do f=$a; done; if grep -q arkalikli "$f"; then echo '{"format": "qcow2", "backing-filename": "/etc/shadow"}'; else echo '{"format": "qcow2"}'; fi ;;
  *) echo "qemu-img: bilinmeyen komut $1" >&2; exit 1 ;;
esac"#;

fn python_available() -> bool {
    let ok = std::process::Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("ATLANDI: python3 yok; sahte QEMU çalıştırılamıyor");
    }
    ok
}


/// Betiği bir alt süreçte yazar. Yazma tanıtıcısı bu süreçte hiç açılmadığı için, koşut çalışan bir
/// testin çatalladığı süreç onu devralıp betiğin çalıştırılmasını "dosya meşgul" hatasıyla engelleyemez.
fn write_script(path: &Path, content: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$0" && chmod 755 "$0""#)
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(content.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
}

fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    write_script(&path, &format!("#!/bin/sh\n{body}\n"));
    path.display().to_string()
}

/// Kısa yollu (soket sınırı) geçici veri dizini ve sahte ikililerle yapılandırma.
fn config() -> (QemuConfig, PathBuf) {
    let dir = PathBuf::from(format!("/tmp/rdx-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-qemu.py");
    let mut cfg = QemuConfig::new(dir.join("data"));
    cfg.qemu_bin = script(&dir, "qemu", &format!("exec python3 '{}' \"$@\"", fake.display()));
    // qemu-img create -f qcow2 YOL BOYUT  →  $4 = YOL
    cfg.qemu_img_bin = script(&dir, "qemu-img", FAKE_IMG);
    cfg.shutdown_timeout = Duration::from_secs(10);
    (cfg, dir)
}

fn spec(name: &str, node: &str) -> GuestSpec {
    GuestSpec {
        vmid: None,
        name: name.into(),
        kind: GuestKind::Vm,
        node: node.into(),
        cores: 1,
        memory_mb: 256,
        disk_gb: 1,
        storage: host::LOCAL_STORAGE.into(),
        os: "linux".into(),
        bridge: "nat".into(),
        onboot: false,
        iso: None,
        hw: Default::default(),
    }
}

fn local_cluster(cfg: &QemuConfig) -> Cluster {
    let node = host::local_node();
    let mut storage = host::local_storage(&node.id, &cfg.data_dir);
    storage.total_gb = storage.total_gb.max(storage.used_gb + 100);
    Cluster::local(node, storage)
}

fn guest(cfg: &QemuConfig, name: &str) -> Guest {
    let mut c = local_cluster(cfg);
    let node = c.nodes[0].id.clone();
    let id = c.create_guest(&spec(name, &node), "u", 0).unwrap();
    c.guest(id).unwrap().clone()
}

fn status(d: &QemuDriver, g: &Guest) -> GuestStatus {
    d.probe(g).expect("gerçek sürücü her zaman gözlem döndürür").status
}

#[test]
fn full_lifecycle_against_fake_qemu() {
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
fn stop_forces_quit_when_guest_ignores_powerdown() {
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
fn failures_surface_the_tool_output() {
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
fn stale_pidfile_is_not_mistaken_for_a_running_guest() {
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
fn server_state_drives_real_guests_end_to_end() {
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
fn failed_start_is_reported_as_a_failed_task() {
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
fn disk_operations_run_qemu_img() {
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
fn console_screenshot_and_keys() {
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

#[test]
fn snapshots_and_backups_work_on_a_running_guest() {
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
fn live_operations_through_server_state() {
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
fn cloud_image_guests_are_copied_resized_and_seeded() {
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

#[test]
fn backups_are_copied_offsite_and_pulled_back_for_restore() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, auth);
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();

    // Hedef yokken: yedek yalnızca yerelde, elle gönderme reddedilir.
    let first = state.backup_create(id, "admin").unwrap();
    assert!(!state.snapshot().backup(&first).unwrap().offsite);
    assert!(matches!(state.backup_push(&first, "admin"), Err(Error::InvalidSpec(m)) if m.contains("yapılandırılmamış")));

    let remote = dir.join("uzak");
    std::fs::create_dir_all(&remote).unwrap();
    let offsite = |pass: &str| redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\n{pass}", remote.display())).unwrap();
    state.set_offsite(offsite("passphrase = dogru-at-pil-zimba\n"));

    // Eski yedek elle gönderilir.
    state.backup_push(&first, "admin").unwrap();
    assert!(state.snapshot().backup(&first).unwrap().offsite);
    assert!(remote.join(format!("{first}.qcow2.enc")).is_file());
    assert!(state.snapshot().tasks[0].command.starts_with("backup push") && state.snapshot().tasks[0].status == TaskStatus::Ok);

    // Yeni yedek kendiliğinden kopyalanır (aynı saniyede ikinci yedek alınamadığı için bekle).
    std::thread::sleep(Duration::from_millis(1100));
    let second = state.backup_create(id, "admin").unwrap();
    assert!(state.snapshot().backup(&second).unwrap().offsite);
    let copy = remote.join(format!("{second}.qcow2.enc"));
    assert_ne!(std::fs::read(&copy).unwrap(), b"disk", "hedefteki kopya şifrelidir");

    // Yerel dosya kaybolursa geri yükleme dış kopyayı indirip çözer.
    let local = cfg.backups_dir().join(format!("{second}.qcow2"));
    std::fs::remove_file(&local).unwrap();
    std::fs::write(cfg.disk_path(id), "bozuk").unwrap();
    state.backup_restore(&second, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert_eq!(std::fs::read_to_string(cfg.disk_path(id)).unwrap(), "disk");
    assert!(local.is_file());

    // Silme dış kopyayı da kaldırır.
    state.backup_delete(&second, "admin").unwrap();
    assert!(!copy.exists() && !local.exists());

    // Hedef erişilemezken yerel yedek yine alınır; başarısız kopya günlükte görünür ve sonra yeniden denenebilir.
    std::fs::remove_dir_all(&remote).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let third = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    assert!(!c.backup(&third).unwrap().offsite && cfg.backups_dir().join(format!("{third}.qcow2")).is_file());
    let failed = c.tasks.iter().find(|t| t.command == format!("backup push {third}")).expect("başarısız kopya kaydı");
    assert_eq!(failed.status, TaskStatus::Failed);
    assert!(failed.message.as_deref().unwrap().contains("bağlı değil"));
    assert!(c.tasks.iter().any(|t| t.command == format!("vm backup {id}") && t.status == TaskStatus::Ok), "yedeğin kendisi başarılı");
    std::fs::create_dir_all(&remote).unwrap();
    state.backup_push(&third, "admin").unwrap();
    assert!(state.snapshot().backup(&third).unwrap().offsite);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Sahte swtpm: soket dosyasını oluşturur, komut satırında soket yolu görünen bir süreç bırakır.
const FAKE_SWTPM: &str = r#"[ "$1" = "--version" ] && { echo "TPM emulator version 0.0"; exit 0; }
SOCK=; PID=
for a; do
  case "$a" in
    type=unixio,path=*) SOCK=${a#type=unixio,path=} ;;
    file=*) PID=${a#file=} ;;
  esac
done
[ -e "$(dirname "$0")/swtpm-fail" ] && { echo "swtpm: durum dizini açılamadı" >&2; exit 1; }
: > "$SOCK"
sh -c "while :; do sleep 1; done # $SOCK" >/dev/null 2>&1 &
echo $! > "$PID""#;

#[test]
fn uefi_and_tpm_guests_prepare_firmware_and_emulator() {
    if !python_available() {
        return;
    }
    use redox_core::hw::Firmware;
    let (mut cfg, dir) = config();
    cfg.ovmf_dir = dir.join("ovmf");
    cfg.swtpm_bin = script(&dir, "swtpm", FAKE_SWTPM);

    // Paketler kurulu değilken: açık bir hata.
    let bare = AppState::new(local_cluster(&cfg), Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let node = bare.snapshot().nodes[0].id.clone();
    let uefi = |tpm: bool, secure: bool| GuestSpec {
        hw: redox_core::Hardware { firmware: Firmware::Uefi, secure_boot: secure, tpm, ..Default::default() },
        ..spec("win", &node)
    };
    assert!(matches!(bare.create_guest(&uefi(false, false), "admin"), Err(Error::InvalidSpec(m)) if m.contains("apt install ovmf")));

    std::fs::create_dir_all(&cfg.ovmf_dir).unwrap();
    for name in ["OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd", "OVMF_CODE_4M.secboot.fd"] {
        std::fs::write(cfg.ovmf_dir.join(name), name).unwrap();
    }
    std::fs::write(cfg.ovmf_dir.join("OVMF_VARS_4M.ms.fd"), "ms-anahtarli-kalip").unwrap();
    let driver = QemuDriver::new(cfg.clone()).unwrap();
    assert!(driver.uefi() && driver.tpm());
    let state = AppState::new(local_cluster(&cfg), Box::new(driver), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));

    let id = state.create_guest(&uefi(true, true), "admin").unwrap();
    let vars = cfg.efivars_path(id, true);
    assert!(!vars.exists(), "değişken deposu ilk başlatmada oluşur");
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert_eq!(std::fs::read_to_string(&vars).unwrap(), "ms-anahtarli-kalip", "güvenli önyüklemede anahtarlı kalıp kopyalanır");
    assert!(cfg.tpm_dir(id).is_dir() && cfg.tpm_sock(id).exists());
    let tpm_pid: u32 = std::fs::read_to_string(cfg.tpm_pid(id)).unwrap().trim().parse().unwrap();
    assert!(Path::new(&format!("/proc/{tpm_pid}")).exists(), "öykünücü çalışıyor");

    // Durdurunca öykünücü de durur; değişken deposu ve TPM durumu kalır.
    std::fs::write(&vars, "konugun-yazdigi-degiskenler").unwrap();
    state.run_action(id, Action::Stop, "admin").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!Path::new(&format!("/proc/{tpm_pid}/cmdline")).exists() || std::fs::read(format!("/proc/{tpm_pid}/cmdline")).unwrap().is_empty(), "öykünücü durduruldu");
    assert!(!cfg.tpm_sock(id).exists() && cfg.tpm_dir(id).is_dir());

    // Yeniden başlatma var olan depoyu ezmez; kapalıyken anlık görüntü depoyu da kapsar.
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(std::fs::read_to_string(&vars).unwrap(), "konugun-yazdigi-degiskenler");
    state.run_action(id, Action::Stop, "admin").unwrap();
    state.snapshot_create(id, "temiz", "admin").unwrap();
    assert_eq!(std::fs::read_to_string(format!("{}.snaps", vars.display())).unwrap().trim(), "temiz");

    // Öykünücü başlamazsa konuk başlatılmaz ve neden görevde yazar.
    std::fs::write(dir.join("swtpm-fail"), "").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    let c = state.snapshot();
    assert_eq!(c.tasks[0].status, TaskStatus::Failed);
    assert!(c.tasks[0].message.as_deref().unwrap().contains("sanal TPM başlatılamadı: swtpm: durum dizini açılamadı"));
    assert_eq!(c.guest(id).unwrap().status, GuestStatus::Stopped);
    std::fs::remove_file(dir.join("swtpm-fail")).unwrap();

    // Donanım yazılımı yalnızca kapalıyken değişir; silme depoyu ve TPM durumunu da kaldırır.
    state.run_action(id, Action::Start, "admin").unwrap();
    let to_bios = redox_core::GuestPatch { firmware: Some(Firmware::Bios), secure_boot: Some(false), ..Default::default() };
    assert!(matches!(state.update_guest(id, &to_bios, "admin"), Err(Error::RequiresStopped { .. })));
    state.run_action(id, Action::Stop, "admin").unwrap();
    state.update_guest(id, &to_bios, "admin").unwrap();
    assert_eq!(state.snapshot().guest(id).unwrap().hw.firmware, Firmware::Bios);
    state.delete_guest(id, "admin").unwrap();
    assert!(!vars.exists() && !cfg.tpm_dir(id).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn extra_disks_follow_the_guest_through_its_life() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&spec("veri", &node), "admin").unwrap();

    assert_eq!(state.disk_add(id, 5, "admin").unwrap(), 1);
    assert_eq!(state.disk_add(id, 7, "admin").unwrap(), 2);
    let (d1, d2) = (cfg.extra_disk_path(id, 1), cfg.extra_disk_path(id, 2));
    assert!(d1.is_file() && d2.is_file());
    assert_eq!(state.snapshot().tasks[0].command, format!("vm disk add {id} 7G"));

    // Kapalıyken anlık görüntü bütün diskleri kapsar; klon hepsini kopyalar.
    state.snapshot_create(id, "tam", "admin").unwrap();
    for disk in [cfg.disk_path(id), d1.clone(), d2.clone()] {
        assert_eq!(std::fs::read_to_string(format!("{}.snaps", disk.display())).unwrap().trim(), "tam", "{}", disk.display());
    }
    std::fs::write(&d2, "veri-diski").unwrap();
    let copy = state.clone_guest(id, "kopya", "admin").unwrap();
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_path(copy, 2)).unwrap(), "veri-diski");
    assert_eq!(state.snapshot().guest(copy).unwrap().hw.disks.len(), 2);

    // Çalışırken disk takılır (hemen konuğa bağlanır) ama çıkarılmaz.
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    let hot = state.disk_add(id, 1, "admin").unwrap();
    assert!(cfg.extra_disk_path(id, hot).exists());
    let plugged = std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(id).display())).unwrap();
    assert!(plugged.contains(&format!("device_add virtio-blk-pci xdev{hot} hp{} xd{hot}", hot - 1)), "{plugged}");
    assert!(matches!(state.disk_remove(id, 1, "admin"), Err(Error::RequiresStopped { .. })));
    state.run_action(id, Action::Stop, "admin").unwrap();
    state.disk_remove(id, hot, "admin").unwrap();
    assert!(!cfg.extra_disk_path(id, hot).exists());

    state.disk_remove(id, 1, "admin").unwrap();
    assert!(!d1.exists() && d2.is_file());
    // Dosya oluşturulamazsa kayıt ve ayrılan yer geri alınır.
    std::fs::create_dir(&d1).unwrap();
    assert!(matches!(state.disk_add(id, 3, "admin"), Err(Error::Driver(m)) if m.contains("zaten var")));
    assert_eq!(state.snapshot().guest(id).unwrap().hw.disks.len(), 1);
    std::fs::remove_dir(&d1).unwrap();

    state.delete_guest(id, "admin").unwrap();
    assert!(!d2.exists(), "konuk silinince ek diskleri de silinir");
    state.delete_guest(copy, "admin").unwrap();
    assert!(!cfg.extra_disk_path(copy, 2).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn backup_files_can_be_downloaded_by_admins() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "indirme-test-tokeni").unwrap();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    // Sahte qemu-img diski kopyalar: içerik büyük olsun ki parça parça akış sınansın.
    let content: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(cfg.disk_path(id), &content).unwrap();
    let backup = state.backup_create(id, "admin").unwrap();

    let app = redox_server::app(state.clone());
    let request = axum::http::Request::builder()
        .uri(format!("/api/v1/backups/{backup}/download"))
        .header("host", "redox.test")
        .header("authorization", "Bearer indirme-test-tokeni")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-length"], content.len().to_string().as_str());
    assert_eq!(response.headers()["content-disposition"], format!("attachment; filename=\"{backup}.qcow2\"").as_str());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(body.as_ref() == content.as_slice(), "indirilen dosya yedeğin aynısı olmalı");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_vm_is_copied_from_a_peer_server() {
    if !python_available() {
        return;
    }
    // Eş sunucu: sahte QEMU sürücüsü, yönetici ve izleyici token'ları, gerçek TCP kapısı.
    let (peer_cfg, peer_dir) = config();
    let peer_cluster = local_cluster(&peer_cfg);
    let node = peer_cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("admin", "t", "es-sunucu-yonetici-tokeni").unwrap();
    users.add_token("izleyici", "t", "es-sunucu-izleyici-tokeni").unwrap();
    let peer = AppState::new(peer_cluster, Box::new(QemuDriver::new(peer_cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let source = peer
        .create_guest(&GuestSpec { cores: 1, memory_mb: 512, disk_gb: 3, hw: redox_core::Hardware { firmware: redox_core::hw::Firmware::Bios, usb: vec!["046d:c52b".into()], ..Default::default() }, ..spec("eski-web", &node) }, "admin")
        .unwrap();
    std::fs::write(peer_cfg.disk_path(source), "es-sunucunun-diski").unwrap();
    peer.update_guest(source, &redox_core::GuestPatch { tags: Some(vec!["web".into()]), ..Default::default() }, "admin").unwrap();
    let running = peer.create_guest(&spec("calisan", &node), "admin").unwrap();
    peer.run_action(running, Action::Start, "admin").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer_app = redox_server::app(peer.clone());
    tokio::spawn(async move { axum::serve(listener, peer_app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });

    // Bu sunucu.
    let (cfg, dir) = config();
    let hub = AppState::new(local_cluster(&cfg), Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    hub.peers().configure(
        redox_server::peers::parse_file(&format!("eski http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\nsalt http://127.0.0.1:{port} es-sunucu-izleyici-tokeni\n")).unwrap(),
    );
    let import = |name: &'static str, vmid: u32| {
        let hub = hub.clone();
        async move { tokio::task::spawn_blocking(move || hub.import_from_peer(name, vmid, "admin")).await.unwrap() }
    };
    let last = |hub: &AppState| hub.snapshot().tasks[0].clone();

    assert!(matches!(import("yok", source).await, Err(Error::InvalidSpec(m)) if m.contains("tanımlı değil")));

    import("eski", source).await.unwrap();
    let task = last(&hub);
    assert_eq!((task.command.as_str(), task.status), (format!("vm import eski/{source}").as_str(), TaskStatus::Ok), "{:?}", task.message);
    let copy = hub.snapshot().guests[0].clone();
    assert_eq!((copy.name.as_str(), copy.cores, copy.mem_total_mb, copy.disk_gb, copy.status), ("eski-web", 1, 512, 3, GuestStatus::Stopped));
    assert_eq!(std::fs::read_to_string(cfg.disk_path(copy.vmid)).unwrap(), "es-sunucunun-diski", "disk eşteki yedeğin aynısı");
    assert_eq!(copy.tags, ["web", "tasindi-eski"], "etiketler gelir ve nereden geldiği işaretlenir");
    assert!(copy.hw.usb.is_empty() && copy.hw.image.is_none(), "sunucuya özgü aygıtlar ve geçici imaj kayıtta kalmaz");
    assert!(std::fs::read_dir(cfg.cloud_dir()).unwrap().next().is_none(), "geçici imaj silinir");
    // Kaynak yerinde durur; eşte geçici yedek kalmaz.
    assert!(peer.snapshot().guest(source).is_ok() && peer.snapshot().backups.is_empty());
    assert!(std::fs::read_dir(peer_cfg.backups_dir()).unwrap().next().is_none());

    // Çalışan konuk bellek durumuyla taşınır: eşte askıya alınır, diski ve belleği gelir, burada kaldığı yerden sürer.
    std::fs::write(peer_cfg.disk_path(running), "calisan-konugun-diski").unwrap();
    import("eski", running).await.unwrap();
    let task = last(&hub);
    assert_eq!((task.command.as_str(), task.status), (format!("vm import eski/{running}").as_str(), TaskStatus::Ok), "{:?}", task.message);
    let moved = hub.snapshot().guest(running).expect("kimlik boşsa aynı kimlikle gelir").clone();
    assert_eq!((moved.status, moved.hw.suspended, moved.name.as_str()), (GuestStatus::Running, false, "calisan"));
    assert_eq!(std::fs::read_to_string(cfg.disk_path(running)).unwrap(), "calisan-konugun-diski");
    let resumed = PathBuf::from(format!("{}.resumed", cfg.qmp_path(running).display()));
    assert!(resumed.exists(), "konuk sıfırdan açılmadı, bellek durumundan sürdü");
    assert!(!cfg.state_path(running).exists(), "kullanılan bellek durumu burada kalmaz");
    assert!(std::fs::read_dir(cfg.cloud_dir()).unwrap().next().is_none(), "geçici dosyalar silinir");
    // Kaynak askıda ve kapalı kalır; bellek durumu orada durur (geri dönüş yolu).
    let left = peer.snapshot().guest(running).unwrap().clone();
    assert_eq!((left.status, left.hw.suspended), (GuestStatus::Stopped, true));
    assert!(peer_cfg.state_path(running).exists() && peer.snapshot().backups.is_empty());
    hub.run_action(running, Action::Stop, "admin").unwrap();

    // Zaten askıda olan konuk da bellek durumuyla gelir; burada askıda bekler ve başlatılınca sürer.
    let parked = peer.create_guest(&spec("bekleyen", &node), "admin").unwrap();
    peer.run_action(parked, Action::Start, "admin").unwrap();
    peer.suspend(parked, "admin").unwrap();
    import("eski", parked).await.unwrap();
    assert_eq!(last(&hub).status, TaskStatus::Ok, "{:?}", last(&hub).message);
    let waiting = hub.snapshot().guest(parked).unwrap().clone();
    assert_eq!((waiting.status, waiting.hw.suspended), (GuestStatus::Stopped, true));
    assert!(cfg.state_path(parked).exists() && cfg.state_path(parked).with_extension("state.running").exists());
    hub.run_action(parked, Action::Start, "admin").unwrap();
    assert_eq!(hub.snapshot().guest(parked).unwrap().status, GuestStatus::Running);
    assert!(PathBuf::from(format!("{}.resumed", cfg.qmp_path(parked).display())).exists());
    hub.run_action(parked, Action::Stop, "admin").unwrap();

    // Askıya alınamayan konuk: burada hiçbir şey oluşmaz, konuk eşte çalışmaya devam eder.
    let stubborn = peer.create_guest(&spec("askisiz", &node), "admin").unwrap();
    peer.run_action(stubborn, Action::Start, "admin").unwrap();
    let before = hub.snapshot().guests.len();
    import("eski", stubborn).await.unwrap();
    let task = last(&hub);
    assert!(task.status == TaskStatus::Failed && task.message.as_deref().unwrap().contains("askıya alınamadı"), "{:?}", task.message);
    assert_eq!(hub.snapshot().guests.len(), before);
    assert_eq!(peer.snapshot().guest(stubborn).unwrap().status, GuestStatus::Running);
    peer.run_action(stubborn, Action::Stop, "admin").unwrap();

    // Bellek durumu yalnızca yöneticiye ve yalnızca askıdaki konuk için verilir.
    let fetch = |token: &'static str, vmid: u32| async move {
        let url = format!("http://127.0.0.1:{port}/api/v1/guests/{vmid}/state");
        tokio::task::spawn_blocking(move || match ureq::get(&url).set("Authorization", &format!("Bearer {token}")).call() {
            Ok(r) => (r.status(), r.header("x-redox-state-layout").map(str::to_string)),
            Err(ureq::Error::Status(code, _)) => (code, None),
            Err(e) => panic!("{e}"),
        })
        .await
        .unwrap()
    };
    assert_eq!(fetch("es-sunucu-yonetici-tokeni", running).await, (200, Some(redox_server::qemu::LAYOUT_CURRENT.to_string())));
    assert_eq!(fetch("es-sunucu-izleyici-tokeni", running).await.0, 403);
    assert_eq!(fetch("es-sunucu-yonetici-tokeni", source).await.0, 409, "askıda olmayan konuğun bellek durumu yok");
    assert_eq!(fetch("es-sunucu-yonetici-tokeni", 999_999).await.0, 404);

    // İzleyici token'ı ve olmayan konuk: hiçbir şey oluşmaz, neden görevde yazar.
    let settled = hub.snapshot().guests.len();
    for (peer_name, vmid, needle) in [("salt", source, "yönetici token'ı gerekir"), ("eski", 999_999, "404")] {
        import(peer_name, vmid).await.unwrap();
        let task = last(&hub);
        assert_eq!(task.status, TaskStatus::Failed, "{peer_name}/{vmid}");
        assert!(task.message.as_deref().unwrap().contains(needle), "{:?}", task.message);
    }
    assert_eq!(hub.snapshot().guests.len(), settled);
    assert!(peer.snapshot().backups.is_empty(), "başarısız denemeler eşte yedek bırakmaz");
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(peer_dir).unwrap();
}

/// Her parçası (4 MiB) farklı içerikli, sonunda kısa bir kuyruk olan disk.
fn patterned(chunks: usize) -> Vec<u8> {
    let size = redox_server::chunkstore::CHUNK_SIZE;
    let mut data: Vec<u8> = (0..chunks).flat_map(|i| std::iter::repeat_n(i as u8 + 1, size)).collect();
    data.extend_from_slice(b"kuyruk");
    data
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.path().is_dir() {
            found.extend(files_under(&entry.path()));
        } else {
            found.push(entry.path());
        }
    }
    found.sort();
    found
}

/// Yedek dizinindeki gizli (geçici) girdiler: iş bitince hiçbiri kalmamalı.
fn temporaries(cfg: &QemuConfig) -> Vec<String> {
    std::fs::read_dir(cfg.backups_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with('.'))
        .collect()
}

#[test]
fn incremental_backups_store_only_changes_and_restore_exactly() {
    let (mut cfg, dir) = config();
    cfg.incremental_backups = true;
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    d.create(&g).unwrap();
    let disk = cfg.disk_path(g.vmid);
    let chunks = || files_under(&cfg.backups_dir().join("chunks"));
    let chunk = redox_server::chunkstore::CHUNK_SIZE;

    let first = patterned(3);
    std::fs::write(&disk, &first).unwrap();
    let a = d.backup(&g, "vm-100-a").unwrap();
    assert!(a.new_mb.is_some(), "artımlı yedek eklenen miktarı bildirir");
    assert!(!cfg.backups_dir().join("vm-100-a.qcow2").exists(), "tek dosyalık kopya yazılmaz");
    assert!(cfg.backups_dir().join("vm-100-a.rdxi.json").is_file());
    assert_eq!(chunks().len(), 4);
    assert!(d.backup_present("vm-100-a") && !d.backup_present("vm-100-yok") && !d.backup_present("../x"));
    assert!(d.backup(&g, "vm-100-a").unwrap_err().contains("zaten var"));
    assert!(temporaries(&cfg).is_empty(), "geçici ham disk bırakılmaz: {:?}", temporaries(&cfg));

    // İkinci parçada birkaç bayt değişir: depoya yalnızca o parça eklenir.
    let mut second = first.clone();
    second[chunk..chunk + 5].copy_from_slice(b"YENI!");
    std::fs::write(&disk, &second).unwrap();
    let b = d.backup(&g, "vm-100-b").unwrap();
    assert_eq!(chunks().len(), 5, "yalnızca değişen parça eklenir");
    assert!(b.new_mb.unwrap() <= b.size_mb);
    // Değişmeyen diskin yedeği depoya hiçbir şey eklemez.
    assert_eq!(d.backup(&g, "vm-100-c").unwrap().new_mb, Some(0));
    assert_eq!(chunks().len(), 5);

    // Her yedek kendi anındaki diski bayt bayt geri verir.
    d.restore(&g, "vm-100-a", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == first, "ilk yedek ilk hali geri getirmeli");
    d.restore(&g, "vm-100-b", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == second, "ikinci yedek ikinci hali geri getirmeli");
    assert_eq!(d.verify_backup("vm-100-b", &[]).unwrap(), "4 parça doğrulandı");

    // En eski (temel) yedek silinir: sonrakiler etkilenmez.
    d.delete_backup("vm-100-a").unwrap();
    assert!(!d.backup_present("vm-100-a"));
    assert_eq!(chunks().len(), 4, "yalnızca silinen yedeğe özgü parça gider");
    std::fs::write(&disk, b"baska").unwrap();
    d.restore(&g, "vm-100-c", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == second);
    assert!(d.restore(&g, "vm-100-a", &[]).unwrap_err().contains("bulunamadı"));

    // Bozuk parça: doğrulama yakalar, geri yükleme reddedilir ve diske dokunulmaz.
    let victim = chunks().remove(0);
    let good = std::fs::read(&victim).unwrap();
    std::fs::write(&victim, b"bozuldu").unwrap();
    std::fs::write(&disk, b"canli-veri").unwrap();
    assert!(d.verify_backup("vm-100-b", &[]).unwrap_err().contains("parça bozuk"));
    assert!(d.restore(&g, "vm-100-b", &[]).unwrap_err().contains("parça bozuk"));
    assert_eq!(std::fs::read(&disk).unwrap(), b"canli-veri", "bozuk yedek diskin üzerine yazılmaz");
    assert_eq!(files_under(&cfg.images_dir()), vec![disk.clone()], "yarım geri yükleme dosyası bırakılmaz");
    assert!(d.export_backup("vm-100-b", 0).is_err());
    assert!(temporaries(&cfg).is_empty(), "başarısız dışa aktarma kalıntı bırakmaz");
    std::fs::write(&victim, &good).unwrap();

    // Dışa aktarma: tek dosyalık geçici kopya (sahte qemu-img içeriği aynen kopyalar).
    let (file, temporary) = d.export_backup("vm-100-b", 0).unwrap().unwrap();
    assert!(temporary && file.file_name().unwrap() == "vm-100-b.qcow2");
    assert!(std::fs::read(&file).unwrap() == second);
    redox_server::state::discard_export(&file);
    assert!(temporaries(&cfg).is_empty());

    d.delete_backup("vm-100-b").unwrap();
    d.delete_backup("vm-100-c").unwrap();
    assert!(chunks().is_empty(), "son yedek silinince depo boşalır");
    d.delete_backup("vm-100-c").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn incremental_and_full_backups_coexist_and_work_on_a_running_guest() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    let g = guest(&cfg, "deneme");
    let disk = cfg.disk_path(g.vmid);
    // Özellik açılmadan önce alınmış tam yedek.
    let old = QemuDriver::new(cfg.clone()).unwrap();
    old.create(&g).unwrap();
    std::fs::write(&disk, b"eski-hal").unwrap();
    assert_eq!(old.backup(&g, "vm-100-eski").unwrap().new_mb, None);
    // Yarım kalmış geçici dosyalar açılışta temizlenir.
    std::fs::write(cfg.backups_dir().join(".vm-100-x.raw.tmp"), b"x").unwrap();
    std::fs::create_dir_all(cfg.backups_dir().join(".export-1-1")).unwrap();
    std::fs::write(cfg.backups_dir().join(".export-1-1/vm-100-x.qcow2"), b"x").unwrap();

    cfg.incremental_backups = true;
    // Sunucu gerçek sürücüyü HostDriver üzerinden kullanır: yeni yöntemler iletilmeli.
    let d = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    assert!(temporaries(&cfg).is_empty(), "{:?}", temporaries(&cfg));
    assert!(d.backup_present("vm-100-eski"));
    assert_eq!(d.verify_backup("vm-100-eski", &[]).unwrap(), "imaj tutarlı");
    assert_eq!(d.export_backup("vm-100-eski", 0).unwrap().unwrap(), (cfg.backups_dir().join("vm-100-eski.qcow2"), false));
    assert!(d.verify_backup("ct-200-1", &[]).unwrap_err().contains("LXC kurulu değil"));
    assert!(d.verify_backup("vm-100-yok", &[]).unwrap_err().contains("bulunamadı"));

    // Çalışan konukta artımlı yedek: disk QEMU üzerinden ham olarak dökülür.
    let live = patterned(2);
    std::fs::write(&disk, &live).unwrap();
    d.apply(&g, Action::Start).unwrap();
    let size = d.backup(&g, "vm-100-canli").unwrap();
    assert!(size.new_mb.is_some() && d.backup_present("vm-100-canli"));
    assert!(!cfg.backups_dir().join("vm-100-canli.qcow2").exists());
    assert_eq!(d.verify_backup("vm-100-canli", &[]).unwrap(), "3 parça doğrulandı");
    let (file, temporary) = d.export_backup("vm-100-canli", 0).unwrap().unwrap();
    assert!(temporary && std::fs::read(&file).unwrap() == live);
    redox_server::state::discard_export(&file);
    // QEMU işi hatayla bitirirse yedek kaydedilmez, kalıntı kalmaz.
    std::fs::write(cfg.backups_dir().join(".vm-100-dolu.d0.raw.tmp.fail"), b"").unwrap();
    assert_eq!(d.backup(&g, "vm-100-dolu").unwrap_err(), "yedekleme başarısız: No space left on device");
    std::fs::remove_file(cfg.backups_dir().join(".vm-100-dolu.d0.raw.tmp.fail")).unwrap();
    assert!(!d.backup_present("vm-100-dolu") && temporaries(&cfg).is_empty());
    d.apply(&g, Action::Stop).unwrap();

    // İki tür de geri yüklenir.
    d.restore(&g, "vm-100-eski", &[]).unwrap();
    assert_eq!(std::fs::read(&disk).unwrap(), b"eski-hal");
    d.restore(&g, "vm-100-canli", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == live);
    // Bozulmuş tam yedek de yakalanır.
    std::fs::write(cfg.backups_dir().join("vm-100-eski.qcow2"), b"bozuk").unwrap();
    assert!(d.verify_backup("vm-100-eski", &[]).unwrap_err().starts_with("yedek bozuk"));
    d.delete_backup("vm-100-eski").unwrap();
    d.delete_backup("vm-100-canli").unwrap();
    assert!(files_under(&cfg.backups_dir()).is_empty(), "{:?}", files_under(&cfg.backups_dir()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn incremental_backups_reach_offsite_download_and_the_backups_page() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (mut cfg, dir) = config();
    cfg.incremental_backups = true;
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "artimli-test-tokeni").unwrap();
    let driver = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    let content = patterned(2);
    std::fs::write(cfg.disk_path(id), &content).unwrap();
    let remote = dir.join("uzak");
    std::fs::create_dir_all(&remote).unwrap();
    state.set_offsite(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\npassphrase = dogru-at-pil-zimba\n", remote.display())).unwrap());
    let remote_chunks = || files_under(&remote.join("chunks")).len();

    // Yedek artımlı alınır; dış hedefe de parça parça (şifreli) gider, tek dosyalık kopya üretilmez.
    let backup = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    assert!(c.tasks.iter().all(|t| t.status == TaskStatus::Ok), "{:?}", c.tasks);
    let record = c.backup(&backup).unwrap();
    assert!(record.new_mb.is_some() && record.offsite);
    assert!(remote.join(format!("{backup}.rdxi.json.enc")).is_file() && !remote.join(format!("{backup}.qcow2.enc")).exists());
    assert_eq!(remote_chunks(), 3);
    assert!(files_under(&remote).iter().all(|f| f.extension().is_some_and(|e| e == "enc")), "hedefteki her dosya şifrelidir");
    assert!(temporaries(&cfg).is_empty(), "{:?}", temporaries(&cfg));

    // İkinci yedek: tek parça değişti, hedefe yalnızca o gider.
    let first_content = content;
    let mut content = first_content.clone();
    content[5..10].copy_from_slice(b"YENI!");
    std::fs::write(cfg.disk_path(id), &content).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let first = backup;
    let backup = state.backup_create(id, "admin").unwrap();
    assert!(state.snapshot().backup(&backup).unwrap().offsite);
    assert_eq!(remote_chunks(), 4, "yalnızca değişen parça gönderilir");
    // Elle yeniden gönderme hedefte kaybolan parçayı tamamlar.
    std::fs::remove_file(files_under(&remote.join("chunks")).remove(0)).unwrap();
    state.backup_push(&first, "admin").unwrap();
    state.backup_push(&backup, "admin").unwrap();
    assert_eq!(remote_chunks(), 4);
    assert!(state.snapshot().tasks.iter().take(2).all(|t| t.command.starts_with("backup push") && t.status == TaskStatus::Ok));

    // Doğrulama görev günlüğüne başarıyla yazılır.
    state.backup_verify(&backup, "admin").unwrap();
    let last = state.snapshot().tasks[0].clone();
    assert_eq!((last.command, last.status), (format!("backup verify {backup}"), TaskStatus::Ok));

    // İndirme: artımlı yedek tek dosya olarak akar, geçici kopya kalmaz.
    let app = redox_server::app(state.clone());
    let fetch = |uri: String| {
        let request = axum::http::Request::builder().uri(uri).header("host", "redox.test").header("authorization", "Bearer artimli-test-tokeni").body(axum::body::Body::empty()).unwrap();
        app.clone().oneshot(request)
    };
    let response = fetch(format!("/api/v1/backups/{backup}/download")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-disposition"], format!("attachment; filename=\"{backup}.qcow2\"").as_str());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(body.as_ref() == content.as_slice(), "indirilen dosya diskin aynısı olmalı");
    assert!(temporaries(&cfg).is_empty());

    // Yedekler sayfası gerçek sürücüde etiketi ve doğrulama düğmesini gösterir.
    let response = fetch(format!("/guests/{id}?tab=yedekler")).await.unwrap();
    let html = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(html.contains("artımlı") && html.contains(&format!(r#"action="/backups/{backup}/verify""#)), "{html}");

    // Yerel depo tümüyle kaybolursa geri yükleme dış kopyanın parçalarını indirir (ikisi için de).
    std::fs::remove_dir_all(cfg.backups_dir()).unwrap();
    std::fs::write(cfg.disk_path(id), b"bozuk").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == content);
    state.backup_restore(&first, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == first_content);
    assert!(temporaries(&cfg).is_empty(), "{:?}", temporaries(&cfg));

    // Hedefte bozulan parça geri yüklenmez: konuğun diski olduğu gibi kalır.
    std::fs::remove_dir_all(cfg.backups_dir()).unwrap();
    let victim = files_under(&remote.join("chunks")).remove(0);
    let good = std::fs::read(&victim).unwrap();
    let mut bad = good.clone();
    let middle = bad.len() / 2;
    bad[middle] ^= 1;
    std::fs::write(&victim, &bad).unwrap();
    let _ = state.backup_restore(&first, "admin");
    let failed = state.snapshot().tasks[0].clone();
    assert_eq!(failed.status, TaskStatus::Failed, "{failed:?}");
    assert!(failed.message.as_deref().unwrap().contains("bozulmuş"), "{failed:?}");
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == first_content, "bozuk dış kopya diskin üzerine yazılmaz");
    std::fs::write(&victim, &good).unwrap();

    // İlk yedek silinir: hedefte yalnızca ona özgü parça gider, ikincisi hâlâ geri yüklenir.
    state.backup_delete(&first, "admin").unwrap();
    assert_eq!(remote_chunks(), 3);
    assert!(!remote.join(format!("{first}.rdxi.json.enc")).exists());
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == content);

    // Son yedek de silinince hedefte hiçbir dosya kalmaz.
    state.backup_delete(&backup, "admin").unwrap();
    // Hedefte yalnızca depo işareti kalır.
    let left: Vec<String> = files_under(&remote).iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert!(left == ["redox-store.json.enc"] && state.snapshot().backups.is_empty(), "{left:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Ek diskli konuk: diskler sürücüde gerçekten oluşturulur.
fn guest_with_disks(cfg: &QemuConfig, d: &QemuDriver, ids: &[u8]) -> Guest {
    let mut g = guest(cfg, "deneme");
    d.create(&g).unwrap();
    for id in ids {
        d.add_disk(&g, *id, 1).unwrap();
        g.hw.disks.push(redox_core::hw::ExtraDisk { id: *id, size_gb: 1, serial: *id as u64 });
    }
    g
}

#[test]
fn extra_disks_are_backed_up_and_restored_together() {
    if !python_available() {
        return;
    }
    for incremental in [false, true] {
        let (mut cfg, dir) = config();
        cfg.incremental_backups = incremental;
        let d = QemuDriver::new(cfg.clone()).unwrap();
        let g = guest_with_disks(&cfg, &d, &[1, 2]);
        let disks = [cfg.disk_path(g.vmid), cfg.extra_disk_path(g.vmid, 1), cfg.extra_disk_path(g.vmid, 2)];
        let fill = |tag: &str| {
            for (i, disk) in disks.iter().enumerate() {
                std::fs::write(disk, format!("disk{i}-{tag}")).unwrap();
            }
        };
        let contents = || disks.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect::<Vec<_>>();
        let part = |id: &str, n: u8| cfg.backups_dir().join(if n == 0 { format!("{id}.qcow2") } else { format!("{id}.disk{n}.qcow2") });

        // Kapalı konuk: üç disk birlikte yedeklenir.
        fill("eski");
        d.backup(&g, "vm-100-a").unwrap();
        assert_eq!(part("vm-100-a", 0).exists() && part("vm-100-a", 1).exists() && part("vm-100-a", 2).exists(), !incremental);
        assert!(d.verify_backup("vm-100-a", &[1, 2]).is_ok(), "{incremental}");
        assert!(d.verify_backup("vm-100-a", &[3]).is_err(), "yedekte olmayan disk doğrulanamaz");

        // Çalışan konuk: bütün diskler tek QMP işlemiyle, aynı anda kopyalanır.
        fill("yeni");
        d.apply(&g, Action::Start).unwrap();
        d.backup(&g, "vm-100-b").unwrap();
        assert!(d.verify_backup("vm-100-b", &[1, 2]).is_ok());
        // Bir diskin kopyası başarısız olursa yedek alınmaz, diğer kopyalar durdurulur, kalıntı kalmaz.
        let marker = if incremental { cfg.backups_dir().join(".vm-100-dolu.d2.raw.tmp.fail") } else { cfg.backups_dir().join(".vm-100-dolu.disk2.qcow2.tmp.fail") };
        std::fs::write(&marker, b"").unwrap();
        assert_eq!(d.backup(&g, "vm-100-dolu").unwrap_err(), "yedekleme başarısız: No space left on device");
        std::fs::remove_file(&marker).unwrap();
        assert!(!d.backup_present("vm-100-dolu") && temporaries(&cfg).is_empty());
        assert!(!part("vm-100-dolu", 0).exists() && !part("vm-100-dolu", 1).exists() && !part("vm-100-dolu", 2).exists());
        // Başarısız denemeden sonra yeni yedek alınabilir (QMP bağlantısı temiz kapanmış olmalı).
        d.backup(&g, "vm-100-c").unwrap();
        d.apply(&g, Action::Stop).unwrap();

        // Geri yükleme: istenen bütün diskler birlikte döner.
        fill("bozuk");
        d.restore(&g, "vm-100-a", &[1, 2]).unwrap();
        assert_eq!(contents(), ["disk0-eski", "disk1-eski", "disk2-eski"]);
        // Yalnızca istenen ek diskler: diğerine dokunulmaz.
        d.restore(&g, "vm-100-b", &[2]).unwrap();
        assert_eq!(contents(), ["disk0-yeni", "disk1-eski", "disk2-yeni"]);
        // Yedekte olmayan bir disk istenirse hiçbir diske dokunulmaz.
        d.add_disk(&g, 3, 1).unwrap();
        fill("simdi");
        assert!(d.restore(&g, "vm-100-a", &[1, 2, 3]).is_err());
        assert_eq!(contents(), ["disk0-simdi", "disk1-simdi", "disk2-simdi"], "yarım geri yükleme olmaz");
        assert!(files_under(&cfg.images_dir()).iter().all(|f| !f.to_string_lossy().ends_with(".restore")));

        // Dışa aktarma: her disk kendi adıyla tek dosya.
        let (file, temporary) = d.export_backup("vm-100-a", 1).unwrap().unwrap();
        assert_eq!((temporary, file.file_name().unwrap().to_str().unwrap()), (incremental, "vm-100-a.disk1.qcow2"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "disk1-eski");
        if temporary {
            redox_server::state::discard_export(&file);
        }
        assert!(incremental == d.export_backup("vm-100-a", 5).is_err(), "artımlıda olmayan disk hata; tam yedekte dosya yolu döner");

        // Tam yedekte ek disk dosyası kaybolmuşsa: doğrulama ve geri yükleme bunu söyler.
        if !incremental {
            std::fs::remove_file(part("vm-100-b", 1)).unwrap();
            assert!(d.verify_backup("vm-100-b", &[1, 2]).unwrap_err().contains("bulunamadı"));
            assert!(d.restore(&g, "vm-100-b", &[1, 2]).unwrap_err().contains("bulunamadı"));
            assert_eq!(contents(), ["disk0-simdi", "disk1-simdi", "disk2-simdi"]);
        }
        // Dış hedeften inmiş olabilecek, başka bir dosyaya bağlanan (arkalıklı) imaj geri yüklenmez.
        if !incremental {
            std::fs::write(part("vm-100-c", 0), "arkalikli").unwrap();
            assert!(d.restore(&g, "vm-100-c", &[]).unwrap_err().contains("başka bir dosyaya bağlı"));
            assert_eq!(contents()[0], "disk0-simdi", "diske dokunulmaz");
        }
        // Yedek dosyaları doğrudan son adlarıyla yazılmaz: çökme sonrası kalan geçici dosyalar açılışta silinir.
        std::fs::write(cfg.backups_dir().join(".vm-100-yarim.qcow2.tmp"), b"x").unwrap();
        std::fs::write(cfg.backups_dir().join("vm-100-yarim.indiriliyor"), b"x").unwrap();
        std::fs::write(cfg.images_dir().join("vm-100-disk-0.qcow2.restore"), b"x").unwrap();
        drop(QemuDriver::new(cfg.clone()).unwrap());
        assert!(temporaries(&cfg).is_empty() && !cfg.backups_dir().join("vm-100-yarim.indiriliyor").exists());
        assert!(files_under(&cfg.images_dir()).iter().all(|f| !f.to_string_lossy().ends_with(".restore")));
        for id in ["vm-100-a", "vm-100-b", "vm-100-c"] {
            d.delete_backup(id).unwrap();
        }
        assert!(files_under(&cfg.backups_dir()).is_empty(), "{:?}", files_under(&cfg.backups_dir()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn full_backups_with_extra_disks_reach_offsite_and_download() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "ek-disk-test-tokeni").unwrap();
    let driver = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    assert_eq!(state.disk_add(id, 1, "admin").unwrap(), 1);
    let (main, extra) = (cfg.disk_path(id), cfg.extra_disk_path(id, 1));
    std::fs::write(&main, "ana-ilk").unwrap();
    std::fs::write(&extra, "ek-ilk").unwrap();
    let remote = dir.join("uzak");
    std::fs::create_dir_all(&remote).unwrap();
    state.set_offsite(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\n", remote.display())).unwrap());

    let backup = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    assert!(c.tasks.iter().all(|t| t.status == TaskStatus::Ok), "{:?}", c.tasks);
    assert_eq!(c.backup(&backup).unwrap().disks.len(), 1);
    assert!(c.backup(&backup).unwrap().offsite);
    // Dış hedefe iki dosya gider.
    assert_eq!(std::fs::read_to_string(remote.join(format!("{backup}.qcow2"))).unwrap(), "ana-ilk");
    assert_eq!(std::fs::read_to_string(remote.join(format!("{backup}.disk1.qcow2"))).unwrap(), "ek-ilk");

    // İndirme: asıl disk ve ek disk ayrı adreslerde.
    let app = redox_server::app(state.clone());
    let fetch = |uri: String| {
        let request = axum::http::Request::builder().uri(uri).header("host", "redox.test").header("authorization", "Bearer ek-disk-test-tokeni").body(axum::body::Body::empty()).unwrap();
        app.clone().oneshot(request)
    };
    let response = fetch(format!("/api/v1/backups/{backup}/download?disk=1")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-disposition"], format!("attachment; filename=\"{backup}.disk1.qcow2\"").as_str());
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(), b"ek-ilk");
    let response = fetch(format!("/api/v1/backups/{backup}/download")).await.unwrap();
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(), b"ana-ilk");
    assert_eq!(fetch(format!("/api/v1/backups/{backup}/download?disk=2")).await.unwrap().status(), 404, "yedekte olmayan disk");
    let response = fetch(format!("/guests/{id}?tab=yedekler")).await.unwrap();
    let html = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(html.contains("asıl disk + 1 ek disk") && html.contains(&format!("/backups/{backup}/download?disk=1")), "{html}");

    // Yerel yedekler kaybolur: geri yükleme iki dosyayı da hedeften indirir ve iki diski de geri getirir.
    std::fs::remove_dir_all(cfg.backups_dir()).unwrap();
    std::fs::write(&main, "ana-bozuk").unwrap();
    std::fs::write(&extra, "ek-bozuk").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert_eq!((std::fs::read_to_string(&main).unwrap(), std::fs::read_to_string(&extra).unwrap()), ("ana-ilk".to_string(), "ek-ilk".to_string()));
    state.backup_verify(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok);

    // Ek disk kaldırılırsa geri yükleme yalnızca asıl diski getirir; silinen diski geri eklemez.
    state.disk_remove(id, 1, "admin").unwrap();
    std::fs::write(&main, "ana-sonra").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok);
    assert_eq!(std::fs::read_to_string(&main).unwrap(), "ana-ilk");
    assert!(!extra.exists() && state.snapshot().guest(id).unwrap().hw.disks.is_empty());

    // Silme: yerel dosyalar ve dış kopyanın iki dosyası da gider.
    state.backup_delete(&backup, "admin").unwrap();
    assert!(files_under(&remote).is_empty() && files_under(&cfg.backups_dir()).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn guest_agent_reports_address_and_freezes_filesystems_for_live_backups() {
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
fn linked_clones_share_the_template_disk() {
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
fn refused_template_deletion_keeps_its_disks() {
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
fn changed_block_tracking_falls_back_to_a_full_read_when_unsupported() {
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
fn live_backup_finishes_when_jobs_complete_before_the_start_reply() {
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

#[tokio::test]
async fn media_files_are_uploaded_listed_and_deleted() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "medya-test-tokeni").unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("izleyici", "test", "medya-izleyici-tokeni").unwrap();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let app = redox_server::app(state.clone());
    let call = |method: &str, uri: &str, token: &str, kind: Option<&str>, body: Vec<u8>| {
        let mut request = axum::http::Request::builder().method(method).uri(uri).header("host", "redox.test").header("authorization", format!("Bearer {token}"));
        if let Some(kind) = kind {
            request = request.header("content-type", kind);
        }
        app.clone().oneshot(request.body(axum::body::Body::from(body)).unwrap())
    };
    let text = |response: axum::response::Response| async move { String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap() };
    const ADMIN: &str = "medya-test-tokeni";
    const VIEWER: &str = "medya-izleyici-tokeni";

    // API: ham gövdeyle yükleme; dosya adı türü ve dizini belirler.
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let response = call("PUT", "/api/v1/media/debian-12.iso", ADMIN, None, big.clone()).await.unwrap();
    assert_eq!(response.status(), 201);
    assert!(std::fs::read(cfg.iso_dir().join("debian-12.iso")).unwrap() == big);
    assert_eq!(call("PUT", "/api/v1/media/bulut.qcow2", ADMIN, None, b"imaj".to_vec()).await.unwrap().status(), 201);
    assert_eq!(std::fs::read(cfg.cloud_dir().join("bulut.qcow2")).unwrap(), b"imaj");
    // Var olanın üzerine yazılmaz; geçersiz adlar ve dizin kaçışı reddedilir; boş dosya kabul edilmez.
    let again = call("PUT", "/api/v1/media/debian-12.iso", ADMIN, None, b"baska".to_vec()).await.unwrap();
    assert_eq!(again.status(), 422);
    assert!(text(again).await.contains("zaten var"));
    assert!(std::fs::read(cfg.iso_dir().join("debian-12.iso")).unwrap() == big);
    for bad in ["betik.sh", ".gizli.iso", "..%2F..%2Fetc%2Fpasswd.iso", "bos%20luk.iso"] {
        assert_eq!(call("PUT", &format!("/api/v1/media/{bad}"), ADMIN, None, b"x".to_vec()).await.unwrap().status(), 422, "{bad}");
    }
    assert_eq!(call("PUT", "/api/v1/media/bos.iso", ADMIN, None, Vec::new()).await.unwrap().status(), 422);
    // Özet başlığı tutmazsa dosya kaydedilmez.
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/v1/media/ozetli.iso")
        .header("host", "redox.test")
        .header("authorization", format!("Bearer {ADMIN}"))
        .header("x-sha256", "0".repeat(64))
        .body(axum::body::Body::from("veri"))
        .unwrap();
    assert_eq!(app.clone().oneshot(request).await.unwrap().status(), 422);
    assert!(!cfg.iso_dir().join("ozetli.iso").exists());
    let hidden = |dir: PathBuf| std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).count();
    assert_eq!(hidden(cfg.iso_dir()) + hidden(cfg.cloud_dir()), 0, "reddedilen yüklemeler geçici dosya bırakmaz");
    // Salt okunur kullanıcı yükleyemez ve silemez, ama listeyi görür.
    assert_eq!(call("PUT", "/api/v1/media/izinsiz.iso", VIEWER, None, b"x".to_vec()).await.unwrap().status(), 403);
    assert_eq!(call("DELETE", "/api/v1/media/debian-12.iso", VIEWER, None, Vec::new()).await.unwrap().status(), 403);
    let list: serde_json::Value = serde_json::from_str(&text(call("GET", "/api/v1/media", VIEWER, None, Vec::new()).await.unwrap()).await).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!((list[0]["name"].as_str(), list[0]["iso"].as_bool(), list[0]["bytes"].as_u64()), (Some("debian-12.iso"), Some(true), Some(300_000)));

    // Tarayıcı formu (çok parçalı): dosya parça parça diske akar; tarayıcının gönderdiği yol atılır.
    let boundary = "----redoxsinir";
    let mut form = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"C:\\\\indirilenler\\\\kurulum.iso\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    form.extend_from_slice(&big);
    form.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let response = call("POST", "/media/upload", ADMIN, Some(&format!("multipart/form-data; boundary={boundary}")), form).await.unwrap();
    assert_eq!(response.status(), 303);
    assert!(response.headers()["location"].to_str().unwrap().starts_with("/storage?notice="), "{:?}", response.headers()["location"]);
    assert!(std::fs::read(cfg.iso_dir().join("kurulum.iso")).unwrap() == big, "2 MB'lık varsayılan gövde sınırı bu adreste uygulanmaz");
    assert!(state.snapshot().tasks.iter().any(|t| t.command == "media upload kurulum.iso" && t.status == TaskStatus::Ok));
    let page = text(call("GET", "/storage", ADMIN, None, Vec::new()).await.unwrap()).await;
    assert!(page.contains("kurulum.iso") && page.contains("bulut.qcow2") && page.contains(r#"action="/media/upload" enctype="multipart/form-data""#), "{page}");

    // Bir konuğa takılı kalıp silinmez; takılı olmayan silinir.
    let id = state.create_guest(&GuestSpec { iso: Some("debian-12.iso".into()), ..spec("web", &node) }, "admin").unwrap();
    let refused = call("DELETE", "/api/v1/media/debian-12.iso", ADMIN, None, Vec::new()).await.unwrap();
    assert_eq!(refused.status(), 422);
    assert!(text(refused).await.contains(&id.to_string()));
    assert!(cfg.iso_dir().join("debian-12.iso").exists());
    assert_eq!(call("POST", "/media/kurulum.iso/delete", ADMIN, Some("application/x-www-form-urlencoded"), Vec::new()).await.unwrap().status(), 303);
    assert!(!cfg.iso_dir().join("kurulum.iso").exists());
    assert_eq!(call("DELETE", "/api/v1/media/yok.iso", ADMIN, None, Vec::new()).await.unwrap().status(), 422);
    // Adresten indirme: iç ağ ve şifresiz adresler istek anında reddedilir.
    for url in ["http://example.com/a.iso", "https://127.0.0.1/a.iso", "https://169.254.169.254/a.iso"] {
        let body = serde_json::json!({ "url": url }).to_string().into_bytes();
        assert_eq!(call("POST", "/api/v1/media/fetch", ADMIN, Some("application/json"), body).await.unwrap().status(), 422, "{url}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn ova_archives_are_imported_and_exported() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "ova-test-tokeni").unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("izleyici", "test", "ova-izleyici-tokeni").unwrap();
    let driver = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let app = redox_server::app(state.clone());
    let call = |method: &str, uri: &str, token: &str, body: Vec<u8>| {
        let request = axum::http::Request::builder().method(method).uri(uri).header("host", "redox.test").header("authorization", format!("Bearer {token}"));
        app.clone().oneshot(request.body(axum::body::Body::from(body)).unwrap())
    };
    const ADMIN: &str = "ova-test-tokeni";

    // Başka bir sanallaştırıcıdan gelen arşiv: tanım, iki disk ve özet dosyası.
    let ovf = r#"<Envelope><References><File ovf:href="eski-disk1.vmdk" ovf:id="f1"/></References><VirtualSystem ovf:id="eski-sunucu"><Item><rasd:ResourceType>3</rasd:ResourceType><rasd:VirtualQuantity>4</rasd:VirtualQuantity></Item><Item><rasd:AllocationUnits>byte * 2^30</rasd:AllocationUnits><rasd:ResourceType>4</rasd:ResourceType><rasd:VirtualQuantity>2</rasd:VirtualQuantity></Item></VirtualSystem></Envelope>"#;
    let disk: Vec<u8> = (0..70_000u32).map(|i| (i % 249) as u8).collect();
    let archive = dir.join("kaynak.ova");
    redox_server::ova::write_tar(&archive, &[("eski.ovf", ovf.as_bytes()), ("../kacis.vmdk", b"yanlis disk"), ("eski-disk1.vmdk", &disk), ("eski.mf", b"x")]).unwrap();
    let response = call("PUT", "/api/v1/media/eski-sunucu.ova", ADMIN, std::fs::read(&archive).unwrap()).await.unwrap();
    assert_eq!(response.status(), 201);
    let list: serde_json::Value = serde_json::from_slice(&call("GET", "/api/v1/media", ADMIN, Vec::new()).await.unwrap().into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!((list[0]["ova"]["cores"].as_u64(), list[0]["ova"]["memory_mb"].as_u64(), list[0]["ova"]["name"].as_str()), (Some(4), Some(2048), Some("eski-sunucu")));
    let page = String::from_utf8(call("GET", "/storage", ADMIN, Vec::new()).await.unwrap().into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(page.contains("OVA arşivi") && page.contains("öneri: 4 çekirdek, 2048 MB"), "{page}");

    // Arşivden sanal makine: tanımın andığı disk çıkarılır (sahte qemu-img içeriği aynen kopyalar).
    let mut from_ova = spec("gelen", &node);
    from_ova.hw.image = Some("eski-sunucu.ova".into());
    let id = state.create_guest(&from_ova, "admin").unwrap();
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == disk);
    let leftovers = std::fs::read_dir(cfg.cloud_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).count();
    assert_eq!(leftovers, 0, "çıkarılan geçici disk silinir");
    assert!(!dir.join("kacis.vmdk").exists() && !cfg.data_dir.join("kacis.vmdk").exists(), "arşivdeki ad yol olarak kullanılmaz");

    // Dışa aktarma: tanım ve bütün diskler tek arşivde.
    assert_eq!(state.disk_add(id, 1, "admin").unwrap(), 1);
    std::fs::write(cfg.extra_disk_path(id, 1), b"ikinci diskin verisi").unwrap();
    let response = call("GET", &format!("/api/v1/guests/{id}/export"), ADMIN, Vec::new()).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-disposition"], "attachment; filename=\"gelen.ova\"");
    let declared: usize = response.headers()["content-length"].to_str().unwrap().parse().unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.len(), declared, "bildirilen uzunluk akan baytlarla aynı");
    let exported = dir.join("gelen.ova");
    std::fs::write(&exported, &body).unwrap();
    let members = redox_server::ova::members(&mut std::fs::File::open(&exported).unwrap()).unwrap();
    assert_eq!(members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["gelen.ovf", "gelen-disk1.vmdk", "gelen-disk2.vmdk"]);
    assert_eq!(&body[members[2].offset as usize..(members[2].offset + members[2].size) as usize], b"ikinci diskin verisi");
    let info = redox_server::ova::inspect(&exported).unwrap();
    assert_eq!((info.cores, info.memory_mb, info.disks.len()), (Some(1), Some(256), 2));
    // Sistemdeki tar da arşivi okuyabiliyor.
    let listing = std::process::Command::new("tar").arg("-tf").arg(&exported).output().unwrap();
    assert!(listing.status.success() && String::from_utf8_lossy(&listing.stdout).lines().count() == 3, "{listing:?}");
    let temporaries = std::fs::read_dir(cfg.backups_dir()).into_iter().flatten().flatten().count();
    assert_eq!(temporaries, 0, "dışa aktarma için üretilen dosyalar silinir");

    // Kendi arşivimiz yeniden içe aktarılabilir.
    assert_eq!(call("PUT", "/api/v1/media/gelen-kopya.ova", ADMIN, body.to_vec()).await.unwrap().status(), 201);
    let mut again = spec("donen", &node);
    again.hw.image = Some("gelen-kopya.ova".into());
    let second = state.create_guest(&again, "admin").unwrap();
    assert!(std::fs::read(cfg.disk_path(second)).unwrap() == disk, "dışa aktarılıp geri alınan disk aynı");

    // Çalışan konuk, olmayan konuk ve yetkisiz kullanıcı.
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(call("GET", &format!("/api/v1/guests/{id}/export"), ADMIN, Vec::new()).await.unwrap().status(), 409);
    state.run_action(id, Action::Stop, "admin").unwrap();
    assert_eq!(call("GET", "/api/v1/guests/9999/export", ADMIN, Vec::new()).await.unwrap().status(), 404);
    assert_eq!(call("GET", &format!("/api/v1/guests/{id}/export"), "ova-izleyici-tokeni", Vec::new()).await.unwrap().status(), 403);
    let edit = String::from_utf8(call("GET", &format!("/guests/{id}?tab=cogalt"), ADMIN, Vec::new()).await.unwrap().into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(edit.contains(&format!("/guests/{id}/export")) && edit.contains("OVA Olarak İndir"));

    // Disksiz arşivden oluşturma açık hatayla reddedilir ve yarım konuk bırakılmaz.
    let empty = dir.join("bos.ova");
    redox_server::ova::write_tar(&empty, &[("bos.ovf", b"<a/>")]).unwrap();
    assert_eq!(call("PUT", "/api/v1/media/bos.ova", ADMIN, std::fs::read(&empty).unwrap()).await.unwrap().status(), 201);
    let mut broken = spec("olmaz", &node);
    broken.hw.image = Some("bos.ova".into());
    let before = state.snapshot().guests.len();
    assert!(matches!(state.create_guest(&broken, "admin"), Err(Error::Driver(m)) if m.contains("disk imajı")));
    assert_eq!(state.snapshot().guests.len(), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn suspend_saves_memory_and_start_resumes_from_it() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let driver = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, auth);
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    let saved = cfg.state_path(id);
    let resumed = PathBuf::from(format!("{}.resumed", cfg.qmp_path(id).display()));
    let last = |state: &AppState| state.snapshot().tasks[0].clone();

    assert!(matches!(state.suspend(id, "admin"), Err(Error::InvalidSpec(m)) if m.contains("çalışmıyor")), "kapalı konuk askıya alınmaz");
    state.run_action(id, Action::Start, "admin").unwrap();
    assert!(!resumed.exists(), "ilk başlatma sıfırdan");
    state.suspend(id, "admin").unwrap();
    let g = state.snapshot().guest(id).unwrap().clone();
    assert_eq!((last(&state).status, g.status, g.hw.suspended), (TaskStatus::Ok, GuestStatus::Stopped, true), "{:?}", last(&state));
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "bellek-durumu");
    assert!(!cfg.pid_path(id).exists() && !cfg.qmp_path(id).exists(), "QEMU kapanmış olmalı");

    // Askıdayken disk ve donanım değiştirilemez; yedek alınabilir.
    let backup = state.backup_create(id, "admin").unwrap();
    assert!(matches!(state.backup_restore(&backup, "admin"), Err(Error::InvalidSpec(m)) if m.contains("askıda")));
    assert!(matches!(state.disk_add(id, 1, "admin"), Err(Error::InvalidSpec(m)) if m.contains("askıda")));
    assert!(matches!(state.set_template(id, true, "admin"), Err(Error::InvalidSpec(_))));

    // Başlatınca kayıtlı durumdan devam eder ve durum dosyası bir daha kullanılmamak üzere silinir.
    state.run_action(id, Action::Start, "admin").unwrap();
    let g = state.snapshot().guest(id).unwrap().clone();
    assert_eq!((last(&state).status, g.status, g.hw.suspended), (TaskStatus::Ok, GuestStatus::Running, false), "{:?}", last(&state));
    assert_eq!(std::fs::read_to_string(&resumed).unwrap(), saved.display().to_string(), "QEMU durum dosyasıyla başlatıldı");
    assert!(!saved.exists());
    state.run_action(id, Action::Stop, "admin").unwrap();
    std::fs::remove_file(&resumed).unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    assert!(!resumed.exists(), "sonraki başlatma yeniden sıfırdan");

    // Askı durumunu atma: konuk sıfırdan açılır.
    state.suspend(id, "admin").unwrap();
    assert!(saved.exists());
    state.discard_suspend(id, "admin").unwrap();
    assert!(!saved.exists() && !state.snapshot().guest(id).unwrap().hw.suspended);
    assert!(matches!(state.discard_suspend(id, "admin"), Err(Error::InvalidSpec(m)) if m.contains("askıda değil")));
    state.run_action(id, Action::Start, "admin").unwrap();
    assert!(!resumed.exists());

    // Bozuk durum dosyası: başlatma açık hatayla başarısız olur, konuk askıda kalır (durum atılabilir).
    state.suspend(id, "admin").unwrap();
    std::fs::write(&saved, "bozuk").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    let g = state.snapshot().guest(id).unwrap().clone();
    assert_eq!((last(&state).status, g.status, g.hw.suspended), (TaskStatus::Failed, GuestStatus::Stopped, true), "{:?}", last(&state));
    state.discard_suspend(id, "admin").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(last(&state).status, TaskStatus::Ok);
    state.run_action(id, Action::Stop, "admin").unwrap();

    // Durum yazılamazsa konuk çalışmaya devam eder ve yarım dosya kalmaz.
    let bad = state.create_guest(&spec("askisiz", &node), "admin").unwrap();
    state.run_action(bad, Action::Start, "admin").unwrap();
    state.suspend(bad, "admin").unwrap();
    let g = state.snapshot().guest(bad).unwrap().clone();
    assert_eq!((last(&state).status, g.hw.suspended), (TaskStatus::Failed, false), "{:?}", last(&state));
    assert!(last(&state).message.unwrap().contains("bellek durumu yazılamadı"));
    assert!(cfg.pid_path(bad).exists() && !cfg.state_path(bad).exists());
    assert!(files_under(&cfg.images_dir()).iter().all(|f| !f.to_string_lossy().ends_with(".tmp")));
    state.run_action(bad, Action::Stop, "admin").unwrap();
    // Silinen konuğun durum dosyası da gider.
    state.run_action(id, Action::Start, "admin").unwrap();
    state.suspend(id, "admin").unwrap();
    state.delete_guest(id, "admin").unwrap();
    assert!(!saved.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn network_limit_that_cannot_be_applied_stops_the_guest() {
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let mut g = guest(&cfg, "sinirli");
    d.create(&g).unwrap();

    // Disk sınırı QEMU satırına girer ve konuk açılır.
    g.hw.disk_mbps = 20;
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Running);
    d.apply(&g, Action::Stop).unwrap();

    // Ağ sınırı: sahte QEMU köprü arayüzü açmaz; sınır uygulanamayınca konuk sınırsız çalışmaya bırakılmaz.
    g.bridge = "vmbr0".into();
    g.hw.net_mbps = 50;
    let err = d.apply(&g, Action::Start).unwrap_err();
    assert!(err.contains("ağ hız sınırı uygulanamadı") && err.contains("TAP"), "{err}");
    assert_eq!(status(&d, &g), GuestStatus::Stopped, "konuk kapatılmış olmalı");
    assert!(!cfg.pid_path(g.vmid).exists(), "süreç kaydı temizlenir");

    // Sınır kaldırılınca aynı konuk açılır.
    g.hw.net_mbps = 0;
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Running);
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn disks_and_nics_are_hot_added_to_a_running_guest() {
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let mut g = guest(&cfg, "sicak");
    d.create(&g).unwrap();
    assert!(d.hotplug(GuestKind::Vm) && !d.hotplug(GuestKind::Ct));
    let log = || std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(g.vmid).display())).unwrap_or_default();

    d.add_disk(&g, 1, 1).unwrap();
    assert!(d.hot_add_disk(&g, 1).unwrap_err().contains("çalışmıyor"), "kapalı konuğa çalışırken takılmaz");
    d.apply(&g, Action::Start).unwrap();
    d.hot_add_disk(&g, 1).unwrap();
    let disk = cfg.extra_disk_path(g.vmid, 1).display().to_string();
    assert!(log().contains(&format!("drive_add xd1 file={disk},if=none,id=xd1,format=qcow2\ndevice_add virtio-blk-pci xdev1 hp0 xd1\n")), "{}", log());
    assert!(d.hot_add_disk(&g, 1).unwrap_err().contains("tanıtılamadı"), "aynı disk ikinci kez takılmaz");
    assert!(!log().contains("drive_del xd1"), "takılı diskin sürücüsüne dokunulmaz");

    d.hot_add_nic(&g, 1, "nat").unwrap();
    d.hot_add_nic(&g, 2, "vmbr9").unwrap();
    assert!(log().contains("netdev_add user net1 -\ndevice_add virtio-net-pci xnic1 hp8 net1\n") && log().contains("netdev_add bridge net2 vmbr9\ndevice_add virtio-net-pci xnic2 hp9 net2\n"), "{}", log());
    assert!(d.hot_add_nic(&g, 3, "br0,script=/x").unwrap_err().contains("geçersiz ağ"));

    // Konuk aygıtı reddederse tanıtılan sürücü ve ağ ucu geri alınır.
    std::fs::write(format!("{}.nohot", cfg.qmp_path(g.vmid).display()), b"").unwrap();
    d.add_disk(&g, 2, 1).unwrap();
    assert!(d.hot_add_disk(&g, 2).unwrap_err().contains("disk konuğa takılamadı"));
    assert!(d.hot_add_nic(&g, 3, "nat").unwrap_err().contains("ağ kartı konuğa takılamadı"));
    assert!(log().contains("drive_del xd2\n") && log().contains("netdev_del net3\n"), "{}", log());
    std::fs::remove_file(format!("{}.nohot", cfg.qmp_path(g.vmid).display())).unwrap();

    // Ağ hız sınırı varken köprüye takılan kartın arayüzü bulunamazsa kart sınırsız bırakılmaz.
    let mut limited = g.clone();
    limited.hw.net_mbps = 10;
    let err = d.hot_add_nic(&limited, 3, "vmbr9").unwrap_err();
    assert!(err.contains("ağ hız sınırı uygulanamadı") && log().contains("device_del xnic3\nnetdev_del net3\n"), "{err}: {}", log());

    // Çalışırken takılan disk canlı yedeğe girer.
    g.hw.disks = vec![redox_core::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 }];
    d.backup(&g, "vm-sicak-1").unwrap();
    assert!(cfg.backups_dir().join("vm-sicak-1.qcow2").exists() && cfg.backups_dir().join("vm-sicak-1.disk1.qcow2").exists());
    d.apply(&g, Action::Stop).unwrap();

    // Yeniden açılışta aynı disk aynı bağlantı noktasında, komut satırından gelir.
    d.apply(&g, Action::Start).unwrap();
    let args = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(g.vmid).display())).unwrap();
    assert!(args.contains(&format!("file={disk},if=none,id=xd1,format=qcow2\n-device\nvirtio-blk-pci,drive=xd1,id=xdev1,bus=hp0")), "{args}");
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn suspended_guests_resume_with_the_layout_they_were_suspended_in() {
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "yerlesim");
    d.create(&g).unwrap();
    let args = || std::fs::read_to_string(format!("{}.args", cfg.qmp_path(g.vmid).display())).unwrap();
    let marker = cfg.state_path(g.vmid).with_extension("state.hp");

    d.apply(&g, Action::Start).unwrap();
    assert!(args().contains("pcie-root-port,id=hp0"));
    d.suspend(&g).unwrap();
    assert!(marker.exists(), "askı kaydı yerleşimi not eder");
    d.apply(&g, Action::Start).unwrap();
    assert!(args().contains("pcie-root-port,id=hp0") && !marker.exists() && !cfg.state_path(g.vmid).exists());

    // Bu düzenden önce askıya alınmış konuk (işaret yok) eski yerleşimle döner; sonraki açılış yeni yerleşimledir.
    d.suspend(&g).unwrap();
    std::fs::remove_file(&marker).unwrap();
    d.apply(&g, Action::Start).unwrap();
    assert!(!args().contains("pcie-root-port"), "{}", args());
    // Eski yerleşimle çalışan konuğa çalışırken takılamaz; askıya alınırsa yine eski yerleşimle döner.
    d.add_disk(&g, 1, 1).unwrap();
    assert!(d.hot_add_disk(&g, 1).unwrap_err().contains("bir kez kapatıp açın"));
    assert!(d.hot_add_nic(&g, 1, "nat").unwrap_err().contains("bir kez kapatıp açın"));
    d.suspend(&g).unwrap();
    assert!(!marker.exists());
    d.discard_suspend(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();
    assert!(args().contains("pcie-root-port,id=hp0"));
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn vlan_cards_get_a_named_tap_on_the_bridge() {
    let (mut cfg, dir) = config();
    let (sys, log) = (dir.join("sys"), dir.join("net.log"));
    for (bridge, filtering) in [("vmbr5", "1"), ("vmbr6", "0")] {
        std::fs::create_dir_all(sys.join(bridge).join("bridge")).unwrap();
        std::fs::write(sys.join(bridge).join("bridge/vlan_filtering"), format!("{filtering}\n")).unwrap();
    }
    cfg.sys_net = sys.clone();
    // Sahte ip: komutu kaydeder; arayüz açılınca sysfs'te görünür, silinince kaybolur. "ret" dosyası varsa reddeder.
    cfg.ip_bin = script(&dir, "ip", &format!(
        "echo \"ip $*\" >> '{log}'\n[ -e '{deny}' ] && [ \"$1\" = tuntap ] && {{ echo 'ioctl(TUNSETIFF): Operation not permitted' >&2; exit 1; }}\ncase \"$1 $2\" in 'tuntap add') mkdir -p '{sys}'/\"$4\";; 'link del') rm -rf '{sys}'/\"$3\";; esac",
        log = log.display(), sys = sys.display(), deny = dir.join("ret").display()
    ));
    cfg.bridge_bin = script(&dir, "bridge", &format!("echo \"bridge $*\" >> '{}'", log.display()));
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let mut g = guest(&cfg, "etiketli");
    d.create(&g).unwrap();
    let lines = || std::fs::read_to_string(&log).unwrap_or_default();
    let uid = { use std::os::unix::fs::MetadataExt; std::fs::metadata("/proc/self").unwrap().uid() };

    g.bridge = "vmbr5".into();
    g.hw.vlan = 20;
    g.hw.nics = vec!["vmbr5:30".into(), "nat".into()];
    d.apply(&g, Action::Start).unwrap();
    let tap = format!("rx{}n0", g.vmid);
    let expected = format!("ip tuntap add dev {tap} mode tap user {uid}\nip link set dev {tap} master vmbr5\nbridge vlan del dev {tap} vid 1\nbridge vlan add dev {tap} vid 20 pvid untagged\nip link set dev {tap} up\n");
    assert!(lines().starts_with(&expected), "{}", lines());
    assert!(lines().contains(&format!("bridge vlan add dev rx{}n1 vid 30 pvid untagged", g.vmid)));
    assert!(sys.join(&tap).exists() && sys.join(format!("rx{}n1", g.vmid)).exists());
    let args = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(g.vmid).display())).unwrap();
    assert!(args.contains(&format!("tap,id=net0,ifname={tap},script=no,downscript=no")) && !args.contains("br=vmbr5"), "{args}");

    // Çalışırken takılan etiketli kart da kendi arayüzünü alır.
    d.hot_add_nic(&g, 3, "vmbr5:40").unwrap();
    assert!(lines().contains(&format!("bridge vlan add dev rx{}n3 vid 40 pvid untagged", g.vmid)));
    let hot = std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(g.vmid).display())).unwrap();
    assert!(hot.contains("netdev_add tap net3 -\ndevice_add virtio-net-pci xnic3 hp10 net3\n"), "{hot}");
    // Konuk kapanınca arayüzler kaldırılır.
    d.apply(&g, Action::Stop).unwrap();
    for n in [0, 1, 3] {
        assert!(!sys.join(format!("rx{}n{n}", g.vmid)).exists(), "rx{}n{n} kaldırılmalı", g.vmid);
    }

    // Köprüde VLAN süzme kapalıysa ya da arayüz açılamıyorsa konuk başlatılmaz ve yarım arayüz bırakılmaz.
    g.bridge = "vmbr6".into();
    g.hw.nics.clear();
    let err = d.apply(&g, Action::Start).unwrap_err();
    assert!(err.contains("VLAN süzme kapalı") && err.contains("vlan_filtering 1"), "{err}");
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    g.bridge = "vmbr5".into();
    g.hw.nics = vec!["vmbr5:30".into()];
    std::fs::write(dir.join("ret"), b"").unwrap();
    let err = d.apply(&g, Action::Start).unwrap_err();
    assert!(err.contains("VLAN arayüzü hazırlanamadı") && err.contains("Operation not permitted") && err.contains("CAP_NET_ADMIN"), "{err}");
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    std::fs::remove_file(dir.join("ret")).unwrap();

    // Etiket kaldırılınca konuk köprü yardımcısıyla, bizim arayüzümüz olmadan açılır.
    g.hw.vlan = 0;
    g.hw.nics.clear();
    let before = lines().len();
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(lines().len(), before, "etiket yokken ip ve bridge çağrılmaz");
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn guests_on_an_extra_storage_keep_their_disks_there() {
    let (mut cfg, dir) = config();
    let fast = dir.join("hizli");
    std::fs::create_dir_all(&fast).unwrap();
    cfg.storages = vec![("hizli".to_string(), fast.clone())];
    // Yarıda kalmış geri yükleme hazırlığı ek depolamada da temizlenir.
    std::fs::write(fast.join("vm-9-disk-0.qcow2.restore"), b"yarim").unwrap();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    assert!(!fast.join("vm-9-disk-0.qcow2.restore").exists());

    let mut g = guest(&cfg, "depolu");
    g.storage = "hizli".into();
    assert_eq!(cfg.disk_of(&g), fast.join(format!("vm-{}-disk-0.qcow2", g.vmid)));
    assert_eq!(cfg.storage_dir("local"), cfg.images_dir());
    assert_eq!(cfg.storage_dir("bilinmeyen"), cfg.images_dir(), "tanımsız ad veri dizinine düşer");
    d.create(&g).unwrap();
    assert!(cfg.disk_of(&g).exists() && !cfg.disk_path(g.vmid).exists(), "disk yalnızca kendi depolamasında");

    d.add_disk(&g, 1, 1).unwrap();
    g.hw.disks = vec![redox_core::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 }];
    assert!(fast.join(format!("vm-{}-disk-1.qcow2", g.vmid)).exists() && !cfg.extra_disk_path(g.vmid, 1).exists());

    d.apply(&g, Action::Start).unwrap();
    let args = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(g.vmid).display())).unwrap();
    assert!(args.contains(&format!("file={},if=virtio", cfg.disk_of(&g).display())) && args.contains(&format!("file={},if=none,id=xd1", cfg.extra_disk_of(&g, 1).display())), "{args}");
    // Canlı yedek ve kapalıyken geri yükleme aynı depolamadaki dosyalarla çalışır.
    std::fs::write(cfg.disk_of(&g), "asil-v1").unwrap();
    std::fs::write(cfg.extra_disk_of(&g, 1), "ek-v1").unwrap();
    d.backup(&g, "vm-depolu-1").unwrap();
    d.apply(&g, Action::Stop).unwrap();
    std::fs::write(cfg.disk_of(&g), "asil-v2").unwrap();
    std::fs::write(cfg.extra_disk_of(&g, 1), "ek-v2").unwrap();
    d.restore(&g, "vm-depolu-1", &[1]).unwrap();
    assert_eq!((std::fs::read_to_string(cfg.disk_of(&g)).unwrap(), std::fs::read_to_string(cfg.extra_disk_of(&g, 1)).unwrap()), ("asil-v1".to_string(), "ek-v1".to_string()));
    assert!(!cfg.disk_path(g.vmid).exists(), "geri yükleme diski yerel depolamaya taşımaz");

    // Klon, hedefin depolamasına yazar: aynı depolamaya ve yerel depolamaya.
    let mut copy = guest(&cfg, "kopya");
    copy.vmid = g.vmid + 1;
    copy.storage = "hizli".into();
    copy.hw.disks = g.hw.disks.clone();
    d.clone_disk(&g, &copy).unwrap();
    assert_eq!(std::fs::read_to_string(cfg.disk_of(&copy)).unwrap(), "asil-v1");
    assert!(cfg.extra_disk_of(&copy, 1).exists() && !cfg.disk_path(copy.vmid).exists());
    let mut local = copy.clone();
    local.vmid = g.vmid + 2;
    local.storage = "local".into();
    d.clone_disk(&g, &local).unwrap();
    assert_eq!(std::fs::read_to_string(cfg.disk_path(local.vmid)).unwrap(), "asil-v1");

    // Silme, diskleri kendi depolamasından kaldırır.
    d.remove_disk(&g, 1).unwrap();
    assert!(!cfg.extra_disk_of(&g, 1).exists());
    d.destroy(&g).unwrap();
    assert!(!cfg.disk_of(&g).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_console_bridges_the_browser_to_the_guest_screen() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::{Error as WsError, Message};
    const ADMIN: &str = "konsol-test-tokeni";
    const VIEWER: &str = "konsol-izleyici-tokeni";
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", ADMIN).unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("izleyici", "test", VIEWER).unwrap();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let state = AppState::new(cluster, Box::new(d), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let id = state.create_guest(&spec("canli", &node), "admin").unwrap();
    let guest = state.snapshot().guest(id).unwrap().clone();
    assert!(state.driver().vnc_socket(&guest).is_none(), "kapalı konuğun konsolu yok");
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(state.driver().vnc_socket(&guest), Some(cfg.vnc_path(id)));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = redox_server::app(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
    });
    let connect = |token: &'static str, origin: Option<String>| async move {
        let mut request = format!("ws://{addr}/guests/{id}/vnc").into_client_request().unwrap();
        request.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
        if let Some(origin) = origin {
            request.headers_mut().insert("origin", origin.parse().unwrap());
        }
        tokio_tungstenite::connect_async(request).await
    };
    let refused = |outcome: Result<_, WsError>| match outcome {
        Err(WsError::Http(response)) => response.status().as_u16(),
        Err(other) => panic!("beklenmeyen hata: {other}"),
        Ok(_) => panic!("bağlantı kabul edilmemeliydi"),
    };

    // Aynı siteden gelen yönetici: konuğun karşılaması tarayıcıya, tarayıcının baytları konuğa ulaşır.
    let (mut ws, _) = connect(ADMIN, Some(format!("http://{addr}"))).await.unwrap();
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(hello.into_data().as_ref(), b"RFB 003.008\n");
    ws.send(Message::Binary(b"RFB 003.008\n".to_vec().into())).await.unwrap();
    // Büyük bir çerçeve iki yönde de eksiksiz taşınır (parçalara bölünse de sıra ve içerik korunur).
    let frame: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    ws.send(Message::Binary(frame.clone().into())).await.unwrap();
    let mut echoed = Vec::new();
    while echoed.len() < 12 + frame.len() {
        let message = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("yankı zamanında gelmeli").unwrap().unwrap();
        echoed.extend_from_slice(message.into_data().as_ref());
    }
    assert_eq!(&echoed[..12], b"RFB 003.008\n");
    assert!(echoed[12..] == frame[..], "çerçeve bozulmadan döndü");
    // Metin iletileri konuğa taşınmaz.
    ws.send(Message::Text("yok say".into())).await.unwrap();
    ws.send(Message::Binary(vec![7, 7, 7].into())).await.unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(next.into_data().as_ref(), [7, 7, 7]);
    assert_eq!(state.auth().audit.recent("console", 10).len(), 1, "konsol oturumu denetim günlüğüne yazılır");

    // İzleyici ve başka siteden gelen sayfa reddedilir; reddedilen girişim oturum sayılmaz.
    assert_eq!(refused(connect(VIEWER, None).await), 403);
    assert_eq!(refused(connect(ADMIN, Some("https://evil.example".to_string())).await), 403);
    assert_eq!(state.auth().audit.recent("console", 10).len(), 1);

    // Konuk kapanınca oturum da kapanır.
    state.run_action(id, Action::Stop, "admin").unwrap();
    let end = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(end.is_ok(), "konuk kapanınca bağlantı düşmeli");
    assert!(!cfg.vnc_path(id).exists(), "soket temizlenir");
    assert_eq!(refused(connect(ADMIN, None).await), 409);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn shared_folders_are_created_and_passed_to_the_guest() {
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    assert!(!d.shared_folders(), "sahte QEMU 9p aygıtını tanıtmaz");
    assert_eq!(d.share_dir("ortak"), Some(cfg.shares_dir().join("ortak")));
    let mut g = guest(&cfg, "paylasimli");
    d.create(&g).unwrap();
    g.hw.shares = redox_core::hw::parse_shares("ortak, veri:ro").unwrap();
    assert!(!cfg.shares_dir().exists());
    d.apply(&g, Action::Start).unwrap();
    assert!(cfg.shares_dir().join("ortak").is_dir() && cfg.shares_dir().join("veri").is_dir(), "dizinler ilk açılışta oluşturulur");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(cfg.shares_dir().join("ortak")).unwrap().permissions().mode() & 0o777, 0o700, "paylaşım yalnızca hizmet kullanıcısına açık oluşturulur");
    }
    let args = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(g.vmid).display())).unwrap();
    assert!(args.contains(&format!("local,id=fs0,path={},security_model=none,multidevs=remap\n-device\nvirtio-9p-pci,fsdev=fs0,mount_tag=ortak", cfg.shares_dir().join("ortak").display())), "{args}");
    assert!(args.contains("multidevs=remap,readonly=on\n-device\nvirtio-9p-pci,fsdev=fs1,mount_tag=veri"), "{args}");
    // Var olan dosyalar yeniden açılışta yerinde kalır; konuk silinince paylaşım silinmez.
    std::fs::write(cfg.shares_dir().join("ortak/not.txt"), "kalsin").unwrap();
    d.apply(&g, Action::Stop).unwrap();
    d.apply(&g, Action::Start).unwrap();
    d.apply(&g, Action::Stop).unwrap();
    d.destroy(&g).unwrap();
    assert_eq!(std::fs::read_to_string(cfg.shares_dir().join("ortak/not.txt")).unwrap(), "kalsin");
    let _ = std::fs::remove_dir_all(dir);
}
