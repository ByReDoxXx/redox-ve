//! Konuk işlemleri: düzenleme, anlık görüntü, yedek, klon, toplu işlem.

use super::*;

pub(super) async fn post_ok(app: &Router, uri: &str, body: &str) -> String {
    let (status, headers, _) = send(app, post_form(uri, body)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
    location(&headers).to_string()
}

#[tokio::test]
pub(super) async fn guest_tabs_depend_on_kind_and_role() {
    let (app, _) = setup();
    let vm = page(&app, "/guests/103").await;
    for tab in ["?tab=konsol", "?tab=donanim", "?tab=secenekler", "?tab=goruntuler", "?tab=yedekler", "?tab=cogalt"] {
        assert!(vm.contains(&format!(r#"href="/guests/103{tab}""#)), "{tab}");
    }
    assert!(!page(&app, "/guests/200").await.contains("?tab=konsol"), "konteynerde konsol sekmesi yok");
    assert!(page(&app, "/guests/103?tab=konsol").await.contains("Simülasyon sürücüsünde konsol yok"));

    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/103?tab=konsol").body(Body::empty()).unwrap()).await;
    assert!(!viewer.contains("?tab=konsol") && !viewer.contains("iframe"), "izleyici konsolu göremez");
    for tab in ["donanim", "secenekler", "cogalt"] {
        let (_, _, viewer) = send(&app, as_viewer(Method::GET, &format!("/guests/103?tab={tab}")).body(Body::empty()).unwrap()).await;
        assert!(!viewer.contains("/guests/103/edit") && !viewer.contains("Kaydet") && !viewer.contains("tab=cogalt"), "{tab}: izleyiciye form ve çoğaltma sunulmaz");
    }
}

#[tokio::test]
pub(super) async fn edit_guest_settings() {
    let (app, state) = setup();
    let form = settings(&app, "/guests/103").await;
    assert!(form.contains(r#"name="name" type="text" value="win-build""#) && form.contains(r#"name="cores""#) && form.contains("Klonla"));
    let running = settings(&app, "/guests/100").await;
    assert!(running.contains("Donanım ayarları yalnızca konuk kapalıyken değişir.") && running.contains(r#"name="cores" type="text" inputmode="numeric" value="4" disabled"#));
    assert!(!form.contains(r#"name="cores" type="text" inputmode="numeric" value="8" disabled"#), "kapalı konukta donanım alanları açıktır");

    let to = post_ok(&app, "/guests/103/edit", "name=derleme&os=windows-2025&tags=ci%2C+Prod&cores=4&memory_mb=8192&iso=&forwards=8080%3A80%2C+2222%3A22&onboot=1").await;
    assert!(to.starts_with("/guests/103?tab=secenekler&notice="), "{to}");
    let g = state.snapshot().guest(103).unwrap().clone();
    assert_eq!((g.name.as_str(), g.cores, g.mem_total_mb, g.onboot), ("derleme", 4, 8192, true));
    assert_eq!((g.tags, g.forwards.len()), (vec!["ci".to_string(), "prod".to_string()], 2));

    let conf = page(&app, "/guests/103").await;
    assert!(conf.contains("8080 → 80") && conf.contains(r#"<a class="tag" href="/guests?q=ci">ci</a>"#));
    let filtered = page(&app, "/guests?q=prod").await;
    // "derleme" adında "prod" geçmez; yalnızca etiketiyle eşleşir. Adında geçenler de listelenir.
    assert!(filtered.contains("derleme") && filtered.contains("web-prod-01") && !filtered.contains("monitoring"));

    // Hatalar düzenleme sayfasına iletiyle döner ve kaydı değiştirmez.
    for (body, needle) in [
        ("name=derleme&cores=4&memory_mb=8192&forwards=80%3A80", "error="),
        ("name=derleme&cores=dort&memory_mb=8192&forwards=", "error="),
        ("name=K%C3%B6t%C3%BC&cores=4&memory_mb=8192&forwards=", "error="),
        ("name=derleme&cores=4&memory_mb=8192&forwards=bozuk", "error="),
    ] {
        let to = post_ok(&app, "/guests/103/edit", body).await;
        assert!(to.starts_with("/guests/103?tab=donanim&") && to.contains(needle), "{body}: {to}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().forwards.len(), 2);

    // Çalışan konukta: donanım alanları gönderilmez (kapalı alan grubu), ad yine de değişir.
    post_ok(&app, "/guests/100/edit", "name=web-yeni&os=debian-12&tags=").await;
    assert_eq!(state.snapshot().guest(100).unwrap().name, "web-yeni");
    // Elle gönderilen donanım değişikliği reddedilir.
    let to = post_ok(&app, "/guests/100/edit", "name=web-yeni&cores=1&memory_mb=8192&forwards=").await;
    assert!(to.starts_with("/guests/100?tab=donanim&error="));
    assert_eq!(state.snapshot().guest(100).unwrap().cores, 4);
    assert_eq!(send(&app, get("/guests/999/edit")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
pub(super) async fn snapshots_with_confirmation() {
    let (app, state) = setup();
    assert!(page(&app, "/guests/103?tab=goruntuler").await.contains("Henüz anlık görüntü yok"));
    let to = post_ok(&app, "/guests/103/snapshots", "name=temiz").await;
    assert!(to.starts_with("/guests/103?tab=goruntuler&notice="));
    let tab = page(&app, "/guests/103?tab=goruntuler").await;
    assert!(tab.contains("temiz") && tab.contains("/guests/103/confirm?op=rollback&amp;name=temiz"));

    // Onay sayfası hiçbir şeyi değiştirmez ve sonucu açıkça söyler.
    let confirm = page(&app, "/guests/103/confirm?op=rollback&name=temiz").await;
    assert!(confirm.contains("kaybolur") && confirm.contains(r#"action="/guests/103/snapshots/temiz/rollback""#));
    assert_eq!(state.snapshot().guest(103).unwrap().snapshots.len(), 1);
    for missing in ["/guests/103/confirm?op=rollback&name=yok", "/guests/103/confirm?op=patlat&name=temiz", "/guests/103/confirm?op=restore&name=temiz"] {
        assert_eq!(send(&app, get(missing)).await.0, StatusCode::NOT_FOUND, "{missing}");
    }

    assert!(post_ok(&app, "/guests/103/snapshots/temiz/rollback", "").await.contains("notice="));
    assert!(post_ok(&app, "/guests/103/snapshots", "name=temiz").await.contains("error="), "aynı ad ikinci kez alınmaz");
    assert!(post_ok(&app, "/guests/100/snapshots", "name=canli").await.contains("error="), "çalışan konukta alınmaz");
    assert!(post_ok(&app, "/guests/103/snapshots/temiz/patlat", "").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/snapshots/temiz/delete", "").await.contains("notice="));
    assert!(state.snapshot().guest(103).unwrap().snapshots.is_empty());
    assert!(page(&app, "/tasks?q=103").await.contains("redox vm delsnapshot 103 temiz"));
}

#[tokio::test]
pub(super) async fn backups_restore_and_delete() {
    let (app, state) = setup();
    assert!(post_ok(&app, "/guests/100/backups", "").await.contains("error="), "çalışan konuk yedeklenmez");
    assert!(post_ok(&app, "/guests/103/backups", "").await.contains("notice="));
    let id = state.snapshot().backups[0].id.clone();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&id) && tab.contains("Geri Yükle"));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains(&id), "yedek yalnızca kendi konuğunda listelenir");

    let confirm = page(&app, &format!("/guests/103/confirm?op=restore&name={id}")).await;
    assert!(confirm.contains("bütün anlık görüntüler kaybolur") && confirm.contains(&format!(r#"action="/backups/{id}/restore""#)));
    assert_eq!(send(&app, get(&format!("/guests/202/confirm?op=restore&name={id}"))).await.0, StatusCode::NOT_FOUND, "başka konuğun yedeği onaylanamaz");

    assert!(post_ok(&app, &format!("/backups/{id}/restore"), "").await.starts_with("/guests/103?tab=yedekler&notice="));
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.contains("notice="));
    assert!(state.snapshot().backups.is_empty());
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.starts_with("/?error="));
}

#[tokio::test]
pub(super) async fn resize_and_clone() {
    let (app, state) = setup();
    assert!(post_ok(&app, "/guests/103/resize", "disk_gb=300").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 300);
    for bad in ["disk_gb=100", "disk_gb=abc", "disk_gb=999999"] {
        assert!(post_ok(&app, "/guests/103/resize", bad).await.starts_with("/guests/103?tab=donanim&error="), "{bad}");
    }
    assert!(post_ok(&app, "/guests/100/resize", "disk_gb=500").await.contains("error="));

    let to = post_ok(&app, "/guests/103/clone", "name=win-build-2").await;
    assert!(to.starts_with("/guests/105?notice="), "{to}");
    let c = state.snapshot();
    assert_eq!((c.guest(105).unwrap().disk_gb, c.guest(105).unwrap().status), (300, GuestStatus::Stopped));
    assert!(post_ok(&app, "/guests/103/clone", "name=K%C3%B6t%C3%BC").await.contains("error="));
    assert!(post_ok(&app, "/guests/100/clone", "name=canli-kopya").await.contains("error="));
}

#[tokio::test]
pub(super) async fn bulk_action_applies_to_selected_guests() {
    let (app, state) = setup();
    let list = page(&app, "/guests").await;
    assert!(list.contains(r#"<form id="bulk" class="end" method="post" action="/guests/bulk""#) && list.contains(r#"form="bulk" name="id" value="103""#) && list.contains("Seçilenler:"));

    let to = post_ok(&app, "/guests/bulk", "back=%2Fguests&id=103&id=202&id=100&action=start").await;
    assert!(to.starts_with("/guests?notice="), "{to}");
    let c = state.snapshot();
    assert_eq!((c.guest(103).unwrap().status, c.guest(202).unwrap().status), (GuestStatus::Running, GuestStatus::Running));
    let notice = page(&app, &to).await;
    assert!(notice.contains("Başlat isteği 2 konuk için alındı.") && notice.contains("atlananlar: 100."), "atlananlar bildirilmeli");

    assert!(post_ok(&app, "/guests/bulk", "action=stop").await.contains("error="), "seçim yoksa hata");
    assert!(post_ok(&app, "/guests/bulk", "id=103&action=patlat").await.contains("error="));
    assert!(post_ok(&app, "/guests/bulk", "back=%2Fnodes%2Fpve-ist-02&id=103&action=stop").await.starts_with("/nodes/pve-ist-02?notice="));
    assert!(post_ok(&app, "/guests/bulk", "back=https%3A%2F%2Fevil.example&id=103&action=start").await.starts_with("/guests?"), "site dışına dönülmez");

    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/bulk"), "id=100&action=stop")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/").body(Body::empty()).unwrap()).await;
    assert!(!viewer.contains(r#"name="id""#) && !viewer.contains("Seçililere uygula"));
}

#[tokio::test]
pub(super) async fn console_is_admin_only_and_frames_only_itself() {
    let (app, _) = setup();
    for uri in ["/guests/102/screen", "/guests/102/screen.png"] {
        let (status, _, _) = send(&app, as_viewer(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        let (status, headers, _) = send(&app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
        assert!(location(&headers).starts_with("/login"));
    }
    let (status, headers, frame) = send(&app, get("/guests/102/screen")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-frame-options").unwrap(), "SAMEORIGIN");
    let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'self'") && csp.contains("default-src 'none'"), "{csp}");
    assert!(frame.contains(r#"http-equiv="refresh" content="2""#) && frame.contains("/guests/102/screen.png?t=") && !frame.to_lowercase().contains("<script"));
    // Diğer sayfalar hâlâ hiçbir yerde çerçevelenemez.
    assert_eq!(send(&app, get("/guests/102")).await.1.get("x-frame-options").unwrap(), "DENY");

    // Simülasyonda ekran yok: görüntü isteği 503, tuş gönderme hata iletisiyle döner.
    assert_eq!(send(&app, get("/guests/102/screen.png")).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(send(&app, get("/guests/999/screen.png")).await.0, StatusCode::NOT_FOUND);
    for body in ["text=root&enter=1", "key=enter", "text=%C5%9Fifre", "key=power", &format!("text={}", "a".repeat(600))] {
        let to = post_ok(&app, "/guests/102/keys", body).await;
        assert!(to.starts_with("/guests/102?tab=konsol&error="), "{body}: {to}");
    }
    assert_eq!(post_ok(&app, "/guests/102/keys", "text=").await, "/guests/102?tab=konsol", "boş metin sessizce geçilir");
}

#[tokio::test]
pub(super) async fn api_edit_snapshots_and_backups() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| {
        request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
    };
    let (status, g) = api(&app, send_json(Method::PATCH, "/api/v1/guests/103", json!({"cores": 2, "tags": ["ci"], "forwards": [{"host_port": 8080, "guest_port": 80}]}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((g["cores"].as_u64(), g["tags"][0].as_str(), g["forwards"][0]["host_port"].as_u64()), (Some(2), Some("ci"), Some(8080)));
    let (status, v) = api(&app, send_json(Method::PATCH, "/api/v1/guests/100", json!({"cores": 1}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("requires_stopped")));
    let (status, v) = api(&app, send_json(Method::PATCH, "/api/v1/guests/202", json!({"forwards": [{"host_port": 8080, "guest_port": 1}]}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("port_taken")));

    let (status, t) = api(&app, post_json("/api/v1/guests/103/snapshots", &json!({"name": "s1"}))).await;
    assert_eq!((status, t["command"].as_str(), t["status"].as_str()), (StatusCode::ACCEPTED, Some("vm snapshot 103 s1"), Some("ok")));
    let (status, v) = api(&app, post_json("/api/v1/guests/103/snapshots", &json!({"name": "s1"}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("snapshot_exists")));
    let rollback = request(Method::POST, "/api/v1/guests/103/snapshots/s1/rollback").body(Body::empty()).unwrap();
    assert_eq!(send(&app, rollback).await.0, StatusCode::ACCEPTED);
    let del = |uri: &str| request(Method::DELETE, uri).body(Body::empty()).unwrap();
    assert_eq!(send(&app, del("/api/v1/guests/103/snapshots/s1")).await.0, StatusCode::ACCEPTED);
    let (status, v) = api(&app, del("/api/v1/guests/103/snapshots/s1")).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("snapshot_not_found")));

    let make = request(Method::POST, "/api/v1/guests/103/backups").body(Body::empty()).unwrap();
    let (status, v) = api(&app, make).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = v["id"].as_str().unwrap().to_string();
    let (_, list) = api(&app, get("/api/v1/backups")).await;
    assert_eq!((list.as_array().unwrap().len(), list[0]["vmid"].as_u64()), (1, Some(103)));
    let restore = request(Method::POST, &format!("/api/v1/backups/{id}/restore")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, restore).await.0, StatusCode::ACCEPTED);
    assert_eq!(send(&app, del(&format!("/api/v1/backups/{id}"))).await.0, StatusCode::NO_CONTENT);
    let (status, v) = api(&app, del(&format!("/api/v1/backups/{id}"))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("backup_not_found")));
    assert!(state.snapshot().backups.is_empty());

    // İzleyici okuyabilir ama değiştiremez.
    assert_eq!(send(&app, as_viewer(Method::GET, "/api/v1/backups").body(Body::empty()).unwrap()).await.0, StatusCode::OK);
    let (status, _, _) = send(&app, as_viewer(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from("{}")).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------- Gömülü yazı tipleri ve ölçümler ----------
