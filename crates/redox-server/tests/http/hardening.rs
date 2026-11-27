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

fn role_of(state: &AppState, name: &str) -> Option<Role> {
    state.auth().list().into_iter().find(|u| u.name == name).map(|u| u.role)
}

/// Dört göz ilkesi tek yöneticiyle aşılamamalı: yeni yönetici açıp ya da başka bir yöneticinin kimliğine bürünüp
/// kendi isteğini onaylayamaz.
#[tokio::test]
async fn four_eyes_cannot_be_satisfied_by_one_administrator() {
    let (app, state) = setup();
    state.auth().edit(|u| u.add_user("ikinci", "ikinci-yonetici-parolasi", Role::Admin, Cost::Test)).unwrap();
    let hash = |name: &str| state.auth().list().into_iter().find(|u| u.name == name).unwrap().password_hash;
    state.set_four_eyes(true);

    // Yeni "yönetici" izleyici olarak eklenir; yönetici olması onay bekler.
    assert!(post_ok(&app, "/users", "name=kukla&role=admin&password=kukla-yonetici-parolasi").await.contains("notice="));
    assert_eq!(role_of(&state, "kukla"), Some(Role::Viewer));
    let pending = state.snapshot().approvals.clone();
    assert_eq!((pending.len(), pending[0].action.as_str(), pending[0].target.as_str()), (1, "user-promote", "kukla"));
    // Var olan hesabı yönetici yapmak da öyle; isteyen kendi isteğini onaylayamaz.
    assert!(post_ok(&app, "/users/izleyici/role", "role=admin").await.contains("notice="));
    assert_eq!(role_of(&state, "izleyici"), Some(Role::Viewer));
    let (kukla, izleyici) = (pending[0].id, state.snapshot().approvals[1].id);
    assert!(state.approve(kukla, "admin").is_err());
    assert_eq!(role_of(&state, "kukla"), Some(Role::Viewer));

    // Başka bir yöneticinin kimliğine panelden bürünülemez.
    let before = hash("ikinci");
    assert!(post_ok(&app, "/users/ikinci/password", "password=ele-gecirilmis-parola-1").await.contains("error="));
    assert_eq!(hash("ikinci"), before, "parola değişmedi");
    assert!(post_ok(&app, "/users/ikinci/totp-reset", "").await.contains("error="));
    let (status, headers, body) = send(&app, post_form("/users/-/tokens", "name=ikinci&label=sizinti")).await;
    assert!(status == StatusCode::SEE_OTHER && location(&headers).contains("error=") && !body.contains("redox_"));
    assert!(state.auth().list().into_iter().find(|u| u.name == "ikinci").unwrap().tokens.is_empty());
    // Yönetici olmayan hesaplar ve kendi hesabı için bu işler sürer.
    assert!(post_ok(&app, "/users/izleyici/password", "password=yeni-izleyici-parolasi-2").await.contains("notice="));
    assert_eq!(send(&app, post_form("/users/-/tokens", "name=admin&label=kendi")).await.0, StatusCode::OK);

    // İkinci yönetici onaylayınca rol değişir; rol düşürmek onay istemez.
    assert_eq!(state.approve(izleyici, "ikinci").unwrap(), "onaylandı: izleyici kullanıcısını yönetici yapmak");
    assert_eq!(role_of(&state, "izleyici"), Some(Role::Admin));
    assert!(post_ok(&app, "/users/izleyici/role", "role=viewer").await.contains("notice="));
    assert_eq!(role_of(&state, "izleyici"), Some(Role::Viewer));

    // İlke kapalıyken eskisi gibi doğrudan yapılır.
    state.set_four_eyes(false);
    assert!(post_ok(&app, "/users", "name=dogrudan&role=admin&password=dogrudan-yonetici-parola").await.contains("notice="));
    assert_eq!(role_of(&state, "dogrudan"), Some(Role::Admin));
    assert!(post_ok(&app, "/users/ikinci/password", "password=sifirlanmis-parola-12").await.contains("notice="));
}

/// Operatör, rolüyle aynı formda seçilen yetki ve kapsamla başlar: bütün konuklarda her şeye yetkili bir ara durum olmaz.
#[tokio::test]
async fn a_new_operator_starts_with_the_chosen_permits_and_scope() {
    use redox_server::auth::Permit;
    let (app, state) = setup();
    let user = |name: &str| state.auth().list().into_iter().find(|u| u.name == name);

    // Yetki seçilmeden operatör eklenmez (hesap da oluşmaz).
    assert!(post_ok(&app, "/users", "name=nobet&role=operator&password=nobetci-parolasi-1").await.contains("error="));
    assert!(user("nobet").is_none());
    assert!(post_ok(&app, "/users", "name=nobet&role=operator&password=nobetci-parolasi-1&power=1&scope=web").await.contains("notice="));
    let nobet = user("nobet").unwrap();
    assert_eq!((nobet.role, nobet.permits, nobet.scope), (Role::Operator, Some(vec![Permit::Power]), vec!["web".to_string()]));

    // Rol değiştirerek operatör yapmak da aynı seçimi ister.
    assert!(post_ok(&app, "/users/izleyici/role", "role=operator").await.contains("error="));
    assert_eq!(role_of(&state, "izleyici"), Some(Role::Viewer), "seçim yapılmadan rol değişmez");
    assert!(post_ok(&app, "/users/izleyici/role", "role=operator&snapshot=1&backup=1").await.contains("notice="));
    let izleyici = user("izleyici").unwrap();
    assert_eq!((izleyici.role, izleyici.permits.map(|p| p.len()), izleyici.scope.len()), (Role::Operator, Some(2), 0));
    // Zaten operatör olan hesabın yetkileri rol formundan değişmez.
    assert!(post_ok(&app, "/users/nobet/role", "role=operator").await.contains("notice="));
    assert_eq!(user("nobet").unwrap().permits, Some(vec![Permit::Power]));

    let form = page(&app, "/users?tab=ekle").await;
    assert!(form.contains(r#"name="power""#) && form.contains(r#"name="scope""#) && !form.contains(r#"name="power" value="1" checked"#), "kutular boş gelir");
}
