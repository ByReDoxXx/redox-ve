//! Kaynak havuzları, kotalar ve depolama alanları arasında taşıma.

use super::*;

#[tokio::test]
pub(super) async fn resource_pools_group_guests_and_enforce_quotas() {
    let (app, state) = setup();
    let empty = page(&app, "/pools").await;
    assert!(empty.contains("Havuz yok") && empty.contains(r#"<form class="inline" method="post" action="/pools">"#));
    assert!(page(&app, "/").await.contains(r#"href="/pools""#), "sol menüde bağlantı var");
    assert!(!empty.contains("<script"), "sayfa betiksizdir");

    // Form: boş alan sınırsızdır; hatalı ad ve sayı reddedilir.
    assert!(post_ok(&app, "/pools", "name=web&cores=12&memory_mb=&disk_gb=&guests=").await.contains("notice="));
    for bad in ["name=B%C3%BCy%C3%BCk&cores=1", "name=x&cores=cok", "name=&cores=1"] {
        assert!(post_ok(&app, "/pools", bad).await.contains("error="), "{bad}");
    }
    let pool = state.snapshot().pool("web").unwrap().clone();
    assert_eq!((pool.cores, pool.memory_mb, pool.disk_gb, pool.guests), (12, 0, 0, 0));

    // Konuk, Seçenekler sekmesinden havuza alınır (103: 8 çekirdek).
    let options = page(&app, "/guests/103?tab=secenekler").await;
    assert!(options.contains(r#"<select id="pool" name="pool">"#) && options.contains(r#"<option value="web">web</option>"#), "{options}");
    let base = "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&pool=web")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().pool.as_deref(), Some("web"));
    assert!(page(&app, "/guests/103?tab=secenekler").await.contains(r#"<option value="web" selected>web</option>"#));
    let listed = page(&app, "/pools").await;
    assert!(listed.contains("8 / 12") && listed.contains("1 / sınırsız") && listed.contains(r#"style="width:66%""#), "{listed}");
    assert!(listed.contains(r#"action="/pools/web/delete""#) && listed.contains(r#"aria-label="web havuzunu sil""#));

    // Kota: 12 çekirdeklik havuza 8 çekirdekli ikinci konuk sığmaz; 103'ün çekirdeği 13'e çıkarılamaz.
    let other = "part=secenekler&name=db-postgres&os=ubuntu-24.04&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=";
    let to = post_ok(&app, "/guests/102/edit", &format!("{other}&pool=web")).await;
    assert!(to.contains("error=") && state.snapshot().guest(102).unwrap().pool.is_none(), "{to}");
    let hardware = "part=donanim&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{hardware}&cores=13")).await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{hardware}&cores=12")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().cores, 12);
    // Dolu havuzdaki konuk klonlanamaz.
    assert!(post_ok(&app, "/guests/103/clone", "name=kopya").await.contains("error="));
    // Seçenekler sekmesini yeniden kaydetmek konuğu havuzda tutar; donanım sekmesi havuza dokunmaz.
    assert_eq!(state.snapshot().guest(103).unwrap().pool.as_deref(), Some("web"));

    // İçinde konuk olan havuz silinmez; kota kullanımın altına indirilemez.
    assert!(post_ok(&app, "/pools/web/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/pools", "name=web&cores=4").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&pool=")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().pool, None);
    assert!(post_ok(&app, "/pools/web/delete", "").await.contains("notice="));
    assert!(post_ok(&app, "/pools/web/delete", "").await.contains("error="));

    // API: liste kullanımı da verir; izleyici değiştiremez.
    let (status, _) = api(&app, post_json("/api/v1/pools", &json!({ "name": "db", "disk_gb": 600, "guests": 2 }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let patch = |vmid: u32, body: Value| request(Method::PATCH, &format!("/api/v1/guests/{vmid}")).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert!(api(&app, patch(102, json!({ "pool": "db" }))).await.0.is_success());
    let (status, list) = api(&app, get("/api/v1/pools")).await;
    assert_eq!((status, list[0]["name"].as_str(), list[0]["used"]["guests"].as_u64(), list[0]["used"]["disk_gb"].as_u64()), (StatusCode::OK, Some("db"), Some(1), Some(500)));
    // 500 GB'lik konuk havuzdayken 200 GB'lik ikinci konuk 600 GB kotaya sığmaz.
    let (status, v) = api(&app, patch(103, json!({ "pool": "db" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("GB disk"), "{v}");
    let viewer = as_viewer(Method::POST, "/api/v1/pools").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "name": "x" }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    let delete = |name: &str| request(Method::DELETE, &format!("/api/v1/pools/{name}")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, delete("db")).await.0, StatusCode::UNPROCESSABLE_ENTITY, "içinde konuk var");
    assert!(api(&app, patch(102, json!({ "pool": "" }))).await.0.is_success());
    assert_eq!(send(&app, delete("db")).await.0, StatusCode::NO_CONTENT);
}

#[tokio::test]
pub(super) async fn unbound_offsite_copies_are_marked() {
    let (app, state) = setup();
    let backup = |id: &str, offsite: bool, bound: bool| redox_core::Backup {
        id: id.into(), vmid: 103, guest_name: "win-build".into(), created: now(), size_mb: 5, auto: false, offsite, new_mb: None, disks: Vec::new(), disk_gb: 200,
        quiesced: None, cbt: false, offsite_bound: bound, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0,
    };
    state.mutate(|c| {
        c.add_backup(backup("vm-103-1", true, false));
        c.add_backup(backup("vm-103-2", true, true));
        c.add_backup(backup("vm-103-3", false, false));
    });
    let all = page(&app, "/backups").await;
    assert_eq!(all.matches("ada bağlı değil").count() - all.matches("dış kopya ada bağlı değil").count(), 1, "yalnızca eski biçimli dış kopya işaretlenir");
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert_eq!(tab.matches(">dış kopya ada bağlı değil<").count(), 1, "{tab}");
}

// ---------- Depolama alanları arasında taşıma ----------

#[tokio::test]
pub(super) async fn guest_disks_are_moved_to_another_storage_from_the_clone_tab() {
    let (app, state) = setup();
    // 103, pve-ist-02 düğümünde ve local-zfs alanında; aynı düğüme disk tutan ikinci bir alan eklenir.
    state.mutate(|c| {
        c.storages.push(redox_core::Storage { id: "hizli".into(), node: Some("pve-ist-02".into()), kind: redox_core::StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 1000 });
        c.storages.push(redox_core::Storage { id: "dar".into(), node: Some("pve-ist-02".into()), kind: redox_core::StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 10 });
    });
    let tab = page(&app, "/guests/103?tab=cogalt").await;
    assert!(tab.contains(r#"action="/guests/103/move""#) && tab.contains(r#"<option value="hizli">hizli · 1000 GB boş</option>"#), "{tab}");
    assert!(tab.contains(r#"<option value="dar" disabled>dar · 10 GB boş</option>"#), "yeri yetmeyen alan seçilemez");
    assert!(!tab.contains(r#"value="nfs-yedek""#) && !tab.contains(r#"value="local-lvm""#), "disk tutmayan ve başka düğümdeki alanlar sunulmaz");
    // Çalışan konukta form kapalıdır ve istek reddedilir.
    let running = page(&app, "/guests/102?tab=cogalt").await;
    assert!(running.contains(r#"<select name="storage" aria-label="Hedef depolama alanı" disabled>"#));
    assert!(post_ok(&app, "/guests/102/move", "storage=hizli").await.contains("error="));
    for bad in ["storage=", "storage=dar", "storage=nfs-yedek", "storage=local-zfs", "storage=yok"] {
        assert!(post_ok(&app, "/guests/103/move", bad).await.contains("error="), "{bad}");
    }

    let before = state.snapshot();
    let used = |c: &Cluster, id: &str| c.storages.iter().find(|s| s.id == id && s.available_on("pve-ist-02")).unwrap().used_gb;
    assert!(post_ok(&app, "/guests/103/move", "storage=hizli").await.contains("notice="));
    let after = state.snapshot();
    assert_eq!(after.guest(103).unwrap().storage, "hizli");
    assert_eq!((used(&after, "hizli"), used(&before, "local-zfs") - used(&after, "local-zfs")), (200, 200));
    assert_eq!(after.tasks[0].command, "vm move 103 hizli");
    assert!(page(&app, "/guests/103?tab=cogalt").await.contains(r#"<option value="local-zfs">"#), "geri taşınabilir");

    // API ve yetki.
    let (status, task) = api(&app, post_json("/api/v1/guests/103/move", &json!({ "storage": "local-zfs" }))).await;
    assert_eq!((status, task["command"].as_str()), (StatusCode::ACCEPTED, Some("vm move 103 local-zfs")));
    assert_eq!(state.snapshot().guest(103).unwrap().storage, "local-zfs");
    let viewer = as_viewer(Method::POST, "/api/v1/guests/103/move").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "storage": "hizli" }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/guests/103/move"), None, "operatöre açık değil");
}

// ---------- Yedekten yeni konuk ----------
