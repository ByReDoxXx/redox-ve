//! Kullanıcı yönetimi, hesap, iki adımlı doğrulama, denetim.

use super::*;

#[tokio::test]
pub(super) async fn admin_pages_are_hidden_from_viewers() {
    let (app, _) = setup();
    let home = page(&app, "/").await;
    assert!(home.contains(r#"href="/users""#) && home.contains(r#"href="/audit""#) && home.contains(r#"href="/account""#));
    assert!(page(&app, "/users").await.contains("izleyici") && page(&app, "/audit").await.contains("Denetim Günlüğü"));

    let viewer = |uri: &str| as_viewer(Method::GET, uri).body(Body::empty()).unwrap();
    let (_, _, home) = send(&app, viewer("/")).await;
    assert!(!home.contains(r#"href="/users""#) && !home.contains(r#"href="/audit""#) && home.contains(r#"href="/account""#));
    for uri in ["/users", "/audit", "/users/admin", "/users?tab=ekle"] {
        let (status, _, body) = send(&app, viewer(uri)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert!(!body.contains("admin"), "{uri}: kullanıcı adları sızmamalı");
    }
    let cookie = login_cookie(&app, "izleyici", VIEWER_PW).await;
    assert_eq!(send(&app, anon(Method::GET, "/account").header(COOKIE, &cookie).body(Body::empty()).unwrap()).await.0, StatusCode::OK);
    for uri in ["/users", "/users/admin/role", "/users/admin/delete", "/users/admin/password", "/users/admin/tokens", "/users/admin/totp-reset"] {
        let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, uri), "name=x&password=cok-uzun-bir-parola&role=admin&confirm=1&label=t")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

/// Liste yalnızca özet ve "Yönet" menüsü taşır; formlar kullanıcının kendi sayfasında ve ekleme sayfasındadır.
#[tokio::test]
async fn each_user_has_a_manage_menu_and_a_page_of_their_own() {
    let (app, _) = setup();
    let list = page(&app, "/users").await;
    assert!(list.contains(r#"href="/users?tab=ekle""#) && list.contains(r#"href="/users/izleyici#parola""#));
    assert!(list.contains(r#"formaction="/users/izleyici/sessions/close""#) && list.contains(r#"href="/users/izleyici#sil""#));
    // Kendi satırında silme yoktur; listede parola alanı da yoktur.
    assert!(!list.contains(r#"href="/users/admin#sil""#) && !list.contains(r#"type="password""#));

    let one = page(&app, "/users/izleyici").await;
    for action in ["role", "password", "tokens", "sessions/close", "delete"] {
        assert!(one.contains(&format!(r#"action="/users/izleyici/{action}""#)), "{action}");
    }
    // İzleyicide operatör ayarları, kendi sayfasında silme görünmez.
    assert!(!one.contains("/users/izleyici/permits") && !page(&app, "/users/admin").await.contains(r#"action="/users/admin/delete""#));
    assert!(page(&app, "/users?tab=ekle").await.contains(r#"<form class="card" method="post" action="/users">"#));
    assert_eq!(send(&app, get("/users/yok")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
pub(super) async fn manage_users() {
    let (app, state) = setup();
    let users = || state.auth().list();

    assert!(post_ok(&app, "/users", "name=ayse&password=ayse-nin-parolasi-1&role=viewer").await.starts_with("/users?notice="));
    assert_eq!(users().len(), 3);
    login_cookie(&app, "ayse", "ayse-nin-parolasi-1").await;
    for bad in ["name=ayse&password=ayse-nin-parolasi-1&role=viewer", "name=K%C3%B6t%C3%BC&password=ayse-nin-parolasi-1&role=viewer", "name=can&password=kisa&role=viewer", "name=can&password=ayse-nin-parolasi-1&role=kral"] {
        assert!(post_ok(&app, "/users", bad).await.starts_with("/users?error="), "{bad}");
    }
    assert_eq!(users().len(), 3);

    // Rol: yükselt, son yöneticiyi düşürme.
    assert!(post_ok(&app, "/users/ayse/role", "role=admin").await.contains("notice="));
    assert!(post_ok(&app, "/users/ayse/role", "role=viewer").await.contains("notice="));
    assert!(post_ok(&app, "/users/admin/role", "role=viewer").await.contains("error="), "son yönetici izleyici yapılamaz");

    // Parola sıfırlama (seçim kutulu form: yol "-" ve alan "name") açık oturumu düşürür.
    let cookie = login_cookie(&app, "ayse", "ayse-nin-parolasi-1").await;
    assert!(post_ok(&app, "/users/-/password", "name=ayse&password=yepyeni-parola-12345").await.contains("notice="));
    let (status, _, _) = send(&app, anon(Method::GET, "/").header(COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "eski oturum geçersiz");
    login_cookie(&app, "ayse", "yepyeni-parola-12345").await;

    // Silme: onay kutusu ister; kendi hesabını silemez.
    assert!(post_ok(&app, "/users/ayse/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/users/admin/delete", "confirm=1").await.contains("error="));
    assert!(post_ok(&app, "/users/ayse/delete", "confirm=1").await.contains("notice="));
    assert!(post_ok(&app, "/users/ayse/delete", "confirm=1").await.contains("error="));
    assert_eq!(users().len(), 2);
}

#[tokio::test]
pub(super) async fn tokens_are_shown_once_and_can_be_revoked() {
    let (app, state) = setup();
    let (status, headers, body) = send(&app, post_form("/users/-/tokens", "name=izleyici&label=ci")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store", "token sayfası önbelleğe yazılmaz");
    let token = body.split("redox_").nth(1).map(|rest| format!("redox_{}", &rest[..64])).expect("token sayfada görünmeli");
    assert!(!serde_json::to_string(&state.auth().list()).unwrap().contains(&token), "düz token saklanmaz");

    let with = |t: &str| anon(Method::GET, "/api/v1/cluster").header(AUTHORIZATION, format!("Bearer {t}")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, with(&token)).await.0, StatusCode::OK);
    assert!(!page(&app, "/users").await.contains(&token), "token bir daha gösterilmez");
    assert!(post_ok(&app, "/users/-/tokens", "name=izleyici&label=ci").await.contains("error="), "aynı etiket ikinci kez verilmez");

    assert!(post_ok(&app, "/users/izleyici/tokens/ci/revoke", "").await.contains("notice="));
    assert_eq!(send(&app, with(&token)).await.0, StatusCode::UNAUTHORIZED, "iptal edilen token çalışmaz");
}

#[tokio::test]
pub(super) async fn viewer_manages_only_their_own_account() {
    let (app, state) = setup();
    let cookie = login_cookie(&app, "izleyici", VIEWER_PW).await;
    let post = |uri: &str, body: &str| form_body(anon(Method::POST, uri).header(COOKIE, &cookie), body);

    let (status, _, body) = send(&app, post("/account/tokens", "label=dizustu")).await;
    assert_eq!(status, StatusCode::OK, "salt okunur hesap kendi token'ını üretebilir");
    assert!(body.contains("redox_"));
    assert_eq!(state.auth().list().iter().find(|u| u.name == "izleyici").unwrap().tokens.len(), 2);
    assert_eq!(send(&app, post("/account/tokens/dizustu/revoke", "")).await.0, StatusCode::SEE_OTHER);

    let (_, headers, _) = send(&app, post("/account/password", "current=yanlis-parola-123&password=yeni-izleyici-parolasi")).await;
    assert!(location(&headers).starts_with("/account?error="));
    let (_, headers, _) = send(&app, post("/account/password", &format!("current={VIEWER_PW}&password=kisa"))).await;
    assert!(location(&headers).starts_with("/account?error="));
    let (status, headers, _) = send(&app, post("/account/password", &format!("current={VIEWER_PW}&password=yeni-izleyici-parolasi"))).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login"));
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(send(&app, login_request("izleyici", VIEWER_PW, "")).await.0, StatusCode::UNAUTHORIZED, "eski parola geçmez");
    login_cookie(&app, "izleyici", "yeni-izleyici-parolasi").await;
    // Hesap sayfası başkasının bilgisini göstermez; diğer değişiklikler hâlâ yasak.
    let cookie = login_cookie(&app, "izleyici", "yeni-izleyici-parolasi").await;
    let (_, _, account) = send(&app, anon(Method::GET, "/account").header(COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert!(account.contains("İzleyici") && !account.contains("Kullanıcı ekle"));
    assert_eq!(send(&app, form_body(anon(Method::POST, "/guests/103/action").header(COOKIE, &cookie), "action=start")).await.0, StatusCode::FORBIDDEN);
}

/// Salt okunur token hesap yollarından yetki kazanamamalı: hesap yalnızca tarayıcı oturumuyla yönetilir.
#[tokio::test]
async fn api_tokens_cannot_manage_the_account() {
    let (app, state) = setup();
    let reading = state.auth().edit(|u| u.create_token_as("admin", "izleme", true)).unwrap();
    let before = state.auth().list();
    let with = |token: &str, method: Method, uri: &str| anon(method, uri).header(AUTHORIZATION, format!("Bearer {token}"));
    for token in [reading.as_str(), ADMIN_TOKEN, VIEWER_TOKEN] {
        for (uri, body) in [("/account/tokens", "label=yeni"), ("/account/tokens/test/revoke", ""), ("/account/totp/start", ""), ("/account/password", "current=x&password=cok-uzun-bir-parola")] {
            let (status, _, text) = send(&app, form_body(with(token, Method::POST, uri), body)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
            assert!(!text.contains("redox_"), "{uri}: yeni token verilmemeli");
        }
        assert_eq!(send(&app, with(token, Method::GET, "/account").body(Body::empty()).unwrap()).await.0, StatusCode::FORBIDDEN);
    }
    assert_eq!(state.auth().list(), before, "hiçbir şey değişmedi");
    // Tarayıcı oturumu hesabını yönetmeye devam eder.
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    assert_eq!(send(&app, form_body(anon(Method::POST, "/account/tokens").header(COOKIE, &cookie), "label=dizustu")).await.0, StatusCode::OK);
}

pub(super) fn totp_code(secret: &str) -> String {
    let key = redox_server::totp::base32_decode(secret).unwrap();
    format!("{:06}", redox_server::totp::code_at(&key, now() / redox_server::totp::STEP_SECS))
}

#[tokio::test]
pub(super) async fn two_factor_enrollment_and_login() {
    let (app, state) = setup();
    let me = || state.auth().list().into_iter().find(|u| u.name == "admin").unwrap();
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    let post = |uri: &str, body: &str| form_body(anon(Method::POST, uri).header(COOKIE, &cookie), body);
    let account = || anon(Method::GET, "/account").header(COOKIE, &cookie).body(Body::empty()).unwrap();

    assert!(send(&app, account()).await.2.contains("Kurulumu Başlat"));
    assert_eq!(send(&app, post("/account/totp/start", "")).await.0, StatusCode::SEE_OTHER);
    let secret = me().totp_pending.expect("kurulum başlatılınca bekleyen anahtar oluşur");
    assert!(me().totp_secret.is_none());
    let (_, _, body) = send(&app, account()).await;
    assert!(body.contains(&secret) && body.contains("<svg") && body.contains(r#"autocomplete="one-time-code""#), "QR ve anahtar gösterilmeli");
    // Henüz etkin değil: parola tek başına yeter.
    login_cookie(&app, "admin", ADMIN_PW).await;

    let (_, headers, _) = send(&app, post("/account/totp/confirm", "code=000000")).await;
    assert!(location(&headers).starts_with("/account?error=") || totp_code(&secret) == "000000");
    let (_, headers, _) = send(&app, post("/account/totp/confirm", &format!("code={}", totp_code(&secret)))).await;
    assert!(location(&headers).starts_with("/account?notice="), "{}", location(&headers));
    assert_eq!(me().totp_secret.as_deref(), Some(secret.as_str()));
    assert!(send(&app, account()).await.2.contains("Kapatmak için parolanız"));
    assert!(page(&app, "/users").await.contains("2 Adımı Sıfırla"));

    // Giriş artık kod ister; yanlış ya da eksik kodda ileti aynıdır.
    let login = |code: &str| form_body(anon(Method::POST, "/login"), &format!("username=admin&password={ADMIN_PW}&code={code}&next="));
    for bad in ["", "123456x", "12345"] {
        let (status, _, body) = send(&app, login(bad)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad:?}");
        assert!(body.contains("Kullanıcı adı ya da parola hatalı."));
    }
    let code = totp_code(&secret);
    assert_eq!(send(&app, login(&code)).await.0, StatusCode::SEE_OTHER);
    assert_eq!(send(&app, login(&code)).await.0, StatusCode::UNAUTHORIZED, "aynı kod ikinci kez kullanılamaz");
    // API token'ları ikinci adımdan etkilenmez (betikler için).
    assert_eq!(send(&app, get("/api/v1/cluster")).await.0, StatusCode::OK);

    let (_, headers, _) = send(&app, post("/account/totp/disable", "current=yanlis-parola-123")).await;
    assert!(location(&headers).starts_with("/account?error="));
    let (_, headers, _) = send(&app, post("/account/totp/disable", &format!("current={ADMIN_PW}"))).await;
    assert!(location(&headers).starts_with("/account?notice="));
    assert!(me().totp_secret.is_none());
}

#[tokio::test]
pub(super) async fn admin_can_reset_a_lost_second_factor() {
    let (app, state) = setup();
    state.auth().edit(|u| { let s = u.begin_totp("izleyici")?; u.confirm_totp("izleyici", &totp_code(&s), now()) }).unwrap();
    assert_eq!(send(&app, login_request("izleyici", VIEWER_PW, "")).await.0, StatusCode::UNAUTHORIZED);
    assert!(post_ok(&app, "/users/izleyici/totp-reset", "").await.contains("notice="));
    login_cookie(&app, "izleyici", VIEWER_PW).await;
}

#[tokio::test]
pub(super) async fn audit_log_records_changes_logins_and_denials() {
    let (app, state) = setup();
    send(&app, post_form("/guests/103/action", "action=start")).await;
    send(&app, get("/guests/103")).await;
    send(&app, login_request("admin", "yanlis-parola-123", "")).await;
    login_cookie(&app, "admin", ADMIN_PW).await;
    send(&app, form_body(as_viewer(Method::POST, "/guests/100/action"), "action=stop")).await;
    send(&app, form_body(anon(Method::POST, "/guests/100/action"), "action=stop")).await;

    let log = state.auth().audit.recent("", 50);
    let seen: Vec<(&str, &str, bool)> = log.iter().rev().map(|e| (e.user.as_str(), e.action.as_str(), e.ok)).collect();
    assert_eq!(
        seen,
        [
            ("admin", "POST /guests/103/action", true),
            ("admin", "giriş (hatalı parola ya da kod)", false),
            ("admin", "giriş", true),
            ("izleyici", "POST /guests/100/action", false),
        ],
        "okuma istekleri ve kimliksiz istekler kaydedilmez; parola hiçbir kayıtta yer almaz"
    );
    assert!(!format!("{log:?}").contains("yanlis-parola"));

    let page_all = page(&app, "/audit").await;
    assert!(page_all.contains("POST /guests/103/action") && page_all.contains("Reddedildi") && page_all.contains("giriş (hatalı parola ya da kod)"));
    let filtered = page(&app, "/audit?q=izleyici").await;
    assert!(filtered.contains("POST /guests/100/action") && !filtered.contains("POST /guests/103/action"));
    assert!(page(&app, "/audit?q=zzz").await.contains("ile eşleşen kayıt yok"));
}

#[tokio::test]
pub(super) async fn login_page_offers_the_code_field() {
    let (app, _) = setup();
    let (_, _, body) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert!(body.contains(r#"name="code""#) && body.contains(r#"autocomplete="one-time-code""#) && body.contains("Yalnızca iki adımlı doğrulamayı açtıysanız."));
}

// ---------- Köprü ağı ve konteynerler (gerçek sürücü kipi) ----------
