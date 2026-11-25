//! Eş sunucudan taşıma.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn a_stopped_vm_is_copied_from_a_peer_server() {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn a_running_vm_is_replicated_from_a_peer_without_touching_it() {
    if !python_available() {
        return;
    }
    let (peer_cfg, peer_dir) = config();
    let peer_cluster = local_cluster(&peer_cfg);
    let node = peer_cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "t", "es-sunucu-yonetici-tokeni").unwrap();
    let peer = AppState::new(peer_cluster, Box::new(QemuDriver::new(peer_cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let source = peer.create_guest(&spec("uretim", &node), "admin").unwrap();
    std::fs::write(peer_cfg.disk_path(source), "birinci-surum").unwrap();
    peer.run_action(source, Action::Start, "admin").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer_app = redox_server::app(peer.clone());
    tokio::spawn(async move { axum::serve(listener, peer_app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });

    let (cfg, dir) = config();
    let hub = AppState::new(local_cluster(&cfg), Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    hub.peers().configure(redox_server::peers::parse_file(&format!("ankara http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\n")).unwrap());
    let blocking = |hub: &AppState, f: fn(&AppState) -> usize| {
        let hub = hub.clone();
        async move { tokio::task::spawn_blocking(move || f(&hub)).await.unwrap() }
    };
    assert!(matches!(hub.replica_put("yok", source, 60, "admin"), Err(Error::InvalidSpec(m)) if m.contains("tanımlı değil")));
    hub.replica_put("ankara", source, 60, "admin").unwrap();

    // İlk tur: kapalı bir yedek kopya oluşur; kaynak çalışmaya devam eder (askıya alınmaz).
    assert_eq!(blocking(&hub, |h| h.replica_tick()).await, 1);
    let c = hub.snapshot();
    let first = c.replicas[0].standby.expect("yedek kopya");
    let copy = c.guest(first).unwrap().clone();
    assert_eq!((copy.name.as_str(), copy.status, copy.tags.clone()), ("uretim-kopya", GuestStatus::Stopped, vec!["kopya-ankara".to_string()]));
    assert_eq!(std::fs::read_to_string(cfg.disk_path(first)).unwrap(), "birinci-surum");
    assert!(c.tasks.iter().any(|t| t.command == format!("vm replicate ankara/{source}") && t.status == TaskStatus::Ok), "{:?}", c.tasks.first());
    let left = peer.snapshot().guest(source).unwrap().clone();
    assert_eq!((left.status, left.hw.suspended), (GuestStatus::Running, false), "kaynağa dokunulmaz");
    assert!(peer.snapshot().backups.is_empty(), "eşte geçici yedek kalmaz");
    assert_eq!(blocking(&hub, |h| h.replica_tick()).await, 0, "aralık dolmadan yeni tur başlamaz");

    // Kaynak değişir; elle başlatılan tur kopyayı tazeler ve eski kopyayı siler.
    std::fs::write(peer_cfg.disk_path(source), "ikinci-surum").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let run = {
        let hub = hub.clone();
        tokio::task::spawn_blocking(move || hub.replica_run("ankara", source)).await.unwrap()
    };
    run.unwrap();
    let c = hub.snapshot();
    let second = c.replicas[0].standby.unwrap();
    assert_eq!(c.guests.len(), 1, "eski kopya silinir: {:?}", c.guests.iter().map(|g| g.vmid).collect::<Vec<_>>());
    assert_eq!(std::fs::read_to_string(cfg.disk_path(second)).unwrap(), "ikinci-surum");

    assert_eq!(c.replicas[0].delta, Some((1, 1)), "küçük disk tek bloktur ve değişmiştir");

    // Artımlı: üç blokluk diskin yalnızca ortadaki bloğu değişir; yalnızca o blok indirilir.
    let block = redox_server::delta::BLOCK as usize;
    let mut content = vec![0x41u8; 2 * block + 4096];
    std::fs::write(peer_cfg.disk_path(source), &content).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    {
        let hub = hub.clone();
        tokio::task::spawn_blocking(move || hub.replica_run("ankara", source)).await.unwrap().unwrap();
    }
    assert_eq!(hub.snapshot().replicas[0].delta, Some((3, 3)), "içerik tümüyle değişti: üç blok da iner");
    content[block + 100] = 0x42;
    std::fs::write(peer_cfg.disk_path(source), &content).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    {
        let hub = hub.clone();
        tokio::task::spawn_blocking(move || hub.replica_run("ankara", source)).await.unwrap().unwrap();
    }
    let c = hub.snapshot();
    let second = c.replicas[0].standby.unwrap();
    assert_eq!(c.replicas[0].delta, Some((1, 3)), "yalnızca değişen blok indirildi");
    assert_eq!(std::fs::read(cfg.disk_path(second)).unwrap(), content, "kopya kaynağın aynısı");
    assert_eq!(c.guests.len(), 1);
    assert!(std::fs::read_dir(peer_cfg.backups_dir()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".ham")), "kaynakta ham önbellek kalmaz");
    assert!(std::fs::read_dir(cfg.cloud_dir()).unwrap().next().is_none(), "hedefte geçici dosya kalmaz");

    // Devralma: kopya başlatılır; çalışan kopyanın üzerine yazılmaz.
    hub.run_action(second, Action::Start, "admin").unwrap();
    let refused = {
        let hub = hub.clone();
        tokio::task::spawn_blocking(move || hub.replica_run("ankara", source)).await.unwrap()
    };
    assert!(refused.is_err());
    assert!(hub.snapshot().replicas[0].error.as_deref().is_some_and(|e| e.contains("devralınmış")));
    hub.run_action(second, Action::Stop, "admin").unwrap();
    peer.run_action(source, Action::Stop, "admin").unwrap();
    hub.replica_remove("ankara", source, "admin").unwrap();
    assert!(hub.snapshot().replicas.is_empty() && hub.snapshot().guest(second).is_ok(), "kaldırınca kopya olağan konuk olarak kalır");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(peer_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn a_standby_copy_takes_over_only_when_a_witness_agrees_the_source_is_dead() {
    if !python_available() {
        return;
    }
    let admin = || {
        let mut users = UserFile::default();
        users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
        users.add_token("admin", "t", "es-sunucu-yonetici-tokeni").unwrap();
        Auth::new(users, None, false, Cost::Test)
    };
    async fn serve(state: AppState) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = redox_server::app(state);
        (port, tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() }))
    }
    // Kaynak sunucu (ankara).
    let (peer_cfg, peer_dir) = config();
    let peer_cluster = local_cluster(&peer_cfg);
    let node = peer_cluster.nodes[0].id.clone();
    let peer = AppState::new(peer_cluster, Box::new(QemuDriver::new(peer_cfg.clone()).unwrap()), Duration::ZERO, None, admin());
    let source = peer.create_guest(&spec("uretim", &node), "admin").unwrap();
    let (port, source_server) = serve(peer.clone()).await;
    // Hakem sunucu (izmir): kaynağı kendi eşi olarak izler.
    let witness = AppState::new(redox_core::Cluster::demo(1_700_000_000), Box::new(redox_core::SimDriver::default()), Duration::ZERO, None, admin());
    witness.peers().configure(redox_server::peers::parse_file(&format!("kaynak http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\n")).unwrap());
    let (witness_port, _witness_server) = serve(witness.clone()).await;
    // Bu sunucu.
    let (cfg, dir) = config();
    let hub = AppState::new(local_cluster(&cfg), Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let both = format!("ankara http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\nizmir http://127.0.0.1:{witness_port} es-sunucu-yonetici-tokeni\n");
    let alone = format!("ankara http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\n");
    hub.peers().configure(redox_server::peers::parse_file(&alone).unwrap());
    hub.replica_put("ankara", source, 60, "admin").unwrap();
    assert!(matches!(hub.replica_failover_set("ankara", source, Some(5), "admin"), Err(Error::InvalidSpec(m)) if m.contains("hakem")), "hakemsiz açılmaz");
    hub.peers().configure(redox_server::peers::parse_file(&both).unwrap());
    hub.replica_failover_set("ankara", source, Some(5), "admin").unwrap();
    // `step`: eşleri yoklar ve devralma döngüsünü verilen saatte çalıştırır.
    async fn step(hub: &AppState, witness: Option<&AppState>, at: u64) -> usize {
        let (hub, witness) = (hub.clone(), witness.cloned());
        tokio::task::spawn_blocking(move || {
            if let Some(w) = witness {
                w.peers().poll(at);
            }
            hub.peers().poll(at);
            hub.failover_tick_at(at)
        })
        .await
        .unwrap()
    }
    {
        let hub = hub.clone();
        assert_eq!(tokio::task::spawn_blocking(move || hub.replica_tick()).await.unwrap(), 1);
    }
    let standby = hub.snapshot().replicas[0].standby.expect("yedek kopya");
    let t0 = redox_server::state::now();
    assert_eq!(step(&hub, Some(&witness), t0 + 3600).await, 0, "kaynak ayakta: hiçbir şey olmaz");
    assert_eq!(hub.snapshot().replicas[0].down_since, None);

    // Kaynak ölür. Süre dolmadan devralınmaz.
    source_server.abort();
    let _ = source_server.await;
    assert_eq!(step(&hub, None, t0 + 4000).await, 0);
    assert_eq!(hub.snapshot().replicas[0].down_since, Some(t0 + 4000));
    assert_eq!(step(&hub, None, t0 + 4000 + 299).await, 0);
    // Süre doldu ama hakem kaynağı hâlâ çalışır görüyor (kendi yoklaması eski): devralınmaz.
    assert_eq!(step(&hub, None, t0 + 4300).await, 0);
    let c = hub.snapshot();
    assert!(c.replicas[0].failover_note.as_deref().unwrap().contains("çalışır görüyor"), "{:?}", c.replicas[0].failover_note);
    assert_eq!(c.guest(standby).unwrap().status, GuestStatus::Stopped);
    // Hakeme ulaşılamıyorsa (bu sunucu yalnız kalmışsa) da devralınmaz.
    hub.peers().configure(redox_server::peers::parse_file(&alone).unwrap());
    assert_eq!(step(&hub, None, t0 + 4400).await, 0);
    assert!(hub.snapshot().replicas[0].failover_note.as_deref().unwrap().contains("hiçbir hakem"));
    hub.peers().configure(redox_server::peers::parse_file(&both).unwrap());
    // Hakem de kaynağın öldüğünü görünce yedek kopya başlatılır; yalnızca bir kez.
    assert_eq!(step(&hub, Some(&witness), t0 + 4500).await, 1);
    let c = hub.snapshot();
    assert_eq!((c.guest(standby).unwrap().status, c.replicas[0].failed_over, c.replicas[0].failover_note.clone()), (GuestStatus::Running, Some(t0 + 4500), None));
    assert!(c.tasks.iter().any(|t| t.command == format!("vm failover ankara/{source}") && t.status == TaskStatus::Ok));
    assert_eq!(step(&hub, Some(&witness), t0 + 9000).await, 0);
    hub.run_action(standby, Action::Stop, "admin").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(peer_dir).unwrap();
}
