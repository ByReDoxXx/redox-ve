//! Açık oturumların listesi ve kapatılması.

use super::*;

async fn sign_in(app: &Router, user: &str, password: &str, agent: &str) -> String {
    let request = form_body(anon(Method::POST, "/login").header("user-agent", agent), &format!("username={user}&password={password}&next="));
    let (status, headers, _) = send(app, request).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    headers.get(SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string()
}

async fn account(app: &Router, cookie: &str) -> (StatusCode, String) {
    let (status, _, body) = send(app, anon(Method::GET, "/account").header(COOKIE, cookie).body(Body::empty()).unwrap()).await;
    (status, body)
}

async fn press(app: &Router, cookie: &str, uri: &str) -> String {
    let (status, headers, body) = send(app, form_body(anon(Method::POST, uri).header(COOKIE, cookie), "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{uri}: {body}");
    location(&headers).to_string()
}

/// Sayfadaki kapatma formlarının oturum adları.
fn closable(page: &str) -> Vec<String> {
    page.split(r#"action="/account/sessions/"#).skip(1).filter_map(|rest| rest.split_once("/close\"").map(|(id, _)| id.to_string())).filter(|id| id.len() == 16).collect()
}

#[tokio::test]
async fn a_user_sees_and_closes_their_other_sessions() {
    let (app, state) = setup();
    let here = sign_in(&app, "admin", ADMIN_PW, "Bu-Tarayici/1.0").await;
    let lost = sign_in(&app, "admin", ADMIN_PW, "Kayip-Telefon/2.0 <script>x</script>").await;
    let third = sign_in(&app, "admin", ADMIN_PW, "Ucuncu/3.0").await;
    let (_, body) = account(&app, &here).await;
    assert!(body.contains("Bu-Tarayici/1.0") && body.contains("Kayip-Telefon/2.0") && body.contains("Bu oturum"), "{body}");
    assert!(!body.contains("<script>x"), "tarayıcı adı kaçışlanır");
    assert!(!body.contains(here.split('=').nth(1).unwrap()), "oturum anahtarı sayfaya yazılmaz");
    assert_eq!(closable(&body).len(), 2, "kendi oturumu için kapatma düğmesi yok");

    let lost_id = state.auth().sessions_of("admin", None, now()).into_iter().find(|s| s.agent.starts_with("Kayip")).unwrap().id;
    // Başka kullanıcı bu adla kapatamaz.
    let viewer = sign_in(&app, "izleyici", VIEWER_PW, "x").await;
    assert!(press(&app, &viewer, &format!("/account/sessions/{lost_id}/close")).await.contains("error="));
    assert_eq!(account(&app, &lost).await.0, StatusCode::OK);
    assert!(press(&app, &here, &format!("/account/sessions/{lost_id}/close")).await.contains("notice="));
    assert_eq!(account(&app, &lost).await.0, StatusCode::SEE_OTHER, "kapatılan oturum düşer");
    assert_eq!(account(&app, &third).await.0, StatusCode::OK);
    assert!(press(&app, &here, "/account/sessions/close-others").await.contains("notice="));
    assert_eq!(account(&app, &third).await.0, StatusCode::SEE_OTHER);
    assert_eq!(account(&app, &here).await.0, StatusCode::OK, "bu oturum kalır");
    assert_eq!(account(&app, &viewer).await.0, StatusCode::OK, "başkasınınki etkilenmez");
}

#[tokio::test]
async fn an_administrator_can_close_all_sessions_of_a_user_even_in_lockdown() {
    let (app, state) = setup();
    let viewer = sign_in(&app, "izleyici", VIEWER_PW, "x").await;
    assert!(page(&app, "/users").await.contains(r#"action="/users/izleyici/sessions/close""#));
    // İzleyici başkasının oturumlarını kapatamaz.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/users/admin/sessions/close"), "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    state.lockdown_set(true, "admin");
    assert!(post_ok(&app, "/users/izleyici/sessions/close", "").await.contains("notice="));
    assert_eq!(account(&app, &viewer).await.0, StatusCode::SEE_OTHER);
    // API token'ı oturum değildir; çalışmaya devam eder.
    assert_eq!(send(&app, as_viewer(Method::GET, "/api/v1/cluster").body(Body::empty()).unwrap()).await.0, StatusCode::OK);
}
