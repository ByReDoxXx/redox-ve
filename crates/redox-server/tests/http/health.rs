//! Donanım sağlığı kartı, uyarıları ve panelden güncelleme isteği.

use super::*;

use redox_core::health::{DiskHealth, HostHealth, PackageUpdate, UpdateRun};
use redox_server::state::HealthFiles;

#[tokio::test]
async fn disk_problems_are_shown_and_updates_are_only_requested_when_pending() {
    let (app, state) = setup();
    let dir = std::env::temp_dir().join(format!("redox-http-health-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let node = state.snapshot().nodes[0].id.clone();
    let url = format!("/nodes/{node}");
    assert!(!page(&app, &url).await.contains("Donanım sağlığı"), "yardımcı yoksa kart yok");
    assert_eq!(api(&app, post_json("/api/v1/host/update", &json!({}))).await.0, StatusCode::UNPROCESSABLE_ENTITY, "bu kurulumda kapalı");

    let files = HealthFiles { results: dir.clone(), request: dir.join("update.request") };
    state.set_health_files(files.clone());
    let health = HostHealth {
        collected: 1_800_000_000,
        disks: vec![DiskHealth { device: "/dev/sda".into(), model: "Deneme <disk>".into(), passed: Some(false), ..Default::default() }],
        notes: vec!["ipmitool çalıştırılamadı: yok".into()],
        ..Default::default()
    };
    redox_server::health::write_json(&dir.join("health.json"), &health).unwrap();
    state.health_tick();
    let shown = page(&app, &url).await;
    assert!(shown.contains("Donanım sağlığı") && shown.contains("SMART sınamasını geçemiyor") && shown.contains("Deneme &lt;disk&gt;"), "{shown}");
    assert!(!shown.contains("/host/update"), "güncelleme yokken düğme yok");
    assert!(redox_server::alerts::evaluate(&state.snapshot()).iter().any(|a| a.key == "disk:/dev/sda:smart"), "uyarı üretilir");
    assert!(post_ok(&app, "/host/update", "").await.contains("error="), "bekleyen güncelleme yokken istek reddedilir");

    let with_updates = HostHealth { updates: vec![PackageUpdate { name: "openssl".into(), from: "1".into(), to: "2".into() }], ..health };
    redox_server::health::write_json(&dir.join("health.json"), &with_updates).unwrap();
    state.health_tick();
    assert!(page(&app, &url).await.contains(r#"action="/host/update""#));
    let denied = send(&app, as_viewer(Method::POST, "/api/v1/host/update").body(Body::empty()).unwrap()).await.0;
    assert_eq!(denied, StatusCode::FORBIDDEN);
    assert!(!files.request.exists());
    assert!(post_ok(&app, "/host/update", "").await.contains("notice="));
    assert!(files.request.exists());
    assert!(page(&app, &url).await.contains("Güncelleme istendi; yardımcı"));
    let (status, v) = api(&app, get("/api/v1/host/health")).await;
    assert_eq!((status, v["update_requested"].as_bool(), v["health"]["updates"][0]["name"].as_str()), (StatusCode::OK, Some(true), Some("openssl")));

    // Yardımcı sonucu yazınca sayfada görünür.
    std::fs::remove_file(&files.request).unwrap();
    redox_server::health::write_json(&dir.join("update.json"), &UpdateRun { finished: 1_800_000_100, ok: false, output: "E: kilit alınamadı".into() }).unwrap();
    let shown = page(&app, &url).await;
    assert!(shown.contains("E: kilit alınamadı") && shown.contains("başarısız"));
    let _ = std::fs::remove_dir_all(dir);
}
