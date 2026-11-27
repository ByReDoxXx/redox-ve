//! Zamanlanmış yedek, canlı işlemler, uyarılar ve yeni API uçları.

use super::*;

#[tokio::test]
pub(super) async fn backup_schedule_is_edited_from_the_guest_form() {
    let (app, state) = setup();
    let form = settings(&app, "/guests/103").await;
    assert!(form.contains(r#"name="backup_time" type="time""#) && form.contains(r#"name="backup_keep" type="text" inputmode="numeric" value="7""#));

    let base = "name=win-build&os=windows-2022&tags=&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_time=03%3A30&backup_keep=3")).await.contains("notice="));
    let g = state.snapshot().guest(103).unwrap().clone();
    assert_eq!((g.backup_time.as_deref(), g.backup_keep), (Some("03:30"), 3));
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(r#"her gün <b class="mono">03:30</b>"#) && tab.contains("son 3 yedek saklanır"), "zamanlama yedekler sekmesinde görünür");

    for bad in ["backup_time=25%3A00&backup_keep=3", "backup_time=03%3A30&backup_keep=0", "backup_time=03%3A30&backup_keep=x"] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await.contains("&error="), "{bad}");
    }
    // Zamanlayıcı turu yedeği alır ve "otomatik" olarak işaretler.
    assert_eq!(state.scheduler_tick("2026-11-24", 300), 1);
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains(r#"<span class="tag">otomatik</span>"#));
    // Saati boşaltmak zamanlamayı kapatır.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_time=&backup_keep=3")).await.contains("notice="));
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains("Otomatik yedek kapalı."));
}

#[tokio::test]
pub(super) async fn live_snapshot_and_backup_follow_driver_capability() {
    // Simülasyon: çalışan konukta kapalı.
    let (sim, _) = setup();
    let tab = page(&sim, "/guests/102?tab=goruntuler").await;
    assert!(tab.contains("için önce konuğu kapatın") && tab.contains(r#"required disabled>"#));
    assert!(post_ok(&sim, "/guests/102/snapshots", "name=canli").await.contains("error="));

    // Canlı işlem yapabilen sürücü: çalışan sanal makinede açık.
    let (app, state) = setup_real();
    let tab = page(&app, "/guests/102?tab=goruntuler").await;
    assert!(tab.contains("bellek durumunu da içerir") && !tab.contains(r#"required disabled>"#));
    assert!(post_ok(&app, "/guests/102/snapshots", "name=canli").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/backups", "").await.contains("notice="));
    let c = state.snapshot();
    assert_eq!((c.guest(102).unwrap().snapshots.len(), c.backups.len(), c.guest(102).unwrap().status), (1, 1, GuestStatus::Running));

    // Üretim denetim noktası: belleksiz; çalışan konukta ona dönülemez.
    assert!(tab.contains(r#"name="production""#));
    assert!(post_ok(&app, "/guests/102/snapshots", "name=uretim&production=1").await.contains("notice="));
    let snaps = state.snapshot().guest(102).unwrap().snapshots.clone();
    assert!(snaps[1].disk_only && !snaps[0].disk_only);
    assert!(page(&app, "/guests/102?tab=goruntuler").await.contains("yalnızca disk"));
    assert!(post_ok(&app, "/guests/102/snapshots/uretim/rollback", "").await.contains("error="), "belleksiz görüntüye dönmek için konuk kapatılır");
    assert!(post_ok(&app, "/guests/102/snapshots/canli/rollback", "").await.contains("notice="));

    let backups = page(&app, "/guests/102?tab=yedekler").await;
    assert!(backups.contains("Şimdi Yedekle") && !backups.contains("Geri Yükle"), "geri yükleme çalışan konukta sunulmaz");
    assert!(backups.contains("Geri yüklemek için önce konuğu kapatın."));
    let id = c.backups[0].id.clone();
    assert!(post_ok(&app, &format!("/backups/{id}/restore"), "").await.contains("error="));
}

#[tokio::test]
pub(super) async fn dashboard_lists_active_alerts() {
    let (app, state) = setup();
    let home = page(&app, "/").await;
    assert!(home.contains(r#"<ul class="attn""#) && home.contains("nfs-yedek deposu %93 dolu"), "dolan depolama ana sayfada görünür");
    state.mutate(|c| {
        c.observe_storage("nfs-yedek", "pve-ist-01", 100, 8000);
        c.nodes[2].status = redox_core::NodeStatus::Offline;
    });
    let home = page(&app, "/").await;
    assert!(!home.contains("nfs-yedek deposu") && home.contains("pve-ank-01 düğümü çevrimdışı"));
    // Olaylar: açılıştan sonraki değişiklikler bir kez döner.
    let (_, st) = setup();
    assert!(st.alerts_tick().is_empty());
    st.mutate(|c| c.nodes[0].status = redox_core::NodeStatus::Offline);
    assert_eq!(st.alerts_tick().len(), 1);
    assert!(st.alerts_tick().is_empty());
}

#[tokio::test]
pub(super) async fn api_clone_resize_and_exec() {
    let (app, state) = setup_real();
    let (status, v) = api(&app, post_json("/api/v1/guests/103/clone", &json!({"name": "kopya"}))).await;
    assert_eq!((status, v["vmid"].as_u64()), (StatusCode::ACCEPTED, Some(105)));
    let (status, v) = api(&app, post_json("/api/v1/guests/100/clone", &json!({"name": "canli"}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("requires_stopped")));

    let (status, t) = api(&app, post_json("/api/v1/guests/103/resize", &json!({"disk_gb": 300}))).await;
    assert_eq!((status, t["command"].as_str()), (StatusCode::ACCEPTED, Some("vm resize 103 300G")));
    assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 300);
    let (status, v) = api(&app, post_json("/api/v1/guests/103/resize", &json!({"disk_gb": 1}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_spec")));

    let (status, v) = api(&app, post_json("/api/v1/guests/200/exec", &json!({"command": "id -u"}))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(v["output"].as_str().unwrap().contains("dns-01 üzerinde: id -u"));
    let (status, v) = api(&app, post_json("/api/v1/guests/200/exec", &json!({"command": "false"}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("driver_error")));
    let (status, _) = api(&app, post_json("/api/v1/guests/202/exec", &json!({"command": "id"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "duran konteyner");

    let viewer = as_viewer(Method::POST, "/api/v1/guests/200/exec").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"command":"id"}"#)).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    // PATCH ile zamanlama.
    let patch = request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"backup_time":"04:15","backup_keep":2}"#)).unwrap();
    let (status, g) = api(&app, patch).await;
    assert_eq!((status, g["backup_time"].as_str(), g["backup_keep"].as_u64()), (StatusCode::OK, Some("04:15"), Some(2)));
}

/// Uygulamayı gerçek bir TCP kapısında çalıştırır (eş sunucu rolü).
pub(super) async fn serve_on_loopback(app: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn cluster_page_shows_peer_guests_read_only() {
    let (hub, hub_state) = setup();
    // Yapılandırma yokken sayfa nasıl kurulacağını anlatır.
    let empty = page(&hub, "/cluster").await;
    assert!(empty.contains("Eş sunucu tanımlı değil") && empty.contains("--peers DOSYA"));
    assert!(page(&hub, "/").await.contains(r#"href="/cluster""#), "kenar çubuğunda bağlantı");

    let (peer_app, peer_state) = setup();
    let id = peer_state.mutate(|c| {
        c.name = "izmir-dc".to_string();
        c.guests[0].name = "uzak-konuk".to_string();
        c.guests[0].vmid
    });
    let port = serve_on_loopback(peer_app).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let file = format!(
        "izmir http://127.0.0.1:{port} {VIEWER_TOKEN}\nyanlis http://127.0.0.1:{port} rdx_gecersiz_token_0123456789\nkapali http://127.0.0.1:{closed} {VIEWER_TOKEN}\n"
    );
    hub_state.peers().configure(redox_server::peers::parse_file(&file).unwrap());
    assert!(page(&hub, "/cluster").await.contains("Bekleniyor"), "ilk okumadan önce");

    let st = hub_state.clone();
    tokio::task::spawn_blocking(move || st.peers().poll(1_000)).await.unwrap();

    let views = hub_state.peers().views();
    assert!(views[0].online() && views[0].guests.iter().any(|g| g.vmid == id && g.name == "uzak-konuk"));
    assert_eq!(views[1].error.as_deref(), Some("token reddedildi (401)"));
    assert!(views[2].error.as_deref().unwrap().starts_with("erişilemiyor"));

    let html = page(&hub, "/cluster").await;
    assert!(html.contains("1/3 eş erişilebilir"), "özet");
    assert!(html.contains(&format!(r#"href="http://127.0.0.1:{port}/guests/{id}""#)), "konuk, kendi sunucusuna bağlanır");
    assert!(html.contains("uzak-konuk") && html.contains("izmir-dc") && html.contains("token reddedildi (401)") && html.contains("erişilemiyor"));
    assert!(!html.contains(VIEWER_TOKEN) && !html.contains("rdx_gecersiz"), "token sayfaya sızmaz");
    assert!(!html.contains(&format!(r#"action="http://127.0.0.1:{port}"#)), "eşe işlem formu yok: salt okunur");

    // API aynı bilgiyi verir; token yine yok. Salt okunur kullanıcı da görebilir.
    let (status, _, body) = send(&hub, as_viewer(Method::GET, "/api/v1/peers").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("uzak-konuk") && body.contains(r#""name":"izmir""#) && !body.contains(VIEWER_TOKEN), "{body}");
    let (status, _, _) = send(&hub, anon(Method::GET, "/api/v1/peers").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
pub(super) async fn cloud_init_fields_are_validated_and_shown() {
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxvcmVtaXBzdW1kb2xvcnNpdGFtZXRjb25zZWN0 ben@dizustu";
    let (app, state) = setup();
    // Simülasyonda bölüm gösterilmez; gerçek sürücüde imaj yoksa nereye konacağı yazar.
    assert!(!page(&app, "/guests/new?kind=vm").await.contains("cloud-init"));
    let (real, _) = setup_real();
    let form = page(&real, "/guests/new?kind=vm").await;
    assert!(form.contains(r#"name="ci_ssh_key""#) && form.contains("cloud/ klasörüne"), "gerçek sürücüde bölüm var");
    assert!(!page(&real, "/guests/new?kind=ct").await.contains(r#"name="ci_ssh_key""#), "konteynerde yok");

    let base = json!({"name": "bulut", "kind": "vm", "node": "pve-ank-01", "cores": 1, "memory_mb": 512, "disk_gb": 8, "storage": "local-lvm"});
    let with = |extra: Value| {
        let mut v = base.clone();
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    for (extra, needle) in [
        (json!({"image": "../debian.qcow2", "ci_ssh_key": KEY}), "geçersiz bulut imajı"),
        (json!({"ci_user": "root", "ci_ssh_key": KEY}), "geçersiz cloud-init kullanıcı"),
        (json!({"ci_ssh_key": "ssh-ed25519 AAAA\nruncmd: [reboot]"}), "geçersiz SSH açık anahtarı"),
        (json!({"kind": "ct", "ci_ssh_key": KEY}), "yalnızca sanal makinelerde"),
    ] {
        let (status, body) = api(&app, post_json("/api/v1/guests", &with(extra))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body["error"].as_str().unwrap().contains(needle), "{body}");
    }
    // Gerçek sürücüde imaj sunucuda bulunmalı.
    let (status, body) = api(&real, post_json("/api/v1/guests", &with(json!({"image": "yok.qcow2", "ci_ssh_key": KEY})))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("bulut imajı bulunamadı: yok.qcow2"), "{body}");

    let (status, g) = api(&app, post_json("/api/v1/guests", &with(json!({"image": "debian.qcow2", "ci_user": "ali", "ci_ssh_key": KEY})))).await;
    assert_eq!(status, StatusCode::CREATED, "{g}");
    assert_eq!((g["image"].as_str(), g["ci_user"].as_str(), g["ci_ssh_key"].as_str()), (Some("debian.qcow2"), Some("ali"), Some(KEY)), "alanlar konuğun kendi alanlarıdır");
    let id = g["vmid"].as_u64().unwrap();
    let conf = page(&app, &format!("/guests/{id}")).await;
    assert!(conf.contains("debian.qcow2") && conf.contains("ali · ssh-ed25519 ben@dizustu"), "anahtarın gövdesi değil, türü ve açıklaması gösterilir");
    assert!(!conf.contains("AAAAC3NzaC1lZDI1NTE5"));

    // Form: sondaki satır sonu atılır; boş alanlar ayar sayılmaz.
    let body = format!("kind=vm&node=pve-ank-01&name=form-bulut&os=debian&storage=local-lvm&disk_gb=8&cores=1&memory_mb=512&bridge=nat&image=&ci_user=&ci_ssh_key={}%0D%0A", KEY.replace(' ', "+").replace('@', "%40"));
    assert!(post_ok(&app, "/guests", &body).await.contains("notice="));
    let made = state.snapshot().guests.iter().find(|g| g.name == "form-bulut").cloned().unwrap();
    assert_eq!((made.hw.ci_ssh_key.as_deref(), made.hw.ci_user, made.hw.image), (Some(KEY), None, None));
}

#[tokio::test]
pub(super) async fn offsite_target_is_explained_and_push_needs_configuration() {
    let (app, state) = setup();
    state.run_action(103, Action::Stop, "admin").ok();
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains("--offsite DOSYA") && tab.contains("Dış kopya"));
    assert!(!tab.contains(&format!(r#"action="/backups/{id}/push""#)), "hedef yokken gönder düğmesi yok");
    assert!(post_ok(&app, &format!("/backups/{id}/push"), "").await.contains("error="));
    let (status, body) = api(&app, request(Method::POST, &format!("/api/v1/backups/{id}/push")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("yapılandırılmamış"));
    let (status, _, _) = send(&app, as_viewer(Method::POST, &format!("/api/v1/backups/{id}/push")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "salt okunur kullanıcı gönderemez");

    // Hedef tanımlıyken (simülasyon dosya tutmadığı için kopya başarısız olur ama yedek alınır).
    let target = std::env::temp_dir().join(format!("redox-http-offsite-{}", std::process::id()));
    std::fs::create_dir_all(&target).unwrap();
    state.set_offsite(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\n", target.display())).unwrap());
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains("sunucu dışına da kopyalanır") && tab.contains(&format!(r#"action="/backups/{id}/push""#)));
    assert!(post_ok(&app, &format!("/backups/{id}/push"), "").await.contains("notice="));
    let c = state.snapshot();
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].status), (format!("backup push {id}").as_str(), TaskStatus::Failed));
    assert!(!c.backup(&id).unwrap().offsite);
    std::fs::remove_dir_all(&target).unwrap();
}

#[tokio::test]
pub(super) async fn firewall_is_edited_from_the_form_and_api() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| {
        request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
    };
    let form = settings(&app, "/guests/103").await;
    assert!(form.contains(r#"name="firewall""#) && form.contains(r#"name="fw_rules""#));
    assert!(page(&app, "/guests/103").await.contains("Güvenlik duvarı"));

    let base = "name=db-yeni&os=debian-12&tags=&backup_time=&backup_keep=7";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&firewall=1&fw_rules=tcp%2F22+from+10.0.0.0%2F8%0D%0Atcp%2F443")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert!(hw.firewall);
    assert_eq!(hw.fw_rules.iter().map(ToString::to_string).collect::<Vec<_>>(), ["tcp/22 from 10.0.0.0/8", "tcp/443"]);
    let conf = page(&app, "/guests/103").await;
    assert!(conf.contains("açık · tcp/22 from 10.0.0.0/8, tcp/443"), "kurallar okunur biçimde gösterilir");
    assert!(settings(&app, "/guests/103").await.contains("tcp/22 from 10.0.0.0/8\ntcp/443"), "form kayıtlı kuralları geri yükler");

    // Geçersiz kural: hiçbir şey değişmez.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&firewall=1&fw_rules=tcp%2F22%3B+flush+ruleset")).await.contains("error="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.fw_rules.len(), 2);
    // İşaret kaldırılınca kapanır.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&fw_rules=")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert!(!hw.firewall && hw.fw_rules.is_empty());

    // API: kısmi güncelleme.
    let patch = json!({"firewall": true, "fw_rules": [{"proto": "udp", "ports": [53, 53], "source": "192.168.1.0/24"}, {"proto": "icmp"}]});
    let (status, _) = api(&app, send_json(Method::PATCH, "/api/v1/guests/103", patch)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state.snapshot().guest(103).unwrap().hw.fw_rules[0].to_string(), "udp/53 from 192.168.1.0/24");
    let bad = json!({"fw_rules": [{"proto": "tcp", "ports": [22, 22], "source": "1.2.3.4 accept; flush ruleset"}]});
    let (status, body) = api(&app, send_json(Method::PATCH, "/api/v1/guests/103", bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // Gerçek sürücüde --firewall yoksa açılamaz.
    let (real, _) = setup_real();
    let (status, body) = api(&real, send_json(Method::PATCH, "/api/v1/guests/103", json!({"firewall": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("--firewall"), "{body}");
}

#[tokio::test]
pub(super) async fn firmware_and_tpm_are_chosen_in_forms() {
    let (app, state) = setup();
    let form = page(&app, "/guests/new?kind=vm").await;
    assert!(form.contains(r#"name="firmware""#) && form.contains(r#"value="uefi-secure""#) && form.contains(r#"name="tpm""#));
    assert!(!page(&app, "/guests/new?kind=ct").await.contains(r#"name="firmware""#), "konteynerde donanım yazılımı yok");

    let body = "kind=vm&node=pve-ank-01&name=win11&os=windows-11&storage=local-lvm&disk_gb=8&cores=2&memory_mb=4096&bridge=nat&firmware=uefi-secure&tpm=1";
    assert!(post_ok(&app, "/guests", body).await.contains("notice="));
    let g = state.snapshot().guests.iter().find(|g| g.name == "win11").cloned().unwrap();
    assert_eq!((g.hw.firmware, g.hw.secure_boot, g.hw.tpm), (redox_core::hw::Firmware::Uefi, true, true));
    let conf = page(&app, &format!("/guests/{}", g.vmid)).await;
    assert!(conf.contains("UEFI + güvenli önyükleme · TPM 2.0"));
    let edit = settings(&app, &format!("/guests/{}", g.vmid)).await;
    assert!(edit.contains(r#"<option value="uefi-secure" selected>"#), "düzenleme formu kayıtlı seçimi gösterir");

    // Düzenleme: kapalı konukta donanım alanlarıyla birlikte değişir.
    let edit_body = "name=win11&os=windows-11&tags=&backup_time=&backup_keep=7&fw_rules=&cores=2&memory_mb=4096&bridge=nat&iso=&forwards=&firmware=uefi";
    assert!(post_ok(&app, &format!("/guests/{}/edit", g.vmid), edit_body).await.contains("notice="));
    let hw = state.snapshot().guest(g.vmid).unwrap().hw.clone();
    assert_eq!((hw.firmware, hw.secure_boot, hw.tpm), (redox_core::hw::Firmware::Uefi, false, false));

    // API: tutarsız birleşim ve konteyner reddedilir.
    let send_json = |method: Method, uri: &str, body: Value| request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let uri = format!("/api/v1/guests/{}", g.vmid);
    let (status, body) = api(&app, send_json(Method::PATCH, &uri, json!({"firmware": "bios", "secure_boot": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("UEFI donanım yazılımı gerektirir"), "{body}");
    let (status, _) = api(&app, send_json(Method::POST, "/api/v1/guests", json!({"name": "kutu-tpm", "kind": "ct", "node": "pve-ank-01", "cores": 1, "memory_mb": 256, "disk_gb": 8, "storage": "local-lvm", "tpm": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "konteynere TPM eklenmez");
    // Gerçek sürücüde paket yoksa neyin kurulacağı söylenir.
    let (real, _) = setup_real();
    let (status, body) = api(&real, send_json(Method::PATCH, "/api/v1/guests/102", json!({"tpm": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("apt install swtpm"), "{body}");
}

#[tokio::test]
pub(super) async fn extra_disks_and_nics_through_ui_and_api() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let vm = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).cloned().unwrap();
    let base = format!("/guests/{}", vm.vmid);

    let edit = settings(&app, &base).await;
    assert!(edit.contains("Ek diskler") && edit.contains(&format!(r#"action="{base}/disks""#)) && edit.contains(r#"name="nics""#));
    assert!(post_ok(&app, &format!("{base}/disks"), "disk_gb=25").await.contains("notice="));
    assert!(post_ok(&app, &format!("{base}/disks"), "disk_gb=cok").await.contains("error="));
    let (status, body) = api(&app, send_json(Method::POST, &format!("/api/v1{base}/disks"), json!({"size_gb": 40}))).await;
    assert_eq!((status, body["id"].as_u64()), (StatusCode::CREATED, Some(2)));
    let g = state.snapshot().guest(vm.vmid).unwrap().clone();
    assert_eq!(g.hw.disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>(), [(1, 25), (2, 40)]);
    assert_eq!(g.total_disk_gb(), vm.disk_gb + 65);

    let edit = settings(&app, &base).await;
    assert!(edit.contains(&format!("vm-{}-disk-1", vm.vmid)) && edit.contains(&format!(r#"href="{base}/confirm?op=deldisk&amp;name=2""#)), "her diskin silme bağlantısı onay sayfasına gider");
    assert!(page(&app, &base).await.contains("+ 25 GB + 40 GB"));
    // Yedek ek diskleri de kapsar: kayıt, etiket ve onay sayfası bunu gösterir.
    let backup = state.backup_create(vm.vmid, "admin").unwrap();
    assert_eq!(state.snapshot().backup(&backup).unwrap().disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>(), [(1, 25), (2, 40)]);
    let tab = page(&app, &format!("{base}?tab=yedekler")).await;
    assert!(tab.contains("asıl disk + 2 ek disk") && tab.contains("ek diskleri de kapsar") && !tab.contains("dahil değil"), "{tab}");
    let confirm = page(&app, &format!("{base}/confirm?op=restore&name={backup}")).await;
    assert!(confirm.contains("Yedekteki ek diskler de (1, 2) geri yüklenir"), "{confirm}");
    let (_, list) = api(&app, get("/api/v1/backups")).await;
    assert_eq!(list[0]["disks"][1]["size_gb"].as_u64(), Some(40));
    assert!(!settings(&app, &base).await.contains("dahil değil"));

    // Silme: onay sayfası hiçbir şeyi değiştirmez; POST siler.
    let confirm = page(&app, &format!("{base}/confirm?op=deldisk&name=1")).await;
    assert!(confirm.contains("Ek disk silinsin mi?") && confirm.contains(&format!(r#"action="{base}/disks/1/delete""#)));
    assert_eq!(state.snapshot().guest(vm.vmid).unwrap().hw.disks.len(), 2);
    let (status, _, _) = send(&app, get(&format!("{base}/confirm?op=deldisk&name=7"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "olmayan disk için onay sayfası yok");
    assert!(post_ok(&app, &format!("{base}/disks/1/delete"), "").await.contains("notice="));
    let (status, _, _) = send(&app, request(Method::DELETE, &format!("/api/v1{base}/disks/2")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state.snapshot().guest(vm.vmid).unwrap().hw.disks.is_empty());

    // Aynı numaraya sonradan takılan disk başka bir disktir: geri yükleme ona dokunmaz, onay sayfası da onu saymaz.
    assert!(post_ok(&app, &format!("{base}/disks"), "disk_gb=25").await.contains("notice="));
    assert!(!page(&app, &format!("{base}/confirm?op=restore&name={backup}")).await.contains("geri yüklenir"));
    assert!(state.snapshot().restore_disks(&backup).unwrap().is_empty());
    assert!(post_ok(&app, &format!("/backups/{backup}/restore"), "").await.contains("notice="));
    assert_eq!(state.snapshot().guest(vm.vmid).unwrap().hw.disks.len(), 1, "yeni disk yerinde durur");
    assert!(post_ok(&app, &format!("{base}/disks/1/delete"), "").await.contains("notice="));
    let (status, _, _) = send(&app, as_viewer(Method::POST, &format!("/api/v1{base}/disks")).header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"size_gb":1}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Çalışan konukta ve konteynerde reddedilir.
    let running = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
    let (status, body) = api(&app, send_json(Method::POST, &format!("/api/v1/guests/{running}/disks"), json!({"size_gb": 1}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = api(&app, send_json(Method::POST, "/api/v1/guests/200/disks", json!({"size_gb": 1}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Ek ağ kartları: form ve API.
    let body = format!("name={}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&cores={}&memory_mb={}&bridge={}&iso=&forwards=&firmware=bios&nics=vmbr1%2C+nat", vm.name, vm.cores, vm.mem_total_mb, vm.bridge);
    assert!(post_ok(&app, &format!("{base}/edit"), &body).await.contains("notice="));
    assert_eq!(state.snapshot().guest(vm.vmid).unwrap().hw.nics, ["vmbr1", "nat"]);
    assert!(page(&app, &base).await.contains("+ vmbr1"));
    let (status, body) = api(&app, send_json(Method::PATCH, &format!("/api/v1{base}"), json!({"nics": ["a", "b", "c", "d"]}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("en fazla 3"));
}

#[tokio::test]
pub(super) async fn passthrough_devices_and_backup_download() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let vm = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).cloned().unwrap();
    let base = format!("/guests/{}", vm.vmid);
    let edit = settings(&app, &base).await;
    assert!(edit.contains(r#"name="usb""#) && edit.contains(r#"name="pci""#));

    let body = format!("name={}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&cores={}&memory_mb={}&bridge={}&iso=&forwards=&firmware=bios&nics=&usb=046D%3AC52B&pci=0000%3A01%3A00.0", vm.name, vm.cores, vm.mem_total_mb, vm.bridge);
    assert!(post_ok(&app, &format!("{base}/edit"), &body).await.contains("notice="));
    let hw = state.snapshot().guest(vm.vmid).unwrap().hw.clone();
    assert_eq!((hw.usb, hw.pci), (vec!["046d:c52b".to_string()], vec!["0000:01:00.0".to_string()]), "büyük harf küçültülür");
    assert!(page(&app, &base).await.contains("USB 046d:c52b, PCI 0000:01:00.0"));
    let (status, body) = api(&app, send_json(Method::PATCH, &format!("/api/v1{base}"), json!({"pci": ["0000:01:00.0,x-vga=on"]}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("geçersiz PCI adresi"), "{body}");
    let (status, _) = api(&app, send_json(Method::PATCH, "/api/v1/guests/200", json!({"usb": ["046d:c52b"]}))).await;
    assert_ne!(status, StatusCode::OK, "konteynere aygıt aktarılmaz");

    // İndirme: simülasyonda dosya yok; salt okunur kullanıcı ve kimliksiz istek reddedilir.
    let id = state.backup_create(vm.vmid, "admin").unwrap();
    let (status, _, body) = send(&app, get(&format!("/backups/{id}/download"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("bu sunucuda yok"));
    let (status, _, _) = send(&app, get("/backups/yok/download")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, get(&format!("/backups/{id}/baska"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, as_viewer(Method::GET, &format!("/api/v1/backups/{id}/download")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = send(&app, anon(Method::GET, &format!("/api/v1/backups/{id}/download")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
pub(super) async fn charts_offer_longer_ranges_on_the_real_driver() {
    use redox_server::history::Range;
    let (app, state) = setup_real();
    // Bir saatlik veri: dakikada bir örnek, zamanı test verir.
    let t0: u64 = 1_800_000_000; // tam saat başı
    for minute in 0..=60u64 {
        state.record_long(&[("node:cpu".to_string(), 30.0), ("guest:102:cpu".to_string(), minute as f64)], t0 + minute * 60);
    }
    assert_eq!(state.history_for(Range::Day)["node:cpu"].len(), 60);
    assert_eq!(state.history_for(Range::Month)["guest:102:cpu"].len(), 1);
    state.refresh(None, None);
    assert!(state.history_for(Range::Recent).contains_key("node:cpu"), "ham örnekler ayrı tutulur");

    let node = state.snapshot().nodes[0].id.clone();
    let recent = page(&app, &format!("/nodes/{node}")).await;
    assert!(recent.contains("Son 10 dakika") && recent.contains(&format!(r#"href="/nodes/{node}?range=24s""#)) && recent.contains("-10 dk"));
    let day = page(&app, &format!("/nodes/{node}?range=24s")).await;
    assert!(day.contains("-24 sa") && day.contains("son 24 saat"), "eksen ve erişilebilir ad aralığı söyler");
    assert!(day.contains(&format!(r#"href="/nodes/{node}""#)) && day.contains(&format!(r#"href="/nodes/{node}?range=30g""#)), "öteki aralıklara bağlantı");
    assert!(day.contains(r#"href="/theme?"#) || day.contains("range%3D24s") || day.contains("range=24s"), "seçim tema değişiminde korunur");
    let month = page(&app, "/guests/102?range=30g").await;
    assert!(month.contains("-30 gün") && month.contains(r#"href="/guests/102?range=24s""#));
    // Sekmeli sayfada aralık sorguya eklenir, sekme korunur.
    let tabbed = page(&app, "/guests/102?tab=ozet&range=30g").await;
    assert!(tabbed.contains("-30 gün"));

    // Simülasyonda geçmiş yoktur: seçici gösterilmez.
    let (sim, _) = setup();
    assert!(!page(&sim, "/guests/102").await.contains("Son 24 saat"));
}

#[tokio::test]
pub(super) async fn operators_act_only_within_their_scope() {
    let (app, state) = setup();
    state.auth().edit(|u| {
        u.add_user("nobet", "operator-parolasi-12", Role::Operator, Cost::Test)?;
        u.add_token("nobet", "test", "redox_test_operator_token")?;
        u.set_scope("nobet", vec!["web".to_string()])
    }).unwrap();
    let as_op = |method: Method, uri: &str| anon(method, uri).header(AUTHORIZATION, "Bearer redox_test_operator_token");
    let post = |uri: &str| as_op(Method::POST, uri).body(Body::empty()).unwrap();

    // İki çalışan sanal makine: biri kapsamda (web etiketli), öteki değil.
    let running: Vec<u32> = state.snapshot().guests.iter().filter(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).map(|g| g.vmid).collect();
    let (inside, outside) = (running[0], running[1]);
    state.update_guest(inside, &redox_core::GuestPatch { tags: Some(vec!["web".into(), "prod".into()]), ..Default::default() }, "admin").unwrap();
    state.update_guest(outside, &redox_core::GuestPatch { tags: Some(vec!["db".into()]), ..Default::default() }, "admin").unwrap();

    // Kapsamdaki konuk: güç, anlık görüntü, yedek.
    let (status, _, _) = send(&app, post(&format!("/api/v1/guests/{inside}/stop"))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(state.snapshot().guest(inside).unwrap().status, GuestStatus::Stopped);
    let snap = as_op(Method::POST, &format!("/api/v1/guests/{inside}/snapshots")).header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"name":"nobetci"}"#)).unwrap();
    assert_eq!(send(&app, snap).await.0, StatusCode::ACCEPTED);
    assert_eq!(send(&app, post(&format!("/api/v1/guests/{inside}/backups"))).await.0, StatusCode::ACCEPTED);
    let (status, _, _) = send(&app, form_body(as_op(Method::POST, &format!("/guests/{inside}/action")), "action=start")).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "arayüz formu da çalışır");
    assert_eq!(state.snapshot().tasks[0].user, "nobet", "işlem operatörün adıyla günlüğe düşer");

    // Kapsam dışındaki konuk ve operatörü aşan işlemler reddedilir; hiçbir şey değişmez.
    let before = state.snapshot();
    let denied = [
        (Method::POST, format!("/api/v1/guests/{outside}/stop")),
        (Method::POST, format!("/api/v1/guests/{outside}/snapshots")),
        (Method::POST, format!("/api/v1/guests/{inside}/snapshots/nobetci/rollback")),
        (Method::DELETE, format!("/api/v1/guests/{inside}/snapshots/nobetci")),
        (Method::DELETE, format!("/api/v1/guests/{inside}")),
        (Method::PATCH, format!("/api/v1/guests/{inside}")),
        (Method::POST, format!("/api/v1/guests/{inside}/disks")),
        (Method::POST, format!("/api/v1/guests/{inside}/clone")),
        (Method::POST, "/api/v1/guests".to_string()),
        (Method::POST, "/guests/bulk".to_string()),
        (Method::POST, format!("/guests/{inside}/edit")),
        (Method::POST, format!("/guests/{inside}/delete")),
        (Method::POST, "/users".to_string()),
        (Method::POST, "/api/v1/guests/999999/stop".to_string()),
    ];
    for (method, uri) in denied {
        let (status, _, body) = send(&app, as_op(method.clone(), &uri).header(CONTENT_TYPE, "application/json").body(Body::from("{}")).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
    }
    assert_eq!(state.snapshot().guests, before.guests);
    let (status, body) = api(&app, post(&format!("/api/v1/guests/{outside}/stop"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("yetki kapsamınızda değil"), "{body}");

    // Arayüz: kapsamdaki konukta düğmeler var, silme yok; kapsam dışında açıklama.
    let cookie = login_cookie(&app, "nobet", "operator-parolasi-12").await;
    let get_as = |uri: &str| anon(Method::GET, uri).header(COOKIE, cookie.clone()).body(Body::empty()).unwrap();
    let (_, _, mine) = send(&app, get_as(&format!("/guests/{inside}"))).await;
    assert!(mine.contains(r#"name="action""#) && mine.contains("<small>operatör</small>") && !mine.contains(&format!(r#"href="/guests/{inside}/delete""#)));
    assert!(mine.contains("tab=konsol"), "kapsamdaki konuğun konsolu açıktır");
    let (_, _, other) = send(&app, get_as(&format!("/guests/{outside}"))).await;
    assert!(other.contains("Bu konuk yetki kapsamınızda değil") && !other.contains(r#"name="action""#) && !other.contains("tab=konsol"));
    let (status, _, _) = send(&app, get_as(&format!("/guests/{outside}/screen"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "kapsam dışındaki konuğun ekranı görülemez");
    let (status, _, _) = send(&app, get_as(&format!("/guests/{inside}/screen"))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&app, get_as("/users")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "kullanıcı yönetimi yöneticiye özeldir");

    // Kapsam boşaltılınca bütün konuklarda yetkili olur; yönetici bunu kullanıcı sayfasından yapar.
    let users = page(&app, "/users").await;
    assert!(users.contains("Operatör") && users.contains("(web)") && users.contains(r#"href="/users/nobet""#));
    let one = page(&app, "/users/nobet").await;
    assert!(one.contains(r#"action="/users/nobet/scope""#) && one.contains(r#"value="web""#));
    assert!(post_ok(&app, "/users/nobet/scope", "scope=").await.contains("notice="));
    assert_eq!(send(&app, post(&format!("/api/v1/guests/{outside}/stop"))).await.0, StatusCode::ACCEPTED);
    assert!(post_ok(&app, "/users/nobet/scope", "scope=Geçersiz+Etiket!").await.contains("error="));
    assert!(post_ok(&app, "/users/nobet/role", "role=viewer").await.contains("notice="));
    assert_eq!(send(&app, post(&format!("/api/v1/guests/{outside}/start"))).await.0, StatusCode::FORBIDDEN, "rol düşünce yetki hemen kalkar");
}

#[test]
pub(super) fn operator_paths_are_an_explicit_allow_list() {
    use redox_server::operator_target;
    for (path, expected) in [
        ("/guests/103/action", Some(103)),
        ("/guests/103/snapshots", Some(103)),
        ("/guests/103/backups", Some(103)),
        ("/guests/103/keys", Some(103)),
        ("/guests/103/exec", Some(103)),
        ("/api/v1/guests/103/start", Some(103)),
        ("/api/v1/guests/103/resume", Some(103)),
        ("/api/v1/guests/103/exec", Some(103)),
        ("/guests/103/delete", None),
        ("/guests/103/edit", None),
        ("/guests/103/resize", None),
        ("/guests/103/disks", None),
        ("/guests/103/snapshots/x/rollback", None),
        ("/guests/103/snapshots/x/delete", None),
        ("/api/v1/guests/103/snapshots/x", None),
        ("/api/v1/guests/103/clone", None),
        ("/api/v1/guests/103", None),
        ("/guests/bulk", None),
        ("/guests/abc/action", None),
        ("/backups/vm-103-1/restore", None),
        ("/backups/vm-103-1/delete", None),
        ("/users/x/role", None),
        ("/guests/103/action/../delete", None),
    ] {
        assert_eq!(operator_target(path).map(|(vmid, _)| vmid), expected, "{path}");
    }
}

/// Sahte kimlik sağlayıcı: keşif belgesi ve token uç noktası. Token isteklerinin gövdesini kaydeder;
/// kimlik belirtecinin alanlarını test belirler.
pub(super) fn fake_idp(claims: std::sync::Arc<std::sync::Mutex<Value>>) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            let mut reader = BufReader::new(socket);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim().to_ascii_lowercase();
                if header.is_empty() {
                    break;
                }
                if let Some(v) = header.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            let base = format!("http://127.0.0.1:{port}");
            let payload = if path == "/.well-known/openid-configuration" {
                json!({"issuer": base, "authorization_endpoint": format!("{base}/authorize"), "token_endpoint": format!("{base}/token")}).to_string()
            } else if path == "/token" {
                log.lock().unwrap().push(String::from_utf8_lossy(&body).to_string());
                let encoded = redox_server::oidc::base64url(claims.lock().unwrap().to_string().as_bytes());
                json!({"access_token": "x", "token_type": "Bearer", "id_token": format!("e30.{encoded}.imza")}).to_string()
            } else {
                String::new()
            };
            let status = if payload.is_empty() { "404 Not Found" } else { "200 OK" };
            let _ = write!(reader.get_mut(), "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len());
        }
    });
    (port, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn single_sign_on_maps_the_provider_identity_to_a_local_user() {
    use sha2::{Digest, Sha256};
    let (app, state) = setup();
    // Yapılandırılmadan: bağlantı yok, adres yok.
    assert!(!page_anon(&app, "/login").await.contains("/login/oidc"));
    let (status, _, _) = send(&app, anon(Method::GET, "/login/oidc").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let claims = std::sync::Arc::new(std::sync::Mutex::new(json!({})));
    let (port, token_requests) = fake_idp(claims.clone());
    let issuer = format!("http://127.0.0.1:{port}");
    state.set_oidc(redox_server::oidc::Config::parse(&format!("issuer = {issuer}\nclient_id = redox-panel\nclient_secret = cok-gizli-anahtar\nredirect_url = http://localhost:8006/login/oidc/callback\n")).unwrap());
    state.auth().edit(|u| u.set_sso("izleyici", "ayse@example.com")).unwrap();
    assert!(page_anon(&app, "/login").await.contains(r#"href="/login/oidc""#));

    // Başlat: sağlayıcıya yönlendirme ve bu tarayıcıya bağlı durum çerezi.
    let start = || async {
        let (status, headers, _) = send(&app, anon(Method::GET, "/login/oidc").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let to = location(&headers).to_string();
        let params: std::collections::HashMap<String, String> = serde_urlencoded::from_str(to.split_once('?').unwrap().1).unwrap();
        let cookie = headers.get(SET_COOKIE).unwrap().to_str().unwrap().to_string();
        (to, params, cookie)
    };
    let (to, params, cookie) = start().await;
    assert!(to.starts_with(&format!("{issuer}/authorize?")));
    assert_eq!((params["response_type"].as_str(), params["client_id"].as_str(), params["code_challenge_method"].as_str()), ("code", "redox-panel", "S256"));
    assert_eq!(params["redirect_uri"], "http://localhost:8006/login/oidc/callback");
    assert!(cookie.starts_with(&format!("redox_oidc={}", params["state"])) && cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax") && cookie.contains("Path=/login/oidc"));
    let state_cookie = cookie.split(';').next().unwrap().to_string();
    let callback = |state: &str, cookie: Option<&str>| {
        let mut builder = anon(Method::GET, &format!("/login/oidc/callback?code=yetki-kodu&state={state}"));
        if let Some(c) = cookie {
            builder = builder.header(COOKIE, c);
        }
        builder.body(Body::empty()).unwrap()
    };
    let good = |nonce: &str, email: &str| json!({"iss": issuer, "aud": "redox-panel", "exp": now() + 300, "nonce": nonce, "email": email, "email_verified": true});

    // Başka tarayıcı (çerez yok) kodu kullanamaz.
    *claims.lock().unwrap() = good(&params["nonce"], "ayse@example.com");
    let (status, headers, _) = send(&app, callback(&params["state"], None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/login?error="));
    assert!(token_requests.lock().unwrap().is_empty(), "çerez tutmadan sağlayıcıya gidilmez");

    // Doğru tarayıcı: oturum açılır.
    let (status, headers, body) = send(&app, callback(&params["state"], Some(&state_cookie))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(r#"http-equiv="refresh""#) && body.contains("url=/"));
    let cookies: Vec<String> = headers.get_all(SET_COOKIE).iter().map(|v| v.to_str().unwrap().to_string()).collect();
    let session = cookies.iter().find(|c| c.starts_with("redox_session=")).expect("oturum çerezi").split(';').next().unwrap().to_string();
    assert!(cookies.iter().any(|c| c.starts_with("redox_oidc=;") && c.contains("Max-Age=0")), "durum çerezi silinir");
    let (status, _, home) = send(&app, anon(Method::GET, "/").header(COOKIE, session).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("<b>izleyici</b><small>salt okunur</small>"), "rol yereldir: sağlayıcı yalnızca kimliği doğrular");

    // Sağlayıcıya giden istek: kod, gizli anahtar ve PKCE doğrulayıcısı (meydan okumanın ön görüntüsü).
    let sent: std::collections::HashMap<String, String> = serde_urlencoded::from_str(&token_requests.lock().unwrap()[0]).unwrap();
    assert_eq!((sent["grant_type"].as_str(), sent["code"].as_str(), sent["client_secret"].as_str()), ("authorization_code", "yetki-kodu", "cok-gizli-anahtar"));
    assert_eq!(redox_server::oidc::base64url(&Sha256::digest(sent["code_verifier"].as_bytes())), params["code_challenge"]);

    // Aynı dönüş ikinci kez kullanılamaz.
    let (_, headers, _) = send(&app, callback(&params["state"], Some(&state_cookie))).await;
    assert!(location(&headers).starts_with("/login?error="));

    // Eşlenmemiş kimlik, başka girişin belirteci ve başka uygulamanın belirteci reddedilir.
    for make in [
        (|n: &str| ("yabanci@example.com".to_string(), n.to_string(), "redox-panel")) as fn(&str) -> (String, String, &'static str),
        |_| ("ayse@example.com".to_string(), "baska-nonce".to_string(), "redox-panel"),
        |n| ("ayse@example.com".to_string(), n.to_string(), "baska-uygulama"),
    ] {
        let (_, params, cookie) = start().await;
        let (email, nonce, aud) = make(&params["nonce"]);
        let mut c = good(&nonce, &email);
        c["aud"] = json!(aud);
        *claims.lock().unwrap() = c;
        let (status, headers, _) = send(&app, callback(&params["state"], Some(cookie.split(';').next().unwrap()))).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert!(location(&headers).starts_with("/login?error=") && headers.get(SET_COOKIE).is_none(), "oturum açılmaz");
    }
    let shown = page_anon(&app, "/login?error=Bu+kimlik+hi%C3%A7bir+kullan%C4%B1c%C4%B1yla+e%C5%9Fle%C5%9Fmiyor.").await;
    assert!(shown.contains("Bu kimlik hiçbir kullanıcıyla eşleşmiyor."), "hata giriş sayfasında gösterilir");
}

pub(super) async fn page_anon(app: &Router, uri: &str) -> String {
    let (status, _, body) = send(app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    body
}

#[tokio::test]
pub(super) async fn incremental_backups_are_labelled_and_verification_is_admin_only() {
    let mut cluster = Cluster::demo(now());
    let guest_name = cluster.guest(103).unwrap().name.clone();
    let backup = |id: &str, new_mb| redox_core::Backup { id: id.into(), vmid: 103, guest_name: guest_name.clone(), created: now(), size_mb: 900, auto: false, offsite: false, new_mb, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 };
    cluster.add_backup(backup("vm-103-tam", None));
    cluster.add_backup(backup("vm-103-artimli", Some(12)));
    let state = AppState::new(cluster, Box::new(RealLike), Duration::ZERO, None, Auth::new(users(), None, false, Cost::Test));
    let app = app(state.clone());

    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert_eq!(tab.matches(r#"<span class="tag gold">artımlı</span>"#).count(), 1, "yalnızca artımlı yedek etiketlenir");
    assert!(tab.contains("· +12 MB yeni") && tab.contains("Birini silmek diğerlerini etkilemez"));
    assert!(tab.contains(r#"action="/backups/vm-103-artimli/verify""#) && tab.contains(r#"action="/backups/vm-103-tam/verify""#));

    // Doğrulama arka planda çalışır; sonucu görev günlüğüne yazılır.
    assert!(post_ok(&app, "/backups/vm-103-artimli/verify", "").await.starts_with("/guests/103?tab=yedekler&notice="));
    assert!(state.snapshot().tasks[0].command == "backup verify vm-103-artimli");
    let (status, _, _) = send(&app, request(Method::POST, "/api/v1/backups/vm-103-tam/verify").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, body) = api(&app, request(Method::POST, "/api/v1/backups/yok/verify").body(Body::empty()).unwrap()).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("backup_not_found")));
    let (status, _, _) = send(&app, as_viewer(Method::POST, "/api/v1/backups/vm-103-tam/verify").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "salt okunur kullanıcı doğrulama başlatamaz");
    let (_, list) = api(&app, get("/api/v1/backups")).await;
    assert_eq!((list[0]["new_mb"].as_u64(), list[1]["new_mb"].as_u64()), (Some(12), None));

    // Simülasyonda dosya yoktur: doğrulama düğmesi sunulmaz.
    let (sim, sim_state) = setup();
    sim_state.run_action(103, Action::Stop, "admin").ok();
    sim_state.backup_create(103, "admin").unwrap();
    let tab = page(&sim, "/guests/103?tab=yedekler").await;
    assert!(!tab.contains("/verify") && !tab.contains("artımlı"));
}

#[tokio::test]
pub(super) async fn templates_and_linked_clones_through_ui_and_api() {
    let (app, state) = setup();
    let del = |uri: &str| request(Method::DELETE, uri).body(Body::empty()).unwrap();
    let vm = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).cloned().unwrap();
    let base = format!("/guests/{}", vm.vmid);
    let edit = settings(&app, &base).await;
    assert!(edit.contains("Şablona Çevir") && !edit.contains(r#"name="linked""#), "şablon olmayan konukta bağlı klon seçeneği yok");
    assert!(post_ok(&app, &format!("{base}/clone"), "name=olmaz&linked=1").await.contains("error="));

    assert!(post_ok(&app, &format!("{base}/template"), "on=1").await.contains("notice="));
    assert!(state.snapshot().guest(vm.vmid).unwrap().hw.template);
    let edit = settings(&app, &base).await;
    assert!(edit.contains("Şablonluktan Çıkar") && edit.contains(r#"name="linked" value="1" checked"#));
    assert!(page(&app, &base).await.contains(r#"<span class="tag gold">şablon</span>"#));
    // Şablon başlatılamaz, geri yüklenemez, anlık görüntüsü alınamaz.
    assert!(post_ok(&app, &format!("{base}/action"), "action=start").await.contains("error="));
    assert!(post_ok(&app, &format!("{base}/snapshots"), "name=olmaz").await.contains("error="));

    let to = post_ok(&app, &format!("{base}/clone"), "name=hizli-klon&linked=1").await;
    assert!(to.contains("notice="), "{to}");
    let c = state.snapshot();
    let clone = c.guests.iter().find(|g| g.name == "hizli-klon").unwrap().clone();
    assert_eq!(clone.hw.base, Some(vm.vmid));
    assert!(c.tasks.iter().any(|t| t.command == format!("vm linkclone {} hizli-klon", vm.vmid)));
    assert!(page(&app, &format!("/guests/{}", clone.vmid)).await.contains("Bağlı klon"));
    // Bağlı klon varken şablon silinemez ve şablonluktan çıkarılamaz.
    assert!(post_ok(&app, &format!("{base}/template"), "on=0").await.contains("error="));
    let (status, body) = api(&app, del(&format!("/api/v1{base}"))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("bağlı klonları"), "{body}");

    // API: bağlı klon ve şablon anahtarı.
    let (status, v) = api(&app, post_json(&format!("/api/v1{base}/clone"), &json!({"name": "api-klon", "linked": true}))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let second = v["vmid"].as_u64().unwrap();
    for id in [clone.vmid as u64, second] {
        assert_eq!(send(&app, del(&format!("/api/v1/guests/{id}"))).await.0, StatusCode::NO_CONTENT);
    }
    let (status, v) = api(&app, post_json(&format!("/api/v1{base}/template"), &json!({"on": false}))).await;
    assert_eq!((status, v["template"].as_bool()), (StatusCode::OK, Some(false)));
    let (status, _, _) = send(&app, as_viewer(Method::POST, &format!("/api/v1{base}/template")).header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"on":true}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
pub(super) async fn media_needs_a_real_driver() {
    let (app, _) = setup();
    assert!(!page(&app, "/storage").await.contains("/media/upload"), "simülasyonda yükleme bölümü yok");
    let (status, list) = api(&app, get("/api/v1/media")).await;
    assert_eq!((status, list.as_array().map(Vec::len)), (StatusCode::OK, Some(0)));
    let (status, body) = api(&app, request(Method::PUT, "/api/v1/media/a.iso").body(Body::from("x")).unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("gerçek sürücü"), "{body}");
}

// ---------- Arayüz v3: konuk listesi, görevler, yedekler, sekmeli ayarlar ----------

#[tokio::test]
pub(super) async fn replication_card_shows_and_sets_witnessed_failover() {
    let (app, st) = setup();
    st.peers().configure(redox_server::peers::parse_file(&format!("ankara http://127.0.0.1:9 {VIEWER_TOKEN}\nizmir http://127.0.0.1:9 {VIEWER_TOKEN}\n")).unwrap());
    st.mutate(|c| {
        c.put_replica("ankara", 500, 60).unwrap();
        c.set_replica_failover("ankara", 500, Some(10)).unwrap();
        c.replicas[0].failover_note = Some("hiçbir hakem sunucuya ulaşılamıyor: bu sunucunun ağı kopmuş olabilir, devralınmadı".into());
        c.replicas[0].delta = Some((1, 3));
    });
    let html = page(&app, "/cluster?tab=koruma").await;
    for needle in ["Devralma", r#"name="failover_min""#, r#"value="10""#, "Engellendi", "Devralınmadı:", "hiçbir hakem sunucuya", "10 dk sonra devralır", "60 dk'da bir", "Önce hakeme", r#"value="save""#, r#"class="set-panel""#] {
        assert!(html.contains(needle), "{needle}");
    }
    // Devralınmış çoğaltma öyle görünür.
    st.mutate(|c| c.replicas[0].failed_over = Some(1_700_000_000));
    let html = page(&app, "/cluster?tab=koruma").await;
    assert!(html.contains("Devralındı") && html.contains("iki kopyadan birini kapatın") && !html.contains("Devralınmadı:"));
    // Ayar bölmesi aralığı ve devralmayı birlikte kaydeder.
    let (status, headers, _) = send(&app, post_form("/cluster/replica", "peer=ankara&vmid=500&op=save&every_min=30&failover_min=20")).await;
    assert!(status.is_redirection() && location(&headers).contains("notice"), "{}", location(&headers));
    let saved = st.snapshot().replicas[0].clone();
    assert_eq!((saved.every_min, saved.failover_min), (30, Some(20)));
    let (_, headers, _) = send(&app, post_form("/cluster/replica", "peer=ankara&vmid=500&op=save&every_min=x&failover_min=20")).await;
    assert!(location(&headers).contains("error"));
    // Form: 0 kapatır, geçersiz süre reddedilir.
    let (status, headers, _) = send(&app, post_form("/cluster/replica", "peer=ankara&vmid=500&op=failover&failover_min=0&every_min=")).await;
    assert!(status.is_redirection() && location(&headers).contains("notice"), "{}", location(&headers));
    assert_eq!(st.snapshot().replicas[0].failover_min, None);
    let (_, headers, _) = send(&app, post_form("/cluster/replica", "peer=ankara&vmid=500&op=failover&failover_min=2&every_min=")).await;
    assert!(location(&headers).contains("error"));
    // API.
    let (status, _) = api(&app, post_json("/api/v1/replicas/ankara/500/failover", &json!({"minutes": 15}))).await;
    assert_eq!((status, st.snapshot().replicas[0].failover_min), (StatusCode::NO_CONTENT, Some(15)));
    let viewer = as_viewer(Method::POST, "/api/v1/replicas/ankara/500/failover").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"minutes":15}"#)).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    // Hakem olacak ikinci eş yoksa açılmaz.
    st.peers().configure(redox_server::peers::parse_file(&format!("ankara http://127.0.0.1:9 {VIEWER_TOKEN}\n")).unwrap());
    let (status, body) = api(&app, post_json("/api/v1/replicas/ankara/500/failover", &json!({"minutes": 15}))).await;
    assert!(status.is_client_error() && body.to_string().contains("hakem"), "{status} {body}");
}
