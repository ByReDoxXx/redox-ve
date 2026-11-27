//! Güvenlik sıkılaştırmaları: açık oturumda parola sorulan işler, giriş girdisinin sınırları.

use super::*;

fn with_cookie(cookie: &str, uri: &str, body: &str) -> Request<Body> {
    form_body(anon(Method::POST, uri).header(COOKIE, cookie), body)
}

/// Ele geçirilmiş bir oturum, parolayı bilmeden kalıcı bir token ya da kendi ikinci adımını ekleyememeli.
#[tokio::test]
async fn an_open_session_cannot_add_a_token_or_a_second_step_without_the_password() {
    let (app, state) = setup();
    let me = || state.auth().list().into_iter().find(|u| u.name == "admin").unwrap();
    let tokens = me().tokens.len();
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;

    for body in ["label=sizinti", "label=sizinti&current=yanlis-parola-123"] {
        let (status, headers, text) = send(&app, with_cookie(&cookie, "/account/tokens", body)).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
        assert!(location(&headers).starts_with("/account?error=") && !text.contains("redox_"), "{body}");
    }
    assert_eq!(me().tokens.len(), tokens, "token oluşmadı");

    let (_, headers, _) = send(&app, with_cookie(&cookie, "/account/totp/start", "")).await;
    assert!(location(&headers).starts_with("/account?error="));
    assert!(me().totp_pending.is_none(), "parolasız kurulum başlamaz");

    // Doğru parolayla ikisi de çalışır.
    assert_eq!(send(&app, with_cookie(&cookie, "/account/tokens", &format!("label=dizustu&current={ADMIN_PW}"))).await.0, StatusCode::OK);
    send(&app, with_cookie(&cookie, "/account/totp/start", &format!("current={ADMIN_PW}"))).await;
    assert!(me().totp_pending.is_some());
    let page = send(&app, anon(Method::GET, "/account").header(COOKIE, &cookie).body(Body::empty()).unwrap()).await.2;
    assert!(page.contains(r#"id="token-current""#), "token formu parola ister");
}

/// Oturumu olan biri hesabın parolasını hesap formlarından sınırsız deneyememeli.
#[tokio::test]
async fn password_guesses_behind_a_session_are_limited() {
    let (app, state) = setup();
    let cookie = login_cookie(&app, "izleyici", VIEWER_PW).await;
    for n in 0..5 {
        let (_, headers, _) = send(&app, with_cookie(&cookie, "/account/totp/disable", &format!("current=yanlis-tahmin-{n:04}"))).await;
        assert!(location(&headers).starts_with("/account?error="));
    }
    // Sınır dolduktan sonra doğru parola da kabul edilmez: tahmin eden, tutturduğunu öğrenemez.
    let before = state.auth().list();
    let (status, _, text) = send(&app, with_cookie(&cookie, "/account/tokens", &format!("label=yeni&current={VIEWER_PW}"))).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(!text.contains("redox_"));
    let (_, headers, _) = send(&app, with_cookie(&cookie, "/account/password", &format!("current={VIEWER_PW}&password=yepyeni-bir-parola-1"))).await;
    assert!(location(&headers).starts_with("/account?error="));
    assert_eq!(state.auth().list(), before, "hiçbir şey değişmedi");
    // Başka kullanıcının sayacı etkilenmez.
    let admin = login_cookie(&app, "admin", ADMIN_PW).await;
    assert_eq!(send(&app, with_cookie(&admin, "/account/tokens", &format!("label=yeni&current={ADMIN_PW}"))).await.0, StatusCode::OK);
}

/// Girişte yazılan ad günlüğe ve belleğe olduğu gibi yazılmaz; dev gövde hiç okunmaz.
#[tokio::test]
async fn login_input_is_bounded() {
    let (app, state) = setup();
    let long = "a".repeat(5_000);
    assert_eq!(send(&app, login_request(&long, "bir-parola-denemesi", "")).await.0, StatusCode::UNAUTHORIZED);
    let entries = state.auth().audit.recent("", 10);
    assert!(!entries.is_empty() && entries.iter().all(|e| e.user.chars().count() <= 64), "ad kısaltılmalı");

    let huge = "a".repeat(100_000);
    assert_eq!(send(&app, login_request(&huge, "bir-parola-denemesi", "")).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    // Olağan giriş etkilenmez.
    login_cookie(&app, "admin", ADMIN_PW).await;
}
