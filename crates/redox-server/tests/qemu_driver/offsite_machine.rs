//! Dış kopya, UEFI/TPM hazırlığı, ek diskler, yedek indirme.

use super::*;

#[test]
pub(super) fn backups_are_copied_offsite_and_pulled_back_for_restore() {
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
pub(super) const FAKE_SWTPM: &str = r#"[ "$1" = "--version" ] && { echo "TPM emulator version 0.0"; exit 0; }
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
pub(super) fn uefi_and_tpm_guests_prepare_firmware_and_emulator() {
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
pub(super) fn extra_disks_follow_the_guest_through_its_life() {
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
pub(super) async fn backup_files_can_be_downloaded_by_admins() {
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
