//! Sayfalar.

use super::*;

#[tokio::test]
pub(super) async fn dashboard_shows_cluster_state() {
    let (app, _) = setup();
    let body = page(&app, "/").await;
    for needle in ["istanbul-01", "Küme sağlıklı", "5<small> / 8 çalışıyor</small>", "db-postgres", "pve-ank-01", "redox cluster status", "Henüz görev yok"] {
        assert!(body.contains(needle), "eksik: {needle}");
    }
    assert!(body.contains(r#"data-theme="dark""#));
    assert!(body.contains(r#"<a href="/" aria-current="page">"#), "sol menüde etkin bölüm işaretli olmalı");
}

#[tokio::test]
pub(super) async fn every_page_renders_without_any_script() {
    let (app, _) = setup();
    for uri in [
        "/", "/guests", "/guests?tab=calisan", "/tasks", "/backups", "/nodes/pve-ist-01", "/guests/102", "/guests/102?tab=donanim", "/guests/102?tab=secenekler", "/guests/102?tab=cogalt",
        "/guests/200", "/storage", "/help", "/guests/new?kind=vm", "/guests/new?kind=ct", "/guests/103/delete",
    ] {
        let body = page(&app, uri).await;
        assert!(body.starts_with("<!DOCTYPE html><html lang=\"tr\""), "{uri}");
        let lower = body.to_lowercase();
        assert!(!lower.contains("<script"), "{uri}: betik etiketi var");
        assert!(!lower.contains("javascript:"), "{uri}: javascript: bağlantısı var");
        assert!(!lower.contains(" onclick=") && !lower.contains(" onsubmit=") && !lower.contains(" onload="), "{uri}: satır içi olay");
        assert!(body.contains(r#"<main id="main""#), "{uri}");
    }
}

#[tokio::test]
pub(super) async fn responses_carry_security_headers() {
    let (app, _) = setup();
    for uri in ["/", "/api/v1/cluster", "/assets/style.css", "/olmayan"] {
        let (_, headers, _) = send(&app, get(uri)).await;
        let csp = headers.get("content-security-policy").expect(uri).to_str().unwrap();
        assert!(csp.contains("script-src 'none'") && csp.contains("frame-ancestors 'none'"), "{uri}: {csp}");
        assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    }
}

#[tokio::test]
pub(super) async fn unknown_resources_return_a_404_page() {
    let (app, _) = setup();
    for uri in ["/nodes/yok", "/guests/999", "/guests/abc", "/guests/999/delete", "/olmayan/sayfa"] {
        let (status, _, body) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(body.contains("Bulunamadı") && body.contains("Genel Bakışa Dön"), "{uri}");
    }
}

#[tokio::test]
pub(super) async fn guest_page_reflects_status_in_buttons() {
    let (app, _) = setup();
    let stopped = page(&app, "/guests/103").await;
    assert!(stopped.contains(r#"name="action" value="start">"#), "durmuş konuk başlatılabilir");
    assert!(stopped.contains(r#"name="action" value="stop" disabled>"#));
    assert!(stopped.contains(r#"name="action" value="reboot" disabled>"#));
    assert!(stopped.contains(r#"href="/guests/103/delete""#), "durmuş konuk silinebilir");

    let running = page(&app, "/guests/102").await;
    assert!(running.contains(r#"name="action" value="start" disabled>"#));
    assert!(running.contains(r#"name="action" value="stop">"#));
    assert!(running.contains(r#"name="action" value="pause">"#));
    assert!(!running.contains(r#"href="/guests/102/delete""#), "çalışan konuk silinemez");
    assert!(running.contains("500 GB · local-zfs") && running.contains("10.10.1.30"));

    let paused = page(&app, "/guests/104").await;
    assert!(paused.contains(r#"name="action" value="resume">"#) && paused.contains("Sürdür"));
}

#[tokio::test]
pub(super) async fn filter_narrows_tree_and_table() {
    let (app, _) = setup();
    let body = page(&app, "/guests?q=web").await;
    assert!(body.contains("web-prod-01") && body.contains("web-prod-02"));
    // "db-postgres" komut satırı ipucunda da geçtiği için bağlantısına bakılır.
    assert!(!body.contains(r#"href="/guests/102""#) && !body.contains("monitoring") && !body.contains("win-build"));
    assert!(body.contains(r#"value="web""#), "filtre kutusu değeri korumalı");

    let none = page(&app, "/guests?q=zzz").await;
    assert!(none.contains("ile eşleşen konuk yok") && none.contains("Filtreyi temizleyin"));

    let node = page(&app, "/nodes/pve-ist-02?q=k8s").await;
    assert!(node.contains("k8s-worker-1") && !node.contains("win-build"));
}

#[tokio::test]
pub(super) async fn user_input_is_escaped() {
    let (app, _) = setup();
    let attack = "%3Cscript%3Ealert(1)%3C%2Fscript%3E";
    for uri in [format!("/guests?q={attack}"), format!("/tasks?q={attack}"), format!("/?notice={attack}"), format!("/guests/102?error={attack}"), format!("/guests/{attack}")] {
        let (_, _, body) = send(&app, get(&uri)).await;
        assert!(!body.contains("<script>alert"), "{uri}: kaçışsız çıktı");
        assert!(body.contains("&lt;script&gt;alert(1)"), "{uri}: girdi kaçışlı olarak görünmeli");
    }
}

// ---------- Eylemler ----------
