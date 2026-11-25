//! Yedek işleri.

use super::*;

#[tokio::test]
pub(super) async fn backup_jobs_are_managed_from_the_backups_page_and_the_api() {
    let (app, state) = setup();
    let empty = page(&app, "/backups").await;
    assert!(empty.contains("Yedek işi yok") && empty.contains(r#"action="/backups/jobs""#), "boş liste ve form");

    // Form: adı, saati, günleri, etiketi ve saklama sayısıyla iş eklenir.
    let weekdays = "day=1&day=2&day=3&day=4&day=5";
    assert!(post_ok(&app, "/backups/jobs", &format!("name=gece&time=03%3A30&{weekdays}&tags=yedekli&keep=2")).await.contains("notice="));
    for bad in ["name=gece&time=03%3A30&tags=&keep=2", "name=Gece+Yedegi&time=03%3A30&day=1&tags=&keep=2", "name=x&time=03%3A30&day=1&tags=&keep=cok", "name=x&time=25%3A00&day=1&tags=&keep=2"] {
        assert!(post_ok(&app, "/backups/jobs", bad).await.contains("error="), "{bad}");
    }
    let job = state.snapshot().job("gece").unwrap().clone();
    assert_eq!((job.time.as_str(), job.days, job.tags.as_slice(), job.keep), ("03:30", 0b001_1111, &["yedekli".to_string()][..], 2));

    // Kapsam: 102 çalışıyor (simülasyon çalışan konuğu yedekleyemez), 103 ve 202 kapalı.
    for vmid in [102, 103, 202] {
        state.update_guest(vmid, &redox_core::GuestPatch { tags: Some(vec!["yedekli".into()]), ..Default::default() }, "admin").unwrap();
    }
    let listed = page(&app, "/backups").await;
    assert!(listed.contains("hafta içi") && listed.contains("etiket: yedekli") && listed.contains("(3 konuk)") && listed.contains("son 2"), "{listed}");
    assert!(listed.contains(r#"action="/backups/jobs/gece/run""#) && listed.contains(r#"action="/backups/jobs/gece/delete""#));

    // Zamanlayıcı: saati gelince tur başlar, konuklar tek tek yedeklenir; alınamayan günlüğe yazılır, tur sürer.
    assert_eq!(state.jobs_tick("2026-11-25", 2, 200), 0, "saati gelmedi");
    assert!(!state.snapshot().job("gece").unwrap().active());
    assert_eq!(state.jobs_tick("2026-11-28", 5, 300), 0, "cumartesi çalışmaz");
    assert_eq!(state.jobs_tick("2026-11-25", 2, 215), 0, "ilk konuk (102) çalışıyor: yedeği alınamaz");
    assert!(state.snapshot().job("gece").unwrap().active(), "tur sürüyor");
    assert!(page(&app, "/backups").await.contains("Sürüyor"));
    assert_eq!(state.jobs_tick("2026-11-25", 2, 216), 1, "103");
    assert_eq!(state.jobs_tick("2026-11-25", 2, 217), 1, "202");
    assert_eq!(state.jobs_tick("2026-11-25", 2, 218), 0);
    let c = state.snapshot();
    assert!(!c.job("gece").unwrap().active());
    let mut done: Vec<_> = c.backups.iter().map(|b| (b.vmid, b.auto, b.job.as_deref())).collect();
    done.sort();
    assert_eq!(done, [(103, true, Some("gece")), (202, true, Some("gece"))]);
    assert!(c.tasks.iter().any(|t| t.command.contains("backup 102 (iş: gece)") && t.user == redox_server::state::SCHEDULER), "alınamayan yedek günlükte");
    let after = page(&app, "/backups").await;
    assert!(after.contains(r#"<span class="tag">iş: gece</span>"#) && after.contains("son: 2026-11-25"), "{after}");
    assert_eq!(state.jobs_tick("2026-11-25", 2, 900), 0, "aynı gün ikinci kez başlamaz");

    // Şimdi Çalıştır: günü işaretlemeden turu başlatır; kapsamı boş işte hata verir.
    assert!(post_ok(&app, "/backups/jobs/gece/run", "").await.contains("notice="));
    assert_eq!(state.snapshot().job("gece").unwrap().queue.len(), 3);
    assert!(post_ok(&app, "/backups/jobs/gece/run", "").await.contains("error="), "tur sürerken yeniden başlatılamaz");
    assert!(post_ok(&app, "/backups/jobs", &format!("name=bos&time=01%3A00&{weekdays}&tags=kimsede-yok&keep=1")).await.contains("notice="));
    assert!(post_ok(&app, "/backups/jobs/bos/run", "").await.contains("error="));
    assert!(post_ok(&app, "/backups/jobs/yok/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/backups/jobs/bos/delete", "").await.contains("notice="));

    // API.
    let (status, list) = api(&app, get("/api/v1/jobs")).await;
    assert_eq!((status, list.as_array().map(Vec::len), list[0]["name"].as_str()), (StatusCode::OK, Some(1), Some("gece")));
    let (status, _) = api(&app, post_json("/api/v1/jobs", &json!({ "name": "hepsi", "time": "01:00" }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, v) = api(&app, post_json("/api/v1/jobs", &json!({ "name": "hepsi", "time": "01:00", "days": "9" }))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_spec")), "{v}");
    let (status, v) = api(&app, request(Method::POST, "/api/v1/jobs/hepsi/run").body(Body::empty()).unwrap()).await;
    assert_eq!((status, v["queued"].as_u64()), (StatusCode::ACCEPTED, Some(state.snapshot().guests.len() as u64)));
    let viewer = as_viewer(Method::POST, "/api/v1/jobs/gece/run").body(Body::empty()).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN, "izleyici iş başlatamaz");
    let delete = || request(Method::DELETE, "/api/v1/jobs/hepsi").body(Body::empty()).unwrap();
    assert_eq!(send(&app, delete()).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, delete()).await.0, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
pub(super) async fn job_backups_follow_the_jobs_own_retention() {
    let (_, state) = setup();
    state.update_guest(103, &redox_core::GuestPatch { tags: Some(vec!["yedekli".into()]), backup_keep: Some(5), ..Default::default() }, "admin").unwrap();
    state.job_put(redox_core::BackupJob::from_text("gece", "03:00", "", "yedekli", Some(2)).unwrap()).unwrap();
    for day in ["2026-11-25", "2026-11-26", "2026-11-27"] {
        assert_eq!(state.jobs_tick(day, 0, 300), 1, "{day}");
        tokio::time::sleep(Duration::from_millis(1100)).await;
    }
    let manual = state.backup_create(103, "admin").unwrap();
    state.scheduler_tick("2026-11-27", 301);
    let c = state.snapshot();
    assert_eq!(c.backups.iter().filter(|b| b.job.as_deref() == Some("gece")).count(), 2, "işin saklama sayısı (2) uygulanır, konuğunki (5) değil");
    assert!(c.backup(&manual).is_ok(), "elle alınan yedek silinmez");
    // İş silinince aldığı yedekler kalır ve artık budanmaz.
    state.job_remove("gece").unwrap();
    state.scheduler_tick("2026-11-27", 302);
    assert_eq!(state.snapshot().backups.len(), 3);
}

// ---------- cloud-init düzenleme ----------
