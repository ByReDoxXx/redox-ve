//! Dizin (LDAP) kullanıcısının panel girişi: parola dizinden, rol yerel kayıttan.

use super::*;

use redox_server::auth::Role;

const DIRECTORY_PASSWORD: &str = "dizin-parolasi-yalnizca-sinama";

fn app_with_directory(port: u16) -> (Router, AppState) {
    let mut file = users();
    // İkinci bir yerel yönetici kalmadan ilk yönetici dizine bağlanamaz; var olan izleyici bağlanır.
    let viewer = file.users.iter().find(|u| u.role == Role::Viewer).expect("izleyici").name.clone();
    file.set_ldap(&viewer, true).unwrap();
    let mut auth = Auth::new(file, None, false, Cost::Test);
    auth.ldap = Some(redox_server::ldap::Config::parse(&format!("url = ldap://127.0.0.1:{port}\nbind = uid={{user}},dc=ornek,dc=com")).unwrap());
    let state = AppState::new(Cluster::demo(now()), Box::new(SimDriver::default()), Duration::ZERO, None, auth);
    (app(state.clone()), state)
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_users_sign_in_with_the_directory_password_only() {
    let viewer = users().users.iter().find(|u| u.role == Role::Viewer).unwrap().name.clone();
    let dn: &'static str = Box::leak(format!("uid={viewer},dc=ornek,dc=com").into_boxed_str());
    let directory = redox_server::ldap::test_server::start(dn, DIRECTORY_PASSWORD);
    let (app, state) = app_with_directory(directory.port());

    let (status, headers, _) = send(&app, login_request(&viewer, DIRECTORY_PASSWORD, "/")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(headers.get("set-cookie").is_some());
    let (status, ..) = send(&app, login_request(&viewer, "yanlis-parola-uzun", "/")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Eski yerel parola artık geçmez.
    assert!(!state.auth().check_password(&viewer, VIEWER_PW));
    assert!(state.auth().check_password(&viewer, DIRECTORY_PASSWORD));
    // Yerel kullanıcılar dizinden etkilenmez.
    let (status, ..) = send(&app, login_request("admin", ADMIN_PW, "/")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // Dizine ulaşılamazsa dizin kullanıcısı giremez, yerel yönetici girer.
    let (app, _) = app_with_directory(1);
    assert_eq!(send(&app, login_request(&viewer, DIRECTORY_PASSWORD, "/")).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(send(&app, login_request("admin", ADMIN_PW, "/")).await.0, StatusCode::SEE_OTHER);
}

#[test]
fn the_last_local_administrator_cannot_be_moved_to_the_directory() {
    let mut file = users();
    file.add_user("yedek", "yedek-yonetici-parolasi", Role::Admin, Cost::Test).unwrap();
    let admins: Vec<String> = file.users.iter().filter(|u| u.role == Role::Admin).map(|u| u.name.clone()).collect();
    for name in &admins[..admins.len() - 1] {
        file.set_ldap(name, true).unwrap();
    }
    let last = admins.last().unwrap();
    assert!(file.set_ldap(last, true).unwrap_err().contains("yerel parolalı"));
    assert!(file.set_ldap(last, false).is_ok());
}
