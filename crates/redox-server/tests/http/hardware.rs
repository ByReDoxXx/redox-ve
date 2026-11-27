//! cloud-init düzenleme, işlemci türü ve bellek balonu.

use super::*;

#[tokio::test]
pub(super) async fn cloud_init_is_edited_from_the_hardware_tab_and_the_api() {
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGtMF0qv2yBAZ8lfmTtk1mKkZHjPl7Yj8d0Lr0oQ1X2w ali@dizustu";
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    for field in ["ci_user", "ci_ssh_key", "ci_ip", "ci_gw", "ci_dns"] {
        assert!(tab.contains(&format!(r#"form="hw" name="{field}""#)), "{field}");
    }
    assert!(tab.contains("cloud-init") && tab.contains("kapalı"));
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="ci_ssh_key""#), "konteynerde yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    let key = KEY.replace(' ', "+");
    let to = post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&ci_ip=10.0.0.5%2F24&ci_gw=10.0.0.1&ci_dns=1.1.1.1%2C+9.9.9.9")).await;
    assert!(to.contains("notice="), "{to}");
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.ci_user.as_deref(), hw.ci_ssh_key.as_deref(), hw.ci_ip.as_deref(), hw.ci_gw.as_deref(), hw.ci_dns.len(), hw.ci_gen), (Some("ali"), Some(KEY), Some("10.0.0.5/24"), Some("10.0.0.1"), 2, 1));
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains("açık · sürüm 1") && tab.contains(r#"name="ci_ip" type="text" value="10.0.0.5/24""#) && tab.contains(r#"value="1.1.1.1, 9.9.9.9""#));
    // Aynı formu yeniden kaydetmek sürümü artırmaz; hatalı adres reddedilir ve kayıt değişmez.
    post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&ci_ip=10.0.0.5%2F24&ci_gw=10.0.0.1&ci_dns=1.1.1.1%2C+9.9.9.9")).await;
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_gen, 1);
    for bad in ["ci_ip=10.0.0.5&ci_gw=&ci_dns=", "ci_ip=10.0.0.5%2F24&ci_gw=gecit&ci_dns=", "ci_ip=&ci_gw=10.0.0.1&ci_dns="] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&{bad}")).await.contains("error="), "{bad}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_ip.as_deref(), Some("10.0.0.5/24"));
    // Seçenekler sekmesinden kaydetmek cloud-init'e dokunmaz.
    post_ok(&app, "/guests/103/edit", "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=").await;
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_gen, 1);
    // Çalışan konukta değişmez.
    assert!(post_ok(&app, "/guests/102/edit", &format!("part=donanim&cores=8&memory_mb=32768&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=&ci_ssh_key={key}")).await.contains("error="));

    // API: adresi boşaltmak DHCP'ye döndürür, anahtarı boşaltmak cloud-init'i kapatır.
    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert!(api(&app, patch(json!({ "ci_ip": "", "ci_gw": "", "ci_dns": "" }))).await.0.is_success());
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.ci_ip.clone(), hw.cloud_init(), hw.ci_gen), (None, true, 2));
    let (status, v) = api(&app, patch(json!({ "ci_ip": "10.0.0.9/99" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(api(&app, patch(json!({ "ci_ssh_key": "" }))).await.0.is_success());
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cloud_init(), hw.ci_user.clone(), hw.ci_gen), (false, None, 3));
}

// ---------- İşlemci türü ve bellek balonu ----------

#[tokio::test]
pub(super) async fn cpu_type_and_memory_balloon_are_set_from_the_hardware_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<select id="cpu_type" form="hw" name="cpu_type""#) && tab.contains(r#"<option value="" selected>Sunucunun işlemcisi (host)</option>"#), "{tab}");
    assert!(tab.contains(r#"<option value="Haswell">Haswell</option>"#) && tab.contains(r#"name="balloon" value="1""#));
    assert!(!tab.contains("Bellek hedefi"), "balon takılı değilken hedef formu yok");
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="cpu_type""#), "konteynerde yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=Haswell&balloon=1")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("Haswell", true, 0));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=Pentium9&balloon=1")).await.contains("error="));
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<option value="Haswell" selected>Haswell</option>"#) && tab.contains(r#"action="/guests/103/balloon""#) && tab.contains("tamamı (16384 MB)"));

    // Bellek hedefi: sınırlar içinde kabul edilir, görev günlüğüne yazılır; 0 balonu boşaltır.
    assert!(post_ok(&app, "/guests/103/balloon", "mb=4096").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 4096);
    assert_eq!(state.snapshot().tasks[0].command, "vm balloon 103 4096");
    assert!(page(&app, "/guests/103?tab=donanim").await.contains("4096 / 16384 MB"));
    for bad in ["mb=100", "mb=99999", "mb=cok", "mb="] {
        assert!(post_ok(&app, "/guests/103/balloon", bad).await.contains("error="), "{bad}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 4096, "reddedilen hedef kaydı değiştirmez");
    assert!(post_ok(&app, "/guests/103/balloon", "mb=0").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 0);
    // Balonu olmayan konukta ve izleyici için reddedilir; operatöre açık değildir.
    assert!(post_ok(&app, "/guests/102/balloon", "mb=4096").await.contains("error="));
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/103/balloon"), "mb=4096")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/guests/103/balloon"), None);

    // API.
    let (status, _) = api(&app, post_json("/api/v1/guests/103/balloon", &json!({ "mb": 8192 }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 8192);
    let (status, v) = api(&app, post_json("/api/v1/guests/103/balloon", &json!({ "mb": 1 }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Başka üreticinin modeli, üreticisi bilinen sunucuda reddedilir.
    let (real, real_state) = setup_real();
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=EPYC-Milan")).await.contains("error="));
    assert_eq!(real_state.snapshot().guest(103).unwrap().hw.cpu_type, "");
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=Skylake-Server")).await.contains("notice="));
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=kvm64")).await.contains("notice="));
    // Balon kaldırılınca hedef sıfırlanır; işlemci türü "host"a döner.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("", false, 0));
}

// ---------- Kaynak havuzları ve kotalar ----------

// ---------- Çalışırken çekirdek ekleme ----------

#[tokio::test]
pub(super) async fn cores_are_added_to_a_running_vm_from_the_hardware_tab() {
    let (app, state) = setup_real();
    let cores = |state: &AppState| state.snapshot().guest(103).unwrap().cores;
    let run = |state: &AppState, status| state.mutate(|c| c.guest_mut_for_import(103).unwrap().status = status);
    let base = "part=donanim&cores=2&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<input id="max_cores" form="hw" name="max_cores""#) && !tab.contains("Çalışırken çekirdek"), "sınır yokken ekleme formu yok");
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="max_cores""#), "konteynerde yok");

    // Sınır çekirdek sayısından küçük olamaz.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&max_cores=1")).await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&max_cores=6")).await.contains("notice="));
    assert_eq!((cores(&state), state.snapshot().guest(103).unwrap().hw.max_cores), (2, 6));
    assert!(post_ok(&app, "/guests/103/cores", "cores=3").await.contains("error="), "kapalı konuğa çalışırken eklenmez");

    run(&state, GuestStatus::Running);
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"action="/guests/103/cores""#) && tab.contains("2 / 6") && tab.contains(r#"name="cores" type="text" inputmode="numeric" value="3""#), "{tab}");

    // API: eklenenleri konuk ajanının etkinleştirip etkinleştirmediği de döner.
    let (status, v) = api(&app, post_json("/api/v1/guests/103/cores", &json!({ "cores": 3 }))).await;
    assert_eq!((status, v["online"].as_bool(), cores(&state)), (StatusCode::OK, Some(true), 3), "{v}");
    let (status, v) = api(&app, post_json("/api/v1/guests/103/cores", &json!({ "cores": 9 }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");

    assert!(post_ok(&app, "/guests/103/cores", "cores=4").await.contains("notice="));
    assert_eq!((cores(&state), state.snapshot().tasks[0].command.as_str()), (4, "vm cores 103 4"));
    for bad in ["cores=4", "cores=3", "cores=7", "cores=cok"] {
        assert!(post_ok(&app, "/guests/103/cores", bad).await.contains("error="), "{bad}");
    }
    assert_eq!(cores(&state), 4, "reddedilen istek kaydı değiştirmez");

    // Yarıda kalan ekleme: kayıt, konukta gerçekten olan sayıyı gösterir.
    assert!(post_ok(&app, "/guests/103/cores", "cores=6").await.contains("error="));
    assert_eq!((cores(&state), state.snapshot().tasks[0].status), (5, TaskStatus::Failed));

    // İzleyiciye ve operatöre açık değildir.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/103/cores"), "cores=6")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/guests/103/cores"), None);

    // Çalışırken takmayı bilmeyen sürücü (benzetim) reddeder.
    let (sim, sim_state) = setup();
    sim_state.mutate(|c| {
        let g = c.guest_mut_for_import(103).unwrap();
        (g.hw.max_cores, g.status) = (16, GuestStatus::Running);
    });
    assert!(post_ok(&sim, "/guests/103/cores", "cores=9").await.contains("error="));
    assert_eq!(sim_state.snapshot().guest(103).unwrap().cores, 8);
}

// ---------- Çalışırken bellek ekleme ----------

#[tokio::test]
pub(super) async fn memory_is_added_to_a_running_vm_from_the_hardware_tab() {
    let (app, state) = setup_real();
    let mem = |state: &AppState| state.snapshot().guest(103).unwrap().mem_total_mb;
    let run = |state: &AppState, status| state.mutate(|c| c.guest_mut_for_import(103).unwrap().status = status);
    let base = "part=donanim&cores=2&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<input id="max_memory_mb" form="hw" name="max_memory_mb""#) && !tab.contains("Çalışırken bellek"), "sınır yokken ekleme formu yok");
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="max_memory_mb""#), "konteynerde yok");

    // Sınır bellekten küçük olamaz.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&max_memory_mb=8192")).await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&max_memory_mb=20480")).await.contains("notice="));
    assert_eq!((mem(&state), state.snapshot().guest(103).unwrap().hw.max_mem_mb), (16384, 20480));
    assert!(post_ok(&app, "/guests/103/memory", "memory_mb=17408").await.contains("error="), "kapalı konuğa çalışırken eklenmez");

    run(&state, GuestStatus::Running);
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"action="/guests/103/memory""#) && tab.contains("16384 / 20480 MB") && tab.contains(r#"name="memory_mb" type="text" inputmode="numeric" value="17408""#), "{tab}");

    // API: konuğun yeni belleği kullandığının doğrulanıp doğrulanmadığı da döner.
    let (status, v) = api(&app, post_json("/api/v1/guests/103/memory", &json!({ "memory_mb": 17408 }))).await;
    assert_eq!((status, v["online"].as_bool(), mem(&state)), (StatusCode::OK, Some(true), 17408), "{v}");
    assert_eq!(state.snapshot().guest(103).unwrap().hw.dimms, vec![1024]);
    let (status, v) = api(&app, post_json("/api/v1/guests/103/memory", &json!({ "memory_mb": 99999 }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");

    assert!(post_ok(&app, "/guests/103/memory", "memory_mb=17536").await.contains("notice="));
    assert_eq!((mem(&state), state.snapshot().tasks[0].command.as_str()), (17536, "vm memory 103 17536"));
    for bad in ["memory_mb=17536", "memory_mb=16384", "memory_mb=17600", "memory_mb=30000", "memory_mb=cok"] {
        assert!(post_ok(&app, "/guests/103/memory", bad).await.contains("error="), "{bad}");
    }
    assert_eq!(mem(&state), 17536, "reddedilen istek kaydı değiştirmez");

    // Sürücü ekleyemezse kayıt değişmez.
    assert!(post_ok(&app, "/guests/103/memory", "memory_mb=18560").await.contains("error="));
    let after = state.snapshot();
    assert_eq!((after.guest(103).unwrap().mem_total_mb, after.guest(103).unwrap().hw.dimms.len(), after.tasks[0].status), (17536, 2, TaskStatus::Failed));

    // İzleyiciye ve operatöre açık değildir.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/103/memory"), "memory_mb=18560")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/guests/103/memory"), None);

    // Çalışırken takmayı bilmeyen sürücü (benzetim) reddeder.
    let (sim, sim_state) = setup();
    sim_state.mutate(|c| {
        let g = c.guest_mut_for_import(103).unwrap();
        (g.hw.max_mem_mb, g.status) = (32768, GuestStatus::Running);
    });
    assert!(post_ok(&sim, "/guests/103/memory", "memory_mb=17408").await.contains("error="));
    assert_eq!(sim_state.snapshot().guest(103).unwrap().mem_total_mb, 16384);
}

// ---------- Canlı taşıma uçları ----------

#[tokio::test]
pub(super) async fn live_migration_endpoints_are_admin_only_and_validate_their_input() {
    let (app, state) = setup_real();
    let key = "0a".repeat(32);
    let body = |host: &str, nbd: u16, ram: u16, key: &str| json!({ "host": host, "nbd_port": nbd, "ram_port": ram, "key": key });
    // Kapalı konuk ve taşınamayan donanım, sürücüye sorulmadan reddedilir.
    let (status, v) = api(&app, post_json("/api/v1/guests/103/migrate/prepare", &json!({}))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("çalışmıyor"), "{status} {v}");
    state.mutate(|c| c.guest_mut_for_import(103).unwrap().status = GuestStatus::Running);
    // Bu test sürücüsü canlı taşımayı bilmez: istek açık bir hatayla döner, konuk yerinde kalır.
    let (status, v) = api(&app, post_json("/api/v1/guests/103/migrate/prepare", &json!({}))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("desteklemiyor"), "{status} {v}");
    for bad in [body("10.0.0.7", 49152, 49153, "kisa"), body("10.0.0.7;rm", 49152, 49153, &key), body("10.0.0.7", 80, 49153, &key), body("10.0.0.7", 49152, 49152, &key)] {
        let (status, v) = api(&app, post_json("/api/v1/guests/103/migrate", &bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad} {v}");
    }
    let (status, v) = api(&app, post_json("/api/v1/guests/103/migrate", &body("10.0.0.7", 49152, 49153, &key))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("desteklemiyor"), "{status} {v}");
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);

    // Hedef tarafı: tanımsız eş ve yetkisiz kullanıcı.
    let (status, v) = api(&app, post_json("/api/v1/peers/yok/migrate/103", &json!({}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(post_ok(&app, "/cluster/migrate", "peer=yok&vmid=103").await.contains("error="));
    assert!(post_ok(&app, "/cluster/migrate", "peer=yok&vmid=yuz").await.contains("error="));
    for path in ["/api/v1/guests/103/migrate/prepare", "/api/v1/guests/103/migrate", "/api/v1/peers/yok/migrate/103"] {
        let request = as_viewer(Method::POST, path).header("content-type", "application/json").body(Body::from(body("10.0.0.7", 49152, 49153, &key).to_string())).unwrap();
        assert_eq!(send(&app, request).await.0, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(redox_server::operator_target(path.trim_start_matches("/api/v1")), None, "{path}");
    }
}

// ---------- İşlemci payı ve bellek garantisi ----------

#[tokio::test]
pub(super) async fn cpu_shares_and_memory_reservation_are_set_on_the_options_tab() {
    let (app, state) = setup_real();
    let hw = |state: &AppState| {
        let g = state.snapshot().guest(103).unwrap().clone();
        (g.hw.cpu_shares, g.hw.mem_reserve_mb)
    };
    let tab = page(&app, "/guests/103?tab=secenekler").await;
    assert!(tab.contains(r#"<input id="cpu_shares" name="cpu_shares" type="text" inputmode="numeric" value="0""#) && tab.contains(r#"name="mem_reserve_mb""#), "{tab}");
    assert!(!page(&app, "/guests/202?tab=secenekler").await.contains(r#"name="cpu_shares""#), "konteynerde yok");
    let base = "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=&pool=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_shares=200&mem_reserve_mb=4096")).await.contains("notice="));
    assert_eq!(hw(&state), (200, 4096));
    for bad in ["cpu_shares=5&mem_reserve_mb=0", "cpu_shares=200&mem_reserve_mb=999999", "cpu_shares=çok&mem_reserve_mb=0"] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await.contains("error="), "{bad}");
    }
    assert_eq!(hw(&state), (200, 4096), "reddedilen istek kaydı değiştirmez");
    // Çalışırken de değişir (donanım ayarı değildir); API'de aynı alanlar.
    state.mutate(|c| c.guest_mut_for_import(103).unwrap().status = GuestStatus::Running);
    let (status, v) = api(&app, request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "cpu_shares": 50, "mem_reserve_mb": 0 }).to_string())).unwrap()).await;
    assert_eq!((status, hw(&state)), (StatusCode::OK, (50, 0)), "{v}");
    let shown = page(&app, "/guests/103?tab=secenekler").await;
    assert!(shown.contains(r#"name="cpu_shares" type="text" inputmode="numeric" value="50""#), "{shown}");
}

// ---------- Bakım kipinde boşaltma ----------

#[tokio::test]
pub(super) async fn maintenance_evacuation_needs_a_known_peer_and_an_admin() {
    let (app, state) = setup_real();
    // Tanımsız eş: bakıma girilmez.
    let (status, v) = api(&app, post_json("/api/v1/maintenance", &json!({ "on": true, "evacuate": "yok" }))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("tanımlı değil"), "{status} {v}");
    assert!(post_ok(&app, "/maintenance/evacuate", "peer=yok").await.contains("error="));
    assert!(!state.in_maintenance());
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/maintenance/evacuate"), "peer=yok")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/maintenance/evacuate"), None);
    // Boşaltma istenmeyen bakım eskisi gibi çalışır.
    let (status, v) = api(&app, post_json("/api/v1/maintenance", &json!({ "on": true }))).await;
    assert_eq!((status, v["on"].as_bool(), state.in_maintenance()), (StatusCode::OK, Some(true), true), "{v}");
}

// ---------- Destek paketi ----------

#[tokio::test]
pub(super) async fn the_support_bundle_is_for_admins_and_carries_no_secrets() {
    let (app, state) = setup_real();
    state.mutate(|c| {
        let g = c.guest_mut_for_import(103).unwrap();
        g.hw.backup_pre = "/usr/bin/dump --password=cok-gizli".into();
    });
    let (status, v) = api(&app, get("/api/v1/support")).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!((v["version"].as_str(), v["driver"]["real"].as_bool()), (Some(env!("CARGO_PKG_VERSION")), Some(true)));
    assert!(v["guests"].as_array().unwrap().len() >= 5 && v["nodes"].is_array() && v["storages"].is_array() && v["tasks"].is_array() && v["settings"]["migrate"]["ports"].is_array(), "{v}");
    let text = v.to_string();
    for secret in ["cok-gizli", "es-sunucu", "dogru-at-pil", "password_hash", "\"token\"", "\"hash\"", "Bearer"] {
        assert!(!text.contains(secret), "pakette gizli bilgi olmamalı: {secret}");
    }
    assert!(text.contains("(gizlendi)"), "maskelenen alan belli edilir");
    // Dosya olarak indirme ve yetki.
    let (status, headers, body) = send(&app, get("/support")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-disposition"].to_str().unwrap().starts_with("attachment; filename=\"redox-destek-") && body.contains("\"version\""));
    assert!(page(&app, "/cluster").await.contains(r#"href="/support" download"#));
    for path in ["/api/v1/support", "/support"] {
        assert_eq!(send(&app, as_viewer(Method::GET, path).body(Body::empty()).unwrap()).await.0, StatusCode::FORBIDDEN, "{path}");
    }
}

// ---------- Kilit kipi ----------

#[tokio::test]
pub(super) async fn lockdown_refuses_every_change_until_an_admin_lifts_it() {
    let (app, state) = setup();
    // Yalnızca yönetici kilitler.
    let viewer = as_viewer(Method::POST, "/api/v1/lockdown").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "on": true }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    assert!(page(&app, "/nodes/pve-ist-01").await.contains(r#"action="/lockdown/on""#));
    let (status, v) = api(&app, post_json("/api/v1/lockdown", &json!({ "on": true }))).await;
    assert_eq!((status, v["on"].as_bool(), state.in_lockdown()), (StatusCode::OK, Some(true), true));
    assert_eq!(state.snapshot().tasks[0].command, "lockdown on");

    // Kilitliyken: API'den ve arayüzden hiçbir değişiklik kabul edilmez, yönetici dahil; okumalar çalışır.
    let before = state.snapshot().guest(103).unwrap().clone();
    let (status, v) = api(&app, post_json("/api/v1/guests/103/start", &json!({}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::LOCKED, Some("lockdown")), "{v}");
    for (path, body) in [("/guests/103/action", "action=start"), ("/guests/103/edit", "part=secenekler&name=x"), ("/maintenance/on", ""), ("/guests/103/delete", "")] {
        let (status, _, text) = send(&app, form_body(request(Method::POST, path), body)).await;
        assert!(status == StatusCode::LOCKED && text.contains("kilit kipinde"), "{path}: {status} {text}");
    }
    let (status, _) = api(&app, request(Method::DELETE, "/api/v1/guests/103").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::LOCKED);
    assert_eq!(state.snapshot().guest(103).unwrap(), &before, "kilitliyken konuk değişmez");
    let (status, v) = api(&app, get("/api/v1/guests")).await;
    assert!(status == StatusCode::OK && v.as_array().is_some());
    let home = page(&app, "/").await;
    assert!(home.contains("Kilit kipi açık") && home.contains(r#"action="/lockdown/off""#));
    assert!(!page(&app, "/nodes/pve-ist-01").await.contains(r#"action="/lockdown/on""#));
    // Açık kalan yollar: kişinin kendi hesabı ve çıkış.
    for open in ["/logout", "/account/password", "/lockdown/off", "/api/v1/lockdown"] {
        assert!(redox_server::lockdown_exempt(open), "{open}");
    }
    for shut in ["/guests/103/action", "/api/v1/guests", "/lockdown/on", "/maintenance/off"] {
        assert!(!redox_server::lockdown_exempt(shut), "{shut}");
    }

    // İzleyici kilidi kaldıramaz; yönetici kaldırır ve işlemler yeniden kabul edilir.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/lockdown/off"), "")).await;
    assert_eq!((status, state.in_lockdown()), (StatusCode::FORBIDDEN, true));
    assert!(post_ok(&app, "/lockdown/off", "").await.contains("notice="));
    assert!(!state.in_lockdown());
    let (status, _) = api(&app, post_json("/api/v1/guests/103/start", &json!({}))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    // Durum dosyasında saklanır: sunucu yeniden başlasa da kilit durur.
    state.lockdown_set(true, "admin");
    let saved: redox_core::Cluster = serde_json::from_value(serde_json::to_value(state.snapshot()).unwrap()).unwrap();
    assert!(saved.lockdown);
}
