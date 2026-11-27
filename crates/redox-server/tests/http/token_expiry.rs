//! Token'ların son kullanma tarihi.

use super::*;

#[tokio::test]
async fn an_expired_token_is_refused_like_a_wrong_one() {
    let (app, state) = setup();
    let start = now();
    let token = state.auth().edit(|u| u.create_token_for("admin", "kisa", false, 30, start)).unwrap();
    let end = start + 30 * 86_400;
    assert!(state.auth().bearer_at(&token, end - 1).is_some());
    assert!(state.auth().bearer_at(&token, end).is_none(), "son an dahil değil");
    assert!(state.auth().bearer_at(&token, end + 1_000_000).is_none());
    // Süresiz token etkilenmez.
    assert!(state.auth().bearer_at(ADMIN_TOKEN, end + 1_000_000_000).is_some());
    // Süresi dolmuş token istekte 401 alır.
    state.auth().edit(|u| u.create_token_for("admin", "eski", false, 1, now() - 2 * 86_400).map(|t| assert!(!t.is_empty()))).unwrap();
    let old = state.auth().edit(|u| u.create_token_for("admin", "eski2", false, 1, now() - 2 * 86_400)).unwrap();
    let with = |t: &str| anon(Method::GET, "/api/v1/cluster").header(AUTHORIZATION, format!("Bearer {t}")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, with(&old)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(send(&app, with(&token)).await.0, StatusCode::OK);
    assert!(page(&app, "/users").await.contains("süresi doldu"));
}

#[tokio::test]
async fn the_forms_offer_a_lifetime_and_refuse_nonsense() {
    let (app, state) = setup();
    let users = page(&app, "/users/izleyici").await;
    assert!(users.contains(r#"<select id="tk-days" name="days">"#) && users.contains(r#"<option value="90">90 gün</option>"#));
    let (status, _, body) = send(&app, post_form("/users/-/tokens", "name=izleyici&label=ci&days=90")).await;
    assert!(status == StatusCode::OK && body.contains("redox_"));
    let made = |label: &str| state.auth().list().iter().find(|u| u.name == "izleyici").unwrap().tokens.iter().find(|t| t.name == label).cloned();
    let end = made("ci").unwrap().expires.unwrap();
    assert!(end.abs_diff(now() + 90 * 86_400) < 60);
    assert!(page(&app, "/users").await.contains("son gün"));
    for bad in ["days=abc", "days=-1", "days=999999", "days=18446744073709551615"] {
        assert!(post_ok(&app, "/users/-/tokens", &format!("name=izleyici&label=kotu&{bad}")).await.contains("error="), "{bad}");
        assert!(made("kotu").is_none(), "{bad}: yarım token kalmamalı");
    }
    // Süre verilmezse eskisi gibi süresiz.
    send(&app, post_form("/users/-/tokens", "name=izleyici&label=hep&days=0")).await;
    assert_eq!(made("hep").unwrap().expires, None);
    // Süresiz token kullanıcı dosyasında alan eklemez.
    assert!(!serde_json::to_string(&made("hep").unwrap()).unwrap().contains("expires"));
}
