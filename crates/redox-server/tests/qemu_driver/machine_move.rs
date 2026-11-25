//! Makine durumu yedeği, balon, sunucular ve depolar arası taşıma.

use super::*;

#[test]
pub(super) fn backups_carry_uefi_variables_and_tpm_state() {
    if !python_available() {
        return;
    }
    use redox_core::hw::Firmware;
    let (mut cfg, dir) = config();
    cfg.ovmf_dir = dir.join("ovmf");
    cfg.swtpm_bin = script(&dir, "swtpm", FAKE_SWTPM);
    std::fs::create_dir_all(&cfg.ovmf_dir).unwrap();
    for name in ["OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd", "OVMF_CODE_4M.secboot.fd", "OVMF_VARS_4M.ms.fd"] {
        std::fs::write(cfg.ovmf_dir.join(name), name).unwrap();
    }
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let ok = |state: &AppState, what: &str| assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{what}: {:?}", state.snapshot().tasks[0].message);

    // BIOS'lu, TPM'siz konukta makine durumu yoktur: arşiv yazılmaz.
    let plain = state.create_guest(&spec("duz", &node), "admin").unwrap();
    let plain_backup = state.backup_create(plain, "admin").unwrap();
    assert!(!state.snapshot().backup(&plain_backup).unwrap().machine);
    assert!(!cfg.backups_dir().join(format!("{plain_backup}.machine.tar")).exists());

    let secure = GuestSpec { hw: redox_core::Hardware { firmware: Firmware::Uefi, secure_boot: true, tpm: true, ..Default::default() }, ..spec("win11", &node) };
    let id = state.create_guest(&secure, "admin").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    ok(&state, "başlatma");
    state.run_action(id, Action::Stop, "admin").unwrap();
    let (vars, tpm) = (cfg.efivars_path(id, true), cfg.tpm_dir(id));
    std::fs::write(&vars, "degiskenler-1").unwrap();
    std::fs::write(tpm.join("tpm2-00.permall"), "tpm-1").unwrap();
    std::fs::write(tpm.join(".lock"), "").unwrap();

    let backup = state.backup_create(id, "admin").unwrap();
    ok(&state, "yedek");
    let archive = cfg.backups_dir().join(format!("{backup}.machine.tar"));
    assert!(state.snapshot().backup(&backup).unwrap().machine && archive.is_file());
    let bytes = std::fs::read(&archive).unwrap();
    let has = |needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(has("efivars-secure.qcow2") && has("degiskenler-1") && has("tpm/tpm2-00.permall") && has("tpm-1") && !has(".lock"));
    assert!(!vars.with_extension("qcow2.backup").exists(), "geçici kopya kalmaz");

    // Konuk sonradan değişir; geri yükleme diskle birlikte değişkenleri ve TPM durumunu da geri getirir.
    std::fs::write(&vars, "degiskenler-2").unwrap();
    std::fs::write(tpm.join("tpm2-00.permall"), "tpm-2").unwrap();
    std::fs::write(tpm.join("sonradan"), "x").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    ok(&state, "geri yükleme");
    assert_eq!(std::fs::read_to_string(&vars).unwrap(), "degiskenler-1");
    assert_eq!(std::fs::read_to_string(tpm.join("tpm2-00.permall")).unwrap(), "tpm-1");
    assert!(!tpm.join("sonradan").exists(), "TPM dizini yedekteki haline döner");

    // Dış hedef: arşiv de gider; yerelde kaybolunca geri yükleme onu da indirir.
    let remote = dir.join("uzak");
    std::fs::create_dir_all(&remote).unwrap();
    state.set_offsite(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\npassphrase = dogru-at-pil-zimba\n", remote.display())).unwrap());
    state.backup_push(&backup, "admin").unwrap();
    let far = remote.join(format!("{backup}.machine.tar.enc"));
    assert!(far.is_file() && remote.join(format!("{backup}.qcow2.enc")).is_file());
    std::fs::remove_file(&archive).unwrap();
    std::fs::remove_file(cfg.backups_dir().join(format!("{backup}.qcow2"))).unwrap();
    std::fs::write(&vars, "degiskenler-3").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    ok(&state, "dış kopyadan geri yükleme");
    assert_eq!(std::fs::read_to_string(&vars).unwrap(), "degiskenler-1");
    assert!(archive.is_file());

    // Donanımı değişmiş konuğa uymayan parça konmaz: güvenli önyükleme kapatılınca eski depo geri yüklenmez,
    // TPM kaldırılınca TPM durumu yazılmaz.
    state.update_guest(id, &redox_core::GuestPatch { secure_boot: Some(false), tpm: Some(false), ..Default::default() }, "admin").unwrap();
    let _ = std::fs::remove_dir_all(&tpm);
    state.backup_restore(&backup, "admin").unwrap();
    ok(&state, "donanımı değişmiş konuğa geri yükleme");
    assert!(!cfg.efivars_path(id, false).exists() && !tpm.exists());
    state.update_guest(id, &redox_core::GuestPatch { secure_boot: Some(true), tpm: Some(true), ..Default::default() }, "admin").unwrap();

    // Başka bir dosyaya bağlanan değişken deposu (dış hedeften gelmiş olabilir) yerine konmaz.
    std::fs::write(&vars, "arkalikli").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let poisoned = state.backup_create(id, "admin").unwrap();
    std::fs::write(&vars, "saglam").unwrap();
    state.backup_restore(&poisoned, "admin").unwrap();
    let task = state.snapshot().tasks[0].clone();
    assert_eq!(task.status, TaskStatus::Failed);
    assert!(task.message.as_deref().unwrap().contains("makine durumu (UEFI/TPM) geri yüklenemedi"), "{:?}", task.message);
    assert_eq!(std::fs::read_to_string(&vars).unwrap(), "saglam");

    // Silme arşivi yerelde ve dış hedefte kaldırır.
    state.backup_delete(&backup, "admin").unwrap();
    assert!(!archive.exists());
    for _ in 0..50 {
        if !far.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!far.exists(), "dış hedefteki arşiv de silinir");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn balloon_target_is_applied_live_and_again_at_start() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&GuestSpec { memory_mb: 2048, ..spec("balonlu", &node) }, "admin").unwrap();
    let hot = || std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(id).display())).unwrap_or_default();
    let launch = || std::fs::read_to_string(format!("{}.args", cfg.qmp_path(id).display())).unwrap();
    let ok = |what: &str| assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{what}: {:?}", state.snapshot().tasks[0].message);

    // Balonu olmayan konukta hedef reddedilir; işlemci modeli komut satırına geçer.
    assert!(matches!(state.balloon_set(id, 1024, "admin"), Err(Error::InvalidSpec(m)) if m.contains("bellek balonu yok")));
    state.update_guest(id, &redox_core::GuestPatch { cpu_type: Some("Haswell".into()), balloon: Some(true), ..Default::default() }, "admin").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    ok("başlatma");
    let args = launch();
    assert!(args.contains("-cpu\nHaswell") && args.trim_end().ends_with("-device\nvirtio-balloon-pci,id=balloon0"), "{args}");
    assert!(!hot().contains("balloon"), "hedef yokken balona dokunulmaz");

    // Çalışırken: hedef hemen uygulanır (bayt olarak); 0 belleğin tamamını geri verir.
    state.balloon_set(id, 1024, "admin").unwrap();
    assert!(hot().contains(&format!("balloon {}", 1024u64 * 1024 * 1024)), "{}", hot());
    state.balloon_set(id, 0, "admin").unwrap();
    assert!(hot().contains(&format!("balloon {}", 2048u64 * 1024 * 1024)));
    state.balloon_set(id, 512, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].command, format!("vm balloon {id} 512"));

    // Yeniden açılışta kayıtlı hedef yeniden uygulanır.
    state.run_action(id, Action::Stop, "admin").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();
    ok("ikinci başlatma");
    assert!(hot().trim_end().ends_with(&format!("balloon {}", 512u64 * 1024 * 1024)), "{}", hot());
    // Kapalıyken hedef yalnızca kaydedilir.
    state.run_action(id, Action::Stop, "admin").unwrap();
    let before = hot();
    state.balloon_set(id, 768, "admin").unwrap();
    assert_eq!((hot(), state.snapshot().guest(id).unwrap().hw.balloon_mb), (before, 768));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
pub(super) async fn extra_disks_and_machine_state_follow_a_vm_to_another_server() {
    if !python_available() {
        return;
    }
    use redox_core::hw::Firmware;
    // İki sunucuda da UEFI kalıpları ve sahte TPM öykünücüsü var.
    let machine_config = || {
        let (mut cfg, dir) = config();
        cfg.ovmf_dir = dir.join("ovmf");
        cfg.swtpm_bin = script(&dir, "swtpm", FAKE_SWTPM);
        std::fs::create_dir_all(&cfg.ovmf_dir).unwrap();
        for name in ["OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd", "OVMF_CODE_4M.secboot.fd", "OVMF_VARS_4M.ms.fd"] {
            std::fs::write(cfg.ovmf_dir.join(name), name).unwrap();
        }
        (cfg, dir)
    };
    let (peer_cfg, peer_dir) = machine_config();
    let peer_cluster = local_cluster(&peer_cfg);
    let node = peer_cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("admin", "t", "es-sunucu-yonetici-tokeni").unwrap();
    users.add_token("izleyici", "t", "es-sunucu-izleyici-tokeni").unwrap();
    let peer = AppState::new(peer_cluster, Box::new(QemuDriver::new(peer_cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));

    // Kaynak: güvenli önyüklemeli UEFI, TPM ve numarası 2 olan tek ek disk (1 numara silinmiş).
    let secure = GuestSpec { hw: redox_core::Hardware { firmware: Firmware::Uefi, secure_boot: true, tpm: true, ..Default::default() }, ..spec("win11", &node) };
    let source = peer.create_guest(&secure, "admin").unwrap();
    assert_eq!((peer.disk_add(source, 2, "admin").unwrap(), peer.disk_add(source, 3, "admin").unwrap()), (1, 2));
    peer.disk_remove(source, 1, "admin").unwrap();
    peer.run_action(source, Action::Start, "admin").unwrap();
    peer.run_action(source, Action::Stop, "admin").unwrap();
    let there = peer.snapshot().guest(source).unwrap().clone();
    std::fs::write(peer_cfg.disk_path(source), "ana-disk").unwrap();
    std::fs::write(peer_cfg.extra_disk_of(&there, 2), "ek-disk-iki").unwrap();
    std::fs::write(peer_cfg.efivars_path(source, true), "kaynagin-degiskenleri").unwrap();
    std::fs::write(peer_cfg.tpm_dir(source).join("tpm2-00.permall"), "kaynagin-tpm-durumu").unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer_app = redox_server::app(peer.clone());
    tokio::spawn(async move { axum::serve(listener, peer_app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });

    let (cfg, dir) = machine_config();
    let hub = AppState::new(local_cluster(&cfg), Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    hub.peers().configure(redox_server::peers::parse_file(&format!("eski http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\n")).unwrap());
    let free_before: u64 = hub.snapshot().storages.iter().map(|s| s.free_gb()).sum();
    let mover = hub.clone();
    tokio::task::spawn_blocking(move || mover.import_from_peer("eski", source, "admin")).await.unwrap().unwrap();
    let task = hub.snapshot().tasks[0].clone();
    assert_eq!(task.status, TaskStatus::Ok, "{:?}", task.message);

    let copy = hub.snapshot().guests[0].clone();
    assert_eq!(copy.hw.disks.iter().map(|d| (d.id, d.size_gb, d.serial)).collect::<Vec<_>>(), there.hw.disks.iter().map(|d| (d.id, d.size_gb, d.serial)).collect::<Vec<_>>(), "ek disk numarası, boyutu ve serisiyle gelir");
    assert_eq!(std::fs::read_to_string(cfg.disk_path(copy.vmid)).unwrap(), "ana-disk");
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_of(&copy, 2)).unwrap(), "ek-disk-iki");
    assert_eq!(std::fs::read_to_string(cfg.efivars_path(copy.vmid, true)).unwrap(), "kaynagin-degiskenleri", "UEFI değişkenleri taşındı");
    assert_eq!(std::fs::read_to_string(cfg.tpm_dir(copy.vmid).join("tpm2-00.permall")).unwrap(), "kaynagin-tpm-durumu", "TPM durumu taşındı");
    let free_after: u64 = hub.snapshot().storages.iter().map(|s| s.free_gb()).sum();
    assert_eq!(free_before - free_after, copy.disk_gb + 3, "ek diskin yeri de ayrıldı");
    assert!(std::fs::read_dir(cfg.cloud_dir()).unwrap().next().is_none(), "geçici dosyalar (ek disk, arşiv) silinir");
    assert!(peer.snapshot().backups.is_empty() && std::fs::read_dir(peer_cfg.backups_dir()).unwrap().next().is_none(), "eşte geçici yedek ve arşiv kalmaz");
    // Taşınan konuk burada açılır ve taşınan depoyu kullanır (kalıptan yeniden oluşturulmaz).
    hub.run_action(copy.vmid, Action::Start, "admin").unwrap();
    assert_eq!(hub.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", hub.snapshot().tasks[0].message);
    assert_eq!(std::fs::read_to_string(cfg.efivars_path(copy.vmid, true)).unwrap(), "kaynagin-degiskenleri");
    hub.run_action(copy.vmid, Action::Stop, "admin").unwrap();

    // Makine durumu arşivi yalnızca yöneticiye verilir; arşivi olmayan ya da bilinmeyen yedek 404'tür.
    let kept = peer.backup_create(source, "admin").unwrap();
    let fetch = |token: &'static str, id: String| async move {
        let url = format!("http://127.0.0.1:{port}/api/v1/backups/{id}/machine");
        tokio::task::spawn_blocking(move || match ureq::get(&url).set("Authorization", &format!("Bearer {token}")).call() {
            Ok(r) => (r.status(), r.header("content-type").map(str::to_string)),
            Err(ureq::Error::Status(code, _)) => (code, None),
            Err(e) => panic!("{e}"),
        })
        .await
        .unwrap()
    };
    assert_eq!(fetch("es-sunucu-yonetici-tokeni", kept.clone()).await, (200, Some("application/x-tar".to_string())));
    assert_eq!(fetch("es-sunucu-izleyici-tokeni", kept.clone()).await.0, 403);
    assert_eq!(fetch("es-sunucu-yonetici-tokeni", "vm-999-1".to_string()).await.0, 404);

    // Ek disk içe aktarılamazsa burada yarım konuk kalmaz.
    std::fs::write(peer_cfg.extra_disk_of(&there, 2), "arkalikli").unwrap();
    // Aynı konuğun aynı saniyede ikinci yedeği alınamaz.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let before = hub.snapshot().guests.len();
    let mover = hub.clone();
    tokio::task::spawn_blocking(move || mover.import_from_peer("eski", source, "admin")).await.unwrap().unwrap();
    let task = hub.snapshot().tasks[0].clone();
    assert!(task.status == TaskStatus::Failed && task.message.as_deref().unwrap().contains("ek disk 2"), "{:?}", task.message);
    assert_eq!(hub.snapshot().guests.len(), before);
    assert!(std::fs::read_dir(cfg.cloud_dir()).unwrap().next().is_none());
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(peer_dir).unwrap();
}

#[test]
pub(super) fn guest_disks_move_between_storages() {
    let (mut cfg, dir) = config();
    let fast = dir.join("hizli");
    std::fs::create_dir_all(&fast).unwrap();
    cfg.storages = vec![("hizli".to_string(), fast.clone())];
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let mut g = guest(&cfg, "tasinan");
    d.create(&g).unwrap();
    d.add_disk(&g, 2, 1).unwrap();
    g.hw.disks = vec![redox_core::hw::ExtraDisk { id: 2, size_gb: 1, serial: 1 }];
    let (main, extra) = (cfg.disk_of(&g), cfg.extra_disk_of(&g, 2));
    std::fs::write(&main, "ana-disk-icerigi").unwrap();
    std::fs::write(&extra, "ek-disk-icerigi").unwrap();

    // Hedefte aynı adda dosya varsa hiçbir şey taşınmaz ve kaynaklar yerinde kalır.
    let clash = fast.join(extra.file_name().unwrap());
    std::fs::write(&clash, "baskasinin").unwrap();
    assert!(d.move_storage(&g, "hizli").unwrap_err().contains("zaten var"));
    assert!(main.exists() && extra.exists() && !fast.join(main.file_name().unwrap()).exists(), "yarım taşıma hedefte dosya bırakmaz");
    assert_eq!(std::fs::read_to_string(&clash).unwrap(), "baskasinin", "başkasının dosyasına dokunulmaz");
    std::fs::remove_file(&clash).unwrap();

    d.move_storage(&g, "hizli").unwrap();
    assert!(!main.exists() && !extra.exists(), "kaynaklar kopya tamamlandıktan sonra silinir");
    g.storage = "hizli".into();
    assert_eq!(std::fs::read_to_string(cfg.disk_of(&g)).unwrap(), "ana-disk-icerigi");
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_of(&g, 2)).unwrap(), "ek-disk-icerigi");
    assert!(std::fs::read_dir(&fast).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("tasiniyor")), "geçici dosya kalmaz");
    // Aynı dizine taşıma reddedilir; geri taşıma çalışır.
    assert!(d.move_storage(&g, "hizli").unwrap_err().contains("aynı dizinde"));
    d.move_storage(&g, "local").unwrap();
    assert!(main.exists() && extra.exists() && !cfg.disk_of(&g).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn storage_move_runs_as_a_task_and_updates_the_records() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    let fast = dir.join("hizli");
    std::fs::create_dir_all(&fast).unwrap();
    cfg.storages = vec![("hizli".to_string(), fast.clone())];
    let mut cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    cluster.storages.push(redox_core::Storage { id: "hizli".into(), node: Some(node.clone()), kind: redox_core::StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 100 });
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&spec("tasinan", &node), "admin").unwrap();
    let size = state.snapshot().guest(id).unwrap().total_disk_gb();
    std::fs::write(cfg.disk_path(id), "icerik").unwrap();

    // Çalışan konuk taşınmaz.
    state.run_action(id, Action::Start, "admin").unwrap();
    assert!(matches!(state.storage_move(id, "hizli", "admin"), Err(Error::RequiresStopped { .. })));
    state.run_action(id, Action::Stop, "admin").unwrap();
    assert!(state.storage_move(id, "yok", "admin").is_err());

    state.storage_move(id, "hizli", "admin").unwrap();
    let c = state.snapshot();
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].status), (format!("vm move {id} hizli").as_str(), TaskStatus::Ok), "{:?}", c.tasks[0].message);
    let g = c.guest(id).unwrap().clone();
    assert_eq!(g.storage, "hizli");
    assert_eq!(c.storages.iter().find(|s| s.id == "hizli").unwrap().used_gb, size, "pay yeni alana geçti");
    assert_eq!(std::fs::read_to_string(cfg.disk_of(&g)).unwrap(), "icerik");
    assert!(!cfg.disk_path(id).exists());
    // Taşınan konuk yeni yerindeki diskle açılır.
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    let args = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(id).display())).unwrap();
    assert!(args.contains(&fast.join(format!("vm-{id}-disk-0.qcow2")).display().to_string()), "{args}");
    state.run_action(id, Action::Stop, "admin").unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
