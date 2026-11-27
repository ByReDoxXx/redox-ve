//! Dağıtık anahtar: API, Küme sayfasındaki form, ağ yardımcısının dosyası ve iki sunucu arasında eşitleme.

use super::*;

fn definition(name: &str, vni: u32) -> Value {
    json!({ "name": name, "vni": vni, "members": ["10.9.0.2", "10.9.0.1"] })
}

#[tokio::test]
pub(super) async fn distributed_switches_are_managed_from_the_api_and_the_cluster_page() {
    let (app, state) = setup();
    let dir = std::env::temp_dir().join(format!("redox-vds-http-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("vswitch.json");
    state.set_vswitch_file(file.clone()).unwrap();
    assert!(redox_server::vswitch::read_spec(&file).unwrap().is_empty(), "başlangıçta boş tanım yazılır");

    let (status, v) = api(&app, post_json("/api/v1/vswitches", &definition("vds-web", 100))).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{v}");
    assert_eq!(redox_server::vswitch::read_spec(&file).unwrap()[0].members[0].to_string(), "10.9.0.1");
    let (_, v) = api(&app, get("/api/v1/vswitches")).await;
    assert!(v[0]["name"] == "vds-web" && v[0]["mtu"] == 1450 && v[0]["local"].is_null() && v[0]["peers"] == json!([]), "{v}");

    for (body, needle) in [(definition("vmbr0", 5), "ile başlamalı"), (definition("vds-b", 100), "zaten vds-web")] {
        let (status, v) = api(&app, post_json("/api/v1/vswitches", &body)).await;
        assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains(needle), "{v}");
    }
    let viewer = as_viewer(Method::POST, "/api/v1/vswitches").header(CONTENT_TYPE, "application/json").body(Body::from(definition("vds-c", 7).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    // Eş tanımlı değilken gönderme açık bir hata verir.
    let (status, v) = api(&app, post_json("/api/v1/vswitches/vds-web/push", &json!({}))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("eş sunucu yok"), "{v}");

    let shown = page(&app, "/cluster").await;
    assert!(shown.contains("Dağıtık anahtarlar") && shown.contains("vds-web") && shown.contains("10.9.0.2") && !shown.contains("Eşlerden de sil"), "{shown}");
    assert!(post_ok(&app, "/cluster/switch", "op=save&name=vds-db&vni=200&mtu=1400&members=10.9.0.1%2C+10.9.0.3").await.contains("notice="));
    assert_eq!(state.snapshot().vswitches.len(), 2);
    for bad in ["op=save&name=vds-x&vni=abc&mtu=1450&members=10.9.0.1,10.9.0.2", "op=save&name=vds-x&vni=3&mtu=1450&members=10.9.0.1,sunucu", "op=sil&name=vds-db"] {
        assert!(post_ok(&app, "/cluster/switch", bad).await.contains("error="), "{bad}");
    }
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/cluster/switch"), "op=remove&name=vds-db")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Konuğun kullandığı anahtar silinmez.
    state.mutate(|c| c.guests[0].bridge = "vds-db".to_string());
    assert!(post_ok(&app, "/cluster/switch", "op=remove&name=vds-db").await.contains("error="));
    state.mutate(|c| c.guests[0].bridge = "vmbr0".to_string());
    assert!(post_ok(&app, "/cluster/switch", "op=remove&name=vds-db").await.contains("notice="));
    let (status, _) = api(&app, request(Method::DELETE, "/api/v1/vswitches/vds-web").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(redox_server::vswitch::read_spec(&file).unwrap().is_empty() && state.snapshot().vswitches.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

/// Gerçek TCP kapısında dinleyen sunucu: `(durum, yönlendirici, kapı)`.
async fn listening() -> (AppState, Router, u16) {
    let (app, state) = setup();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    (state, app, port)
}

async fn poll(state: &AppState) {
    let state = state.clone();
    tokio::task::spawn_blocking(move || state.peers().poll(now())).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(super) async fn a_switch_is_sent_to_the_peers_and_removed_from_them() {
    let (hub, app, _) = listening().await;
    let (remote, _, port) = listening().await;
    // İkinci eş kapalı bir kapıdır: sonuçlar eş eş bildirilir.
    let peers = format!("uzak http://127.0.0.1:{port} {ADMIN_TOKEN}\nkapali http://127.0.0.1:1 {ADMIN_TOKEN}\n");
    hub.peers().configure(redox_server::peers::parse_file(&peers).unwrap());
    assert_eq!(api(&app, post_json("/api/v1/vswitches", &definition("vds-web", 100))).await.0, StatusCode::NO_CONTENT);
    let states = |v: &Value| v[0]["peers"].clone();
    poll(&hub).await;
    assert_eq!(states(&api(&app, get("/api/v1/vswitches")).await.1), json!([["uzak", "missing"], ["kapali", "unknown"]]));

    let (status, v) = api(&app, post_json("/api/v1/vswitches/vds-web/push", &json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["peers"][0]["ok"] == true && v["peers"][1]["ok"] == false && v["peers"][1]["error"].as_str().unwrap().contains("erişilemiyor"), "{v}");
    assert_eq!(remote.snapshot().vswitches, hub.snapshot().vswitches, "eşteki tanım birebir aynı");
    poll(&hub).await;
    assert_eq!(states(&api(&app, get("/api/v1/vswitches")).await.1)[0], json!(["uzak", "same"]));
    assert!(page(&app, "/cluster").await.contains("uzak: aynı"));
    // Eşte elle değiştirilen tanım "farklı" görünür.
    remote.mutate(|c| c.vswitches[0].mtu = 1300);
    poll(&hub).await;
    assert_eq!(states(&api(&app, get("/api/v1/vswitches")).await.1)[0], json!(["uzak", "different"]));

    // Bir eşten silinemiyorsa burada da kalır.
    let everywhere = || request(Method::DELETE, "/api/v1/vswitches/vds-web?everywhere=true").body(Body::empty()).unwrap();
    let (status, v) = api(&app, everywhere()).await;
    assert!(status == StatusCode::OK && v["peers"][0]["ok"] == true && v["peers"][1]["ok"] == false, "{v}");
    assert!(remote.snapshot().vswitches.is_empty() && hub.snapshot().vswitches.len() == 1);
    // Ulaşılamayan eş listeden çıkınca (eşte artık yok sayılır) silme tamamlanır.
    hub.peers().configure(redox_server::peers::parse_file(&format!("uzak http://127.0.0.1:{port} {ADMIN_TOKEN}\n")).unwrap());
    let (_, v) = api(&app, everywhere()).await;
    assert!(v["peers"][0]["ok"] == true && hub.snapshot().vswitches.is_empty(), "{v}");
}
