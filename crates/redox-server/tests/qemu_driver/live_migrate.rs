//! Canlı taşıma: iki sunucu, sahte QEMU sürücüsü ve aralarında gerçek TCP bağlantıları.

use super::*;

/// Her test sürecine ayrı bir kapı aralığı: koşut çalışan testler birbirinin kapısını tutmasın.
fn ports(offset: u16) -> (u16, u16) {
    let first = 50_000 + (std::process::id() % 400) as u16 * 16 + offset * 4;
    (first, first + 3)
}

struct Pair {
    peer: AppState,
    peer_cfg: QemuConfig,
    peer_dir: PathBuf,
    hub: AppState,
    cfg: QemuConfig,
    dir: PathBuf,
    node: String,
}

/// Kaynak sunucu ("eski", gerçek bir TCP kapısında) ve bu sunucu.
async fn pair(offset: u16) -> Pair {
    let (peer_cfg, peer_dir) = config();
    let peer_cluster = local_cluster(&peer_cfg);
    let node = peer_cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("admin", "t", "es-sunucu-yonetici-tokeni").unwrap();
    users.add_token("izleyici", "t", "es-sunucu-izleyici-tokeni").unwrap();
    let peer = AppState::new(peer_cluster, Box::new(QemuDriver::new(peer_cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer_app = redox_server::app(peer.clone());
    tokio::spawn(async move { axum::serve(listener, peer_app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let (cfg, dir) = config();
    let hub = AppState::new(local_cluster(&cfg), Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    hub.peers().configure(
        redox_server::peers::parse_file(&format!("eski http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\nsalt http://127.0.0.1:{port} es-sunucu-izleyici-tokeni\n")).unwrap(),
    );
    hub.set_migrate(redox_server::state::MigrateSettings { ports: ports(offset), host: Some("127.0.0.1".into()), bind: "127.0.0.1".into() });
    Pair { peer, peer_cfg, peer_dir, hub, cfg, dir, node }
}

async fn migrate(hub: &AppState, name: &'static str, vmid: u32) -> Result<(), Error> {
    let hub = hub.clone();
    tokio::task::spawn_blocking(move || hub.migrate_from_peer(name, vmid, "admin")).await.unwrap()
}

/// En son taşıma görevi (başarısız taşımadan sonra yarım kopyanın silinmesi de günlüğe düşer).
fn last(state: &AppState) -> (String, TaskStatus, String) {
    let c = state.snapshot();
    let task = c.tasks.iter().find(|t| t.command.starts_with("vm migrate")).expect("taşıma görevi");
    (task.command.clone(), task.status, task.message.clone().unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(super) async fn a_running_vm_is_migrated_live_from_a_peer_server() {
    if !python_available() {
        return;
    }
    let Pair { peer, peer_cfg, peer_dir, hub, cfg, dir, node } = pair(0).await;
    let source = peer
        .create_guest(&GuestSpec { cores: 2, memory_mb: 512, disk_gb: 3, onboot: true, hw: redox_core::Hardware { restart: true, balloon: true, max_cores: 4, max_mem_mb: 1024, ..Default::default() }, ..spec("canli", &node) }, "admin")
        .unwrap();
    peer.update_guest(source, &redox_core::GuestPatch { tags: Some(vec!["web".into()]), ..Default::default() }, "admin").unwrap();
    peer.disk_add(source, 2, "admin").unwrap();
    std::fs::write(peer_cfg.disk_path(source), "kaynagin-asil-diski").unwrap();
    std::fs::write(peer_cfg.extra_disk_path(source, 1), "kaynagin-ek-diski").unwrap();
    peer.run_action(source, Action::Start, "admin").unwrap();
    // Çalışırken eklenen çekirdek ve bellek de konukla birlikte gelir.
    peer.cores_add(source, 3, "admin").unwrap();
    peer.memory_add(source, 640, "admin").unwrap();

    assert!(matches!(migrate(&hub, "yok", source).await, Err(Error::InvalidSpec(m)) if m.contains("tanımlı değil")));
    migrate(&hub, "eski", source).await.unwrap();
    assert_eq!(last(&hub), (format!("vm migrate eski/{source}"), TaskStatus::Ok, String::new()), "{:?}", hub.snapshot().tasks.iter().map(|t| (t.command.clone(), t.message.clone())).collect::<Vec<_>>());

    // Hedef: aynı kimlik, aynı donanım, çalışır durumda; diskler ve "bellek" kaynaktan gelmiş.
    let moved = hub.snapshot().guest(source).unwrap().clone();
    assert_eq!((moved.name.as_str(), moved.cores, moved.mem_total_mb, moved.disk_gb, moved.status, moved.onboot), ("canli", 3, 640, 3, GuestStatus::Running, true));
    assert_eq!((moved.hw.max_cores, moved.hw.max_mem_mb, moved.hw.dimms.clone(), moved.hw.disks.len(), moved.lock), (4, 1024, vec![128], 1, None));
    assert_eq!(moved.tags, ["web", "tasindi-eski"]);
    assert_eq!(std::fs::read_to_string(cfg.disk_path(source)).unwrap(), "kaynagin-asil-diski");
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_path(source, 1)).unwrap(), "kaynagin-ek-diski");
    let here = |suffix: &str| std::fs::read_to_string(format!("{}{suffix}", cfg.qmp_path(source).display())).unwrap_or_default();
    assert_eq!(here(".ram"), "canli", "bellek hedefe ulaşmış");
    let args = here(".args");
    assert!(args.ends_with("-incoming\ndefer\n-S") && args.contains("-smp\n3,maxcpus=4\n") && args.contains("pc-dimm,id=xdimm0,memdev=xmem0,slot=0\n"), "{args}");
    let (first, _) = ports(0);
    let log = here(".hot");
    assert!(log.contains(&format!("object-add tls-creds-psk migtls server\nnbd-server-start {first}\nnbd-server-add virtio0\nnbd-server-add xd1\nmigrate-incoming {}\nnbd-server-stop\n", first + 1)), "{log}");
    assert!(!cfg.run_dir().join(format!("{source}.mig")).exists(), "tek kullanımlık anahtar hedefte bırakılmaz");

    // Kaynak: konuk kapalı, kendiliğinden açılmayacak biçimde işaretli; diskleri ve kaydı duruyor.
    let left = peer.snapshot().guest(source).unwrap().clone();
    assert_eq!((left.status, left.onboot, left.hw.restart, left.lock), (GuestStatus::Stopped, false, false, None));
    assert!(left.tags.iter().any(|t| t == redox_core::MIGRATED_TAG), "{:?}", left.tags);
    assert_eq!(last(&peer), (format!("vm migrate {source}"), TaskStatus::Ok, String::new()));
    assert!(!peer_cfg.pid_path(source).exists() && !peer_cfg.run_dir().join(format!("{source}.mig")).exists());
    assert!(peer_cfg.disk_path(source).exists(), "kaynağın diski silinmez");
    let there = std::fs::read_to_string(format!("{}.hot", peer_cfg.qmp_path(source).display())).unwrap();
    assert!(there.contains(&format!("object-add tls-creds-psk migtls client\nblockdev-add nbd mig-virtio0 virtio0\nblockdev-mirror virtio0 mig-virtio0\nblockdev-add nbd mig-xd1 xd1\nblockdev-mirror xd1 mig-xd1\nmigrate {}\nblock-job-cancel mig-virtio0\nblock-job-cancel mig-xd1\n", first + 1)), "{there}");

    // Taşınan konuk hedefte olağan bir konuktur: durdurulur, yeniden açılır.
    hub.run_action(source, Action::Stop, "admin").unwrap();
    hub.run_action(source, Action::Start, "admin").unwrap();
    assert!(!here(".args").contains("-incoming"));
    hub.run_action(source, Action::Stop, "admin").unwrap();
    // Aynı kimlik burada doluyken ikinci kez taşınmaz (kaynakta yeniden açılsa bile).
    peer.run_action(source, Action::Start, "admin").unwrap();
    migrate(&hub, "eski", source).await.unwrap();
    assert!(last(&hub).1 == TaskStatus::Failed && last(&hub).2.contains("kimliği dolu"), "{:?}", last(&hub));
    assert_eq!(peer.snapshot().guest(source).unwrap().status, GuestStatus::Running);
    peer.run_action(source, Action::Stop, "admin").unwrap();
    let _ = std::fs::remove_dir_all(peer_dir);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(super) async fn a_failed_live_migration_leaves_the_guest_running_at_the_source() {
    if !python_available() {
        return;
    }
    let Pair { peer, peer_cfg, peer_dir, hub, cfg, dir, node } = pair(1).await;
    // Hedefte 100 kimliği dolsun diye değil: kaynaktaki kimlikler hedefte boş kalsın diye hedefe konuk eklenmez.
    let source = peer.create_guest(&spec("yerinde", &node), "admin").unwrap();
    std::fs::write(peer_cfg.disk_path(source), "yerinde-kalan-disk").unwrap();
    let nothing_here = |hub: &AppState| hub.snapshot().guests.is_empty() && !cfg.pid_path(source).exists() && !cfg.disk_path(source).exists() && !cfg.run_dir().join(format!("{source}.mig")).exists();
    let still_running = |peer: &AppState| {
        let g = peer.snapshot().guest(source).unwrap().clone();
        g.status == GuestStatus::Running && g.lock.is_none() && peer.driver().probe(&g).is_some_and(|p| p.status == GuestStatus::Running) && !peer_cfg.run_dir().join(format!("{source}.mig")).exists()
    };
    let marker = |name: &str| format!("{}.{name}", peer_cfg.qmp_path(source).display());

    // Kapalı konuk canlı taşınmaz.
    migrate(&hub, "eski", source).await.unwrap();
    assert!(last(&hub).1 == TaskStatus::Failed && last(&hub).2.contains("çalışmıyor"), "{:?}", last(&hub));
    assert_eq!(last(&hub).0, format!("vm migrate eski/{source}"));
    assert!(nothing_here(&hub));
    peer.run_action(source, Action::Start, "admin").unwrap();

    // Yetkisiz eş (izleyici token'ı) taşıma başlatamaz.
    migrate(&hub, "salt", source).await.unwrap();
    assert!(last(&hub).1 == TaskStatus::Failed && last(&hub).2.contains("yönetici"), "{:?}", last(&hub));
    assert!(nothing_here(&hub) && still_running(&peer));

    // Hedef belleği kabul etmezse: kaynak aynaları ve anahtarı kaldırır, konuk çalışmayı sürdürür; hedefte iz kalmaz.
    std::fs::write(marker("nomigrate"), b"").unwrap();
    migrate(&hub, "eski", source).await.unwrap();
    assert!(last(&hub).1 == TaskStatus::Failed && last(&hub).2.contains("eş sunucu konuğu gönderemedi") && last(&hub).2.contains("bellek taşınamadı"), "{:?}", last(&hub));
    assert!(last(&peer).1 == TaskStatus::Failed && last(&peer).2.contains("load of migration failed"), "{:?}", last(&peer));
    assert!(nothing_here(&hub) && still_running(&peer));
    let log = std::fs::read_to_string(marker("hot")).unwrap();
    assert!(log.ends_with("migrate_cancel\nblock-job-cancel mig-virtio0\nblockdev-del mig-virtio0\nobject-del migtls\n"), "{log}");
    std::fs::remove_file(marker("nomigrate")).unwrap();

    // Disk aynası hiç kurulamazsa bellek taşınmaz.
    std::fs::write(marker("nomirror"), b"").unwrap();
    migrate(&hub, "eski", source).await.unwrap();
    assert!(last(&hub).1 == TaskStatus::Failed && last(&hub).2.contains("disk aynalama yarıda kesildi"), "{:?}", last(&hub));
    assert!(nothing_here(&hub) && still_running(&peer));
    assert!(!std::fs::read_to_string(marker("hot")).unwrap().rsplit("blockdev-mirror").next().unwrap().contains("migrate 5"), "ayna yokken bellek gönderilmez");
    std::fs::remove_file(marker("nomirror")).unwrap();

    // Ayna, bellek taşınırken koparsa hedef eksik diskle sürdürülmez: konuk kaynakta yeniden çalıştırılır.
    std::fs::write(marker("dropmirror"), b"").unwrap();
    migrate(&hub, "eski", source).await.unwrap();
    assert!(last(&hub).1 == TaskStatus::Failed && last(&hub).2.contains("disk aynası koptu") && last(&hub).2.contains("bu sunucuda sürdürüldü"), "{:?}", last(&hub));
    assert!(nothing_here(&hub) && still_running(&peer));
    std::fs::remove_file(marker("dropmirror")).unwrap();

    // Taşınamayan donanım: hedef, konuk oluşturmadan vazgeçer.
    peer.run_action(source, Action::Stop, "admin").unwrap();
    peer.update_guest(source, &redox_core::GuestPatch { firmware: Some(redox_core::hw::Firmware::Uefi), ..Default::default() }, "admin").unwrap_or_default();
    peer.mutate(|c| {
        let g = c.guest_mut_for_import(source).unwrap();
        (g.hw.firmware, g.status) = (redox_core::hw::Firmware::Uefi, GuestStatus::Running);
    });
    migrate(&hub, "eski", source).await.unwrap();
    assert!(last(&hub).1 == TaskStatus::Failed && last(&hub).2.contains("UEFI"), "{:?}", last(&hub));
    assert!(nothing_here(&hub));
    peer.mutate(|c| {
        let g = c.guest_mut_for_import(source).unwrap();
        (g.hw.firmware, g.status) = (redox_core::hw::Firmware::Bios, GuestStatus::Stopped);
    });

    // Engeller kalkınca aynı konuk taşınır.
    peer.run_action(source, Action::Start, "admin").unwrap();
    migrate(&hub, "eski", source).await.unwrap();
    assert_eq!(last(&hub).1, TaskStatus::Ok, "{:?}", last(&hub));
    assert_eq!(std::fs::read_to_string(cfg.disk_path(source)).unwrap(), "yerinde-kalan-disk");
    assert_eq!(peer.snapshot().guest(source).unwrap().status, GuestStatus::Stopped);
    hub.run_action(source, Action::Stop, "admin").unwrap();
    let _ = std::fs::remove_dir_all(peer_dir);
    let _ = std::fs::remove_dir_all(dir);
}

/// Birbirini eş olarak tanıyan iki sunucu ("a" ve "b"), ikisi de gerçek TCP kapısında.
async fn mutual(offset: u16) -> ((AppState, QemuConfig, PathBuf), (AppState, QemuConfig, PathBuf)) {
    let auth = || {
        let mut users = UserFile::default();
        users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
        users.add_token("admin", "t", "es-sunucu-yonetici-tokeni").unwrap();
        Auth::new(users, None, false, Cost::Test)
    };
    let mut made = Vec::new();
    for _ in 0..2 {
        let (cfg, dir) = config();
        let state = AppState::new(local_cluster(&cfg), Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, auth());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = redox_server::app(state.clone());
        tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
        made.push((state, cfg, dir, port));
    }
    let (b, a) = (made.pop().unwrap(), made.pop().unwrap());
    a.0.peers().configure(redox_server::peers::parse_file(&format!("b http://127.0.0.1:{} es-sunucu-yonetici-tokeni\n", b.3)).unwrap());
    b.0.peers().configure(redox_server::peers::parse_file(&format!("a http://127.0.0.1:{} es-sunucu-yonetici-tokeni\n", a.3)).unwrap());
    a.0.set_migrate(redox_server::state::MigrateSettings { ports: ports(offset), host: Some("127.0.0.1".into()), bind: "127.0.0.1".into() });
    b.0.set_migrate(redox_server::state::MigrateSettings { ports: ports(offset + 1), host: Some("127.0.0.1".into()), bind: "127.0.0.1".into() });
    ((a.0, a.1, a.2), (b.0, b.1, b.2))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
pub(super) async fn maintenance_evacuates_running_vms_to_a_peer_and_they_can_come_back() {
    if !python_available() {
        return;
    }
    let ((a, a_cfg, a_dir), (b, b_cfg, b_dir)) = mutual(2).await;
    let node = a.snapshot().nodes[0].id.clone();
    let moving = a.create_guest(&spec("giden", &node), "admin").unwrap();
    let pinned = a.create_guest(&GuestSpec { hw: redox_core::Hardware { ci_ssh_key: Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxvcmVtaXBzdW1kb2xvcnNpdGFtZXRjb25zZWN0 ben@dizustu".into()), ..Default::default() }, ..spec("kalan", &node) }, "admin").unwrap();
    let idle = a.create_guest(&spec("kapali", &node), "admin").unwrap();
    std::fs::write(a_cfg.disk_path(moving), "giden-konugun-diski").unwrap();
    a.run_action(moving, Action::Start, "admin").unwrap();
    a.run_action(pinned, Action::Start, "admin").unwrap();
    let evacuate = |as_name: Option<&'static str>, peer: &'static str| {
        let a = a.clone();
        async move { tokio::task::spawn_blocking(move || a.maintenance_evacuate(peer, as_name, true, "admin")).await.unwrap() }
    };
    let last = |s: &AppState, prefix: &str| {
        let c = s.snapshot();
        let t = c.tasks.iter().find(|t| t.command.starts_with(prefix)).unwrap_or_else(|| panic!("{prefix} görevi yok")).clone();
        (t.command, t.status, t.message.unwrap_or_default())
    };

    // Eş tanımlı değilse ya da eş bu sunucuyu o adla tanımıyorsa bakıma girilmez.
    assert!(matches!(evacuate(Some("a"), "yok").await, Err(Error::InvalidSpec(m)) if m.contains("tanımlı değil")));
    assert!(matches!(evacuate(Some("baska"), "b").await, Err(Error::InvalidSpec(m)) if m.contains("adıyla tanımıyor")));
    assert!(matches!(evacuate(None, "b").await, Err(Error::InvalidSpec(m)) if m.contains("bulunamadı") && m.contains("--as")), "ad, adresten bulunamıyorsa sorulur");
    assert!(!a.in_maintenance());

    // Boşaltma: canlı taşınabilen konuk eşe geçer; taşınamayan (cloud-init sürücülü) konuk kapatılır.
    assert_eq!(evacuate(Some("a"), "b").await.unwrap(), 1, "yalnızca canlı taşınabilen konuk sayılır");
    assert!(a.in_maintenance());
    let logs = |s: &AppState| s.snapshot().tasks.iter().map(|t| (t.command.clone(), t.status, t.message.clone())).collect::<Vec<_>>();
    assert_eq!(last(&a, "maintenance evacuate"), ("maintenance evacuate b 1/1".to_string(), TaskStatus::Ok, String::new()), "a: {:?}
b: {:?}", logs(&a), logs(&b));
    let there = b.snapshot().guest(moving).unwrap().clone();
    assert_eq!((there.name.as_str(), there.status), ("giden", GuestStatus::Running));
    assert_eq!(std::fs::read_to_string(b_cfg.disk_path(moving)).unwrap(), "giden-konugun-diski");
    let here = a.snapshot();
    assert_eq!(here.guest(moving).unwrap().status, GuestStatus::Stopped);
    assert!(here.guest(moving).unwrap().tags.iter().any(|t| t == redox_core::MIGRATED_TAG));
    assert_eq!((here.guest(pinned).unwrap().status, here.guest(idle).unwrap().status), (GuestStatus::Stopped, GuestStatus::Stopped));
    assert_eq!(here.maintenance_resume, vec![pinned], "kapatılan konuk bakım bitince açılacak; taşınan ve zaten kapalı olan değil");

    // Bakım biter: kapatılan konuk yeniden açılır, taşınan konuk eşte kalır.
    assert_eq!(a.maintenance_off("admin"), 1);
    assert_eq!((a.snapshot().guest(pinned).unwrap().status, a.snapshot().guest(moving).unwrap().status), (GuestStatus::Running, GuestStatus::Stopped));

    // Geri dönüş: konuk aynı kimlikle gelir ve burada kalan bayat kopyanın yerini alır.
    std::fs::write(b_cfg.disk_path(moving), "este-degisen-disk").unwrap();
    let back = a.clone();
    tokio::task::spawn_blocking(move || back.migrate_from_peer("b", moving, "admin")).await.unwrap().unwrap();
    assert_eq!(last(&a, "vm migrate b/"), (format!("vm migrate b/{moving}"), TaskStatus::Ok, String::new()));
    let home = a.snapshot().guest(moving).unwrap().clone();
    assert_eq!((home.status, home.tags.iter().any(|t| t == redox_core::MIGRATED_TAG)), (GuestStatus::Running, false));
    assert_eq!(std::fs::read_to_string(a_cfg.disk_path(moving)).unwrap(), "este-degisen-disk", "bayat kopya değil, eşteki disk gelir");
    assert_eq!(b.snapshot().guest(moving).unwrap().status, GuestStatus::Stopped);

    // Geride kalan kopya elle başlatılmışsa artık asıl konuktur: üzerine yazılmaz.
    b.run_action(moving, Action::Start, "admin").unwrap();
    b.run_action(moving, Action::Stop, "admin").unwrap();
    assert!(!b.snapshot().guest(moving).unwrap().tags.iter().any(|t| t == redox_core::MIGRATED_TAG));
    let again = b.clone();
    tokio::task::spawn_blocking(move || again.migrate_from_peer("a", moving, "admin")).await.unwrap().unwrap();
    let refused = last(&b, "vm migrate a/");
    assert!(refused.1 == TaskStatus::Failed && refused.2.contains("kimliği dolu"), "{refused:?}");
    assert_eq!(a.snapshot().guest(moving).unwrap().status, GuestStatus::Running);

    a.run_action(moving, Action::Stop, "admin").unwrap();
    a.run_action(pinned, Action::Stop, "admin").unwrap();
    let _ = std::fs::remove_dir_all(a_dir);
    let _ = std::fs::remove_dir_all(b_dir);
}
