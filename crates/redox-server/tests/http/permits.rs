//! Operatörün yetki türleri.

use super::*;

use redox_server::auth::Permit;

/// Konuk 100 çalışıyor; her türden birer istek.
const REQUESTS: &[(Permit, &str, &str)] = &[
    (Permit::Power, "/guests/100/action", "action=pause"),
    (Permit::Power, "/guests/100/suspend", ""),
    (Permit::Power, "/api/v1/guests/100/pause", ""),
    (Permit::Power, "/api/v1/guests/100/suspend", ""),
    (Permit::Console, "/guests/100/keys", "text=a"),
    (Permit::Console, "/guests/100/exec", "command=ls"),
    (Permit::Console, "/api/v1/guests/100/exec", "{}"),
    (Permit::Snapshot, "/guests/100/snapshots", "name=a"),
    (Permit::Snapshot, "/api/v1/guests/100/snapshots", "{}"),
    (Permit::Backup, "/guests/100/backups", ""),
    (Permit::Backup, "/api/v1/guests/100/backups", ""),
];

const CONSOLE_PAGES: [&str; 5] = ["/guests/100/console", "/guests/100/screen", "/guests/100/screen.png", "/guests/100/terminal", "/guests/100/vnc"];

async fn world() -> (Router, AppState) {
    let (app, state) = setup_real();
    state
        .auth()
        .edit(|u| {
            u.add_user("nobet", "nobetci-parolasi-1", Role::Operator, Cost::Test)?;
            u.add_token("nobet", "t", "redox_nobet")
        })
        .unwrap();
    (app, state)
}

async fn status(app: &Router, method: Method, uri: &str, body: &str) -> StatusCode {
    send(app, form_body(anon(method, uri).header(AUTHORIZATION, "Bearer redox_nobet"), body)).await.0
}

#[tokio::test]
async fn each_permit_opens_only_its_own_requests() {
    for only in Permit::ALL {
        let (app, state) = world().await;
        state.auth().edit(|u| u.set_permits("nobet", vec![only])).unwrap();
        for (needs, uri, body) in REQUESTS {
            let got = status(&app, Method::POST, uri, body).await;
            assert_eq!(got != StatusCode::FORBIDDEN, *needs == only, "yalnızca {only:?}: {uri} → {got}");
        }
        for uri in CONSOLE_PAGES {
            let got = status(&app, Method::GET, uri, "").await;
            assert_eq!(got != StatusCode::FORBIDDEN, only == Permit::Console, "yalnızca {only:?}: {uri} → {got}");
        }
    }
}

#[tokio::test]
async fn without_a_restriction_an_operator_keeps_all_four_and_scope_still_applies() {
    let (app, state) = world().await;
    for (_, uri, body) in REQUESTS {
        assert_ne!(status(&app, Method::POST, uri, body).await, StatusCode::FORBIDDEN, "{uri}");
    }
    state.auth().edit(|u| u.set_scope("nobet", vec!["olmayan".into()])).unwrap();
    for (_, uri, body) in REQUESTS {
        assert_eq!(status(&app, Method::POST, uri, body).await, StatusCode::FORBIDDEN, "kapsam dışı: {uri}");
    }
}

#[tokio::test]
async fn the_users_page_saves_permits_and_refuses_an_empty_set() {
    let (app, state) = world().await;
    let permits = || state.auth().list().into_iter().find(|u| u.name == "nobet").unwrap().permits;
    let page_before = page(&app, "/users/nobet").await;
    assert!(page_before.contains(r#"action="/users/nobet/permits""#));
    assert!(post_ok(&app, "/users/nobet/permits", "backup=1").await.contains("notice="));
    assert_eq!(permits(), Some(vec![Permit::Backup]));
    assert!(page(&app, "/users").await.contains("Yalnızca: yedek"));
    assert!(post_ok(&app, "/users/nobet/permits", "").await.contains("error="));
    assert_eq!(permits(), Some(vec![Permit::Backup]));
    assert!(post_ok(&app, "/users/nobet/permits", "power=1&console=1&snapshot=1&backup=1").await.contains("notice="));
    assert_eq!(permits(), None, "dördü birden kısıtsızdır");
    assert!(!serde_json::to_string(&state.auth().list()).unwrap().contains("permits"));
    // Operatör kendi yetkisini genişletemez.
    state.auth().edit(|u| u.set_permits("nobet", vec![Permit::Backup])).unwrap();
    assert_eq!(status(&app, Method::POST, "/users/nobet/permits", "power=1").await, StatusCode::FORBIDDEN);
    // Yönetici ve izleyici kısıttan etkilenmez.
    state.auth().edit(|u| u.set_permits("izleyici", vec![Permit::Power])).unwrap();
    let (got, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/100/action"), "action=pause")).await;
    assert_eq!(got, StatusCode::FORBIDDEN, "izleyiciye yetki türü bir şey kazandırmaz");
    state.auth().edit(|u| u.set_permits("admin", vec![Permit::Backup])).unwrap();
    assert_ne!(send(&app, post_form("/guests/100/action", "action=pause")).await.0, StatusCode::FORBIDDEN);
}
