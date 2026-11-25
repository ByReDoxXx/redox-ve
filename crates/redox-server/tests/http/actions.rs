//! Eylemler: güç işlemleri ve formlar.

use super::*;

#[tokio::test]
pub(super) async fn start_form_runs_task_and_redirects() {
    let (app, state) = setup();
    let (status, headers, _) = send(&app, post_form("/guests/103/action", "action=start")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/103?notice="), "{}", location(&headers));
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);

    let body = page(&app, location(&headers)).await;
    assert!(body.contains(r#"role="status""#) && body.contains("Başlat isteği alındı: 103"));
    assert!(body.contains("redox vm start 103") && body.contains("Tamam"), "görev günlüğe düşmeli");
    assert!(!body.contains(r#"http-equiv="refresh""#), "süren görev yokken yenileme olmamalı");
}

#[tokio::test]
pub(super) async fn invalid_action_reports_error_and_changes_nothing() {
    let (app, state) = setup();
    let before = state.snapshot();
    let (status, headers, _) = send(&app, post_form("/guests/100/action", "action=start")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/100?error="));
    let body = page(&app, location(&headers)).await;
    assert!(body.contains(r#"role="alert""#) && body.contains("uygulanamaz"));

    for (uri, form) in [("/guests/100/action", "action=explode"), ("/guests/999/action", "action=start"), ("/guests/abc/action", "action=start")] {
        let (status, headers, _) = send(&app, post_form(uri, form)).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
        assert!(location(&headers).contains("error="), "{uri}");
    }
    assert_eq!(state.snapshot(), before);
}

#[tokio::test]
pub(super) async fn slow_task_locks_guest_and_page_auto_refreshes() {
    let (app, state) = setup_with(Duration::from_millis(80), SimDriver::default());
    let (_, headers, _) = send(&app, post_form("/guests/103/action", "action=start")).await;
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Stopped, "görev bitmeden durum değişmez");

    let body = page(&app, location(&headers)).await;
    assert!(body.contains(r#"<meta http-equiv="refresh" content="2;url=/guests/103">"#), "yenileme bildirimsiz adrese dönmeli");
    assert!(body.contains("İşlem sürüyor") && body.contains(r#"aria-label="1 görev sürüyor""#));
    assert!(body.contains(r#"name="action" value="start" disabled>"#), "kilitliyken düğmeler kapalı");

    // İkinci istek kilide takılır.
    let (_, headers, _) = send(&app, post_form("/guests/103/action", "action=start")).await;
    assert!(location(&headers).contains("error="));

    tokio::time::sleep(Duration::from_millis(400)).await;
    let c = state.snapshot();
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Running);
    assert!(!c.has_running_tasks());
    assert!(!page(&app, "/guests/103").await.contains(r#"http-equiv="refresh""#));
}

#[tokio::test]
pub(super) async fn driver_failure_is_shown_in_task_log() {
    let (app, state) = setup_with(Duration::ZERO, SimDriver::failing_on(103));
    send(&app, post_form("/guests/103/action", "action=start")).await;
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Stopped);
    let body = page(&app, "/tasks?q=103").await;
    assert!(body.contains("Hata") && body.contains("simülasyon: 103 start başarısız oldu"));
}

#[tokio::test]
pub(super) async fn create_form_happy_path() {
    let (app, state) = setup();
    let (status, headers, _) = send(&app, post_form("/guests", VALID_FORM)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/105?notice="));
    let c = state.snapshot();
    let g = c.guest(105).unwrap();
    assert_eq!((g.name.as_str(), g.status, g.onboot, g.mem_total_mb), ("yeni-vm", GuestStatus::Stopped, true, 2048));
    let body = page(&app, "/").await;
    assert!(body.contains("yeni-vm") && body.contains("redox vm create 105"));
}

#[tokio::test]
pub(super) async fn create_form_shows_errors_and_keeps_input() {
    let (app, state) = setup();
    let before = state.snapshot();

    let bad = VALID_FORM.replace("name=yeni-vm", "name=K%C3%B6t%C3%BC+Ad").replace("cores=2", "cores=iki");
    let (status, _, body) = send(&app, post_form("/guests", &bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("Konuk oluşturulamadı") && body.contains("Çekirdek bir tam sayı olmalı."));
    assert!(body.contains(r#"value="Kötü Ad""#) && body.contains(r#"value="iki""#), "girilen değerler korunmalı");
    assert!(body.contains(r##"<a href="#cores">"##), "özet ilgili alana bağlanmalı");

    let taken = VALID_FORM.replace("vmid=", "vmid=100");
    let (status, _, body) = send(&app, post_form("/guests", &taken)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("100 kimliği zaten kullanılıyor.") && body.contains(r#"id="vmid-err""#));

    let wrong_storage = VALID_FORM.replace("storage=local-zfs", "storage=local-lvm");
    let (status, _, body) = send(&app, post_form("/guests", &wrong_storage)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("Local-lvm deposu pve-ist-01 düğümünde kullanılamıyor.") && body.contains(r#"id="storage-err""#));

    let bad_name = VALID_FORM.replace("name=yeni-vm", "name=-x");
    let (_, _, body) = send(&app, post_form("/guests", &bad_name)).await;
    assert!(body.contains(r#"id="name-err""#));

    assert_eq!(state.snapshot(), before, "hatalı formlar konuk ya da görev oluşturmamalı");
}

#[tokio::test]
pub(super) async fn delete_needs_confirmation_and_stopped_guest() {
    let (app, state) = setup();
    let confirm = page(&app, "/guests/103/delete").await;
    assert!(confirm.contains("kalıcı olarak silinecek") && confirm.contains("Kalıcı Olarak Sil") && confirm.contains("200 GB"));
    assert!(state.snapshot().guest(103).is_ok(), "onay sayfasını açmak silmez");

    let (_, headers, _) = send(&app, post_form("/guests/100/delete", "")).await;
    assert!(location(&headers).starts_with("/guests/100?error="), "çalışan konuk silinemez");
    assert!(state.snapshot().guest(100).is_ok());

    let (status, headers, _) = send(&app, post_form("/guests/103/delete", "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/?notice="));
    assert!(state.snapshot().guest(103).is_err());
    assert_eq!(send(&app, get("/guests/103")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
pub(super) async fn command_line_navigates_and_acts() {
    let (app, state) = setup();
    for (q, expected) in [
        ("open+postgres", "/guests/102"),
        ("pve-ank-01", "/nodes/pve-ank-01"),
        ("200", "/guests/200"),
        ("storage", "/storage"),
        ("help", "/help"),
        ("home", "/"),
        ("zzz", "/guests?q=zzz"),
    ] {
        let (status, headers, _) = send(&app, post_form("/cmd", &format!("q={q}"))).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{q}");
        assert_eq!(location(&headers), expected, "{q}");
    }
    for q in ["", "stop", "start+999", "open+web-prod"] {
        let (_, headers, _) = send(&app, post_form("/cmd", &format!("q={q}"))).await;
        assert!(location(&headers).starts_with("/?error="), "{q}: {}", location(&headers));
    }

    let (_, headers, _) = send(&app, post_form("/cmd", "q=start+win-build")).await;
    assert!(location(&headers).starts_with("/guests/103?notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);
}

#[tokio::test]
pub(super) async fn theme_cookie_and_safe_redirect() {
    let (app, _) = setup();
    let (status, headers, _) = send(&app, get("/theme?to=light&back=%2Fstorage")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/storage");
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().starts_with("theme=light;"));

    let req = request(Method::GET, "/").header(COOKIE, "theme=light").body(Body::empty()).unwrap();
    let (_, _, body) = send(&app, req).await;
    assert!(body.contains(r#"data-theme="light""#) && body.contains("/theme?to=dark&amp;back=%2F"));

    let (_, headers, _) = send(&app, get("/theme?to=dark&back=%2F%2Fevil.example")).await;
    assert_eq!(location(&headers), "/", "site dışına yönlendirmemeli");
}

#[tokio::test]
pub(super) async fn cross_site_posts_are_rejected() {
    let (app, state) = setup();
    let before = state.snapshot();
    let cross = |uri: &str, name: &'static str, value: &str| {
        request(Method::POST, uri)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(name, value)
            .body(Body::from("action=start"))
            .unwrap()
    };
    for req in [
        cross("/guests/103/action", "origin", "http://evil.example"),
        cross("/guests/103/action", "origin", "null"),
        cross("/guests/103/action", "sec-fetch-site", "cross-site"),
        cross("/api/v1/guests/103/start", "origin", "http://evil.example"),
        cross("/guests/103/delete", "origin", "http://127.0.0.1:9999"),
    ] {
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    }
    assert_eq!(state.snapshot(), before);

    let same = request(Method::POST, "/guests/103/action")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(ORIGIN, format!("http://{HOST_NAME}"))
        .header("sec-fetch-site", "same-origin")
        .body(Body::from("action=start"))
        .unwrap();
    assert_eq!(send(&app, same).await.0, StatusCode::SEE_OTHER);
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);
}

#[tokio::test]
pub(super) async fn assets_are_served_with_types() {
    let (app, _) = setup();
    for (file, mime, needle) in [
        ("style.css", "text/css; charset=utf-8", "--ember:"),
        ("icons.svg", "image/svg+xml", r#"id="i-vm""#),
        ("logo.svg", "image/svg+xml", "<svg"),
        ("wordmark.svg", "image/svg+xml", "<svg"),
    ] {
        let (status, headers, body) = send(&app, get(&format!("/assets/{file}"))).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), mime, "{file}");
        assert!(body.contains(needle), "{file}");
    }
    assert_eq!(send(&app, get("/assets/yok.js")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
pub(super) async fn every_icon_used_in_pages_exists_in_sprite() {
    let (app, _) = setup();
    let sprite = page(&app, "/assets/icons.svg").await;
    for uri in ["/", "/guests/102", "/guests/104", "/guests/103/delete", "/storage", "/help", "/guests/new?kind=vm"] {
        let body = page(&app, uri).await;
        let mut used = 0;
        for part in body.split("/assets/icons.svg?v=").skip(1) {
            let (version, rest) = part.split_once('#').unwrap();
            assert_eq!(version, redox_server::view::asset_version(), "{uri}: sürüm damgası");
            let id = rest.split('"').next().unwrap();
            used += 1;
            assert!(sprite.contains(&format!(r#"id="{id}""#)), "{uri}: sprite içinde {id} yok");
        }
        assert!(used > 5, "{uri}: ikon bulunamadı");
        assert!(body.contains(&format!("/assets/style.css?v={}", redox_server::view::asset_version())), "{uri}: stil damgasız");
    }
}

// ---------- JSON API ----------
