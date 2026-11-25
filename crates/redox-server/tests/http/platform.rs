//! Dinamik bellek, kaynak ölçümü, ağ kartı koruması ve dört göz ilkesi.

use super::*;

#[tokio::test]
pub(super) async fn dynamic_memory_sets_the_balloon_target_from_guest_usage() {
    let (app, state) = setup_real();
    // 103 kapalı: balon takılır, dinamik bellek açılır; çalışmayan konukta hedef hesaplanmaz.
    assert!(post_ok(&app, "/guests/103/balloon", "min=1024").await.contains("error="), "balon yokken açılmaz");
    state.update_guest(103, &redox_core::GuestPatch { balloon: Some(true), ..Default::default() }, "admin").unwrap();
    assert!(post_ok(&app, "/guests/103/balloon", "min=cok").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/balloon", "min=1024").await.contains("notice="));
    assert_eq!(state.dynamic_memory_tick(), 0);
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"name="min""#) && tab.contains("Dinamik bellek"));
    assert!(post_ok(&app, "/guests/103/balloon", "mb=2048").await.contains("error="), "dinamik bellek açıkken elle hedef verilmez");

    // Çalışınca: 1 GB kullanım + 256 MB pay = 1280 MB; ikinci turda değişecek bir şey yoktur.
    state.mutate(|c| c.guests.iter_mut().filter(|g| g.vmid == 103).for_each(|g| g.status = GuestStatus::Running));
    assert_eq!(state.dynamic_memory_tick(), 1);
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 1280);
    assert_eq!(state.dynamic_memory_tick(), 0);

    // API ile kapatılır: balon boşalır, elle hedef yeniden verilebilir.
    let (status, _) = api(&app, post_json("/api/v1/guests/103/balloon", &serde_json::json!({ "min": 0 }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.mem_min_mb, hw.balloon_mb), (0, 0));
    assert_eq!(state.dynamic_memory_tick(), 0);
    let (status, _) = api(&app, post_json("/api/v1/guests/103/balloon", &serde_json::json!({ "mb": 2048 }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
pub(super) async fn usage_counters_are_listed_and_reset() {
    let (app, state) = setup_real();
    state.mutate(|c| {
        c.record_usage(102, Some((0, 0)), 1_700_000_000);
        c.record_usage(102, Some((5 << 20, 7 << 20)), 1_700_000_060);
    });
    let (status, one) = api(&app, request(Method::GET, "/api/v1/guests/102/usage").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((one["uptime_secs"].as_u64(), one["disk_read_mb"].as_u64(), one["disk_write_mb"].as_u64()), (Some(60), Some(5), Some(7)));
    assert!(one["mem_gb_hours"].as_f64().unwrap() > 0.0 && one["name"].is_string());
    let (_, all) = api(&app, request(Method::GET, "/api/v1/usage").body(Body::empty()).unwrap()).await;
    assert_eq!(all.as_array().unwrap().len(), state.snapshot().guests.len());
    assert!(page(&app, "/guests/102").await.contains("Kaynak ölçümü"));
    assert!(!page(&setup().0, "/guests/102").await.contains("Kaynak ölçümü"), "simülasyonda ölçüm yok");

    let (status, _) = api(&app, request(Method::DELETE, "/api/v1/guests/102/usage").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state.snapshot().usage_of(102).uptime_secs, 0);
    let (status, _) = api(&app, request(Method::GET, "/api/v1/guests/999/usage").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
pub(super) async fn nic_guard_is_set_from_the_options_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/102?tab=secenekler").await;
    assert!(tab.contains(r#"name="nic_guard""#) && !tab.contains(r#"name="nic_guard" value="1" checked"#));
    let (status, _) = api(&app, request(Method::PATCH, "/api/v1/guests/102").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"nic_guard":true}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.snapshot().guest(102).unwrap().hw.nic_guard, "çalışan konukta da açılır");
    assert!(page(&app, "/guests/102?tab=secenekler").await.contains(r#"name="nic_guard" value="1" checked"#));
    assert!(page(&app, "/guests/102").await.contains("Ağ kartı koruması"));
    state.guards_tick().unwrap();
}

#[tokio::test]
pub(super) async fn four_eyes_holds_deletions_until_another_admin_approves() {
    let (app, state) = setup();
    state.set_four_eyes(true);
    let guests = state.snapshot().guests.len();
    // API: silme yapılmaz, 202 ve istek numarası döner; aynı istek yinelenmez.
    let (status, body) = api(&app, request(Method::DELETE, "/api/v1/guests/103").body(Body::empty()).unwrap()).await;
    assert_eq!((status, body["approval"].as_u64()), (StatusCode::ACCEPTED, Some(1)));
    let (_, again) = api(&app, request(Method::DELETE, "/api/v1/guests/103").body(Body::empty()).unwrap()).await;
    assert_eq!(again["approval"].as_u64(), Some(1));
    assert_eq!(state.snapshot().guests.len(), guests, "konuk duruyor");
    let (_, list) = api(&app, request(Method::GET, "/api/v1/approvals").body(Body::empty()).unwrap()).await;
    assert_eq!((list[0]["action"].as_str(), list[0]["target"].as_str()), (Some("guest-delete"), Some("103")));
    // Silinemeyecek konuk için istek açılmaz (çalışan konuk).
    let (status, _) = api(&app, request(Method::DELETE, "/api/v1/guests/100").body(Body::empty()).unwrap()).await;
    assert_ne!(status, StatusCode::ACCEPTED);

    // İsteği yapan kendi isteğini onaylayamaz; sayfada yalnızca geri çekme düğmesi görür.
    let requester = state.snapshot().approvals[0].by.clone();
    assert!(matches!(state.approve(1, &requester), Err(redox_core::Error::InvalidSpec(m)) if m.contains("kendi isteğinizi onaylayamazsınız")));
    let home = page(&app, "/").await;
    assert!(home.contains("Onay bekliyor (#1)") && home.contains("103 kimlikli konuğu silmek") && home.contains("Geri Çek") && !home.contains(r#"action="/approvals/1/approve""#), "{}", &home[..home.len().min(200)]);
    // Başka bir yönetici onaylar: işlem iki adla günlüğe geçer.
    assert_eq!(state.approve(1, "ikinci-yonetici").unwrap(), "onaylandı: 103 kimlikli konuğu silmek");
    let c = state.snapshot();
    assert_eq!(c.guests.len(), guests - 1);
    assert!(c.tasks.iter().any(|t| t.user == format!("{requester}+ikinci-yonetici") && t.command.ends_with("destroy 103")), "{:?}", c.tasks.first());
    assert!(state.approve(1, "ikinci-yonetici").is_err(), "onay bir kez kullanılır");

    // Reddetme: istek düşer, hiçbir şey silinmez. Kapalıyken silme hemen yapılır.
    let other = state.snapshot().guests.iter().find(|g| state.snapshot().check_deletable(g.vmid).is_ok()).map(|g| g.vmid).expect("silinebilir bir konuk");
    let id = state.request_guest_delete(other, "admin").unwrap().expect("istek açılır");
    assert!(post_ok(&app, &format!("/approvals/{id}/reject"), "").await.contains("notice="));
    assert!(state.snapshot().approvals.is_empty() && state.snapshot().guests.len() == guests - 1);
    state.set_four_eyes(false);
    let (status, _) = api(&app, request(Method::DELETE, &format!("/api/v1/guests/{other}")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state.snapshot().guests.len(), guests - 2);
}
