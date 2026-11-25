//! Askıya alma, çalışırken takma, VLAN, depolar, canlı konsol, paylaşılan klasörler.

use super::*;

#[test]
pub(super) fn suspend_saves_memory_and_start_resumes_from_it() {
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
pub(super) fn network_limit_that_cannot_be_applied_stops_the_guest() {
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
pub(super) fn disks_and_nics_are_hot_added_to_a_running_guest() {
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
pub(super) fn suspended_guests_resume_with_the_layout_they_were_suspended_in() {
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
pub(super) fn vlan_cards_get_a_named_tap_on_the_bridge() {
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
pub(super) fn guests_on_an_extra_storage_keep_their_disks_there() {
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
pub(super) async fn live_console_bridges_the_browser_to_the_guest_screen() {
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
pub(super) fn shared_folders_are_created_and_passed_to_the_guest() {
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
