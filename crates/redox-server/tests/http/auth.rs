//! Kimlik doğrulama ve yetki.

use super::*;

#[tokio::test]
pub(super) async fn anonymous_requests_are_turned_away() {
    let (app, state) = setup();
    let before = state.snapshot();

    for (uri, next) in [("/", "%2F"), ("/guests/102?tab=yedekler", "%2Fguests%2F102%3Ftab%3Dyedekler"), ("/olmayan", "%2Folmayan")] {
        let (status, headers, body) = send(&app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
        assert_eq!(location(&headers), format!("/login?next={next}"), "{uri}");
        assert!(!body.contains("db-postgres"), "{uri}: kimliksiz yanıtta veri sızmamalı");
    }
    for uri in ["/guests/103/action", "/guests/103/delete", "/guests", "/cmd", "/logout"] {
        let (status, headers, _) = send(&app, form_body(anon(Method::POST, uri), "action=start&q=start+103")).await;
        assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login"), "{uri}");
    }
    for (method, uri) in [(Method::GET, "/api/v1/cluster"), (Method::GET, "/api/v1/guests"), (Method::POST, "/api/v1/guests/103/start"), (Method::DELETE, "/api/v1/guests/103")] {
        let (status, headers, body) = send(&app, anon(method, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(headers.get("www-authenticate").unwrap(), "Bearer");
        assert!(body.contains(r#""code":"unauthorized""#) && !body.contains("istanbul"), "{uri}: {body}");
    }
    assert_eq!(state.snapshot(), before);
}

#[tokio::test]
pub(super) async fn public_endpoints_need_no_login() {
    let (app, _) = setup();
    for uri in ["/login", "/healthz", "/assets/style.css", "/assets/logo.svg"] {
        let (status, _, _) = send(&app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    let (_, _, body) = send(&app, anon(Method::GET, "/login?next=%2Fstorage").body(Body::empty()).unwrap()).await;
    assert!(body.contains(r#"type="password""#) && body.contains(r#"autocomplete="current-password""#));
    assert!(body.contains(r#"name="next" value="/storage""#));
    assert!(!body.to_lowercase().contains("<script") && !body.contains("db-postgres"));
}

#[tokio::test]
pub(super) async fn bad_tokens_are_rejected_and_do_not_fall_back_to_cookie() {
    let (app, _) = setup();
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    for bad in ["Bearer yanlis", "Bearer ", "Basic YWRtaW46eA==", "redox_test_admin_token"] {
        let req = anon(Method::GET, "/api/v1/cluster").header(AUTHORIZATION, bad).header(COOKIE, &cookie).body(Body::empty()).unwrap();
        assert_eq!(send(&app, req).await.0, StatusCode::UNAUTHORIZED, "{bad}");
    }
}

#[tokio::test]
pub(super) async fn login_session_and_logout() {
    let (app, _) = setup();

    let (status, headers, body) = send(&app, login_request("admin", "yanlis-parola-123", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.get(SET_COOKIE).is_none(), "başarısız girişte çerez verilmez");
    assert!(body.contains("Kullanıcı adı ya da parola hatalı.") && body.contains(r#"value="admin""#));
    let (status, _, unknown) = send(&app, login_request("yok", ADMIN_PW, "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(unknown.contains("Kullanıcı adı ya da parola hatalı."), "kullanıcının var olup olmadığı belli edilmez");

    let (status, headers, _) = send(&app, login_request("admin", ADMIN_PW, "%2Fstorage")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/storage"));
    let set = headers.get(SET_COOKIE).unwrap().to_str().unwrap().to_string();
    assert!(set.starts_with("redox_session=") && set.contains("HttpOnly") && set.contains("SameSite=Strict"), "{set}");
    assert!(!set.contains("Secure"), "düz HTTP'de Secure çerez tarayıcıda kaybolur");
    let cookie = set.split(';').next().unwrap().to_string();
    assert_eq!(cookie.len(), "redox_session=".len() + 64);

    let with = |method: Method, uri: &str| anon(method, uri).header(COOKIE, &cookie);
    let (status, _, body) = send(&app, with(Method::GET, "/").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Çıkış") && body.contains(r#"action="/logout""#) && body.contains("Yeni Konuk"));
    let (status, headers, _) = send(&app, with(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/"), "oturum açıkken giriş sayfası yönlendirir");

    let (status, headers, _) = send(&app, form_body(with(Method::POST, "/logout"), "")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login"));
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    let (status, headers, _) = send(&app, with(Method::GET, "/").body(Body::empty()).unwrap()).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login?next=%2F"), "çıkıştan sonra eski çerez geçersiz");

    let (_, headers, _) = send(&app, login_request("admin", ADMIN_PW, "%2F%2Fevil.example")).await;
    assert_eq!(location(&headers), "/", "giriş sonrası site dışına yönlendirmemeli");
}

#[tokio::test]
pub(super) async fn repeated_failures_lock_the_login() {
    let (app, _) = setup();
    for _ in 0..4 {
        assert_eq!(send(&app, login_request("admin", "yanlis-parola-123", "")).await.0, StatusCode::UNAUTHORIZED);
    }
    let (status, _, body) = send(&app, login_request("admin", "yanlis-parola-123", "")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body.contains("Çok fazla hatalı deneme. 15 dakika sonra yeniden deneyin."));
    let (status, headers, _) = send(&app, login_request("admin", ADMIN_PW, "")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "kilitliyken doğru parola da reddedilir");
    assert!(headers.get(SET_COOKIE).is_none());
    // Başka hesap etkilenmez.
    login_cookie(&app, "izleyici", VIEWER_PW).await;
}

#[tokio::test]
pub(super) async fn viewer_can_read_but_not_change() {
    let (app, state) = setup();
    let before = state.snapshot();
    let get = |uri: &str| as_viewer(Method::GET, uri).body(Body::empty()).unwrap();

    let (status, _, home) = send(&app, get("/")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("<b>izleyici</b><small>salt okunur</small>") && !home.contains("Yeni Konuk") && !home.contains("/guests/new"));
    let (_, _, guest) = send(&app, get("/guests/103")).await;
    assert!(guest.contains("Salt okunur hesap") && !guest.contains(r#"name="action""#) && !guest.contains("/guests/103/delete"));
    assert_eq!(send(&app, get("/api/v1/guests")).await.0, StatusCode::OK);

    for uri in ["/guests/103/action", "/guests/103/delete", "/guests", "/cmd"] {
        let (status, _, body) = send(&app, form_body(as_viewer(Method::POST, uri), "action=start&q=start+103")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert!(body.contains("yönetici yetkisi gerekir"));
    }
    for (method, uri) in [(Method::POST, "/api/v1/guests/103/start"), (Method::DELETE, "/api/v1/guests/103")] {
        let (status, _, body) = send(&app, as_viewer(method, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert!(body.contains(r#""code":"forbidden""#));
    }
    assert_eq!(state.snapshot(), before);

    // Çerezli izleyici oturumu da aynı kısıtta; çıkış yapabilir.
    let cookie = login_cookie(&app, "izleyici", VIEWER_PW).await;
    let post = |uri: &str| form_body(anon(Method::POST, uri).header(COOKIE, &cookie), "action=start");
    assert_eq!(send(&app, post("/guests/103/action")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, post("/logout")).await.0, StatusCode::SEE_OTHER);
}

#[tokio::test]
pub(super) async fn tasks_are_attributed_to_the_acting_user() {
    let (app, state) = setup();
    send(&app, post_form("/guests/103/action", "action=start")).await;
    send(&app, post_form("/guests", VALID_FORM)).await;
    let c = state.snapshot();
    assert!(c.tasks.len() == 2 && c.tasks.iter().all(|t| t.user == "admin"), "{:?}", c.tasks);
    assert!(page(&app, "/tasks").await.contains(r#"<td class="muted">admin</td>"#));
}

#[tokio::test]
pub(super) async fn pages_are_not_cacheable_but_assets_are() {
    let (app, _) = setup();
    for uri in ["/", "/api/v1/cluster", "/guests/102"] {
        let (_, headers, _) = send(&app, get(uri)).await;
        assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store", "{uri}");
    }
    let (_, headers, _) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store");
    let (_, headers, _) = send(&app, get("/assets/style.css")).await;
    assert!(headers.get(CACHE_CONTROL).unwrap().to_str().unwrap().contains("max-age"));
    assert!(headers.get("strict-transport-security").is_none(), "TLS vekili yokken HSTS gönderilmez");
}

#[tokio::test]
pub(super) async fn behind_tls_proxy_hardens_cookies_and_uses_forwarded_ip() {
    let (app, _) = setup_full(Duration::ZERO, SimDriver::default(), true);
    let from = |ip: &str, password: &str| {
        form_body(
            anon(Method::POST, "/login").header("x-forwarded-for", format!("9.9.9.9, {ip}")),
            &format!("username=admin&password={password}&next="),
        )
    };
    let (status, headers, _) = send(&app, from("1.1.1.1", ADMIN_PW)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().ends_with("; Secure"));
    assert_eq!(headers.get("strict-transport-security").unwrap(), "max-age=31536000");

    // Kilit, vekilin bildirdiği (son) adrese göredir; istemcinin uydurduğu ilk değere göre değil.
    for _ in 0..5 {
        send(&app, from("1.1.1.1", "yanlis-parola-123")).await;
    }
    assert_eq!(send(&app, from("1.1.1.1", ADMIN_PW)).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(send(&app, from("2.2.2.2", ADMIN_PW)).await.0, StatusCode::SEE_OTHER, "başka adres kilitlenmez");
}

// ---------- Gerçek sürücü kipi ----------

#[tokio::test]
async fn api_requests_are_rate_limited_per_user_when_enabled() {
    let (app, state) = setup();
    state.set_api_rate(2);
    let get = || request(Method::GET, "/api/v1/cluster").body(Body::empty()).unwrap();
    assert_eq!(api(&app, get()).await.0, StatusCode::OK);
    assert_eq!(api(&app, get()).await.0, StatusCode::OK);
    let (status, headers, body) = send(&app, get()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.contains_key("retry-after") && body.contains("rate_limited"), "{body}");
    // Sayfalar sınırın dışındadır.
    assert!(page(&app, "/").await.contains("<html"));
    state.set_api_rate(0);
    assert_eq!(api(&app, get()).await.0, StatusCode::OK);
}
