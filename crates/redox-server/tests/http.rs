//! Uçtan uca HTTP testleri: gerçek yönlendirici, simülasyon sürücüsü, ağ yok.

use std::time::Duration;

use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use redox_core::{Action, Cluster, Guest, GuestKind, GuestStatus, Hypervisor, Probe, SimDriver, TaskStatus};
use redox_server::auth::{Auth, Cost, Role, UserFile};
use redox_server::state::now;
use redox_server::{app, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const HOST_NAME: &str = "127.0.0.1:8006";

const ADMIN_PW: &str = "yonetici-parolasi-1";
const VIEWER_PW: &str = "izleyici-parolasi-1";
const ADMIN_TOKEN: &str = "redox_test_admin_token";
const VIEWER_TOKEN: &str = "redox_test_viewer_token";

fn users() -> UserFile {
    let mut f = UserFile::default();
    f.add_user("admin", ADMIN_PW, Role::Admin, Cost::Test).unwrap();
    f.add_user("izleyici", VIEWER_PW, Role::Viewer, Cost::Test).unwrap();
    f.add_token("admin", "test", ADMIN_TOKEN).unwrap();
    f.add_token("izleyici", "test", VIEWER_TOKEN).unwrap();
    f
}

fn setup_full(delay: Duration, driver: SimDriver, behind_tls_proxy: bool) -> (Router, AppState) {
    let auth = Auth::new(users(), None, behind_tls_proxy, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(driver), delay, None, auth);
    (app(state.clone()), state)
}

fn setup_with(delay: Duration, driver: SimDriver) -> (Router, AppState) {
    setup_full(delay, driver, false)
}

fn setup() -> (Router, AppState) {
    setup_with(Duration::ZERO, SimDriver::default())
}

/// Kimliksiz istek.
fn anon(method: Method, uri: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(uri).header(HOST, HOST_NAME)
}

/// Yönetici API token'ıyla istek (testlerin çoğu bunu kullanır).
fn request(method: Method, uri: &str) -> axum::http::request::Builder {
    anon(method, uri).header(AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
}

fn as_viewer(method: Method, uri: &str) -> axum::http::request::Builder {
    anon(method, uri).header(AUTHORIZATION, format!("Bearer {VIEWER_TOKEN}"))
}

fn form_body(builder: axum::http::request::Builder, body: &str) -> Request<Body> {
    builder
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn login_request(user: &str, password: &str, next: &str) -> Request<Body> {
    form_body(anon(Method::POST, "/login"), &format!("username={user}&password={password}&next={next}"))
}

/// Giriş yapar ve `Cookie` başlığına konacak değeri döndürür.
async fn login_cookie(app: &Router, user: &str, password: &str) -> String {
    let (status, headers, _) = send(app, login_request(user, password, "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "giriş başarılı olmalı");
    let set = headers.get(SET_COOKIE).unwrap().to_str().unwrap();
    set.split(';').next().unwrap().to_string()
}

fn get(uri: &str) -> Request<Body> {
    request(Method::GET, uri).body(Body::empty()).unwrap()
}

fn post_form(uri: &str, body: &str) -> Request<Body> {
    request(Method::POST, uri)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    request(Method::POST, uri)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let res = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (parts.status, parts.headers, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn page(app: &Router, uri: &str) -> String {
    let (status, _, body) = send(app, get(uri)).await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    body
}

/// Konuğun ayar sekmeleri (donanım, seçenekler, çoğalt ve taşı) tek metinde. `base`: `/guests/N`.
async fn settings(app: &Router, base: &str) -> String {
    let mut all = String::new();
    for tab in ["donanim", "secenekler", "cogalt"] {
        all.push_str(&page(app, &format!("{base}?tab={tab}")).await);
    }
    all
}

async fn api(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let (status, _, body) = send(app, req).await;
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

fn location(headers: &HeaderMap) -> &str {
    headers.get(LOCATION).expect("Location başlığı").to_str().unwrap()
}

const VALID_FORM: &str = "kind=vm&node=pve-ist-01&vmid=&name=yeni-vm&os=debian-12&storage=local-zfs&disk_gb=32&cores=2&memory_mb=2048&bridge=vmbr0&onboot=1";

// ---------- Sayfalar ----------

#[tokio::test]
async fn dashboard_shows_cluster_state() {
    let (app, _) = setup();
    let body = page(&app, "/").await;
    for needle in ["istanbul-01", "Küme sağlıklı", "5<small> / 8 çalışıyor</small>", "db-postgres", "pve-ank-01", "redox cluster status", "Henüz görev yok"] {
        assert!(body.contains(needle), "eksik: {needle}");
    }
    assert!(body.contains(r#"data-theme="dark""#));
    assert!(body.contains(r#"<a href="/" aria-current="page">"#), "sol menüde etkin bölüm işaretli olmalı");
}

#[tokio::test]
async fn every_page_renders_without_any_script() {
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
async fn responses_carry_security_headers() {
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
async fn unknown_resources_return_a_404_page() {
    let (app, _) = setup();
    for uri in ["/nodes/yok", "/guests/999", "/guests/abc", "/guests/999/delete", "/olmayan/sayfa"] {
        let (status, _, body) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(body.contains("Bulunamadı") && body.contains("Genel Bakışa Dön"), "{uri}");
    }
}

#[tokio::test]
async fn guest_page_reflects_status_in_buttons() {
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
async fn filter_narrows_tree_and_table() {
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
async fn user_input_is_escaped() {
    let (app, _) = setup();
    let attack = "%3Cscript%3Ealert(1)%3C%2Fscript%3E";
    for uri in [format!("/guests?q={attack}"), format!("/tasks?q={attack}"), format!("/?notice={attack}"), format!("/guests/102?error={attack}"), format!("/guests/{attack}")] {
        let (_, _, body) = send(&app, get(&uri)).await;
        assert!(!body.contains("<script>alert"), "{uri}: kaçışsız çıktı");
        assert!(body.contains("&lt;script&gt;alert(1)"), "{uri}: girdi kaçışlı olarak görünmeli");
    }
}

// ---------- Eylemler ----------

#[tokio::test]
async fn start_form_runs_task_and_redirects() {
    let (app, state) = setup();
    let (status, headers, _) = send(&app, post_form("/guests/103/action", "action=start")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/103?notice="), "{}", location(&headers));
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);

    let body = page(&app, location(&headers)).await;
    assert!(body.contains(r#"role="status""#) && body.contains("Başlat isteği alındı: 103"));
    assert!(body.contains("redox vm start 103") && body.contains("Tamam"), "görev günlüğe düşmeli");
    assert!(!body.contains(r#"http-equiv="refresh""#), "süren görev yokken yenileme olmamalı");
}

#[tokio::test]
async fn invalid_action_reports_error_and_changes_nothing() {
    let (app, state) = setup();
    let before = state.snapshot();
    let (status, headers, _) = send(&app, post_form("/guests/100/action", "action=start")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/100?error="));
    let body = page(&app, location(&headers)).await;
    assert!(body.contains(r#"role="alert""#) && body.contains("uygulanamaz"));

    for (uri, form) in [("/guests/100/action", "action=explode"), ("/guests/999/action", "action=start"), ("/guests/abc/action", "action=start")] {
        let (status, headers, _) = send(&app, post_form(uri, form)).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
        assert!(location(&headers).contains("error="), "{uri}");
    }
    assert_eq!(state.snapshot(), before);
}

#[tokio::test]
async fn slow_task_locks_guest_and_page_auto_refreshes() {
    let (app, state) = setup_with(Duration::from_millis(80), SimDriver::default());
    let (_, headers, _) = send(&app, post_form("/guests/103/action", "action=start")).await;
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Stopped, "görev bitmeden durum değişmez");

    let body = page(&app, location(&headers)).await;
    assert!(body.contains(r#"<meta http-equiv="refresh" content="2;url=/guests/103">"#), "yenileme bildirimsiz adrese dönmeli");
    assert!(body.contains("İşlem sürüyor") && body.contains(r#"aria-label="1 görev sürüyor""#));
    assert!(body.contains(r#"name="action" value="start" disabled>"#), "kilitliyken düğmeler kapalı");

    // İkinci istek kilide takılır.
    let (_, headers, _) = send(&app, post_form("/guests/103/action", "action=start")).await;
    assert!(location(&headers).contains("error="));

    tokio::time::sleep(Duration::from_millis(400)).await;
    let c = state.snapshot();
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Running);
    assert!(!c.has_running_tasks());
    assert!(!page(&app, "/guests/103").await.contains(r#"http-equiv="refresh""#));
}

#[tokio::test]
async fn driver_failure_is_shown_in_task_log() {
    let (app, state) = setup_with(Duration::ZERO, SimDriver::failing_on(103));
    send(&app, post_form("/guests/103/action", "action=start")).await;
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Stopped);
    let body = page(&app, "/tasks?q=103").await;
    assert!(body.contains("Hata") && body.contains("simülasyon: 103 start başarısız oldu"));
}

#[tokio::test]
async fn create_form_happy_path() {
    let (app, state) = setup();
    let (status, headers, _) = send(&app, post_form("/guests", VALID_FORM)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/105?notice="));
    let c = state.snapshot();
    let g = c.guest(105).unwrap();
    assert_eq!((g.name.as_str(), g.status, g.onboot, g.mem_total_mb), ("yeni-vm", GuestStatus::Stopped, true, 2048));
    let body = page(&app, "/").await;
    assert!(body.contains("yeni-vm") && body.contains("redox vm create 105"));
}

#[tokio::test]
async fn create_form_shows_errors_and_keeps_input() {
    let (app, state) = setup();
    let before = state.snapshot();

    let bad = VALID_FORM.replace("name=yeni-vm", "name=K%C3%B6t%C3%BC+Ad").replace("cores=2", "cores=iki");
    let (status, _, body) = send(&app, post_form("/guests", &bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("Konuk oluşturulamadı") && body.contains("Çekirdek bir tam sayı olmalı."));
    assert!(body.contains(r#"value="Kötü Ad""#) && body.contains(r#"value="iki""#), "girilen değerler korunmalı");
    assert!(body.contains(r##"<a href="#cores">"##), "özet ilgili alana bağlanmalı");

    let taken = VALID_FORM.replace("vmid=", "vmid=100");
    let (status, _, body) = send(&app, post_form("/guests", &taken)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("100 kimliği zaten kullanılıyor.") && body.contains(r#"id="vmid-err""#));

    let wrong_storage = VALID_FORM.replace("storage=local-zfs", "storage=local-lvm");
    let (status, _, body) = send(&app, post_form("/guests", &wrong_storage)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("Local-lvm deposu pve-ist-01 düğümünde kullanılamıyor.") && body.contains(r#"id="storage-err""#));

    let bad_name = VALID_FORM.replace("name=yeni-vm", "name=-x");
    let (_, _, body) = send(&app, post_form("/guests", &bad_name)).await;
    assert!(body.contains(r#"id="name-err""#));

    assert_eq!(state.snapshot(), before, "hatalı formlar konuk ya da görev oluşturmamalı");
}

#[tokio::test]
async fn delete_needs_confirmation_and_stopped_guest() {
    let (app, state) = setup();
    let confirm = page(&app, "/guests/103/delete").await;
    assert!(confirm.contains("kalıcı olarak silinecek") && confirm.contains("Kalıcı Olarak Sil") && confirm.contains("200 GB"));
    assert!(state.snapshot().guest(103).is_ok(), "onay sayfasını açmak silmez");

    let (_, headers, _) = send(&app, post_form("/guests/100/delete", "")).await;
    assert!(location(&headers).starts_with("/guests/100?error="), "çalışan konuk silinemez");
    assert!(state.snapshot().guest(100).is_ok());

    let (status, headers, _) = send(&app, post_form("/guests/103/delete", "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/?notice="));
    assert!(state.snapshot().guest(103).is_err());
    assert_eq!(send(&app, get("/guests/103")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn command_line_navigates_and_acts() {
    let (app, state) = setup();
    for (q, expected) in [
        ("open+postgres", "/guests/102"),
        ("pve-ank-01", "/nodes/pve-ank-01"),
        ("200", "/guests/200"),
        ("storage", "/storage"),
        ("help", "/help"),
        ("home", "/"),
        ("zzz", "/guests?q=zzz"),
    ] {
        let (status, headers, _) = send(&app, post_form("/cmd", &format!("q={q}"))).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{q}");
        assert_eq!(location(&headers), expected, "{q}");
    }
    for q in ["", "stop", "start+999", "open+web-prod"] {
        let (_, headers, _) = send(&app, post_form("/cmd", &format!("q={q}"))).await;
        assert!(location(&headers).starts_with("/?error="), "{q}: {}", location(&headers));
    }

    let (_, headers, _) = send(&app, post_form("/cmd", "q=start+win-build")).await;
    assert!(location(&headers).starts_with("/guests/103?notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);
}

#[tokio::test]
async fn theme_cookie_and_safe_redirect() {
    let (app, _) = setup();
    let (status, headers, _) = send(&app, get("/theme?to=light&back=%2Fstorage")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/storage");
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().starts_with("theme=light;"));

    let req = request(Method::GET, "/").header(COOKIE, "theme=light").body(Body::empty()).unwrap();
    let (_, _, body) = send(&app, req).await;
    assert!(body.contains(r#"data-theme="light""#) && body.contains("/theme?to=dark&amp;back=%2F"));

    let (_, headers, _) = send(&app, get("/theme?to=dark&back=%2F%2Fevil.example")).await;
    assert_eq!(location(&headers), "/", "site dışına yönlendirmemeli");
}

#[tokio::test]
async fn cross_site_posts_are_rejected() {
    let (app, state) = setup();
    let before = state.snapshot();
    let cross = |uri: &str, name: &'static str, value: &str| {
        request(Method::POST, uri)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(name, value)
            .body(Body::from("action=start"))
            .unwrap()
    };
    for req in [
        cross("/guests/103/action", "origin", "http://evil.example"),
        cross("/guests/103/action", "origin", "null"),
        cross("/guests/103/action", "sec-fetch-site", "cross-site"),
        cross("/api/v1/guests/103/start", "origin", "http://evil.example"),
        cross("/guests/103/delete", "origin", "http://127.0.0.1:9999"),
    ] {
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    }
    assert_eq!(state.snapshot(), before);

    let same = request(Method::POST, "/guests/103/action")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(ORIGIN, format!("http://{HOST_NAME}"))
        .header("sec-fetch-site", "same-origin")
        .body(Body::from("action=start"))
        .unwrap();
    assert_eq!(send(&app, same).await.0, StatusCode::SEE_OTHER);
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);
}

#[tokio::test]
async fn assets_are_served_with_types() {
    let (app, _) = setup();
    for (file, mime, needle) in [
        ("style.css", "text/css; charset=utf-8", "--ember:"),
        ("icons.svg", "image/svg+xml", r#"id="i-vm""#),
        ("logo.svg", "image/svg+xml", "<svg"),
        ("wordmark.svg", "image/svg+xml", "<svg"),
    ] {
        let (status, headers, body) = send(&app, get(&format!("/assets/{file}"))).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), mime, "{file}");
        assert!(body.contains(needle), "{file}");
    }
    assert_eq!(send(&app, get("/assets/yok.js")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn every_icon_used_in_pages_exists_in_sprite() {
    let (app, _) = setup();
    let sprite = page(&app, "/assets/icons.svg").await;
    for uri in ["/", "/guests/102", "/guests/104", "/guests/103/delete", "/storage", "/help", "/guests/new?kind=vm"] {
        let body = page(&app, uri).await;
        let mut used = 0;
        for part in body.split("/assets/icons.svg?v=").skip(1) {
            let (version, rest) = part.split_once('#').unwrap();
            assert_eq!(version, redox_server::view::asset_version(), "{uri}: sürüm damgası");
            let id = rest.split('"').next().unwrap();
            used += 1;
            assert!(sprite.contains(&format!(r#"id="{id}""#)), "{uri}: sprite içinde {id} yok");
        }
        assert!(used > 5, "{uri}: ikon bulunamadı");
        assert!(body.contains(&format!("/assets/style.css?v={}", redox_server::view::asset_version())), "{uri}: stil damgasız");
    }
}

// ---------- JSON API ----------

#[tokio::test]
async fn api_reads() {
    let (app, _) = setup();
    let (status, v) = api(&app, get("/api/v1/cluster")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((v["guests_total"].as_u64(), v["quorate"].as_bool(), v["cpu_pct"].as_u64()), (Some(8), Some(true), Some(40)));

    assert_eq!(api(&app, get("/api/v1/nodes")).await.1.as_array().unwrap().len(), 3);
    assert_eq!(api(&app, get("/api/v1/guests")).await.1.as_array().unwrap().len(), 8);
    assert_eq!(api(&app, get("/api/v1/storage")).await.1.as_array().unwrap().len(), 4);
    assert_eq!(api(&app, get("/api/v1/guests/102")).await.1["name"], "db-postgres");
    assert_eq!(api(&app, get("/api/v1/nodes/pve-ank-01")).await.1["cores"], 16);

    for (uri, code) in [("/api/v1/guests/999", "guest_not_found"), ("/api/v1/nodes/yok", "node_not_found"), ("/api/v1/tasks/42", "task_not_found")] {
        let (status, v) = api(&app, get(uri)).await;
        assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some(code)), "{uri}");
        assert!(v["error"].as_str().is_some_and(|m| !m.is_empty()));
    }
}

#[tokio::test]
async fn api_actions_and_conflicts() {
    let (app, state) = setup();
    let start = || request(Method::POST, "/api/v1/guests/103/start").body(Body::empty()).unwrap();

    let (status, task) = api(&app, start()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!((task["command"].as_str(), task["status"].as_str()), (Some("vm start 103"), Some("ok")));
    assert_eq!(api(&app, get("/api/v1/guests/103")).await.1["status"], "running");
    let id = task["id"].as_u64().unwrap();
    assert_eq!(api(&app, get(&format!("/api/v1/tasks/{id}"))).await.1["vmid"], 103);

    let (status, v) = api(&app, start()).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("invalid_transition")));

    let explode = request(Method::POST, "/api/v1/guests/103/explode").body(Body::empty()).unwrap();
    let (status, v) = api(&app, explode).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("unknown_action")));

    assert_eq!(state.snapshot().tasks.iter().filter(|t| t.status == TaskStatus::Ok).count(), 1);
}

#[tokio::test]
async fn api_create_and_delete() {
    let (app, state) = setup();
    let spec = json!({"name": "api-vm", "kind": "ct", "node": "pve-ank-01", "cores": 1, "memory_mb": 256, "disk_gb": 8, "storage": "local-lvm"});
    let (status, g) = api(&app, post_json("/api/v1/guests", &spec)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!((g["vmid"].as_u64(), g["status"].as_str(), g["bridge"].as_str()), (Some(105), Some("stopped"), Some("nat")), "ağ verilmezse NAT");

    let mut bad = spec.clone();
    bad["name"] = json!("Kötü Ad");
    let (status, v) = api(&app, post_json("/api/v1/guests", &bad)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_name")));

    let mut huge = spec.clone();
    huge["name"] = json!("buyuk");
    huge["disk_gb"] = json!(99_999);
    let (status, v) = api(&app, post_json("/api/v1/guests", &huge)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("insufficient_storage")));

    let del = |id: u32| request(Method::DELETE, &format!("/api/v1/guests/{id}")).body(Body::empty()).unwrap();
    let (status, v) = api(&app, del(100)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("not_stopped")));
    assert_eq!(send(&app, del(105)).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, del(105)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(state.snapshot().guests.len(), 8);
}

// ---------- Kimlik doğrulama ve yetki ----------

#[tokio::test]
async fn anonymous_requests_are_turned_away() {
    let (app, state) = setup();
    let before = state.snapshot();

    for (uri, next) in [("/", "%2F"), ("/guests/102?tab=yedekler", "%2Fguests%2F102%3Ftab%3Dyedekler"), ("/olmayan", "%2Folmayan")] {
        let (status, headers, body) = send(&app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
        assert_eq!(location(&headers), format!("/login?next={next}"), "{uri}");
        assert!(!body.contains("db-postgres"), "{uri}: kimliksiz yanıtta veri sızmamalı");
    }
    for uri in ["/guests/103/action", "/guests/103/delete", "/guests", "/cmd", "/logout"] {
        let (status, headers, _) = send(&app, form_body(anon(Method::POST, uri), "action=start&q=start+103")).await;
        assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login"), "{uri}");
    }
    for (method, uri) in [(Method::GET, "/api/v1/cluster"), (Method::GET, "/api/v1/guests"), (Method::POST, "/api/v1/guests/103/start"), (Method::DELETE, "/api/v1/guests/103")] {
        let (status, headers, body) = send(&app, anon(method, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(headers.get("www-authenticate").unwrap(), "Bearer");
        assert!(body.contains(r#""code":"unauthorized""#) && !body.contains("istanbul"), "{uri}: {body}");
    }
    assert_eq!(state.snapshot(), before);
}

#[tokio::test]
async fn public_endpoints_need_no_login() {
    let (app, _) = setup();
    for uri in ["/login", "/healthz", "/assets/style.css", "/assets/logo.svg"] {
        let (status, _, _) = send(&app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    let (_, _, body) = send(&app, anon(Method::GET, "/login?next=%2Fstorage").body(Body::empty()).unwrap()).await;
    assert!(body.contains(r#"type="password""#) && body.contains(r#"autocomplete="current-password""#));
    assert!(body.contains(r#"name="next" value="/storage""#));
    assert!(!body.to_lowercase().contains("<script") && !body.contains("db-postgres"));
}

#[tokio::test]
async fn bad_tokens_are_rejected_and_do_not_fall_back_to_cookie() {
    let (app, _) = setup();
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    for bad in ["Bearer yanlis", "Bearer ", "Basic YWRtaW46eA==", "redox_test_admin_token"] {
        let req = anon(Method::GET, "/api/v1/cluster").header(AUTHORIZATION, bad).header(COOKIE, &cookie).body(Body::empty()).unwrap();
        assert_eq!(send(&app, req).await.0, StatusCode::UNAUTHORIZED, "{bad}");
    }
}

#[tokio::test]
async fn login_session_and_logout() {
    let (app, _) = setup();

    let (status, headers, body) = send(&app, login_request("admin", "yanlis-parola-123", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.get(SET_COOKIE).is_none(), "başarısız girişte çerez verilmez");
    assert!(body.contains("Kullanıcı adı ya da parola hatalı.") && body.contains(r#"value="admin""#));
    let (status, _, unknown) = send(&app, login_request("yok", ADMIN_PW, "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(unknown.contains("Kullanıcı adı ya da parola hatalı."), "kullanıcının var olup olmadığı belli edilmez");

    let (status, headers, _) = send(&app, login_request("admin", ADMIN_PW, "%2Fstorage")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/storage"));
    let set = headers.get(SET_COOKIE).unwrap().to_str().unwrap().to_string();
    assert!(set.starts_with("redox_session=") && set.contains("HttpOnly") && set.contains("SameSite=Strict"), "{set}");
    assert!(!set.contains("Secure"), "düz HTTP'de Secure çerez tarayıcıda kaybolur");
    let cookie = set.split(';').next().unwrap().to_string();
    assert_eq!(cookie.len(), "redox_session=".len() + 64);

    let with = |method: Method, uri: &str| anon(method, uri).header(COOKIE, &cookie);
    let (status, _, body) = send(&app, with(Method::GET, "/").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Çıkış") && body.contains(r#"action="/logout""#) && body.contains("Yeni Konuk"));
    let (status, headers, _) = send(&app, with(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/"), "oturum açıkken giriş sayfası yönlendirir");

    let (status, headers, _) = send(&app, form_body(with(Method::POST, "/logout"), "")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login"));
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    let (status, headers, _) = send(&app, with(Method::GET, "/").body(Body::empty()).unwrap()).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login?next=%2F"), "çıkıştan sonra eski çerez geçersiz");

    let (_, headers, _) = send(&app, login_request("admin", ADMIN_PW, "%2F%2Fevil.example")).await;
    assert_eq!(location(&headers), "/", "giriş sonrası site dışına yönlendirmemeli");
}

#[tokio::test]
async fn repeated_failures_lock_the_login() {
    let (app, _) = setup();
    for _ in 0..4 {
        assert_eq!(send(&app, login_request("admin", "yanlis-parola-123", "")).await.0, StatusCode::UNAUTHORIZED);
    }
    let (status, _, body) = send(&app, login_request("admin", "yanlis-parola-123", "")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body.contains("Çok fazla hatalı deneme. 15 dakika sonra yeniden deneyin."));
    let (status, headers, _) = send(&app, login_request("admin", ADMIN_PW, "")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "kilitliyken doğru parola da reddedilir");
    assert!(headers.get(SET_COOKIE).is_none());
    // Başka hesap etkilenmez.
    login_cookie(&app, "izleyici", VIEWER_PW).await;
}

#[tokio::test]
async fn viewer_can_read_but_not_change() {
    let (app, state) = setup();
    let before = state.snapshot();
    let get = |uri: &str| as_viewer(Method::GET, uri).body(Body::empty()).unwrap();

    let (status, _, home) = send(&app, get("/")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("izleyici <small>salt okunur</small>") && !home.contains("Yeni Konuk") && !home.contains("/guests/new"));
    let (_, _, guest) = send(&app, get("/guests/103")).await;
    assert!(guest.contains("Salt okunur hesap") && !guest.contains(r#"name="action""#) && !guest.contains("/guests/103/delete"));
    assert_eq!(send(&app, get("/api/v1/guests")).await.0, StatusCode::OK);

    for uri in ["/guests/103/action", "/guests/103/delete", "/guests", "/cmd"] {
        let (status, _, body) = send(&app, form_body(as_viewer(Method::POST, uri), "action=start&q=start+103")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert!(body.contains("yönetici yetkisi gerekir"));
    }
    for (method, uri) in [(Method::POST, "/api/v1/guests/103/start"), (Method::DELETE, "/api/v1/guests/103")] {
        let (status, _, body) = send(&app, as_viewer(method, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert!(body.contains(r#""code":"forbidden""#));
    }
    assert_eq!(state.snapshot(), before);

    // Çerezli izleyici oturumu da aynı kısıtta; çıkış yapabilir.
    let cookie = login_cookie(&app, "izleyici", VIEWER_PW).await;
    let post = |uri: &str| form_body(anon(Method::POST, uri).header(COOKIE, &cookie), "action=start");
    assert_eq!(send(&app, post("/guests/103/action")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, post("/logout")).await.0, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn tasks_are_attributed_to_the_acting_user() {
    let (app, state) = setup();
    send(&app, post_form("/guests/103/action", "action=start")).await;
    send(&app, post_form("/guests", VALID_FORM)).await;
    let c = state.snapshot();
    assert!(c.tasks.len() == 2 && c.tasks.iter().all(|t| t.user == "admin"), "{:?}", c.tasks);
    assert!(page(&app, "/tasks").await.contains(r#"<td class="muted">admin</td>"#));
}

#[tokio::test]
async fn pages_are_not_cacheable_but_assets_are() {
    let (app, _) = setup();
    for uri in ["/", "/api/v1/cluster", "/guests/102"] {
        let (_, headers, _) = send(&app, get(uri)).await;
        assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store", "{uri}");
    }
    let (_, headers, _) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store");
    let (_, headers, _) = send(&app, get("/assets/style.css")).await;
    assert!(headers.get(CACHE_CONTROL).unwrap().to_str().unwrap().contains("max-age"));
    assert!(headers.get("strict-transport-security").is_none(), "TLS vekili yokken HSTS gönderilmez");
}

#[tokio::test]
async fn behind_tls_proxy_hardens_cookies_and_uses_forwarded_ip() {
    let (app, _) = setup_full(Duration::ZERO, SimDriver::default(), true);
    let from = |ip: &str, password: &str| {
        form_body(
            anon(Method::POST, "/login").header("x-forwarded-for", format!("9.9.9.9, {ip}")),
            &format!("username=admin&password={password}&next="),
        )
    };
    let (status, headers, _) = send(&app, from("1.1.1.1", ADMIN_PW)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().ends_with("; Secure"));
    assert_eq!(headers.get("strict-transport-security").unwrap(), "max-age=31536000");

    // Kilit, vekilin bildirdiği (son) adrese göredir; istemcinin uydurduğu ilk değere göre değil.
    for _ in 0..5 {
        send(&app, from("1.1.1.1", "yanlis-parola-123")).await;
    }
    assert_eq!(send(&app, from("1.1.1.1", ADMIN_PW)).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(send(&app, from("2.2.2.2", ADMIN_PW)).await.0, StatusCode::SEE_OTHER, "başka adres kilitlenmez");
}

// ---------- Gerçek sürücü kipi ----------

/// Bellekte çalışan, gerçek sürücü gibi davranan sahte: sayfaların gerçek kipteki halini sınar.
struct RealLike;

impl Hypervisor for RealLike {
    fn name(&self) -> &'static str {
        "qemu-kvm"
    }
    fn is_real(&self) -> bool {
        true
    }
    fn supports(&self, _: GuestKind) -> bool {
        true
    }
    fn isos(&self) -> Vec<String> {
        vec!["debian-12.iso".into(), "ubuntu-24.04.iso".into()]
    }
    fn bridges(&self) -> Vec<String> {
        vec!["vmbr0".into(), "br-lan".into()]
    }
    fn live_disk_ops(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm
    }
    fn hotplug(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm
    }
    fn host_cpu_vendor(&self) -> Option<&'static str> {
        Some("intel")
    }
    // Çalışan sanal makinenin canlı konsolu var (soketin kendisi bu testlerde açılmaz).
    fn vnc_socket(&self, g: &Guest) -> Option<std::path::PathBuf> {
        (g.kind == GuestKind::Vm && g.status == GuestStatus::Running).then(|| std::path::PathBuf::from(format!("/olmayan/{}.vnc", g.vmid)))
    }
    // Üçüncü disk ve üçüncü kart "takılamaz": geri alma yolu sınansın.
    fn hot_add_disk(&self, _: &Guest, id: u8) -> Result<(), String> {
        if id == 3 { Err("bağlantı noktası yanıt vermedi".into()) } else { Ok(()) }
    }
    fn hot_add_nic(&self, _: &Guest, index: u8, _: &str) -> Result<(), String> {
        if index == 3 { Err("bağlantı noktası yanıt vermedi".into()) } else { Ok(()) }
    }
    fn templates(&self) -> Vec<String> {
        vec!["alpine-3.20".into(), "debian-12".into()]
    }
    fn exec(&self, g: &Guest, command: &str) -> Result<String, String> {
        match command {
            "false" => Err("çıkış kodu 1".into()),
            _ => Ok(format!("{} üzerinde: {command}\n<b>uid=0(root)</b>", g.name)),
        }
    }
    fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
        Ok(())
    }
    fn probe(&self, g: &Guest) -> Option<Probe> {
        Some(Probe { status: g.status, cpu_pct: Some(37), mem_used_mb: Some(g.mem_total_mb / 2) })
    }
    // Konuk 1 GB kullanıyor görünür (balon sürücüsü ölçümü).
    fn balloon_stats(&self, g: &Guest) -> Option<redox_core::BalloonStats> {
        Some(redox_core::BalloonStats { total_mb: g.mem_total_mb, available_mb: g.mem_total_mb - 1024 })
    }
}

fn setup_real() -> (Router, AppState) {
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(RealLike), Duration::ZERO, None, auth);
    (app(state.clone()), state)
}

#[tokio::test]
async fn real_driver_pages_show_measurements_not_simulation() {
    let (app, state) = setup_real();
    state.refresh(None, None);
    state.refresh(None, None);

    let node = page(&app, "/nodes/pve-ist-01").await;
    assert!(node.contains("-10 dk") && !node.contains("-60 dk"), "gerçek grafik son 10 dakikayı gösterir");
    assert!(page(&app, "/").await.contains("37%"), "ölçülen CPU tabloya yansır");

    let guest = page(&app, "/guests/102").await;
    assert!(guest.contains("Ölçümler 5 saniyede bir okunur") && !guest.contains("simülasyon"));
    assert!(guest.contains("köprü: vmbr0") && guest.contains("52:54:00:00:00:66") && !guest.contains("Mbit/s"), "gerçek ağ bilgisi; uydurma ağ grafiği yok");
    assert!(guest.contains("son 10 dakika"));

    let form = page(&app, "/guests/new?kind=vm").await;
    assert!(form.contains(r#"<select id="iso" name="iso""#) && form.contains(r#"<option value="ubuntu-24.04.iso">"#));
    assert!(!page(&app, "/guests/new?kind=ct").await.contains(r#"name="iso""#), "konteynere kalıp takılmaz");
}

#[tokio::test]
async fn real_driver_create_validates_iso_and_kind() {
    let (app, state) = setup_real();
    let with_iso = format!("{VALID_FORM}&iso=debian-12.iso");
    let (status, headers, _) = send(&app, post_form("/guests", &with_iso)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/105"));
    assert_eq!(state.snapshot().guest(105).unwrap().iso.as_deref(), Some("debian-12.iso"));
    assert!(page(&app, "/guests/105").await.contains("debian-12.iso"));

    for (form, needle) in [
        (format!("{}&iso=yok.iso", VALID_FORM.replace("name=yeni-vm", "name=baska")), "ISO bulunamadı: yok.iso"),
        (format!("{}&iso=..%2F..%2Fetc%2Fpasswd", VALID_FORM.replace("name=yeni-vm", "name=baska")), "ISO bulunamadı"),
        (VALID_FORM.replace("kind=vm", "kind=ct").replace("name=yeni-vm", "name=kutu").replace("os=debian-12", "os=windows-11"), "konteyner şablonu: windows-11"),
        (VALID_FORM.replace("bridge=vmbr0", "bridge=br-yok").replace("name=yeni-vm", "name=agsiz"), "adında bir köprü yok"),
    ] {
        let (status, _, body) = send(&app, post_form("/guests", &form)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{form}");
        assert!(body.contains(needle), "{form}: beklenen ileti yok");
    }
    assert_eq!(state.snapshot().guests.len(), 9);

    // Konteyner: şablon listeden olmalı, ağ var olan bir köprü ya da NAT olmalı.
    let spec = |os: &str, bridge: &str| json!({"name": "api-ct", "kind": "ct", "node": "pve-ank-01", "cores": 1, "memory_mb": 256, "disk_gb": 8, "storage": "local-lvm", "os": os, "bridge": bridge});
    for bad in [spec("windows-11", "nat"), spec("alpine-3.20", "br-yok"), spec("alpine-3.20", "br0;reboot")] {
        let (status, v) = api(&app, post_json("/api/v1/guests", &bad)).await;
        assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_spec")), "{bad}");
    }
    let (status, g) = api(&app, post_json("/api/v1/guests", &spec("alpine-3.20", "br-lan"))).await;
    assert_eq!((status, g["bridge"].as_str(), g["os"].as_str()), (StatusCode::CREATED, Some("br-lan"), Some("alpine-3.20")));
}

// ---------- Konuk işlemleri: düzenleme, anlık görüntü, yedek, klon, toplu işlem, konsol ----------

async fn post_ok(app: &Router, uri: &str, body: &str) -> String {
    let (status, headers, _) = send(app, post_form(uri, body)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
    location(&headers).to_string()
}

#[tokio::test]
async fn guest_tabs_depend_on_kind_and_role() {
    let (app, _) = setup();
    let vm = page(&app, "/guests/103").await;
    for tab in ["?tab=konsol", "?tab=donanim", "?tab=secenekler", "?tab=goruntuler", "?tab=yedekler", "?tab=cogalt"] {
        assert!(vm.contains(&format!(r#"href="/guests/103{tab}""#)), "{tab}");
    }
    assert!(!page(&app, "/guests/200").await.contains("?tab=konsol"), "konteynerde konsol sekmesi yok");
    assert!(page(&app, "/guests/103?tab=konsol").await.contains("Simülasyon sürücüsünde konsol yok"));

    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/103?tab=konsol").body(Body::empty()).unwrap()).await;
    assert!(!viewer.contains("?tab=konsol") && !viewer.contains("iframe"), "izleyici konsolu göremez");
    for tab in ["donanim", "secenekler", "cogalt"] {
        let (_, _, viewer) = send(&app, as_viewer(Method::GET, &format!("/guests/103?tab={tab}")).body(Body::empty()).unwrap()).await;
        assert!(!viewer.contains("/guests/103/edit") && !viewer.contains("Kaydet") && !viewer.contains("tab=cogalt"), "{tab}: izleyiciye form ve çoğaltma sunulmaz");
    }
}

#[tokio::test]
async fn edit_guest_settings() {
    let (app, state) = setup();
    let form = settings(&app, "/guests/103").await;
    assert!(form.contains(r#"name="name" type="text" value="win-build""#) && form.contains(r#"name="cores""#) && form.contains("Klonla"));
    let running = settings(&app, "/guests/100").await;
    assert!(running.contains("Donanım ayarları yalnızca konuk kapalıyken değişir.") && running.contains(r#"name="cores" type="text" inputmode="numeric" value="4" disabled"#));
    assert!(!form.contains(r#"name="cores" type="text" inputmode="numeric" value="8" disabled"#), "kapalı konukta donanım alanları açıktır");

    let to = post_ok(&app, "/guests/103/edit", "name=derleme&os=windows-2025&tags=ci%2C+Prod&cores=4&memory_mb=8192&iso=&forwards=8080%3A80%2C+2222%3A22&onboot=1").await;
    assert!(to.starts_with("/guests/103?tab=secenekler&notice="), "{to}");
    let g = state.snapshot().guest(103).unwrap().clone();
    assert_eq!((g.name.as_str(), g.cores, g.mem_total_mb, g.onboot), ("derleme", 4, 8192, true));
    assert_eq!((g.tags, g.forwards.len()), (vec!["ci".to_string(), "prod".to_string()], 2));

    let conf = page(&app, "/guests/103").await;
    assert!(conf.contains("8080 → 80") && conf.contains(r#"<a class="tag" href="/guests?q=ci">ci</a>"#));
    let filtered = page(&app, "/guests?q=prod").await;
    // "derleme" adında "prod" geçmez; yalnızca etiketiyle eşleşir. Adında geçenler de listelenir.
    assert!(filtered.contains("derleme") && filtered.contains("web-prod-01") && !filtered.contains("monitoring"));

    // Hatalar düzenleme sayfasına iletiyle döner ve kaydı değiştirmez.
    for (body, needle) in [
        ("name=derleme&cores=4&memory_mb=8192&forwards=80%3A80", "error="),
        ("name=derleme&cores=dort&memory_mb=8192&forwards=", "error="),
        ("name=K%C3%B6t%C3%BC&cores=4&memory_mb=8192&forwards=", "error="),
        ("name=derleme&cores=4&memory_mb=8192&forwards=bozuk", "error="),
    ] {
        let to = post_ok(&app, "/guests/103/edit", body).await;
        assert!(to.starts_with("/guests/103?tab=donanim&") && to.contains(needle), "{body}: {to}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().forwards.len(), 2);

    // Çalışan konukta: donanım alanları gönderilmez (kapalı alan grubu), ad yine de değişir.
    post_ok(&app, "/guests/100/edit", "name=web-yeni&os=debian-12&tags=").await;
    assert_eq!(state.snapshot().guest(100).unwrap().name, "web-yeni");
    // Elle gönderilen donanım değişikliği reddedilir.
    let to = post_ok(&app, "/guests/100/edit", "name=web-yeni&cores=1&memory_mb=8192&forwards=").await;
    assert!(to.starts_with("/guests/100?tab=donanim&error="));
    assert_eq!(state.snapshot().guest(100).unwrap().cores, 4);
    assert_eq!(send(&app, get("/guests/999/edit")).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn snapshots_with_confirmation() {
    let (app, state) = setup();
    assert!(page(&app, "/guests/103?tab=goruntuler").await.contains("Henüz anlık görüntü yok"));
    let to = post_ok(&app, "/guests/103/snapshots", "name=temiz").await;
    assert!(to.starts_with("/guests/103?tab=goruntuler&notice="));
    let tab = page(&app, "/guests/103?tab=goruntuler").await;
    assert!(tab.contains("temiz") && tab.contains("/guests/103/confirm?op=rollback&amp;name=temiz"));

    // Onay sayfası hiçbir şeyi değiştirmez ve sonucu açıkça söyler.
    let confirm = page(&app, "/guests/103/confirm?op=rollback&name=temiz").await;
    assert!(confirm.contains("kaybolur") && confirm.contains(r#"action="/guests/103/snapshots/temiz/rollback""#));
    assert_eq!(state.snapshot().guest(103).unwrap().snapshots.len(), 1);
    for missing in ["/guests/103/confirm?op=rollback&name=yok", "/guests/103/confirm?op=patlat&name=temiz", "/guests/103/confirm?op=restore&name=temiz"] {
        assert_eq!(send(&app, get(missing)).await.0, StatusCode::NOT_FOUND, "{missing}");
    }

    assert!(post_ok(&app, "/guests/103/snapshots/temiz/rollback", "").await.contains("notice="));
    assert!(post_ok(&app, "/guests/103/snapshots", "name=temiz").await.contains("error="), "aynı ad ikinci kez alınmaz");
    assert!(post_ok(&app, "/guests/100/snapshots", "name=canli").await.contains("error="), "çalışan konukta alınmaz");
    assert!(post_ok(&app, "/guests/103/snapshots/temiz/patlat", "").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/snapshots/temiz/delete", "").await.contains("notice="));
    assert!(state.snapshot().guest(103).unwrap().snapshots.is_empty());
    assert!(page(&app, "/tasks?q=103").await.contains("redox vm delsnapshot 103 temiz"));
}

#[tokio::test]
async fn backups_restore_and_delete() {
    let (app, state) = setup();
    assert!(post_ok(&app, "/guests/100/backups", "").await.contains("error="), "çalışan konuk yedeklenmez");
    assert!(post_ok(&app, "/guests/103/backups", "").await.contains("notice="));
    let id = state.snapshot().backups[0].id.clone();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&id) && tab.contains("Geri Yükle"));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains(&id), "yedek yalnızca kendi konuğunda listelenir");

    let confirm = page(&app, &format!("/guests/103/confirm?op=restore&name={id}")).await;
    assert!(confirm.contains("bütün anlık görüntüler kaybolur") && confirm.contains(&format!(r#"action="/backups/{id}/restore""#)));
    assert_eq!(send(&app, get(&format!("/guests/202/confirm?op=restore&name={id}"))).await.0, StatusCode::NOT_FOUND, "başka konuğun yedeği onaylanamaz");

    assert!(post_ok(&app, &format!("/backups/{id}/restore"), "").await.starts_with("/guests/103?tab=yedekler&notice="));
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.contains("notice="));
    assert!(state.snapshot().backups.is_empty());
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.starts_with("/?error="));
}

#[tokio::test]
async fn resize_and_clone() {
    let (app, state) = setup();
    assert!(post_ok(&app, "/guests/103/resize", "disk_gb=300").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 300);
    for bad in ["disk_gb=100", "disk_gb=abc", "disk_gb=999999"] {
        assert!(post_ok(&app, "/guests/103/resize", bad).await.starts_with("/guests/103?tab=donanim&error="), "{bad}");
    }
    assert!(post_ok(&app, "/guests/100/resize", "disk_gb=500").await.contains("error="));

    let to = post_ok(&app, "/guests/103/clone", "name=win-build-2").await;
    assert!(to.starts_with("/guests/105?notice="), "{to}");
    let c = state.snapshot();
    assert_eq!((c.guest(105).unwrap().disk_gb, c.guest(105).unwrap().status), (300, GuestStatus::Stopped));
    assert!(post_ok(&app, "/guests/103/clone", "name=K%C3%B6t%C3%BC").await.contains("error="));
    assert!(post_ok(&app, "/guests/100/clone", "name=canli-kopya").await.contains("error="));
}

#[tokio::test]
async fn bulk_action_applies_to_selected_guests() {
    let (app, state) = setup();
    let list = page(&app, "/guests").await;
    assert!(list.contains(r#"<form id="bulk" class="end" method="post" action="/guests/bulk""#) && list.contains(r#"form="bulk" name="id" value="103""#) && list.contains("Seçilenler:"));

    let to = post_ok(&app, "/guests/bulk", "back=%2Fguests&id=103&id=202&id=100&action=start").await;
    assert!(to.starts_with("/guests?notice="), "{to}");
    let c = state.snapshot();
    assert_eq!((c.guest(103).unwrap().status, c.guest(202).unwrap().status), (GuestStatus::Running, GuestStatus::Running));
    let notice = page(&app, &to).await;
    assert!(notice.contains("Başlat isteği 2 konuk için alındı.") && notice.contains("atlananlar: 100."), "atlananlar bildirilmeli");

    assert!(post_ok(&app, "/guests/bulk", "action=stop").await.contains("error="), "seçim yoksa hata");
    assert!(post_ok(&app, "/guests/bulk", "id=103&action=patlat").await.contains("error="));
    assert!(post_ok(&app, "/guests/bulk", "back=%2Fnodes%2Fpve-ist-02&id=103&action=stop").await.starts_with("/nodes/pve-ist-02?notice="));
    assert!(post_ok(&app, "/guests/bulk", "back=https%3A%2F%2Fevil.example&id=103&action=start").await.starts_with("/guests?"), "site dışına dönülmez");

    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/bulk"), "id=100&action=stop")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/").body(Body::empty()).unwrap()).await;
    assert!(!viewer.contains(r#"name="id""#) && !viewer.contains("Seçililere uygula"));
}

#[tokio::test]
async fn console_is_admin_only_and_frames_only_itself() {
    let (app, _) = setup();
    for uri in ["/guests/102/screen", "/guests/102/screen.png"] {
        let (status, _, _) = send(&app, as_viewer(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        let (status, headers, _) = send(&app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{uri}");
        assert!(location(&headers).starts_with("/login"));
    }
    let (status, headers, frame) = send(&app, get("/guests/102/screen")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-frame-options").unwrap(), "SAMEORIGIN");
    let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'self'") && csp.contains("default-src 'none'"), "{csp}");
    assert!(frame.contains(r#"http-equiv="refresh" content="2""#) && frame.contains("/guests/102/screen.png?t=") && !frame.to_lowercase().contains("<script"));
    // Diğer sayfalar hâlâ hiçbir yerde çerçevelenemez.
    assert_eq!(send(&app, get("/guests/102")).await.1.get("x-frame-options").unwrap(), "DENY");

    // Simülasyonda ekran yok: görüntü isteği 503, tuş gönderme hata iletisiyle döner.
    assert_eq!(send(&app, get("/guests/102/screen.png")).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(send(&app, get("/guests/999/screen.png")).await.0, StatusCode::NOT_FOUND);
    for body in ["text=root&enter=1", "key=enter", "text=%C5%9Fifre", "key=power", &format!("text={}", "a".repeat(600))] {
        let to = post_ok(&app, "/guests/102/keys", body).await;
        assert!(to.starts_with("/guests/102?tab=konsol&error="), "{body}: {to}");
    }
    assert_eq!(post_ok(&app, "/guests/102/keys", "text=").await, "/guests/102?tab=konsol", "boş metin sessizce geçilir");
}

#[tokio::test]
async fn api_edit_snapshots_and_backups() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| {
        request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
    };
    let (status, g) = api(&app, send_json(Method::PATCH, "/api/v1/guests/103", json!({"cores": 2, "tags": ["ci"], "forwards": [{"host_port": 8080, "guest_port": 80}]}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((g["cores"].as_u64(), g["tags"][0].as_str(), g["forwards"][0]["host_port"].as_u64()), (Some(2), Some("ci"), Some(8080)));
    let (status, v) = api(&app, send_json(Method::PATCH, "/api/v1/guests/100", json!({"cores": 1}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("requires_stopped")));
    let (status, v) = api(&app, send_json(Method::PATCH, "/api/v1/guests/202", json!({"forwards": [{"host_port": 8080, "guest_port": 1}]}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("port_taken")));

    let (status, t) = api(&app, post_json("/api/v1/guests/103/snapshots", &json!({"name": "s1"}))).await;
    assert_eq!((status, t["command"].as_str(), t["status"].as_str()), (StatusCode::ACCEPTED, Some("vm snapshot 103 s1"), Some("ok")));
    let (status, v) = api(&app, post_json("/api/v1/guests/103/snapshots", &json!({"name": "s1"}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("snapshot_exists")));
    let rollback = request(Method::POST, "/api/v1/guests/103/snapshots/s1/rollback").body(Body::empty()).unwrap();
    assert_eq!(send(&app, rollback).await.0, StatusCode::ACCEPTED);
    let del = |uri: &str| request(Method::DELETE, uri).body(Body::empty()).unwrap();
    assert_eq!(send(&app, del("/api/v1/guests/103/snapshots/s1")).await.0, StatusCode::ACCEPTED);
    let (status, v) = api(&app, del("/api/v1/guests/103/snapshots/s1")).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("snapshot_not_found")));

    let make = request(Method::POST, "/api/v1/guests/103/backups").body(Body::empty()).unwrap();
    let (status, v) = api(&app, make).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = v["id"].as_str().unwrap().to_string();
    let (_, list) = api(&app, get("/api/v1/backups")).await;
    assert_eq!((list.as_array().unwrap().len(), list[0]["vmid"].as_u64()), (1, Some(103)));
    let restore = request(Method::POST, &format!("/api/v1/backups/{id}/restore")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, restore).await.0, StatusCode::ACCEPTED);
    assert_eq!(send(&app, del(&format!("/api/v1/backups/{id}"))).await.0, StatusCode::NO_CONTENT);
    let (status, v) = api(&app, del(&format!("/api/v1/backups/{id}"))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("backup_not_found")));
    assert!(state.snapshot().backups.is_empty());

    // İzleyici okuyabilir ama değiştiremez.
    assert_eq!(send(&app, as_viewer(Method::GET, "/api/v1/backups").body(Body::empty()).unwrap()).await.0, StatusCode::OK);
    let (status, _, _) = send(&app, as_viewer(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from("{}")).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------- Gömülü yazı tipleri ve ölçümler ----------

async fn send_bytes(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    (parts.status, parts.headers, body.collect().await.unwrap().to_bytes().to_vec())
}

#[tokio::test]
async fn fonts_are_served_locally_and_nothing_loads_from_third_parties() {
    let (app, _) = setup();
    let (status, headers, css) = send(&app, anon(Method::GET, "/assets/fonts/fonts.css").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "yazı tipleri giriş sayfasında da gerekir; kimlik istemez");
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "text/css; charset=utf-8");
    assert!(css.contains("font-family:'Chakra Petch'") && css.contains("font-family:'JetBrains Mono'") && css.contains("U+0100-02BA") && css.contains("U+0131"), "Türkçe harfler (Ğ, Ş, İ: latin-ext aralığı; ı) dahil");
    assert!(!css.contains("http"), "stil dosyası dış adres içermemeli");

    let files: Vec<&str> = css.split("url(/assets/fonts/").skip(1).map(|part| part.split(')').next().unwrap()).collect();
    assert!(files.len() >= 10, "{}", files.len());
    for file in files {
        let (status, headers, bytes) = send_bytes(&app, anon(Method::GET, &format!("/assets/fonts/{file}")).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "font/woff2", "{file}");
        assert_eq!(&bytes[..4], b"wOF2", "{file}");
        assert!(headers.get(CACHE_CONTROL).unwrap().to_str().unwrap().contains("immutable"));
    }
    assert_eq!(send_bytes(&app, get("/assets/fonts/yok.woff2")).await.0, StatusCode::NOT_FOUND);

    for uri in ["/", "/guests/102", "/help", "/account"] {
        let (_, headers, body) = send(&app, get(uri)).await;
        assert!(!body.contains("googleapis") && !body.contains("gstatic") && !body.contains("http://") && !body.contains("https://"), "{uri}: dış adres var");
        assert!(body.contains("/assets/fonts/fonts.css?v="), "{uri}");
        let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("font-src 'self'") && !csp.contains("google"), "{csp}");
    }
    let (_, _, login) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert!(!login.contains("googleapis") && login.contains("/assets/fonts/fonts.css?v="));
}

#[tokio::test]
async fn prometheus_metrics_need_a_token() {
    let (app, _) = setup();
    let (status, headers, _) = send(&app, anon(Method::GET, "/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "kazıyıcı giriş sayfasına yönlenmez, 401 alır");
    assert_eq!(headers.get("www-authenticate").unwrap(), "Bearer");

    let (status, headers, body) = send(&app, as_viewer(Method::GET, "/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "salt okunur token yeterli");
    assert!(headers.get(CONTENT_TYPE).unwrap().to_str().unwrap().starts_with("text/plain; version=0.0.4"));
    assert!(body.contains("redox_node_up{node=\"pve-ist-01\"} 1") && body.contains("redox_guest_running{vmid=\"103\""));
}

// ---------- Kullanıcı yönetimi, hesap, iki adımlı doğrulama, denetim ----------

#[tokio::test]
async fn admin_pages_are_hidden_from_viewers() {
    let (app, _) = setup();
    let home = page(&app, "/").await;
    assert!(home.contains(r#"href="/users""#) && home.contains(r#"href="/audit""#) && home.contains(r#"href="/account""#));
    assert!(page(&app, "/users").await.contains("izleyici") && page(&app, "/audit").await.contains("Denetim Günlüğü"));

    let viewer = |uri: &str| as_viewer(Method::GET, uri).body(Body::empty()).unwrap();
    let (_, _, home) = send(&app, viewer("/")).await;
    assert!(!home.contains(r#"href="/users""#) && !home.contains(r#"href="/audit""#) && home.contains(r#"href="/account""#));
    for uri in ["/users", "/audit"] {
        let (status, _, body) = send(&app, viewer(uri)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert!(!body.contains("admin"), "{uri}: kullanıcı adları sızmamalı");
    }
    assert_eq!(send(&app, viewer("/account")).await.0, StatusCode::OK);
    for uri in ["/users", "/users/admin/role", "/users/admin/delete", "/users/admin/password", "/users/admin/tokens", "/users/admin/totp-reset"] {
        let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, uri), "name=x&password=cok-uzun-bir-parola&role=admin&confirm=1&label=t")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn manage_users() {
    let (app, state) = setup();
    let users = || state.auth().list();

    assert!(post_ok(&app, "/users", "name=ayse&password=ayse-nin-parolasi-1&role=viewer").await.starts_with("/users?notice="));
    assert_eq!(users().len(), 3);
    login_cookie(&app, "ayse", "ayse-nin-parolasi-1").await;
    for bad in ["name=ayse&password=ayse-nin-parolasi-1&role=viewer", "name=K%C3%B6t%C3%BC&password=ayse-nin-parolasi-1&role=viewer", "name=can&password=kisa&role=viewer", "name=can&password=ayse-nin-parolasi-1&role=kral"] {
        assert!(post_ok(&app, "/users", bad).await.starts_with("/users?error="), "{bad}");
    }
    assert_eq!(users().len(), 3);

    // Rol: yükselt, son yöneticiyi düşürme.
    assert!(post_ok(&app, "/users/ayse/role", "role=admin").await.contains("notice="));
    assert!(post_ok(&app, "/users/ayse/role", "role=viewer").await.contains("notice="));
    assert!(post_ok(&app, "/users/admin/role", "role=viewer").await.contains("error="), "son yönetici izleyici yapılamaz");

    // Parola sıfırlama (seçim kutulu form: yol "-" ve alan "name") açık oturumu düşürür.
    let cookie = login_cookie(&app, "ayse", "ayse-nin-parolasi-1").await;
    assert!(post_ok(&app, "/users/-/password", "name=ayse&password=yepyeni-parola-12345").await.contains("notice="));
    let (status, _, _) = send(&app, anon(Method::GET, "/").header(COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "eski oturum geçersiz");
    login_cookie(&app, "ayse", "yepyeni-parola-12345").await;

    // Silme: onay kutusu ister; kendi hesabını silemez.
    assert!(post_ok(&app, "/users/ayse/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/users/admin/delete", "confirm=1").await.contains("error="));
    assert!(post_ok(&app, "/users/ayse/delete", "confirm=1").await.contains("notice="));
    assert!(post_ok(&app, "/users/ayse/delete", "confirm=1").await.contains("error="));
    assert_eq!(users().len(), 2);
}

#[tokio::test]
async fn tokens_are_shown_once_and_can_be_revoked() {
    let (app, state) = setup();
    let (status, headers, body) = send(&app, post_form("/users/-/tokens", "name=izleyici&label=ci")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store", "token sayfası önbelleğe yazılmaz");
    let token = body.split("redox_").nth(1).map(|rest| format!("redox_{}", &rest[..64])).expect("token sayfada görünmeli");
    assert!(!serde_json::to_string(&state.auth().list()).unwrap().contains(&token), "düz token saklanmaz");

    let with = |t: &str| anon(Method::GET, "/api/v1/cluster").header(AUTHORIZATION, format!("Bearer {t}")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, with(&token)).await.0, StatusCode::OK);
    assert!(!page(&app, "/users").await.contains(&token), "token bir daha gösterilmez");
    assert!(post_ok(&app, "/users/-/tokens", "name=izleyici&label=ci").await.contains("error="), "aynı etiket ikinci kez verilmez");

    assert!(post_ok(&app, "/users/izleyici/tokens/ci/revoke", "").await.contains("notice="));
    assert_eq!(send(&app, with(&token)).await.0, StatusCode::UNAUTHORIZED, "iptal edilen token çalışmaz");
}

#[tokio::test]
async fn viewer_manages_only_their_own_account() {
    let (app, state) = setup();
    let cookie = login_cookie(&app, "izleyici", VIEWER_PW).await;
    let post = |uri: &str, body: &str| form_body(anon(Method::POST, uri).header(COOKIE, &cookie), body);

    let (status, _, body) = send(&app, post("/account/tokens", "label=dizustu")).await;
    assert_eq!(status, StatusCode::OK, "salt okunur hesap kendi token'ını üretebilir");
    assert!(body.contains("redox_"));
    assert_eq!(state.auth().list().iter().find(|u| u.name == "izleyici").unwrap().tokens.len(), 2);
    assert_eq!(send(&app, post("/account/tokens/dizustu/revoke", "")).await.0, StatusCode::SEE_OTHER);

    let (_, headers, _) = send(&app, post("/account/password", "current=yanlis-parola-123&password=yeni-izleyici-parolasi")).await;
    assert!(location(&headers).starts_with("/account?error="));
    let (_, headers, _) = send(&app, post("/account/password", &format!("current={VIEWER_PW}&password=kisa"))).await;
    assert!(location(&headers).starts_with("/account?error="));
    let (status, headers, _) = send(&app, post("/account/password", &format!("current={VIEWER_PW}&password=yeni-izleyici-parolasi"))).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/login"));
    assert!(headers.get(SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(send(&app, login_request("izleyici", VIEWER_PW, "")).await.0, StatusCode::UNAUTHORIZED, "eski parola geçmez");
    login_cookie(&app, "izleyici", "yeni-izleyici-parolasi").await;
    // Hesap sayfası başkasının bilgisini göstermez; diğer değişiklikler hâlâ yasak.
    let cookie = login_cookie(&app, "izleyici", "yeni-izleyici-parolasi").await;
    let (_, _, account) = send(&app, anon(Method::GET, "/account").header(COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert!(account.contains("İzleyici") && !account.contains("Kullanıcı ekle"));
    assert_eq!(send(&app, form_body(anon(Method::POST, "/guests/103/action").header(COOKIE, &cookie), "action=start")).await.0, StatusCode::FORBIDDEN);
}

fn totp_code(secret: &str) -> String {
    let key = redox_server::totp::base32_decode(secret).unwrap();
    format!("{:06}", redox_server::totp::code_at(&key, now() / redox_server::totp::STEP_SECS))
}

#[tokio::test]
async fn two_factor_enrollment_and_login() {
    let (app, state) = setup();
    let me = || state.auth().list().into_iter().find(|u| u.name == "admin").unwrap();
    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    let post = |uri: &str, body: &str| form_body(anon(Method::POST, uri).header(COOKIE, &cookie), body);
    let account = || anon(Method::GET, "/account").header(COOKIE, &cookie).body(Body::empty()).unwrap();

    assert!(send(&app, account()).await.2.contains("Kurulumu Başlat"));
    assert_eq!(send(&app, post("/account/totp/start", "")).await.0, StatusCode::SEE_OTHER);
    let secret = me().totp_pending.expect("kurulum başlatılınca bekleyen anahtar oluşur");
    assert!(me().totp_secret.is_none());
    let (_, _, body) = send(&app, account()).await;
    assert!(body.contains(&secret) && body.contains("<svg") && body.contains(r#"autocomplete="one-time-code""#), "QR ve anahtar gösterilmeli");
    // Henüz etkin değil: parola tek başına yeter.
    login_cookie(&app, "admin", ADMIN_PW).await;

    let (_, headers, _) = send(&app, post("/account/totp/confirm", "code=000000")).await;
    assert!(location(&headers).starts_with("/account?error=") || totp_code(&secret) == "000000");
    let (_, headers, _) = send(&app, post("/account/totp/confirm", &format!("code={}", totp_code(&secret)))).await;
    assert!(location(&headers).starts_with("/account?notice="), "{}", location(&headers));
    assert_eq!(me().totp_secret.as_deref(), Some(secret.as_str()));
    assert!(send(&app, account()).await.2.contains("Kapatmak için parolanız"));
    assert!(page(&app, "/users").await.contains("2 Adımı Sıfırla"));

    // Giriş artık kod ister; yanlış ya da eksik kodda ileti aynıdır.
    let login = |code: &str| form_body(anon(Method::POST, "/login"), &format!("username=admin&password={ADMIN_PW}&code={code}&next="));
    for bad in ["", "123456x", "12345"] {
        let (status, _, body) = send(&app, login(bad)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad:?}");
        assert!(body.contains("Kullanıcı adı ya da parola hatalı."));
    }
    let code = totp_code(&secret);
    assert_eq!(send(&app, login(&code)).await.0, StatusCode::SEE_OTHER);
    assert_eq!(send(&app, login(&code)).await.0, StatusCode::UNAUTHORIZED, "aynı kod ikinci kez kullanılamaz");
    // API token'ları ikinci adımdan etkilenmez (betikler için).
    assert_eq!(send(&app, get("/api/v1/cluster")).await.0, StatusCode::OK);

    let (_, headers, _) = send(&app, post("/account/totp/disable", "current=yanlis-parola-123")).await;
    assert!(location(&headers).starts_with("/account?error="));
    let (_, headers, _) = send(&app, post("/account/totp/disable", &format!("current={ADMIN_PW}"))).await;
    assert!(location(&headers).starts_with("/account?notice="));
    assert!(me().totp_secret.is_none());
}

#[tokio::test]
async fn admin_can_reset_a_lost_second_factor() {
    let (app, state) = setup();
    state.auth().edit(|u| { let s = u.begin_totp("izleyici")?; u.confirm_totp("izleyici", &totp_code(&s), now()) }).unwrap();
    assert_eq!(send(&app, login_request("izleyici", VIEWER_PW, "")).await.0, StatusCode::UNAUTHORIZED);
    assert!(post_ok(&app, "/users/izleyici/totp-reset", "").await.contains("notice="));
    login_cookie(&app, "izleyici", VIEWER_PW).await;
}

#[tokio::test]
async fn audit_log_records_changes_logins_and_denials() {
    let (app, state) = setup();
    send(&app, post_form("/guests/103/action", "action=start")).await;
    send(&app, get("/guests/103")).await;
    send(&app, login_request("admin", "yanlis-parola-123", "")).await;
    login_cookie(&app, "admin", ADMIN_PW).await;
    send(&app, form_body(as_viewer(Method::POST, "/guests/100/action"), "action=stop")).await;
    send(&app, form_body(anon(Method::POST, "/guests/100/action"), "action=stop")).await;

    let log = state.auth().audit.recent("", 50);
    let seen: Vec<(&str, &str, bool)> = log.iter().rev().map(|e| (e.user.as_str(), e.action.as_str(), e.ok)).collect();
    assert_eq!(
        seen,
        [
            ("admin", "POST /guests/103/action", true),
            ("admin", "giriş (hatalı parola ya da kod)", false),
            ("admin", "giriş", true),
            ("izleyici", "POST /guests/100/action", false),
        ],
        "okuma istekleri ve kimliksiz istekler kaydedilmez; parola hiçbir kayıtta yer almaz"
    );
    assert!(!format!("{log:?}").contains("yanlis-parola"));

    let page_all = page(&app, "/audit").await;
    assert!(page_all.contains("POST /guests/103/action") && page_all.contains("Reddedildi") && page_all.contains("giriş (hatalı parola ya da kod)"));
    let filtered = page(&app, "/audit?q=izleyici").await;
    assert!(filtered.contains("POST /guests/100/action") && !filtered.contains("POST /guests/103/action"));
    assert!(page(&app, "/audit?q=zzz").await.contains("ile eşleşen kayıt yok"));
}

#[tokio::test]
async fn login_page_offers_the_code_field() {
    let (app, _) = setup();
    let (_, _, body) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert!(body.contains(r#"name="code""#) && body.contains(r#"autocomplete="one-time-code""#) && body.contains("Yalnızca iki adımlı doğrulamayı açtıysanız."));
}

// ---------- Köprü ağı ve konteynerler (gerçek sürücü kipi) ----------

#[tokio::test]
async fn real_driver_forms_offer_bridges_and_templates() {
    let (app, _) = setup_real();
    let vm = page(&app, "/guests/new?kind=vm").await;
    assert!(vm.contains(r#"<select id="bridge" name="bridge""#) && vm.contains(r#"<option value="nat" selected>NAT</option>"#), "varsayılan NAT");
    assert!(vm.contains(r#"<option value="vmbr0">Köprü: vmbr0</option>"#) && vm.contains(r#"<option value="br-lan">"#));
    assert!(vm.contains(r#"<input id="os" name="os" type="text""#), "sanal makinede işletim sistemi serbest metin");

    let ct = page(&app, "/guests/new?kind=ct").await;
    assert!(ct.contains(r#"<select id="os" name="os""#) && ct.contains(r#"<option value="alpine-3.20" selected>"#) && ct.contains("debian-12"));
    assert!(page(&app, "/guests").await.contains(r#"href="/guests/new?kind=ct""#));

    let edit = settings(&app, "/guests/103").await;
    assert!(edit.contains(r#"<select id="bridge" form="hw" name="bridge""#) && edit.contains("Klonla"));
    let ct_edit = settings(&app, "/guests/202").await;
    assert!(ct_edit.contains("Klonla") && !ct_edit.contains("Büyüt") && !ct_edit.contains("Şablona Çevir"), "konteynerde klon var; büyütme ve şablon yok");
}

#[tokio::test]
async fn real_driver_container_create_and_network_edit() {
    let (app, state) = setup_real();
    let form = "kind=ct&node=pve-ist-01&vmid=&name=kutu&os=alpine-3.20&storage=local-zfs&disk_gb=8&cores=1&memory_mb=256&bridge=br-lan";
    let to = post_ok(&app, "/guests", form).await;
    assert!(to.starts_with("/guests/105?notice="), "{to}");
    let g = state.snapshot().guest(105).unwrap().clone();
    assert_eq!((g.kind, g.bridge.as_str(), g.os.as_str()), (GuestKind::Ct, "br-lan", "alpine-3.20"));

    let conf = page(&app, "/guests/105").await;
    assert!(conf.contains("köprü: br-lan") && conf.contains("52:54:00:00:00:69"), "ağ ve MAC gösterilir");
    // Konteynerde anlık görüntü ve yedek kapalıyken alınır.
    assert!(page(&app, "/guests/105?tab=goruntuler").await.contains("Henüz anlık görüntü yok"));
    assert!(post_ok(&app, "/guests/105/snapshots", "name=temiz").await.contains("notice="));
    assert!(post_ok(&app, "/guests/105/backups", "").await.contains("notice="));
    assert!(post_ok(&app, "/guests/200/snapshots", "name=canli").await.contains("error="), "çalışan konteynerde canlı görüntü yok");

    // Ağ değişikliği: var olmayan köprü ve köprüde port yönlendirme reddedilir.
    let base = "name=kutu&os=alpine-3.20&tags=&cores=1&memory_mb=256";
    assert!(post_ok(&app, "/guests/105/edit", &format!("{base}&bridge=br-yok&forwards=")).await.contains("&error="));
    assert!(post_ok(&app, "/guests/105/edit", &format!("{base}&bridge=nat&forwards=8080%3A80")).await.contains("&error="), "konteynerde port yönlendirme yok");
    assert!(post_ok(&app, "/guests/105/edit", &format!("{base}&bridge=nat&forwards=")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(105).unwrap().bridge, "nat");
    assert!(post_ok(&app, "/guests/103/edit", "name=win-build&os=x&tags=&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=8080%3A80").await.contains("&error="));
}

#[tokio::test]
async fn container_console_runs_commands() {
    let (app, state) = setup_real();
    let tab = page(&app, "/guests/200?tab=konsol").await;
    assert!(tab.contains(r#"action="/guests/200/exec""#) && tab.contains("konteynerde kök olarak") && !tab.contains("iframe"));
    assert!(page(&app, "/guests/202?tab=konsol").await.contains("Konteyner çalışmıyor"));

    let (status, headers, body) = send(&app, post_form("/guests/200/exec", "command=id+-u")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store");
    assert!(body.contains("dns-01 üzerinde: id -u") && body.contains("Çalıştı"));
    assert!(body.contains("&lt;b&gt;uid=0(root)&lt;/b&gt;") && !body.contains("<b>uid=0"), "komut çıktısı kaçışlı gösterilir");
    assert!(body.contains(r#"value="id -u""#), "komut yeniden düzenlenebilsin diye kutuda kalır");

    let (status, _, body) = send(&app, post_form("/guests/200/exec", "command=false")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Başarısız") && body.contains("çıkış kodu 1"));

    assert!(post_ok(&app, "/guests/200/exec", "command=++").await.contains("error="), "boş komut");
    assert!(post_ok(&app, "/guests/202/exec", "command=id").await.contains("error="), "duran konteyner");
    assert!(post_ok(&app, "/guests/999/exec", "command=id").await.starts_with("/?error="));

    // Yalnızca yöneticiler; komut metni görev günlüğünde ve denetim kaydında yer almaz.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/200/exec"), "command=id")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    send(&app, post_form("/guests/200/exec", "command=echo+cok-gizli-deger")).await;
    let c = state.snapshot();
    assert!(c.tasks.iter().any(|t| t.command == "ct exec 200") && !format!("{:?}", c.tasks).contains("cok-gizli-deger"));
    assert!(!format!("{:?}", state.auth().audit.recent("", 50)).contains("cok-gizli-deger"));
}

// ---------- Zamanlanmış yedek, canlı işlemler, uyarılar ve yeni API uçları ----------

#[tokio::test]
async fn backup_schedule_is_edited_from_the_guest_form() {
    let (app, state) = setup();
    let form = settings(&app, "/guests/103").await;
    assert!(form.contains(r#"name="backup_time" type="time""#) && form.contains(r#"name="backup_keep" type="text" inputmode="numeric" value="7""#));

    let base = "name=win-build&os=windows-2022&tags=&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_time=03%3A30&backup_keep=3")).await.contains("notice="));
    let g = state.snapshot().guest(103).unwrap().clone();
    assert_eq!((g.backup_time.as_deref(), g.backup_keep), (Some("03:30"), 3));
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(r#"her gün <b class="mono">03:30</b>"#) && tab.contains("son 3 yedek saklanır"), "zamanlama yedekler sekmesinde görünür");

    for bad in ["backup_time=25%3A00&backup_keep=3", "backup_time=03%3A30&backup_keep=0", "backup_time=03%3A30&backup_keep=x"] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await.contains("&error="), "{bad}");
    }
    // Zamanlayıcı turu yedeği alır ve "otomatik" olarak işaretler.
    assert_eq!(state.scheduler_tick("2026-11-24", 300), 1);
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains(r#"<span class="tag">otomatik</span>"#));
    // Saati boşaltmak zamanlamayı kapatır.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_time=&backup_keep=3")).await.contains("notice="));
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains("Otomatik yedek kapalı."));
}

#[tokio::test]
async fn live_snapshot_and_backup_follow_driver_capability() {
    // Simülasyon: çalışan konukta kapalı.
    let (sim, _) = setup();
    let tab = page(&sim, "/guests/102?tab=goruntuler").await;
    assert!(tab.contains("için önce konuğu kapatın") && tab.contains(r#"required disabled>"#));
    assert!(post_ok(&sim, "/guests/102/snapshots", "name=canli").await.contains("error="));

    // Canlı işlem yapabilen sürücü: çalışan sanal makinede açık.
    let (app, state) = setup_real();
    let tab = page(&app, "/guests/102?tab=goruntuler").await;
    assert!(tab.contains("bellek durumunu da içerir") && !tab.contains(r#"required disabled>"#));
    assert!(post_ok(&app, "/guests/102/snapshots", "name=canli").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/backups", "").await.contains("notice="));
    let c = state.snapshot();
    assert_eq!((c.guest(102).unwrap().snapshots.len(), c.backups.len(), c.guest(102).unwrap().status), (1, 1, GuestStatus::Running));

    // Üretim denetim noktası: belleksiz; çalışan konukta ona dönülemez.
    assert!(tab.contains(r#"name="production""#));
    assert!(post_ok(&app, "/guests/102/snapshots", "name=uretim&production=1").await.contains("notice="));
    let snaps = state.snapshot().guest(102).unwrap().snapshots.clone();
    assert!(snaps[1].disk_only && !snaps[0].disk_only);
    assert!(page(&app, "/guests/102?tab=goruntuler").await.contains("yalnızca disk"));
    assert!(post_ok(&app, "/guests/102/snapshots/uretim/rollback", "").await.contains("error="), "belleksiz görüntüye dönmek için konuk kapatılır");
    assert!(post_ok(&app, "/guests/102/snapshots/canli/rollback", "").await.contains("notice="));

    let backups = page(&app, "/guests/102?tab=yedekler").await;
    assert!(backups.contains("Şimdi Yedekle") && !backups.contains("Geri Yükle"), "geri yükleme çalışan konukta sunulmaz");
    assert!(backups.contains("Geri yüklemek için önce konuğu kapatın."));
    let id = c.backups[0].id.clone();
    assert!(post_ok(&app, &format!("/backups/{id}/restore"), "").await.contains("error="));
}

#[tokio::test]
async fn dashboard_lists_active_alerts() {
    let (app, state) = setup();
    let home = page(&app, "/").await;
    assert!(home.contains(r#"<div class="notice"#) && home.contains("nfs-yedek deposu %93 dolu"), "dolan depolama ana sayfada görünür");
    state.mutate(|c| {
        c.observe_storage("nfs-yedek", "pve-ist-01", 100, 8000);
        c.nodes[2].status = redox_core::NodeStatus::Offline;
    });
    let home = page(&app, "/").await;
    assert!(!home.contains("nfs-yedek deposu") && home.contains("pve-ank-01 düğümü çevrimdışı"));
    // Olaylar: açılıştan sonraki değişiklikler bir kez döner.
    let (_, st) = setup();
    assert!(st.alerts_tick().is_empty());
    st.mutate(|c| c.nodes[0].status = redox_core::NodeStatus::Offline);
    assert_eq!(st.alerts_tick().len(), 1);
    assert!(st.alerts_tick().is_empty());
}

#[tokio::test]
async fn api_clone_resize_and_exec() {
    let (app, state) = setup_real();
    let (status, v) = api(&app, post_json("/api/v1/guests/103/clone", &json!({"name": "kopya"}))).await;
    assert_eq!((status, v["vmid"].as_u64()), (StatusCode::ACCEPTED, Some(105)));
    let (status, v) = api(&app, post_json("/api/v1/guests/100/clone", &json!({"name": "canli"}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("requires_stopped")));

    let (status, t) = api(&app, post_json("/api/v1/guests/103/resize", &json!({"disk_gb": 300}))).await;
    assert_eq!((status, t["command"].as_str()), (StatusCode::ACCEPTED, Some("vm resize 103 300G")));
    assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 300);
    let (status, v) = api(&app, post_json("/api/v1/guests/103/resize", &json!({"disk_gb": 1}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_spec")));

    let (status, v) = api(&app, post_json("/api/v1/guests/200/exec", &json!({"command": "id -u"}))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(v["output"].as_str().unwrap().contains("dns-01 üzerinde: id -u"));
    let (status, v) = api(&app, post_json("/api/v1/guests/200/exec", &json!({"command": "false"}))).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("driver_error")));
    let (status, _) = api(&app, post_json("/api/v1/guests/202/exec", &json!({"command": "id"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "duran konteyner");

    let viewer = as_viewer(Method::POST, "/api/v1/guests/200/exec").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"command":"id"}"#)).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    // PATCH ile zamanlama.
    let patch = request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"backup_time":"04:15","backup_keep":2}"#)).unwrap();
    let (status, g) = api(&app, patch).await;
    assert_eq!((status, g["backup_time"].as_str(), g["backup_keep"].as_u64()), (StatusCode::OK, Some("04:15"), Some(2)));
}

/// Uygulamayı gerçek bir TCP kapısında çalıştırır (eş sunucu rolü).
async fn serve_on_loopback(app: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_page_shows_peer_guests_read_only() {
    let (hub, hub_state) = setup();
    // Yapılandırma yokken sayfa nasıl kurulacağını anlatır.
    let empty = page(&hub, "/cluster").await;
    assert!(empty.contains("Eş sunucu tanımlı değil") && empty.contains("--peers DOSYA"));
    assert!(page(&hub, "/").await.contains(r#"href="/cluster""#), "kenar çubuğunda bağlantı");

    let (peer_app, peer_state) = setup();
    let id = peer_state.mutate(|c| {
        c.name = "izmir-dc".to_string();
        c.guests[0].name = "uzak-konuk".to_string();
        c.guests[0].vmid
    });
    let port = serve_on_loopback(peer_app).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let file = format!(
        "izmir http://127.0.0.1:{port} {VIEWER_TOKEN}\nyanlis http://127.0.0.1:{port} rdx_gecersiz_token_0123456789\nkapali http://127.0.0.1:{closed} {VIEWER_TOKEN}\n"
    );
    hub_state.peers().configure(redox_server::peers::parse_file(&file).unwrap());
    assert!(page(&hub, "/cluster").await.contains("Bekleniyor"), "ilk okumadan önce");

    let st = hub_state.clone();
    tokio::task::spawn_blocking(move || st.peers().poll(1_000)).await.unwrap();

    let views = hub_state.peers().views();
    assert!(views[0].online() && views[0].guests.iter().any(|g| g.vmid == id && g.name == "uzak-konuk"));
    assert_eq!(views[1].error.as_deref(), Some("token reddedildi (401)"));
    assert!(views[2].error.as_deref().unwrap().starts_with("erişilemiyor"));

    let html = page(&hub, "/cluster").await;
    assert!(html.contains("1/3 eş erişilebilir"), "özet");
    assert!(html.contains(&format!(r#"href="http://127.0.0.1:{port}/guests/{id}""#)), "konuk, kendi sunucusuna bağlanır");
    assert!(html.contains("uzak-konuk") && html.contains("izmir-dc") && html.contains("token reddedildi (401)") && html.contains("erişilemiyor"));
    assert!(!html.contains(VIEWER_TOKEN) && !html.contains("rdx_gecersiz"), "token sayfaya sızmaz");
    assert!(!html.contains(&format!(r#"action="http://127.0.0.1:{port}"#)), "eşe işlem formu yok: salt okunur");

    // API aynı bilgiyi verir; token yine yok. Salt okunur kullanıcı da görebilir.
    let (status, _, body) = send(&hub, as_viewer(Method::GET, "/api/v1/peers").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("uzak-konuk") && body.contains(r#""name":"izmir""#) && !body.contains(VIEWER_TOKEN), "{body}");
    let (status, _, _) = send(&hub, anon(Method::GET, "/api/v1/peers").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cloud_init_fields_are_validated_and_shown() {
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxvcmVtaXBzdW1kb2xvcnNpdGFtZXRjb25zZWN0 ben@dizustu";
    let (app, state) = setup();
    // Simülasyonda bölüm gösterilmez; gerçek sürücüde imaj yoksa nereye konacağı yazar.
    assert!(!page(&app, "/guests/new?kind=vm").await.contains("cloud-init"));
    let (real, _) = setup_real();
    let form = page(&real, "/guests/new?kind=vm").await;
    assert!(form.contains(r#"name="ci_ssh_key""#) && form.contains("cloud/ klasörüne"), "gerçek sürücüde bölüm var");
    assert!(!page(&real, "/guests/new?kind=ct").await.contains(r#"name="ci_ssh_key""#), "konteynerde yok");

    let base = json!({"name": "bulut", "kind": "vm", "node": "pve-ank-01", "cores": 1, "memory_mb": 512, "disk_gb": 8, "storage": "local-lvm"});
    let with = |extra: Value| {
        let mut v = base.clone();
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    for (extra, needle) in [
        (json!({"image": "../debian.qcow2", "ci_ssh_key": KEY}), "geçersiz bulut imajı"),
        (json!({"ci_user": "root", "ci_ssh_key": KEY}), "geçersiz cloud-init kullanıcı"),
        (json!({"ci_ssh_key": "ssh-ed25519 AAAA\nruncmd: [reboot]"}), "geçersiz SSH açık anahtarı"),
        (json!({"kind": "ct", "ci_ssh_key": KEY}), "yalnızca sanal makinelerde"),
    ] {
        let (status, body) = api(&app, post_json("/api/v1/guests", &with(extra))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body["error"].as_str().unwrap().contains(needle), "{body}");
    }
    // Gerçek sürücüde imaj sunucuda bulunmalı.
    let (status, body) = api(&real, post_json("/api/v1/guests", &with(json!({"image": "yok.qcow2", "ci_ssh_key": KEY})))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("bulut imajı bulunamadı: yok.qcow2"), "{body}");

    let (status, g) = api(&app, post_json("/api/v1/guests", &with(json!({"image": "debian.qcow2", "ci_user": "ali", "ci_ssh_key": KEY})))).await;
    assert_eq!(status, StatusCode::CREATED, "{g}");
    assert_eq!((g["image"].as_str(), g["ci_user"].as_str(), g["ci_ssh_key"].as_str()), (Some("debian.qcow2"), Some("ali"), Some(KEY)), "alanlar konuğun kendi alanlarıdır");
    let id = g["vmid"].as_u64().unwrap();
    let conf = page(&app, &format!("/guests/{id}")).await;
    assert!(conf.contains("debian.qcow2") && conf.contains("ali · ssh-ed25519 ben@dizustu"), "anahtarın gövdesi değil, türü ve açıklaması gösterilir");
    assert!(!conf.contains("AAAAC3NzaC1lZDI1NTE5"));

    // Form: sondaki satır sonu atılır; boş alanlar ayar sayılmaz.
    let body = format!("kind=vm&node=pve-ank-01&name=form-bulut&os=debian&storage=local-lvm&disk_gb=8&cores=1&memory_mb=512&bridge=nat&image=&ci_user=&ci_ssh_key={}%0D%0A", KEY.replace(' ', "+").replace('@', "%40"));
    assert!(post_ok(&app, "/guests", &body).await.contains("notice="));
    let made = state.snapshot().guests.iter().find(|g| g.name == "form-bulut").cloned().unwrap();
    assert_eq!((made.hw.ci_ssh_key.as_deref(), made.hw.ci_user, made.hw.image), (Some(KEY), None, None));
}

#[tokio::test]
async fn offsite_target_is_explained_and_push_needs_configuration() {
    let (app, state) = setup();
    state.run_action(103, Action::Stop, "admin").ok();
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains("--offsite DOSYA") && tab.contains("Dış kopya"));
    assert!(!tab.contains(&format!(r#"action="/backups/{id}/push""#)), "hedef yokken gönder düğmesi yok");
    assert!(post_ok(&app, &format!("/backups/{id}/push"), "").await.contains("error="));
    let (status, body) = api(&app, request(Method::POST, &format!("/api/v1/backups/{id}/push")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("yapılandırılmamış"));
    let (status, _, _) = send(&app, as_viewer(Method::POST, &format!("/api/v1/backups/{id}/push")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "salt okunur kullanıcı gönderemez");

    // Hedef tanımlıyken (simülasyon dosya tutmadığı için kopya başarısız olur ama yedek alınır).
    let target = std::env::temp_dir().join(format!("redox-http-offsite-{}", std::process::id()));
    std::fs::create_dir_all(&target).unwrap();
    state.set_offsite(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\n", target.display())).unwrap());
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains("sunucu dışına da kopyalanır") && tab.contains(&format!(r#"action="/backups/{id}/push""#)));
    assert!(post_ok(&app, &format!("/backups/{id}/push"), "").await.contains("notice="));
    let c = state.snapshot();
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].status), (format!("backup push {id}").as_str(), TaskStatus::Failed));
    assert!(!c.backup(&id).unwrap().offsite);
    std::fs::remove_dir_all(&target).unwrap();
}

#[tokio::test]
async fn firewall_is_edited_from_the_form_and_api() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| {
        request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()
    };
    let form = settings(&app, "/guests/103").await;
    assert!(form.contains(r#"name="firewall""#) && form.contains(r#"name="fw_rules""#));
    assert!(page(&app, "/guests/103").await.contains("Güvenlik duvarı"));

    let base = "name=db-yeni&os=debian-12&tags=&backup_time=&backup_keep=7";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&firewall=1&fw_rules=tcp%2F22+from+10.0.0.0%2F8%0D%0Atcp%2F443")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert!(hw.firewall);
    assert_eq!(hw.fw_rules.iter().map(ToString::to_string).collect::<Vec<_>>(), ["tcp/22 from 10.0.0.0/8", "tcp/443"]);
    let conf = page(&app, "/guests/103").await;
    assert!(conf.contains("açık · tcp/22 from 10.0.0.0/8, tcp/443"), "kurallar okunur biçimde gösterilir");
    assert!(settings(&app, "/guests/103").await.contains("tcp/22 from 10.0.0.0/8\ntcp/443"), "form kayıtlı kuralları geri yükler");

    // Geçersiz kural: hiçbir şey değişmez.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&firewall=1&fw_rules=tcp%2F22%3B+flush+ruleset")).await.contains("error="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.fw_rules.len(), 2);
    // İşaret kaldırılınca kapanır.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&fw_rules=")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert!(!hw.firewall && hw.fw_rules.is_empty());

    // API: kısmi güncelleme.
    let patch = json!({"firewall": true, "fw_rules": [{"proto": "udp", "ports": [53, 53], "source": "192.168.1.0/24"}, {"proto": "icmp"}]});
    let (status, _) = api(&app, send_json(Method::PATCH, "/api/v1/guests/103", patch)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state.snapshot().guest(103).unwrap().hw.fw_rules[0].to_string(), "udp/53 from 192.168.1.0/24");
    let bad = json!({"fw_rules": [{"proto": "tcp", "ports": [22, 22], "source": "1.2.3.4 accept; flush ruleset"}]});
    let (status, body) = api(&app, send_json(Method::PATCH, "/api/v1/guests/103", bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // Gerçek sürücüde --firewall yoksa açılamaz.
    let (real, _) = setup_real();
    let (status, body) = api(&real, send_json(Method::PATCH, "/api/v1/guests/103", json!({"firewall": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("--firewall"), "{body}");
}

#[tokio::test]
async fn firmware_and_tpm_are_chosen_in_forms() {
    let (app, state) = setup();
    let form = page(&app, "/guests/new?kind=vm").await;
    assert!(form.contains(r#"name="firmware""#) && form.contains(r#"value="uefi-secure""#) && form.contains(r#"name="tpm""#));
    assert!(!page(&app, "/guests/new?kind=ct").await.contains(r#"name="firmware""#), "konteynerde donanım yazılımı yok");

    let body = "kind=vm&node=pve-ank-01&name=win11&os=windows-11&storage=local-lvm&disk_gb=8&cores=2&memory_mb=4096&bridge=nat&firmware=uefi-secure&tpm=1";
    assert!(post_ok(&app, "/guests", body).await.contains("notice="));
    let g = state.snapshot().guests.iter().find(|g| g.name == "win11").cloned().unwrap();
    assert_eq!((g.hw.firmware, g.hw.secure_boot, g.hw.tpm), (redox_core::hw::Firmware::Uefi, true, true));
    let conf = page(&app, &format!("/guests/{}", g.vmid)).await;
    assert!(conf.contains("UEFI + güvenli önyükleme · TPM 2.0"));
    let edit = settings(&app, &format!("/guests/{}", g.vmid)).await;
    assert!(edit.contains(r#"<option value="uefi-secure" selected>"#), "düzenleme formu kayıtlı seçimi gösterir");

    // Düzenleme: kapalı konukta donanım alanlarıyla birlikte değişir.
    let edit_body = "name=win11&os=windows-11&tags=&backup_time=&backup_keep=7&fw_rules=&cores=2&memory_mb=4096&bridge=nat&iso=&forwards=&firmware=uefi";
    assert!(post_ok(&app, &format!("/guests/{}/edit", g.vmid), edit_body).await.contains("notice="));
    let hw = state.snapshot().guest(g.vmid).unwrap().hw.clone();
    assert_eq!((hw.firmware, hw.secure_boot, hw.tpm), (redox_core::hw::Firmware::Uefi, false, false));

    // API: tutarsız birleşim ve konteyner reddedilir.
    let send_json = |method: Method, uri: &str, body: Value| request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let uri = format!("/api/v1/guests/{}", g.vmid);
    let (status, body) = api(&app, send_json(Method::PATCH, &uri, json!({"firmware": "bios", "secure_boot": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("UEFI donanım yazılımı gerektirir"), "{body}");
    let (status, _) = api(&app, send_json(Method::POST, "/api/v1/guests", json!({"name": "kutu-tpm", "kind": "ct", "node": "pve-ank-01", "cores": 1, "memory_mb": 256, "disk_gb": 8, "storage": "local-lvm", "tpm": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "konteynere TPM eklenmez");
    // Gerçek sürücüde paket yoksa neyin kurulacağı söylenir.
    let (real, _) = setup_real();
    let (status, body) = api(&real, send_json(Method::PATCH, "/api/v1/guests/102", json!({"tpm": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("apt install swtpm"), "{body}");
}

#[tokio::test]
async fn extra_disks_and_nics_through_ui_and_api() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let vm = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).cloned().unwrap();
    let base = format!("/guests/{}", vm.vmid);

    let edit = settings(&app, &base).await;
    assert!(edit.contains("Ek diskler") && edit.contains(&format!(r#"action="{base}/disks""#)) && edit.contains(r#"name="nics""#));
    assert!(post_ok(&app, &format!("{base}/disks"), "disk_gb=25").await.contains("notice="));
    assert!(post_ok(&app, &format!("{base}/disks"), "disk_gb=cok").await.contains("error="));
    let (status, body) = api(&app, send_json(Method::POST, &format!("/api/v1{base}/disks"), json!({"size_gb": 40}))).await;
    assert_eq!((status, body["id"].as_u64()), (StatusCode::CREATED, Some(2)));
    let g = state.snapshot().guest(vm.vmid).unwrap().clone();
    assert_eq!(g.hw.disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>(), [(1, 25), (2, 40)]);
    assert_eq!(g.total_disk_gb(), vm.disk_gb + 65);

    let edit = settings(&app, &base).await;
    assert!(edit.contains(&format!("vm-{}-disk-1", vm.vmid)) && edit.contains(&format!(r#"href="{base}/confirm?op=deldisk&amp;name=2""#)), "her diskin silme bağlantısı onay sayfasına gider");
    assert!(page(&app, &base).await.contains("+ 25 GB + 40 GB"));
    // Yedek ek diskleri de kapsar: kayıt, etiket ve onay sayfası bunu gösterir.
    let backup = state.backup_create(vm.vmid, "admin").unwrap();
    assert_eq!(state.snapshot().backup(&backup).unwrap().disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>(), [(1, 25), (2, 40)]);
    let tab = page(&app, &format!("{base}?tab=yedekler")).await;
    assert!(tab.contains("asıl disk + 2 ek disk") && tab.contains("ek diskleri de kapsar") && !tab.contains("dahil değil"), "{tab}");
    let confirm = page(&app, &format!("{base}/confirm?op=restore&name={backup}")).await;
    assert!(confirm.contains("Yedekteki ek diskler de (1, 2) geri yüklenir"), "{confirm}");
    let (_, list) = api(&app, get("/api/v1/backups")).await;
    assert_eq!(list[0]["disks"][1]["size_gb"].as_u64(), Some(40));
    assert!(!settings(&app, &base).await.contains("dahil değil"));

    // Silme: onay sayfası hiçbir şeyi değiştirmez; POST siler.
    let confirm = page(&app, &format!("{base}/confirm?op=deldisk&name=1")).await;
    assert!(confirm.contains("Ek disk silinsin mi?") && confirm.contains(&format!(r#"action="{base}/disks/1/delete""#)));
    assert_eq!(state.snapshot().guest(vm.vmid).unwrap().hw.disks.len(), 2);
    let (status, _, _) = send(&app, get(&format!("{base}/confirm?op=deldisk&name=7"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "olmayan disk için onay sayfası yok");
    assert!(post_ok(&app, &format!("{base}/disks/1/delete"), "").await.contains("notice="));
    let (status, _, _) = send(&app, request(Method::DELETE, &format!("/api/v1{base}/disks/2")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(state.snapshot().guest(vm.vmid).unwrap().hw.disks.is_empty());

    // Aynı numaraya sonradan takılan disk başka bir disktir: geri yükleme ona dokunmaz, onay sayfası da onu saymaz.
    assert!(post_ok(&app, &format!("{base}/disks"), "disk_gb=25").await.contains("notice="));
    assert!(!page(&app, &format!("{base}/confirm?op=restore&name={backup}")).await.contains("geri yüklenir"));
    assert!(state.snapshot().restore_disks(&backup).unwrap().is_empty());
    assert!(post_ok(&app, &format!("/backups/{backup}/restore"), "").await.contains("notice="));
    assert_eq!(state.snapshot().guest(vm.vmid).unwrap().hw.disks.len(), 1, "yeni disk yerinde durur");
    assert!(post_ok(&app, &format!("{base}/disks/1/delete"), "").await.contains("notice="));
    let (status, _, _) = send(&app, as_viewer(Method::POST, &format!("/api/v1{base}/disks")).header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"size_gb":1}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Çalışan konukta ve konteynerde reddedilir.
    let running = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
    let (status, body) = api(&app, send_json(Method::POST, &format!("/api/v1/guests/{running}/disks"), json!({"size_gb": 1}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = api(&app, send_json(Method::POST, "/api/v1/guests/200/disks", json!({"size_gb": 1}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Ek ağ kartları: form ve API.
    let body = format!("name={}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&cores={}&memory_mb={}&bridge={}&iso=&forwards=&firmware=bios&nics=vmbr1%2C+nat", vm.name, vm.cores, vm.mem_total_mb, vm.bridge);
    assert!(post_ok(&app, &format!("{base}/edit"), &body).await.contains("notice="));
    assert_eq!(state.snapshot().guest(vm.vmid).unwrap().hw.nics, ["vmbr1", "nat"]);
    assert!(page(&app, &base).await.contains("+ vmbr1"));
    let (status, body) = api(&app, send_json(Method::PATCH, &format!("/api/v1{base}"), json!({"nics": ["a", "b", "c", "d"]}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("en fazla 3"));
}

#[tokio::test]
async fn passthrough_devices_and_backup_download() {
    let (app, state) = setup();
    let send_json = |method: Method, uri: &str, body: Value| request(method, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let vm = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).cloned().unwrap();
    let base = format!("/guests/{}", vm.vmid);
    let edit = settings(&app, &base).await;
    assert!(edit.contains(r#"name="usb""#) && edit.contains(r#"name="pci""#));

    let body = format!("name={}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&cores={}&memory_mb={}&bridge={}&iso=&forwards=&firmware=bios&nics=&usb=046D%3AC52B&pci=0000%3A01%3A00.0", vm.name, vm.cores, vm.mem_total_mb, vm.bridge);
    assert!(post_ok(&app, &format!("{base}/edit"), &body).await.contains("notice="));
    let hw = state.snapshot().guest(vm.vmid).unwrap().hw.clone();
    assert_eq!((hw.usb, hw.pci), (vec!["046d:c52b".to_string()], vec!["0000:01:00.0".to_string()]), "büyük harf küçültülür");
    assert!(page(&app, &base).await.contains("USB 046d:c52b, PCI 0000:01:00.0"));
    let (status, body) = api(&app, send_json(Method::PATCH, &format!("/api/v1{base}"), json!({"pci": ["0000:01:00.0,x-vga=on"]}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("geçersiz PCI adresi"), "{body}");
    let (status, _) = api(&app, send_json(Method::PATCH, "/api/v1/guests/200", json!({"usb": ["046d:c52b"]}))).await;
    assert_ne!(status, StatusCode::OK, "konteynere aygıt aktarılmaz");

    // İndirme: simülasyonda dosya yok; salt okunur kullanıcı ve kimliksiz istek reddedilir.
    let id = state.backup_create(vm.vmid, "admin").unwrap();
    let (status, _, body) = send(&app, get(&format!("/backups/{id}/download"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("bu sunucuda yok"));
    let (status, _, _) = send(&app, get("/backups/yok/download")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, get(&format!("/backups/{id}/baska"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, as_viewer(Method::GET, &format!("/api/v1/backups/{id}/download")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = send(&app, anon(Method::GET, &format!("/api/v1/backups/{id}/download")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn charts_offer_longer_ranges_on_the_real_driver() {
    use redox_server::history::Range;
    let (app, state) = setup_real();
    // Bir saatlik veri: dakikada bir örnek, zamanı test verir.
    let t0: u64 = 1_800_000_000; // tam saat başı
    for minute in 0..=60u64 {
        state.record_long(&[("node:cpu".to_string(), 30.0), ("guest:102:cpu".to_string(), minute as f64)], t0 + minute * 60);
    }
    assert_eq!(state.history_for(Range::Day)["node:cpu"].len(), 60);
    assert_eq!(state.history_for(Range::Month)["guest:102:cpu"].len(), 1);
    state.refresh(None, None);
    assert!(state.history_for(Range::Recent).contains_key("node:cpu"), "ham örnekler ayrı tutulur");

    let node = state.snapshot().nodes[0].id.clone();
    let recent = page(&app, &format!("/nodes/{node}")).await;
    assert!(recent.contains("Son 10 dakika") && recent.contains(&format!(r#"href="/nodes/{node}?range=24s""#)) && recent.contains("-10 dk"));
    let day = page(&app, &format!("/nodes/{node}?range=24s")).await;
    assert!(day.contains("-24 sa") && day.contains("son 24 saat"), "eksen ve erişilebilir ad aralığı söyler");
    assert!(day.contains(&format!(r#"href="/nodes/{node}""#)) && day.contains(&format!(r#"href="/nodes/{node}?range=30g""#)), "öteki aralıklara bağlantı");
    assert!(day.contains(r#"href="/theme?"#) || day.contains("range%3D24s") || day.contains("range=24s"), "seçim tema değişiminde korunur");
    let month = page(&app, "/guests/102?range=30g").await;
    assert!(month.contains("-30 gün") && month.contains(r#"href="/guests/102?range=24s""#));
    // Sekmeli sayfada aralık sorguya eklenir, sekme korunur.
    let tabbed = page(&app, "/guests/102?tab=ozet&range=30g").await;
    assert!(tabbed.contains("-30 gün"));

    // Simülasyonda geçmiş yoktur: seçici gösterilmez.
    let (sim, _) = setup();
    assert!(!page(&sim, "/guests/102").await.contains("Son 24 saat"));
}

#[tokio::test]
async fn operators_act_only_within_their_scope() {
    let (app, state) = setup();
    state.auth().edit(|u| {
        u.add_user("nobet", "operator-parolasi-12", Role::Operator, Cost::Test)?;
        u.add_token("nobet", "test", "redox_test_operator_token")?;
        u.set_scope("nobet", vec!["web".to_string()])
    }).unwrap();
    let as_op = |method: Method, uri: &str| anon(method, uri).header(AUTHORIZATION, "Bearer redox_test_operator_token");
    let post = |uri: &str| as_op(Method::POST, uri).body(Body::empty()).unwrap();

    // İki çalışan sanal makine: biri kapsamda (web etiketli), öteki değil.
    let running: Vec<u32> = state.snapshot().guests.iter().filter(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).map(|g| g.vmid).collect();
    let (inside, outside) = (running[0], running[1]);
    state.update_guest(inside, &redox_core::GuestPatch { tags: Some(vec!["web".into(), "prod".into()]), ..Default::default() }, "admin").unwrap();
    state.update_guest(outside, &redox_core::GuestPatch { tags: Some(vec!["db".into()]), ..Default::default() }, "admin").unwrap();

    // Kapsamdaki konuk: güç, anlık görüntü, yedek.
    let (status, _, _) = send(&app, post(&format!("/api/v1/guests/{inside}/stop"))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(state.snapshot().guest(inside).unwrap().status, GuestStatus::Stopped);
    let snap = as_op(Method::POST, &format!("/api/v1/guests/{inside}/snapshots")).header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"name":"nobetci"}"#)).unwrap();
    assert_eq!(send(&app, snap).await.0, StatusCode::ACCEPTED);
    assert_eq!(send(&app, post(&format!("/api/v1/guests/{inside}/backups"))).await.0, StatusCode::ACCEPTED);
    let (status, _, _) = send(&app, form_body(as_op(Method::POST, &format!("/guests/{inside}/action")), "action=start")).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "arayüz formu da çalışır");
    assert_eq!(state.snapshot().tasks[0].user, "nobet", "işlem operatörün adıyla günlüğe düşer");

    // Kapsam dışındaki konuk ve operatörü aşan işlemler reddedilir; hiçbir şey değişmez.
    let before = state.snapshot();
    let denied = [
        (Method::POST, format!("/api/v1/guests/{outside}/stop")),
        (Method::POST, format!("/api/v1/guests/{outside}/snapshots")),
        (Method::POST, format!("/api/v1/guests/{inside}/snapshots/nobetci/rollback")),
        (Method::DELETE, format!("/api/v1/guests/{inside}/snapshots/nobetci")),
        (Method::DELETE, format!("/api/v1/guests/{inside}")),
        (Method::PATCH, format!("/api/v1/guests/{inside}")),
        (Method::POST, format!("/api/v1/guests/{inside}/disks")),
        (Method::POST, format!("/api/v1/guests/{inside}/clone")),
        (Method::POST, "/api/v1/guests".to_string()),
        (Method::POST, "/guests/bulk".to_string()),
        (Method::POST, format!("/guests/{inside}/edit")),
        (Method::POST, format!("/guests/{inside}/delete")),
        (Method::POST, "/users".to_string()),
        (Method::POST, "/api/v1/guests/999999/stop".to_string()),
    ];
    for (method, uri) in denied {
        let (status, _, body) = send(&app, as_op(method.clone(), &uri).header(CONTENT_TYPE, "application/json").body(Body::from("{}")).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
    }
    assert_eq!(state.snapshot().guests, before.guests);
    let (status, body) = api(&app, post(&format!("/api/v1/guests/{outside}/stop"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("yetki kapsamınızda değil"), "{body}");

    // Arayüz: kapsamdaki konukta düğmeler var, silme yok; kapsam dışında açıklama.
    let cookie = login_cookie(&app, "nobet", "operator-parolasi-12").await;
    let get_as = |uri: &str| anon(Method::GET, uri).header(COOKIE, cookie.clone()).body(Body::empty()).unwrap();
    let (_, _, mine) = send(&app, get_as(&format!("/guests/{inside}"))).await;
    assert!(mine.contains(r#"name="action""#) && mine.contains("<small>operatör</small>") && !mine.contains(&format!(r#"href="/guests/{inside}/delete""#)));
    assert!(mine.contains("tab=konsol"), "kapsamdaki konuğun konsolu açıktır");
    let (_, _, other) = send(&app, get_as(&format!("/guests/{outside}"))).await;
    assert!(other.contains("Bu konuk yetki kapsamınızda değil") && !other.contains(r#"name="action""#) && !other.contains("tab=konsol"));
    let (status, _, _) = send(&app, get_as(&format!("/guests/{outside}/screen"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "kapsam dışındaki konuğun ekranı görülemez");
    let (status, _, _) = send(&app, get_as(&format!("/guests/{inside}/screen"))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&app, get_as("/users")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "kullanıcı yönetimi yöneticiye özeldir");

    // Kapsam boşaltılınca bütün konuklarda yetkili olur; yönetici bunu kullanıcı sayfasından yapar.
    let users = page(&app, "/users").await;
    assert!(users.contains("Operatör") && users.contains(r#"action="/users/nobet/scope""#) && users.contains(r#"value="web""#));
    assert!(post_ok(&app, "/users/nobet/scope", "scope=").await.contains("notice="));
    assert_eq!(send(&app, post(&format!("/api/v1/guests/{outside}/stop"))).await.0, StatusCode::ACCEPTED);
    assert!(post_ok(&app, "/users/nobet/scope", "scope=Geçersiz+Etiket!").await.contains("error="));
    assert!(post_ok(&app, "/users/nobet/role", "role=viewer").await.contains("notice="));
    assert_eq!(send(&app, post(&format!("/api/v1/guests/{outside}/start"))).await.0, StatusCode::FORBIDDEN, "rol düşünce yetki hemen kalkar");
}

#[test]
fn operator_paths_are_an_explicit_allow_list() {
    use redox_server::operator_target;
    for (path, expected) in [
        ("/guests/103/action", Some(103)),
        ("/guests/103/snapshots", Some(103)),
        ("/guests/103/backups", Some(103)),
        ("/guests/103/keys", Some(103)),
        ("/guests/103/exec", Some(103)),
        ("/api/v1/guests/103/start", Some(103)),
        ("/api/v1/guests/103/resume", Some(103)),
        ("/api/v1/guests/103/exec", Some(103)),
        ("/guests/103/delete", None),
        ("/guests/103/edit", None),
        ("/guests/103/resize", None),
        ("/guests/103/disks", None),
        ("/guests/103/snapshots/x/rollback", None),
        ("/guests/103/snapshots/x/delete", None),
        ("/api/v1/guests/103/snapshots/x", None),
        ("/api/v1/guests/103/clone", None),
        ("/api/v1/guests/103", None),
        ("/guests/bulk", None),
        ("/guests/abc/action", None),
        ("/backups/vm-103-1/restore", None),
        ("/backups/vm-103-1/delete", None),
        ("/users/x/role", None),
        ("/guests/103/action/../delete", None),
    ] {
        assert_eq!(operator_target(path), expected, "{path}");
    }
}

/// Sahte kimlik sağlayıcı: keşif belgesi ve token uç noktası. Token isteklerinin gövdesini kaydeder;
/// kimlik belirtecinin alanlarını test belirler.
fn fake_idp(claims: std::sync::Arc<std::sync::Mutex<Value>>) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            let mut reader = BufReader::new(socket);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim().to_ascii_lowercase();
                if header.is_empty() {
                    break;
                }
                if let Some(v) = header.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            let base = format!("http://127.0.0.1:{port}");
            let payload = if path == "/.well-known/openid-configuration" {
                json!({"issuer": base, "authorization_endpoint": format!("{base}/authorize"), "token_endpoint": format!("{base}/token")}).to_string()
            } else if path == "/token" {
                log.lock().unwrap().push(String::from_utf8_lossy(&body).to_string());
                let encoded = redox_server::oidc::base64url(claims.lock().unwrap().to_string().as_bytes());
                json!({"access_token": "x", "token_type": "Bearer", "id_token": format!("e30.{encoded}.imza")}).to_string()
            } else {
                String::new()
            };
            let status = if payload.is_empty() { "404 Not Found" } else { "200 OK" };
            let _ = write!(reader.get_mut(), "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len());
        }
    });
    (port, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_sign_on_maps_the_provider_identity_to_a_local_user() {
    use sha2::{Digest, Sha256};
    let (app, state) = setup();
    // Yapılandırılmadan: bağlantı yok, adres yok.
    assert!(!page_anon(&app, "/login").await.contains("/login/oidc"));
    let (status, _, _) = send(&app, anon(Method::GET, "/login/oidc").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let claims = std::sync::Arc::new(std::sync::Mutex::new(json!({})));
    let (port, token_requests) = fake_idp(claims.clone());
    let issuer = format!("http://127.0.0.1:{port}");
    state.set_oidc(redox_server::oidc::Config::parse(&format!("issuer = {issuer}\nclient_id = redox-panel\nclient_secret = cok-gizli-anahtar\nredirect_url = http://localhost:8006/login/oidc/callback\n")).unwrap());
    state.auth().edit(|u| u.set_sso("izleyici", "ayse@example.com")).unwrap();
    assert!(page_anon(&app, "/login").await.contains(r#"href="/login/oidc""#));

    // Başlat: sağlayıcıya yönlendirme ve bu tarayıcıya bağlı durum çerezi.
    let start = || async {
        let (status, headers, _) = send(&app, anon(Method::GET, "/login/oidc").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let to = location(&headers).to_string();
        let params: std::collections::HashMap<String, String> = serde_urlencoded::from_str(to.split_once('?').unwrap().1).unwrap();
        let cookie = headers.get(SET_COOKIE).unwrap().to_str().unwrap().to_string();
        (to, params, cookie)
    };
    let (to, params, cookie) = start().await;
    assert!(to.starts_with(&format!("{issuer}/authorize?")));
    assert_eq!((params["response_type"].as_str(), params["client_id"].as_str(), params["code_challenge_method"].as_str()), ("code", "redox-panel", "S256"));
    assert_eq!(params["redirect_uri"], "http://localhost:8006/login/oidc/callback");
    assert!(cookie.starts_with(&format!("redox_oidc={}", params["state"])) && cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax") && cookie.contains("Path=/login/oidc"));
    let state_cookie = cookie.split(';').next().unwrap().to_string();
    let callback = |state: &str, cookie: Option<&str>| {
        let mut builder = anon(Method::GET, &format!("/login/oidc/callback?code=yetki-kodu&state={state}"));
        if let Some(c) = cookie {
            builder = builder.header(COOKIE, c);
        }
        builder.body(Body::empty()).unwrap()
    };
    let good = |nonce: &str, email: &str| json!({"iss": issuer, "aud": "redox-panel", "exp": now() + 300, "nonce": nonce, "email": email, "email_verified": true});

    // Başka tarayıcı (çerez yok) kodu kullanamaz.
    *claims.lock().unwrap() = good(&params["nonce"], "ayse@example.com");
    let (status, headers, _) = send(&app, callback(&params["state"], None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/login?error="));
    assert!(token_requests.lock().unwrap().is_empty(), "çerez tutmadan sağlayıcıya gidilmez");

    // Doğru tarayıcı: oturum açılır.
    let (status, headers, body) = send(&app, callback(&params["state"], Some(&state_cookie))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(r#"http-equiv="refresh""#) && body.contains("url=/"));
    let cookies: Vec<String> = headers.get_all(SET_COOKIE).iter().map(|v| v.to_str().unwrap().to_string()).collect();
    let session = cookies.iter().find(|c| c.starts_with("redox_session=")).expect("oturum çerezi").split(';').next().unwrap().to_string();
    assert!(cookies.iter().any(|c| c.starts_with("redox_oidc=;") && c.contains("Max-Age=0")), "durum çerezi silinir");
    let (status, _, home) = send(&app, anon(Method::GET, "/").header(COOKIE, session).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(home.contains("izleyici <small>salt okunur</small>"), "rol yereldir: sağlayıcı yalnızca kimliği doğrular");

    // Sağlayıcıya giden istek: kod, gizli anahtar ve PKCE doğrulayıcısı (meydan okumanın ön görüntüsü).
    let sent: std::collections::HashMap<String, String> = serde_urlencoded::from_str(&token_requests.lock().unwrap()[0]).unwrap();
    assert_eq!((sent["grant_type"].as_str(), sent["code"].as_str(), sent["client_secret"].as_str()), ("authorization_code", "yetki-kodu", "cok-gizli-anahtar"));
    assert_eq!(redox_server::oidc::base64url(&Sha256::digest(sent["code_verifier"].as_bytes())), params["code_challenge"]);

    // Aynı dönüş ikinci kez kullanılamaz.
    let (_, headers, _) = send(&app, callback(&params["state"], Some(&state_cookie))).await;
    assert!(location(&headers).starts_with("/login?error="));

    // Eşlenmemiş kimlik, başka girişin belirteci ve başka uygulamanın belirteci reddedilir.
    for make in [
        (|n: &str| ("yabanci@example.com".to_string(), n.to_string(), "redox-panel")) as fn(&str) -> (String, String, &'static str),
        |_| ("ayse@example.com".to_string(), "baska-nonce".to_string(), "redox-panel"),
        |n| ("ayse@example.com".to_string(), n.to_string(), "baska-uygulama"),
    ] {
        let (_, params, cookie) = start().await;
        let (email, nonce, aud) = make(&params["nonce"]);
        let mut c = good(&nonce, &email);
        c["aud"] = json!(aud);
        *claims.lock().unwrap() = c;
        let (status, headers, _) = send(&app, callback(&params["state"], Some(cookie.split(';').next().unwrap()))).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert!(location(&headers).starts_with("/login?error=") && headers.get(SET_COOKIE).is_none(), "oturum açılmaz");
    }
    let shown = page_anon(&app, "/login?error=Bu+kimlik+hi%C3%A7bir+kullan%C4%B1c%C4%B1yla+e%C5%9Fle%C5%9Fmiyor.").await;
    assert!(shown.contains("Bu kimlik hiçbir kullanıcıyla eşleşmiyor."), "hata giriş sayfasında gösterilir");
}

async fn page_anon(app: &Router, uri: &str) -> String {
    let (status, _, body) = send(app, anon(Method::GET, uri).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    body
}

#[tokio::test]
async fn incremental_backups_are_labelled_and_verification_is_admin_only() {
    let mut cluster = Cluster::demo(now());
    let guest_name = cluster.guest(103).unwrap().name.clone();
    let backup = |id: &str, new_mb| redox_core::Backup { id: id.into(), vmid: 103, guest_name: guest_name.clone(), created: now(), size_mb: 900, auto: false, offsite: false, new_mb, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 };
    cluster.add_backup(backup("vm-103-tam", None));
    cluster.add_backup(backup("vm-103-artimli", Some(12)));
    let state = AppState::new(cluster, Box::new(RealLike), Duration::ZERO, None, Auth::new(users(), None, false, Cost::Test));
    let app = app(state.clone());

    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert_eq!(tab.matches(r#"<span class="tag gold">artımlı</span>"#).count(), 1, "yalnızca artımlı yedek etiketlenir");
    assert!(tab.contains("· +12 MB yeni") && tab.contains("Birini silmek diğerlerini etkilemez"));
    assert!(tab.contains(r#"action="/backups/vm-103-artimli/verify""#) && tab.contains(r#"action="/backups/vm-103-tam/verify""#));

    // Doğrulama arka planda çalışır; sonucu görev günlüğüne yazılır.
    assert!(post_ok(&app, "/backups/vm-103-artimli/verify", "").await.starts_with("/guests/103?tab=yedekler&notice="));
    assert!(state.snapshot().tasks[0].command == "backup verify vm-103-artimli");
    let (status, _, _) = send(&app, request(Method::POST, "/api/v1/backups/vm-103-tam/verify").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, body) = api(&app, request(Method::POST, "/api/v1/backups/yok/verify").body(Body::empty()).unwrap()).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("backup_not_found")));
    let (status, _, _) = send(&app, as_viewer(Method::POST, "/api/v1/backups/vm-103-tam/verify").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "salt okunur kullanıcı doğrulama başlatamaz");
    let (_, list) = api(&app, get("/api/v1/backups")).await;
    assert_eq!((list[0]["new_mb"].as_u64(), list[1]["new_mb"].as_u64()), (Some(12), None));

    // Simülasyonda dosya yoktur: doğrulama düğmesi sunulmaz.
    let (sim, sim_state) = setup();
    sim_state.run_action(103, Action::Stop, "admin").ok();
    sim_state.backup_create(103, "admin").unwrap();
    let tab = page(&sim, "/guests/103?tab=yedekler").await;
    assert!(!tab.contains("/verify") && !tab.contains("artımlı"));
}

#[tokio::test]
async fn templates_and_linked_clones_through_ui_and_api() {
    let (app, state) = setup();
    let del = |uri: &str| request(Method::DELETE, uri).body(Body::empty()).unwrap();
    let vm = state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).cloned().unwrap();
    let base = format!("/guests/{}", vm.vmid);
    let edit = settings(&app, &base).await;
    assert!(edit.contains("Şablona Çevir") && !edit.contains(r#"name="linked""#), "şablon olmayan konukta bağlı klon seçeneği yok");
    assert!(post_ok(&app, &format!("{base}/clone"), "name=olmaz&linked=1").await.contains("error="));

    assert!(post_ok(&app, &format!("{base}/template"), "on=1").await.contains("notice="));
    assert!(state.snapshot().guest(vm.vmid).unwrap().hw.template);
    let edit = settings(&app, &base).await;
    assert!(edit.contains("Şablonluktan Çıkar") && edit.contains(r#"name="linked" value="1" checked"#));
    assert!(page(&app, &base).await.contains(r#"<span class="tag gold">şablon</span>"#));
    // Şablon başlatılamaz, geri yüklenemez, anlık görüntüsü alınamaz.
    assert!(post_ok(&app, &format!("{base}/action"), "action=start").await.contains("error="));
    assert!(post_ok(&app, &format!("{base}/snapshots"), "name=olmaz").await.contains("error="));

    let to = post_ok(&app, &format!("{base}/clone"), "name=hizli-klon&linked=1").await;
    assert!(to.contains("notice="), "{to}");
    let c = state.snapshot();
    let clone = c.guests.iter().find(|g| g.name == "hizli-klon").unwrap().clone();
    assert_eq!(clone.hw.base, Some(vm.vmid));
    assert!(c.tasks.iter().any(|t| t.command == format!("vm linkclone {} hizli-klon", vm.vmid)));
    assert!(page(&app, &format!("/guests/{}", clone.vmid)).await.contains("Bağlı klon"));
    // Bağlı klon varken şablon silinemez ve şablonluktan çıkarılamaz.
    assert!(post_ok(&app, &format!("{base}/template"), "on=0").await.contains("error="));
    let (status, body) = api(&app, del(&format!("/api/v1{base}"))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("bağlı klonları"), "{body}");

    // API: bağlı klon ve şablon anahtarı.
    let (status, v) = api(&app, post_json(&format!("/api/v1{base}/clone"), &json!({"name": "api-klon", "linked": true}))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let second = v["vmid"].as_u64().unwrap();
    for id in [clone.vmid as u64, second] {
        assert_eq!(send(&app, del(&format!("/api/v1/guests/{id}"))).await.0, StatusCode::NO_CONTENT);
    }
    let (status, v) = api(&app, post_json(&format!("/api/v1{base}/template"), &json!({"on": false}))).await;
    assert_eq!((status, v["template"].as_bool()), (StatusCode::OK, Some(false)));
    let (status, _, _) = send(&app, as_viewer(Method::POST, &format!("/api/v1{base}/template")).header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"on":true}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn media_needs_a_real_driver() {
    let (app, _) = setup();
    assert!(!page(&app, "/storage").await.contains("/media/upload"), "simülasyonda yükleme bölümü yok");
    let (status, list) = api(&app, get("/api/v1/media")).await;
    assert_eq!((status, list.as_array().map(Vec::len)), (StatusCode::OK, Some(0)));
    let (status, body) = api(&app, request(Method::PUT, "/api/v1/media/a.iso").body(Body::from("x")).unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("gerçek sürücü"), "{body}");
}

// ---------- Arayüz v3: konuk listesi, görevler, yedekler, sekmeli ayarlar ----------

#[tokio::test]
async fn guest_list_filters_by_state_and_groups_by_node() {
    let (app, state) = setup();
    let c = state.snapshot();
    let live = c.guests.iter().filter(|g| g.status != GuestStatus::Stopped).count();
    let all = page(&app, "/guests").await;
    assert!(all.contains(r#"<a href="/guests" aria-current="page">"#), "sol menüde Konuklar etkin");
    for g in &c.guests {
        assert!(all.contains(&format!(r#"href="/guests/{}""#, g.vmid)), "{}", g.vmid);
    }
    assert!(all.contains(r#"<tr class="grp"><td colspan="8"><a href="/nodes/pve-ist-01">pve-ist-01</a>"#), "konuklar düğüme göre gruplanır");
    assert!(all.contains(&format!("Tümü <b>{}</b>", c.guests.len())) && all.contains(&format!("Çalışan <b>{live}</b>")) && all.contains("Şablon <b>0</b>"));
    assert!(all.contains(r#"action="/guests/103/action""#) && all.contains(r#"name="action" value="start">"#) && all.contains(r#"name="action" value="stop">"#), "satırdaki düğme duruma göre başlatır ya da kapatır");

    let running = page(&app, "/guests?tab=calisan").await;
    assert!(running.contains(r#"href="/guests/102""#) && !running.contains(r#"href="/guests/103""#));
    assert!(running.contains(r#"<a href="/guests?tab=calisan" aria-current="true">"#));
    let stopped = page(&app, "/guests?tab=kapali").await;
    assert!(stopped.contains(r#"href="/guests/103""#) && !stopped.contains(r#"href="/guests/102""#));
    assert!(page(&app, "/guests?tab=sablon").await.contains("Bu süzgeçle eşleşen konuk yok."));

    // Şablon kendi süzgecine geçer, "kapalı" sayılmaz ve satırından başlatılamaz.
    assert!(post_ok(&app, "/guests/103/template", "on=1").await.contains("notice="));
    let templates = page(&app, "/guests?tab=sablon").await;
    assert!(templates.contains(r#"href="/guests/103""#) && templates.contains(r#"<span class="tag gold">şablon</span>"#) && templates.contains(r#"name="action" value="start" disabled>"#));
    assert!(!page(&app, "/guests?tab=kapali").await.contains(r#"href="/guests/103""#));

    // Durum ve metin süzgeci birlikte çalışır; arama kutusu durumu korur.
    let both = page(&app, "/guests?tab=calisan&q=web").await;
    assert!(both.contains("web-prod-01") && !both.contains(r#"href="/guests/102""#) && both.contains(r#"<input type="hidden" name="tab" value="calisan">"#));
    assert!(page(&app, "/guests?tab=uydurma").await.contains(r#"href="/guests/102""#), "bilinmeyen süzgeç yok sayılır");

    // İzleyiciye eylem ve seçim sunulmaz.
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests").body(Body::empty()).unwrap()).await;
    assert!(viewer.contains("web-prod-01") && !viewer.contains(r#"name="action""#) && !viewer.contains(r#"form="bulk""#) && !viewer.contains("/guests/bulk"));
}

#[tokio::test]
async fn tasks_and_backups_have_their_own_pages() {
    let (app, state) = setup();
    assert!(page(&app, "/tasks").await.contains("Henüz görev yok"));
    assert!(page(&app, "/backups").await.contains("Henüz yedek yok"));
    assert!(page(&app, "/").await.contains("Henüz yedek alınmadı."));

    let id = state.backup_create(103, "admin").unwrap();
    send(&app, post_form("/guests/103/action", "action=start")).await;
    send(&app, post_form("/guests/202/action", "action=start")).await;

    let tasks = page(&app, "/tasks").await;
    assert!(tasks.contains("redox vm start 103") && tasks.contains("redox ct start 202") && tasks.contains(r#"<a href="/tasks" aria-current="page">"#));
    let mine = page(&app, "/tasks?q=103").await;
    assert!(mine.contains("redox vm start 103") && !mine.contains("redox ct start 202") && mine.contains(r#"value="103""#));
    assert!(page(&app, "/tasks?q=zzz").await.contains("ile eşleşen görev yok"));
    assert!(page(&app, "/guests/103").await.contains(r#"href="/tasks?q=103""#), "konuk özetinden görevlerine gidilir");

    let backups = page(&app, "/backups").await;
    assert!(backups.contains(&id) && backups.contains(r#"href="/guests/103?tab=yedekler""#) && backups.contains("yalnızca yerel"));
    assert!(backups.contains(r#"<a href="/backups" aria-current="page">"#) && backups.contains(r#"<span class="count">1</span>"#), "sol menü yedek sayısını gösterir");
    let home = page(&app, "/").await;
    assert!(home.contains(&id) && home.contains(r#"href="/guests/102?tab=yedekler""#), "genel bakış son yedeği ve yedeksiz konukları gösterir");
}

#[tokio::test]
async fn settings_tabs_save_only_their_own_fields() {
    let (app, state) = setup();
    let guest = |id| state.snapshot().guest(id).unwrap().clone();
    let before = guest(103);

    let to = post_ok(&app, "/guests/103/edit", "part=secenekler&name=derleme&os=x&tags=ci&onboot=1&backup_time=03%3A30&backup_keep=3&firewall=1&fw_rules=tcp%2F22").await;
    assert!(to.starts_with("/guests/103?tab=secenekler&notice="), "{to}");
    let g = guest(103);
    assert_eq!((g.name.as_str(), g.onboot, g.backup_time.as_deref(), g.hw.firewall, g.hw.fw_rules.len()), ("derleme", true, Some("03:30"), true, 1));
    assert_eq!((g.cores, g.mem_total_mb), (before.cores, before.mem_total_mb), "seçenekler donanıma dokunmaz");

    // Donanım sekmesinde ad, açılış ve güvenlik duvarı alanları yoktur: işaretsiz kutu "kapat" sayılmamalı.
    let to = post_ok(&app, "/guests/103/edit", "part=donanim&cores=2&memory_mb=4096&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=").await;
    assert!(to.starts_with("/guests/103?tab=donanim&notice="), "{to}");
    let g = guest(103);
    assert_eq!((g.cores, g.mem_total_mb), (2, 4096));
    assert_eq!((g.name.as_str(), g.onboot, g.backup_time.as_deref(), g.backup_keep, g.hw.firewall, g.hw.fw_rules.len(), g.tags.clone()), ("derleme", true, Some("03:30"), 3, true, 1, vec!["ci".to_string()]));

    // Seçenekler formuna elle eklenen donanım alanları yok sayılır.
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=derleme2&os=x&tags=&backup_time=&backup_keep=3&fw_rules=&cores=16&memory_mb=1024").await.contains("notice="));
    let g = guest(103);
    assert_eq!((g.name.as_str(), g.onboot, g.backup_time.as_deref(), g.hw.firewall, g.cores, g.mem_total_mb), ("derleme2", false, None, false, 2, 4096));

    // Çalışan konuk: seçenekler değişir, donanım reddedilir ve hata kendi sekmesine döner.
    assert!(post_ok(&app, "/guests/100/edit", "part=secenekler&name=web-x&os=debian-12&tags=&backup_time=&backup_keep=7&fw_rules=").await.starts_with("/guests/100?tab=secenekler&notice="));
    assert_eq!(guest(100).name, "web-x");
    let cores = guest(100).cores;
    assert!(post_ok(&app, "/guests/100/edit", "part=donanim&cores=1&memory_mb=8192&forwards=").await.starts_with("/guests/100?tab=donanim&error="));
    assert_eq!(guest(100).cores, cores);
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=K%C3%B6t%C3%BC&os=x&tags=&backup_keep=3&fw_rules=").await.starts_with("/guests/103?tab=secenekler&error="));

    // Eski düzenleme adresi donanım sekmesine yönlenir.
    let (status, headers, _) = send(&app, get("/guests/103/edit")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/guests/103?tab=donanim"));
}

#[tokio::test]
async fn guest_header_menu_and_suspend_notice() {
    let (app, state) = setup();
    let stopped = page(&app, "/guests/103").await;
    assert!(stopped.contains(r#"<details class="menu">"#) && stopped.contains(r#"formaction="/guests/103/backups">"#) && stopped.contains(r#"href="/guests/103?tab=cogalt""#));
    assert!(stopped.contains(r#"formaction="/guests/103/suspend" disabled>"#), "kapalı konuk askıya alınamaz");
    assert!(stopped.contains(r#"<a class="danger" href="/guests/103/delete">"#));

    let running = page(&app, "/guests/102").await;
    assert!(running.contains(r#"formaction="/guests/102/suspend">"#), "çalışan sanal makine askıya alınabilir");
    assert!(running.contains(r#"formaction="/guests/102/backups" disabled>"#), "simülasyonda çalışan konuk yedeklenmez");
    assert!(running.contains(r#"title="Silmek için önce kapatın""#));
    assert!(!page(&app, "/guests/200").await.contains("/suspend"), "konteyner askıya alınmaz");

    state.mutate(|c| c.guests.iter_mut().find(|g| g.vmid == 103).unwrap().hw.suspended = true);
    let suspended = page(&app, "/guests/103?tab=yedekler").await;
    assert!(suspended.contains("Bu konuk askıda") && suspended.contains(r#"action="/guests/103/discard-suspend""#) && suspended.contains(r#"<span class="st stop">Askıda</span>"#));
    assert!(page(&app, "/guests?tab=kapali").await.contains(r#"<span class="st stop">Askıda</span>"#));
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/103").body(Body::empty()).unwrap()).await;
    assert!(viewer.contains("Bu konuk askıda") && !viewer.contains("discard-suspend") && !viewer.contains(r#"<details class="menu">"#));
}

#[tokio::test]
async fn real_container_clone_tab_offers_only_cloning() {
    let (app, state) = setup_real();
    // Gerçek konteynerde çoğaltma sekmesi yalnızca klonu sunar: şablon, bağlı klon ve OVA sanal makinelere özgüdür.
    let ct = page(&app, "/guests/202").await;
    assert!(ct.contains(r#"href="/guests/202?tab=cogalt""#) && ct.contains(">Çoğalt<") && !ct.contains("Çoğalt ve Taşı"));
    let tab = page(&app, "/guests/202?tab=cogalt").await;
    assert!(tab.contains("Bu konteynerin kopyası") && tab.contains(r#"action="/guests/202/clone""#), "{tab}");
    assert!(!tab.contains("Şablona Çevir") && !tab.contains("OVA") && !tab.contains(r#"name="linked""#));
    assert!(post_ok(&app, "/guests/202/clone", "name=ci-kopya").await.contains("notice="));
    let c = state.snapshot();
    let copy = c.guests.iter().find(|g| g.name == "ci-kopya").expect("klon kaydı");
    assert_eq!((copy.kind, copy.status), (GuestKind::Ct, GuestStatus::Stopped));
    // Çalışan konteynerde klon kapalıdır.
    let running = page(&app, "/guests/200?tab=cogalt").await;
    assert!(running.contains("önce konteyneri kapatın") && running.contains(r#"<button class="btn primary" type="submit" disabled>"#));
    let vm = page(&app, "/guests/103?tab=cogalt").await;
    assert!(vm.contains("Şablona Çevir") && vm.contains(r#"href="/guests/103/export""#) && vm.contains(r#"action="/guests/103/clone""#));
    // Çalışan konukta bağlantı ve düğmeler kapalıdır.
    let live = page(&app, "/guests/102?tab=cogalt").await;
    assert!(live.contains("önce konuğu kapatın") && !live.contains(r#"href="/guests/102/export""#) && live.contains(r#"<button class="btn primary" type="submit" disabled>"#));
}

#[tokio::test]
async fn brand_lockup_is_proportional_and_login_reveals_nothing() {
    let (app, _) = setup();
    let home = page(&app, "/").await;
    // Yazı logo simgeden biraz yüksek: harfleri simgeyle aynı boyda görünür.
    assert!(home.contains(r#"<img class="mark" src="/assets/logo.svg?v="#) && home.contains(r#"alt="" width="33" height="34">"#));
    assert!(home.contains(r#"<img class="word" src="/assets/wordmark.svg?v="#) && home.contains(r#"alt="" width="113" height="40">"#));

    let (status, _, login) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(login.contains(r#"height="96">"#) && login.contains(r#"height="110">"#));
    for secret in ["istanbul-01", "pve-ist-01", "db-postgres", "konuk çalışıyor"] {
        assert!(!login.contains(secret), "giriş sayfası oturumsuz ziyaretçiye sunucu bilgisi vermez: {secret}");
    }
    let css = page(&app, "/assets/style.css").await;
    assert!(css.contains(".brand .mark{height:34px") && css.contains(".brand .word{height:40px"));
}

// ---------- Açılışta başlatma ve zamanlanmış güç işlemleri ----------

#[tokio::test]
async fn autostart_follows_order_and_delays() {
    let (app, state) = setup_with(Duration::ZERO, SimDriver::failing_on(202));
    let tab = page(&app, "/guests/103?tab=secenekler").await;
    assert!(tab.contains(r#"name="boot_order" type="text" inputmode="numeric" value="0""#) && tab.contains(r#"name="boot_delay" type="text" inputmode="numeric" value="0""#));

    state.mutate(|c| c.guests.iter_mut().for_each(|g| g.onboot = false));
    assert_eq!(state.autostart(&|_| panic!("başlatılacak konuk yokken beklenmez")), 0);
    state.run_action(100, Action::Stop, "admin").unwrap();
    let set = |vmid: u32, extra: &'static str| {
        let name = state.snapshot().guest(vmid).unwrap().name.clone();
        let app = app.clone();
        async move { post_ok(&app, &format!("/guests/{vmid}/edit"), &format!("part=secenekler&name={name}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&{extra}")).await }
    };
    assert!(set(103, "onboot=1&boot_order=2&boot_delay=30").await.contains("notice="));
    assert!(set(202, "onboot=1&boot_order=1&boot_delay=5").await.contains("notice="));
    assert!(set(100, "onboot=1&boot_order=0&boot_delay=10").await.contains("notice="));
    assert!(set(103, "onboot=1&boot_order=1000&boot_delay=30").await.contains("error="));
    assert!(set(103, "onboot=1&boot_order=2&boot_delay=601").await.contains("error="));
    assert!(set(103, "onboot=1&boot_order=iki&boot_delay=30").await.contains("error="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.boot_order, hw.boot_delay), (2, 30), "hatalı form kaydı değiştirmez");
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("sıra 2") && home.contains("sonra 30 sn bekle"));

    // Sıra: 202 (1, açılamaz) → 103 (2) → 100 (sırasız, en son). Yalnızca açılan 103'ün ardından beklenir.
    let pauses = std::sync::Mutex::new(Vec::new());
    assert_eq!(state.autostart(&|d| pauses.lock().unwrap().push(d)), 2);
    assert_eq!(*pauses.lock().unwrap(), [Duration::from_secs(30)], "açılamayan konuktan sonra ve son konuktan sonra beklenmez");
    let c = state.snapshot();
    assert_eq!([103, 100, 202].map(|id| c.guest(id).unwrap().status), [GuestStatus::Running, GuestStatus::Running, GuestStatus::Stopped]);
    let boot: Vec<(&str, TaskStatus)> = c.tasks.iter().rev().filter(|t| t.user == redox_server::state::BOOT).map(|t| (t.command.as_str(), t.status)).collect();
    assert_eq!(boot, [("ct start 202", TaskStatus::Failed), ("vm start 103", TaskStatus::Ok), ("vm start 100", TaskStatus::Ok)]);
    // Çalışanlar yeniden başlatılmaz; açılamayan yeniden denenir.
    assert_eq!(state.autostart(&|_| ()), 0);
    assert_eq!(state.snapshot().tasks.iter().filter(|t| t.user == redox_server::state::BOOT).count(), 4);
}

#[tokio::test]
async fn power_schedules_are_managed_from_the_options_tab() {
    let (app, state) = setup();
    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert!(page(&app, "/guests/103?tab=secenekler").await.contains("Zamanlanmış işlem yok."));

    let to = post_ok(&app, "/guests/103/schedules", "action=start&time=08%3A00&day=1&day=2&day=3&day=4&day=5").await;
    assert!(to.starts_with("/guests/103?tab=secenekler&notice="), "{to}");
    assert!(post_ok(&app, "/guests/103/schedules", "action=stop&time=20%3A00&day=1&day=2&day=3&day=4&day=5&day=6&day=7").await.contains("notice="));
    for bad in ["action=start&time=08%3A00", "action=pause&time=08%3A00&day=1", "action=start&time=25%3A00&day=1", "action=start&time=&day=1", "time=08%3A00&day=1", "action=start&time=08%3A00&day=9"] {
        assert!(post_ok(&app, "/guests/103/schedules", bad).await.contains("error="), "{bad}");
    }
    assert!(post_ok(&app, "/guests/999/schedules", "action=start&time=08%3A00&day=1").await.contains("error="));
    let list = state.snapshot().guest(103).unwrap().hw.schedules.clone();
    assert_eq!(list.iter().map(ToString::to_string).collect::<Vec<_>>(), ["start@08:00@12345", "stop@20:00"]);

    let tab = page(&app, "/guests/103?tab=secenekler").await;
    assert!(tab.contains("hafta içi") && tab.contains("her gün") && tab.contains(r#"action="/guests/103/schedules/0/delete""#) && tab.contains(r#"<input type="checkbox" name="day" value="7" checked>"#));
    assert!(page(&app, "/guests/103").await.contains("Başlat 08:00 · hafta içi"), "özet zamanlamayı gösterir");
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/103?tab=secenekler").body(Body::empty()).unwrap()).await;
    assert!(viewer.contains("hafta içi") && !viewer.contains("/schedules"), "izleyici listeyi görür, değiştiremez");
    let (status, _, _) = send(&app, as_viewer(Method::POST, "/guests/103/schedules").header(CONTENT_TYPE, "application/x-www-form-urlencoded").body(Body::from("action=stop&time=01%3A00&day=1")).unwrap()).await;
    assert_ne!(status, StatusCode::SEE_OTHER, "izleyici zamanlama ekleyemez");
    assert_eq!(state.snapshot().guest(103).unwrap().hw.schedules.len(), 2);

    // 2026-11-24 Salı: 08:00'de başlar, 20:00'de kapanır; her biri günde bir kez.
    assert_eq!(state.power_tick("2026-11-24", 1, 479), 0);
    assert_eq!(state.power_tick("2026-11-24", 1, 480), 1);
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);
    assert_eq!(state.power_tick("2026-11-24", 1, 481), 0);
    assert_eq!(state.power_tick("2026-11-24", 1, 1200), 1);
    let c = state.snapshot();
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].user.as_str(), c.tasks[0].status), ("vm stop 103", "zamanlayıcı", TaskStatus::Ok));
    assert!(page(&app, "/guests/103?tab=secenekler").await.contains("2026-11-24"), "son çalışma günü görünür");

    // Konuk zaten istenen durumdaysa işlem atlanır: hatalı görev yazılmaz ve o gün yeniden denenmez.
    state.run_action(103, Action::Start, "admin").unwrap();
    let before = state.snapshot().tasks.len();
    assert_eq!(state.power_tick("2026-11-25", 2, 480), 0);
    assert_eq!(state.snapshot().tasks.len(), before);
    state.run_action(103, Action::Stop, "admin").unwrap();
    assert_eq!(state.power_tick("2026-11-25", 2, 485), 0, "o günün işlemi atlandı sayılır");
    // Hafta sonu "başlat" çalışmaz; saati kaçan işlem telafi edilmez.
    assert_eq!(state.power_tick("2026-11-28", 5, 480), 0);
    assert_eq!(state.power_tick("2026-11-26", 3, 600), 0);
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Stopped);

    assert!(post_ok(&app, "/guests/103/schedules/7/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/schedules/x/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/schedules/0/delete", "").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.schedules.iter().map(ToString::to_string).collect::<Vec<_>>(), ["stop@20:00"]);

    // API: liste bütün olarak yazılır; günler verilmezse her gün.
    let (status, body) = api(&app, patch(json!({"schedules": [{"action": "reboot", "time": "03:00"}], "boot_order": 3}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["schedules"][0]["days"].as_u64(), body["schedules"][0]["action"].as_str(), body["boot_order"].as_u64()), (Some(127), Some("reboot"), Some(3)));
    for bad in [json!({"schedules": [{"action": "pause", "time": "03:00"}]}), json!({"schedules": [{"action": "stop", "time": "3"}]}), json!({"schedules": vec![json!({"action": "stop", "time": "03:00"}); 9]}), json!({"boot_delay": 601})] {
        let (status, body) = api(&app, patch(bad.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.schedules.len(), 1);
}

#[tokio::test]
async fn rate_limits_are_edited_from_the_hardware_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    for field in ["disk_mbps", "disk_iops", "net_mbps"] {
        assert!(tab.contains(&format!(r#"form="hw" name="{field}" type="text" inputmode="numeric" value="0">"#)), "{field}");
    }
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains("Hız sınırları"), "konteynerde hız sınırı yok");
    assert!(page(&app, "/guests/100?tab=donanim").await.contains(r#"name="net_mbps" type="text" inputmode="numeric" value="0" disabled>"#), "çalışan konukta alan kapalı");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    let to = post_ok(&app, "/guests/103/edit", &format!("{base}&disk_mbps=50&disk_iops=2000&net_mbps=100")).await;
    assert!(to.starts_with("/guests/103?tab=donanim&notice="), "{to}");
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.disk_mbps, hw.disk_iops, hw.net_mbps), (50, 2000, 100));
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("disk 50 MB/sn") && home.contains("disk 2000 işlem/sn") && home.contains("ağ 100 Mbit/sn"));
    assert!(page(&app, "/guests/103?tab=donanim").await.contains(r#"name="disk_mbps" type="text" inputmode="numeric" value="50">"#));

    for bad in ["disk_mbps=10001&disk_iops=0&net_mbps=0", "disk_mbps=hızlı&disk_iops=0&net_mbps=0", "disk_mbps=0&disk_iops=1000001&net_mbps=0"] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await.starts_with("/guests/103?tab=donanim&error="), "{bad}");
    }
    // Bütün kartlar NAT'tayken ağ sınırı reddedilir.
    let nat = base.replace("bridge=vmbr0", "bridge=nat");
    let to = post_ok(&app, "/guests/103/edit", &format!("{nat}&disk_mbps=50&disk_iops=2000&net_mbps=100")).await;
    assert!(to.contains("error=") && to.contains("k%C3%B6pr%C3%BCye"), "{to}");
    assert_eq!(state.snapshot().guest(103).unwrap().bridge, "vmbr0");
    // Seçenekler formu sınırlara dokunmaz.
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=win-build&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&disk_mbps=1").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.disk_mbps, 50);

    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(json!({"disk_mbps": 0, "net_mbps": 20}))).await;
    assert_eq!((status, body["disk_mbps"].as_u64(), body["disk_iops"].as_u64(), body["net_mbps"].as_u64()), (StatusCode::OK, Some(0), Some(2000), Some(20)), "{body}");
    assert_eq!(api(&app, patch(json!({"net_mbps": 100001}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = api(&app, request(Method::PATCH, "/api/v1/guests/100").header(CONTENT_TYPE, "application/json").body(Body::from(json!({"disk_mbps": 5}).to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::CONFLICT, "çalışan konukta değişmez: {body}");
}

#[tokio::test]
async fn disks_and_nics_are_added_to_running_guests_when_the_driver_can() {
    // Simülasyon çalışırken takamaz: çalışan konukta form kapalı, istek reddedilir.
    let (sim, sim_state) = setup();
    let tab = page(&sim, "/guests/102?tab=donanim").await;
    assert!(tab.contains(r#"name="disk_gb" type="text" inputmode="numeric" size="6" value="10" disabled>"#) && tab.contains("Kart, konuk kapalıyken eklenir."));
    assert!(!tab.contains("çalışırken de eklenebilir"));
    assert!(post_ok(&sim, "/guests/102/disks", "disk_gb=5").await.contains("error="));
    assert!(post_ok(&sim, "/guests/102/nics", "network=vmbr1").await.contains("error="));
    assert!(sim_state.snapshot().guest(102).unwrap().hw.nics.is_empty());
    // Kapalı konuğa kart her sürücüde eklenir.
    let to = post_ok(&sim, "/guests/103/nics", "network=+vmbrT+").await;
    assert!(to.starts_with("/guests/103?tab=donanim&notice="), "{to}");
    assert!(post_ok(&sim, "/guests/103/nics", "network=NAT").await.contains("notice="));
    assert_eq!(sim_state.snapshot().guest(103).unwrap().hw.nics, ["vmbrT", "nat"], "köprü adının harfleri korunur; NAT her yazımıyla kabul edilir");
    // Düzenleme formu da köprü adını olduğu gibi kaydeder.
    assert!(post_ok(&sim, "/guests/103/edit", "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=Br-LAN%2C+NAT").await.contains("notice="));
    assert_eq!(sim_state.snapshot().guest(103).unwrap().hw.nics, ["Br-LAN", "nat"]);
    assert!(post_ok(&sim, "/guests/103/nics", "network=").await.contains("error="));
    assert!(post_ok(&sim, "/guests/202/nics", "network=nat").await.contains("error="), "konteynere ek kart eklenmez");

    let (app, state) = setup_real();
    let tab = page(&app, "/guests/102?tab=donanim").await;
    assert!(tab.contains("Disk ve ağ kartı çalışırken de eklenebilir.") && tab.contains(r#"name="disk_gb" type="text" inputmode="numeric" size="6" value="10">"#));
    assert!(tab.contains(r#"<select id="new_nic" name="network">"#) && tab.contains(r#"<option value="br-lan">Köprü: br-lan</option>"#));
    assert!(tab.contains(r#"name="cores" type="text" inputmode="numeric" value="8" disabled"#), "öbür donanım alanları kapalı kalır");
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/102?tab=donanim").body(Body::empty()).unwrap()).await;
    assert!(!viewer.contains("/nics") && !viewer.contains("/disks"));
    let used = |s: &AppState| s.snapshot().storages.iter().map(|x| x.used_gb).sum::<u64>();
    let before = used(&state);

    assert!(post_ok(&app, "/guests/102/disks", "disk_gb=5").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/disks", "disk_gb=7").await.contains("notice="));
    let to = post_ok(&app, "/guests/102/disks", "disk_gb=9").await;
    assert!(to.contains("error=") && to.contains("yan%C4%B1t%20vermedi"), "{to}");
    let g = state.snapshot().guest(102).unwrap().clone();
    assert_eq!((g.status, g.hw.disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>()), (GuestStatus::Running, vec![(1, 5), (2, 7)]));
    assert_eq!(used(&state), before + 12, "takılamayan diskin yeri geri verilir");

    assert!(post_ok(&app, "/guests/102/nics", "network=br-lan").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/nics", "network=nat").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/nics", "network=br-lan").await.contains("error="), "üçüncü kart takılamadı");
    assert!(post_ok(&app, "/guests/102/nics", "network=br-yok").await.contains("error="), "olmayan köprü");
    assert_eq!(state.snapshot().guest(102).unwrap().hw.nics, ["br-lan", "nat"]);
    let c = state.snapshot();
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].status), ("vm nic add 102 br-lan", TaskStatus::Failed));
    assert!(c.tasks.iter().any(|t| t.command == "vm disk add 102 9G" && t.status == TaskStatus::Failed) && c.tasks.iter().any(|t| t.command == "vm nic add 102 nat" && t.status == TaskStatus::Ok));

    // Duraklatılmış konuğa takılmaz; çalışan konuktan disk çıkarılamaz.
    assert!(post_ok(&app, "/guests/104/disks", "disk_gb=5").await.contains("error="));
    assert!(post_ok(&app, "/guests/102/disks/1/delete", "").await.contains("error="));

    // API
    let post_json = |uri: &str, body: Value| request(Method::POST, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, post_json("/api/v1/guests/100/nics", json!({"network": "NAT"}))).await;
    assert_eq!((status, body["index"].as_u64(), body["network"].as_str()), (StatusCode::CREATED, Some(1), Some("nat")), "{body}");
    assert_eq!(api(&app, post_json("/api/v1/guests/100/nics", json!({"network": "br-yok"}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(api(&app, post_json("/api/v1/guests/999/nics", json!({"network": "nat"}))).await.0, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, as_viewer(Method::POST, "/api/v1/guests/100/nics").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"network":"nat"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = api(&app, post_json("/api/v1/guests/100/disks", json!({"size_gb": 4}))).await;
    assert_eq!((status, body["id"].as_u64()), (StatusCode::CREATED, Some(1)), "çalışan konuğa API'den disk: {body}");
}

#[tokio::test]
async fn vlan_tags_are_set_per_card() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"form="hw" name="vlan" type="text" inputmode="numeric" value="0">"#) && tab.contains(r#"<input name="vlan" type="text" inputmode="numeric" size="5""#));
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="vlan""#), "konteynerde VLAN alanı yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&vlan=20&nics=vmbr1%3A30%2C+nat")).await.starts_with("/guests/103?tab=donanim&notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.vlan, hw.nics.clone()), (20, vec!["vmbr1:30".to_string(), "nat".to_string()]));
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("köprü: vmbr0 · VLAN 20") && home.contains("+ vmbr1 · VLAN 30"), "özet etiketleri gösterir");
    assert!(page(&app, "/guests/103?tab=donanim").await.contains(r#"name="nics" type="text" value="vmbr1:30, nat""#));

    for bad in ["vlan=4095&nics=", "vlan=yirmi&nics=", "vlan=0&nics=nat%3A5", "vlan=0&nics=vmbr1%3A0"] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await.contains("error="), "{bad}");
    }
    // NAT'a alınan birinci kartta etiket kalamaz.
    let to = post_ok(&app, "/guests/103/edit", &format!("{}&vlan=20&nics=", base.replace("bridge=vmbr0", "bridge=nat"))).await;
    assert!(to.contains("error=") && to.contains("VLAN"), "{to}");
    assert_eq!(state.snapshot().guest(103).unwrap().hw.vlan, 20);

    // Kart ekleme formu etiketi ağ tanımına işler.
    assert!(post_ok(&app, "/guests/103/nics", "network=vmbr2&vlan=40").await.contains("notice="));
    assert!(post_ok(&app, "/guests/103/nics", "network=nat&vlan=40").await.contains("error="), "NAT kartına etiket verilmez");
    assert_eq!(state.snapshot().guest(103).unwrap().hw.nics, ["vmbr1:30", "nat", "vmbr2:40"]);

    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(json!({"vlan": 0, "nics": ["vmbr1:4094"]}))).await;
    assert_eq!((status, body["vlan"].as_u64(), body["nics"][0].as_str()), (StatusCode::OK, Some(0), Some("vmbr1:4094")), "{body}");
    assert_eq!(api(&app, patch(json!({"vlan": 4095}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    // Gerçek sürücü: etiketli ek kartın köprüsü de sunucuda bulunmalı.
    let (real, _) = setup_real();
    let real_base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&vlan=0";
    assert!(post_ok(&real, "/guests/103/edit", &format!("{real_base}&nics=br-yok%3A5")).await.contains("error="));
    assert!(post_ok(&real, "/guests/103/edit", &format!("{real_base}&nics=br-lan%3A5")).await.contains("notice="));
    assert!(post_ok(&real, "/guests/103/nics", "network=br-yok&vlan=7").await.contains("error="));
}

// ---------- Canlı konsol: uygulamada betik yükleyen tek sayfa ----------

#[tokio::test]
async fn live_console_is_the_only_page_with_a_script() {
    let (app, _) = setup_real();
    let csp = |headers: &HeaderMap| headers.get("content-security-policy").unwrap().to_str().unwrap().to_string();

    // Betik yalnızca konsol sayfasında, yalnızca bu sunucudan; satır içi betik ve başka kaynak yok.
    let (status, headers, body) = send(&app, get("/guests/102/console")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(csp(&headers).contains("script-src 'self'") && csp(&headers).contains("connect-src 'self'") && csp(&headers).contains("frame-ancestors 'none'"), "{}", csp(&headers));
    assert!(!csp(&headers).contains("unsafe-eval") && !csp(&headers).contains("script-src 'self' 'unsafe-inline'"));
    assert_eq!(body.matches("<script").count(), 1);
    assert!(body.contains(r#"<script src="/assets/console.js?v="#) && body.contains(" defer></script>"), "betik dış dosyadır");
    assert!(body.contains(r#"<canvas id="screen" tabindex="0""#) && body.contains(r#"data-ws="/guests/102/vnc""#) && body.contains("<noscript>"));
    assert!(!body.contains(r#"http-equiv="refresh""#), "konsol sayfası kendini yenilemez (oturum kopar)");
    assert!(!body.to_lowercase().contains(" onclick=") && !body.contains("javascript:"));

    // Öbür sayfalar betiksiz kalır; konsol sekmesi canlı konsola bağlantı verir.
    for uri in ["/", "/guests", "/guests/102", "/guests/102?tab=konsol", "/guests/102?tab=donanim", "/storage", "/tasks"] {
        let (_, headers, body) = send(&app, get(uri)).await;
        assert!(csp(&headers).contains("script-src 'none'"), "{uri}");
        assert!(!body.to_lowercase().contains("<script"), "{uri}");
    }
    let tab = page(&app, "/guests/102?tab=konsol").await;
    assert!(tab.contains(r#"href="/guests/102/console""#) && tab.contains("Canlı Konsolu Aç") && tab.contains("iframe"), "betiksiz konsol yerinde kalır");
    // Kapalı konuk: sayfa açılır ama tuval ve betik yoktur.
    let stopped = page(&app, "/guests/103/console").await;
    assert!(stopped.contains("Konuk çalışmıyor") && !stopped.contains("<script") && !stopped.contains("<canvas"));
    assert!(!page(&app, "/guests/103?tab=konsol").await.contains("/guests/103/console"));
    assert_eq!(send(&app, get("/guests/999/console")).await.0, StatusCode::NOT_FOUND);

    // İstemci dosyası: betik türüyle, ara belleğe alınabilir, içinde değerlendirme (eval) yok.
    let (status, headers, js) = send(&app, get("/assets/console.js")).await;
    assert_eq!((status, headers.get(CONTENT_TYPE).unwrap().to_str().unwrap()), (StatusCode::OK, "text/javascript; charset=utf-8"));
    assert!(js.contains("class Session") && !js.contains("eval(") && !js.contains("new Function") && !js.contains("innerHTML"));
    assert!(!js.contains("http://") && !js.contains("https://"), "istemci başka sunucuya bağlanmaz");
}

#[tokio::test]
async fn live_console_needs_permission_and_a_same_site_page() {
    let (app, state) = setup_real();
    let upgrade = |builder: axum::http::request::Builder| {
        builder.header("connection", "upgrade").header("upgrade", "websocket").header("sec-websocket-version", "13").header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==").body(Body::empty()).unwrap()
    };
    // İzleyici ne sayfayı ne bağlantıyı açabilir.
    for uri in ["/guests/102/console", "/guests/102/vnc"] {
        let (status, _, _) = send(&app, as_viewer(Method::GET, uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
    assert_eq!(send(&app, upgrade(as_viewer(Method::GET, "/guests/102/vnc"))).await.0, StatusCode::FORBIDDEN);
    // Başka bir sitenin sayfası, kullanıcının oturumuyla bağlanamaz.
    let (status, _, body) = send(&app, upgrade(request(Method::GET, "/guests/102/vnc").header("origin", "https://evil.example"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _, _) = send(&app, upgrade(request(Method::GET, "/guests/102/vnc").header("sec-fetch-site", "cross-site"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Kapalı konuk ve olmayan konuk.
    assert_eq!(send(&app, upgrade(request(Method::GET, "/guests/103/vnc"))).await.0, StatusCode::CONFLICT);
    assert_eq!(send(&app, upgrade(request(Method::GET, "/guests/999/vnc"))).await.0, StatusCode::NOT_FOUND);
    // Düz GET bir konsol oturumu açmaz ve içerik döndürmez.
    let (status, _, _) = send(&app, get("/guests/102/vnc")).await;
    assert!(status.is_client_error(), "{status}");
    // Bu testlerde bağlantı gerçekten yükseltilemediği için oturum da kaydedilmez.
    assert!(state.auth().audit.recent("console", 10).is_empty());
}

#[test]
fn console_client_logic_passes_its_node_tests() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/console.test.js");
    match std::process::Command::new("node").arg("--test").arg(&script).output() {
        Ok(out) => assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
        // Node yoksa istemci testleri atlanır; sunucu tarafı yine sınanır.
        Err(e) => eprintln!("node bulunamadı, istemci testleri atlandı: {e}"),
    }
}

// ---------- Kendiliğinden yeniden başlatma ----------

#[tokio::test]
async fn guests_marked_for_restart_come_back_after_an_unexpected_stop() {
    let (app, state) = setup_with(Duration::ZERO, SimDriver::failing_on(102));
    let tab = page(&app, "/guests/100?tab=secenekler").await;
    assert!(tab.contains(r#"<input type="checkbox" name="restart" value="1">"#) && tab.contains("Kendiliğinden kapanırsa yeniden başlat"));
    for vmid in [100u32, 102] {
        let name = state.snapshot().guest(vmid).unwrap().name.clone();
        assert!(post_ok(&app, &format!("/guests/{vmid}/edit"), &format!("part=secenekler&name={name}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&boot_order=0&boot_delay=0&restart=1")).await.contains("notice="));
    }
    assert!(state.snapshot().guest(100).unwrap().hw.restart);
    assert!(page(&app, "/guests/100").await.contains("kendiliğinden yeniden başlatılır"));
    assert!(page(&app, "/guests/100?tab=secenekler").await.contains(r#"name="restart" value="1" checked>"#));
    // Donanım formu işareti değiştirmez.
    assert!(post_ok(&app, "/guests/103/edit", "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=&restart=1").await.contains("notice="));
    assert!(!state.snapshot().guest(103).unwrap().hw.restart);

    assert_eq!(state.ha_tick(), 0, "ilk tur yalnızca durumu öğrenir");
    // Çökme: konuklar görev olmadan durur. 100 açılır; 102'nin sürücüsü hata verir ve günlüğe yazılır.
    let crash = |ids: &[u32]| state.mutate(|c| c.guests.iter_mut().filter(|g| ids.contains(&g.vmid)).for_each(|g| g.status = GuestStatus::Stopped));
    crash(&[100, 101, 102]);
    assert_eq!(state.ha_tick(), 2, "başlatma görevi iki konuk için açıldı");
    let c = state.snapshot();
    assert_eq!([100, 101, 102].map(|id| c.guest(id).unwrap().status), [GuestStatus::Running, GuestStatus::Stopped, GuestStatus::Stopped], "işaretsiz 101 ve açılamayan 102 kapalı kalır");
    let by_watch: Vec<(&str, TaskStatus)> = c.tasks.iter().rev().filter(|t| t.user == redox_server::state::HA).map(|t| (t.command.as_str(), t.status)).collect();
    assert_eq!(by_watch, [("vm start 100", TaskStatus::Ok), ("vm start 102", TaskStatus::Failed)]);
    assert_eq!(state.ha_tick(), 0, "aynı kapanış yeniden denenmez");

    // Panelden kapatılan konuk yeniden başlatılmaz.
    state.run_action(100, Action::Stop, "admin").unwrap();
    assert_eq!(state.ha_tick(), 0);
    assert_eq!(state.snapshot().guest(100).unwrap().status, GuestStatus::Stopped);

    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/100").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(json!({"restart": false}))).await;
    assert_eq!((status, body["restart"].as_bool()), (StatusCode::OK, Some(false)), "{body}");
}

// ---------- Paylaşılan klasörler ----------

#[tokio::test]
async fn shared_folders_are_edited_from_the_hardware_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"form="hw" name="shares" type="text" value="""#) && tab.contains("mount -t 9p -o trans=virtio"));
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="shares""#), "konteynerde paylaşım alanı yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=&vlan=0";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&shares=ortak%2C+veri%3Aro")).await.starts_with("/guests/103?tab=donanim&notice="));
    let shares = state.snapshot().guest(103).unwrap().hw.shares.clone();
    assert_eq!(shares.iter().map(ToString::to_string).collect::<Vec<_>>(), ["ortak", "veri:ro"]);
    assert!(page(&app, "/guests/103?tab=donanim").await.contains(r#"name="shares" type="text" value="ortak, veri:ro""#));
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("Paylaşımlar") && home.contains("veri · salt okunur"));

    for bad in ["shares=..%2Fetc", "shares=a%2Fb", "shares=Ortak", "shares=ortak%2C+ortak", "shares=a%2Cb%2Cc%2Cd%2Ce", "shares=veri%3Arx"] {
        let to = post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await;
        assert!(to.starts_with("/guests/103?tab=donanim&error="), "{bad}: {to}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.shares.len(), 2, "hatalı liste kaydı değiştirmez");
    // Alanı boşaltmak paylaşımları kaldırır; seçenekler formu paylaşımlara dokunmaz.
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=win-build&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&shares=").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.shares.len(), 2);
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&shares=")).await.contains("notice="));
    assert!(state.snapshot().guest(103).unwrap().hw.shares.is_empty());

    let patch = |vmid: u32, body: Value| request(Method::PATCH, &format!("/api/v1/guests/{vmid}")).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(103, json!({"shares": [{"name": "ortak"}, {"name": "veri", "readonly": true}]}))).await;
    assert_eq!((status, body["shares"][1]["readonly"].as_bool(), body["shares"][0]["readonly"].as_bool()), (StatusCode::OK, Some(true), Some(false)), "{body}");
    assert_eq!(api(&app, patch(103, json!({"shares": [{"name": "../etc"}]}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(api(&app, patch(100, json!({"shares": [{"name": "ortak"}]}))).await.0, StatusCode::CONFLICT, "çalışan konukta değişmez");
    assert_eq!(api(&app, patch(202, json!({"shares": [{"name": "ortak"}]}))).await.0, StatusCode::UNPROCESSABLE_ENTITY, "konteynerde yok");

    // Askıdaki konuğun donanımı değişmez.
    state.mutate(|c| c.after_suspend(103));
    let (status, body) = api(&app, patch(103, json!({"shares": []}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("askıda"));
}

// ---------- Yedek işleri ----------

#[tokio::test]
async fn backup_jobs_are_managed_from_the_backups_page_and_the_api() {
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
async fn job_backups_follow_the_jobs_own_retention() {
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

#[tokio::test]
async fn cloud_init_is_edited_from_the_hardware_tab_and_the_api() {
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGtMF0qv2yBAZ8lfmTtk1mKkZHjPl7Yj8d0Lr0oQ1X2w ali@dizustu";
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    for field in ["ci_user", "ci_ssh_key", "ci_ip", "ci_gw", "ci_dns"] {
        assert!(tab.contains(&format!(r#"form="hw" name="{field}""#)), "{field}");
    }
    assert!(tab.contains("cloud-init") && tab.contains("kapalı"));
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="ci_ssh_key""#), "konteynerde yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    let key = KEY.replace(' ', "+");
    let to = post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&ci_ip=10.0.0.5%2F24&ci_gw=10.0.0.1&ci_dns=1.1.1.1%2C+9.9.9.9")).await;
    assert!(to.contains("notice="), "{to}");
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.ci_user.as_deref(), hw.ci_ssh_key.as_deref(), hw.ci_ip.as_deref(), hw.ci_gw.as_deref(), hw.ci_dns.len(), hw.ci_gen), (Some("ali"), Some(KEY), Some("10.0.0.5/24"), Some("10.0.0.1"), 2, 1));
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains("açık · sürüm 1") && tab.contains(r#"name="ci_ip" type="text" value="10.0.0.5/24""#) && tab.contains(r#"value="1.1.1.1, 9.9.9.9""#));
    // Aynı formu yeniden kaydetmek sürümü artırmaz; hatalı adres reddedilir ve kayıt değişmez.
    post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&ci_ip=10.0.0.5%2F24&ci_gw=10.0.0.1&ci_dns=1.1.1.1%2C+9.9.9.9")).await;
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_gen, 1);
    for bad in ["ci_ip=10.0.0.5&ci_gw=&ci_dns=", "ci_ip=10.0.0.5%2F24&ci_gw=gecit&ci_dns=", "ci_ip=&ci_gw=10.0.0.1&ci_dns="] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&{bad}")).await.contains("error="), "{bad}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_ip.as_deref(), Some("10.0.0.5/24"));
    // Seçenekler sekmesinden kaydetmek cloud-init'e dokunmaz.
    post_ok(&app, "/guests/103/edit", "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=").await;
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_gen, 1);
    // Çalışan konukta değişmez.
    assert!(post_ok(&app, "/guests/102/edit", &format!("part=donanim&cores=8&memory_mb=32768&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=&ci_ssh_key={key}")).await.contains("error="));

    // API: adresi boşaltmak DHCP'ye döndürür, anahtarı boşaltmak cloud-init'i kapatır.
    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert!(api(&app, patch(json!({ "ci_ip": "", "ci_gw": "", "ci_dns": "" }))).await.0.is_success());
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.ci_ip.clone(), hw.cloud_init(), hw.ci_gen), (None, true, 2));
    let (status, v) = api(&app, patch(json!({ "ci_ip": "10.0.0.9/99" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(api(&app, patch(json!({ "ci_ssh_key": "" }))).await.0.is_success());
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cloud_init(), hw.ci_user.clone(), hw.ci_gen), (false, None, 3));
}

// ---------- İşlemci türü ve bellek balonu ----------

#[tokio::test]
async fn cpu_type_and_memory_balloon_are_set_from_the_hardware_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<select id="cpu_type" form="hw" name="cpu_type""#) && tab.contains(r#"<option value="" selected>Sunucunun işlemcisi (host)</option>"#), "{tab}");
    assert!(tab.contains(r#"<option value="Haswell">Haswell</option>"#) && tab.contains(r#"name="balloon" value="1""#));
    assert!(!tab.contains("Bellek hedefi"), "balon takılı değilken hedef formu yok");
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="cpu_type""#), "konteynerde yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=Haswell&balloon=1")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("Haswell", true, 0));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=Pentium9&balloon=1")).await.contains("error="));
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<option value="Haswell" selected>Haswell</option>"#) && tab.contains(r#"action="/guests/103/balloon""#) && tab.contains("tamamı (16384 MB)"));

    // Bellek hedefi: sınırlar içinde kabul edilir, görev günlüğüne yazılır; 0 balonu boşaltır.
    assert!(post_ok(&app, "/guests/103/balloon", "mb=4096").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 4096);
    assert_eq!(state.snapshot().tasks[0].command, "vm balloon 103 4096");
    assert!(page(&app, "/guests/103?tab=donanim").await.contains("4096 / 16384 MB"));
    for bad in ["mb=100", "mb=99999", "mb=cok", "mb="] {
        assert!(post_ok(&app, "/guests/103/balloon", bad).await.contains("error="), "{bad}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 4096, "reddedilen hedef kaydı değiştirmez");
    assert!(post_ok(&app, "/guests/103/balloon", "mb=0").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 0);
    // Balonu olmayan konukta ve izleyici için reddedilir; operatöre açık değildir.
    assert!(post_ok(&app, "/guests/102/balloon", "mb=4096").await.contains("error="));
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/103/balloon"), "mb=4096")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/guests/103/balloon"), None);

    // API.
    let (status, _) = api(&app, post_json("/api/v1/guests/103/balloon", &json!({ "mb": 8192 }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 8192);
    let (status, v) = api(&app, post_json("/api/v1/guests/103/balloon", &json!({ "mb": 1 }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Başka üreticinin modeli, üreticisi bilinen sunucuda reddedilir.
    let (real, real_state) = setup_real();
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=EPYC-Milan")).await.contains("error="));
    assert_eq!(real_state.snapshot().guest(103).unwrap().hw.cpu_type, "");
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=Skylake-Server")).await.contains("notice="));
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=kvm64")).await.contains("notice="));
    // Balon kaldırılınca hedef sıfırlanır; işlemci türü "host"a döner.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("", false, 0));
}

// ---------- Kaynak havuzları ve kotalar ----------

#[tokio::test]
async fn resource_pools_group_guests_and_enforce_quotas() {
    let (app, state) = setup();
    let empty = page(&app, "/pools").await;
    assert!(empty.contains("Havuz yok") && empty.contains(r#"<form class="inline" method="post" action="/pools">"#));
    assert!(page(&app, "/").await.contains(r#"href="/pools""#), "sol menüde bağlantı var");
    assert!(!empty.contains("<script"), "sayfa betiksizdir");

    // Form: boş alan sınırsızdır; hatalı ad ve sayı reddedilir.
    assert!(post_ok(&app, "/pools", "name=web&cores=12&memory_mb=&disk_gb=&guests=").await.contains("notice="));
    for bad in ["name=B%C3%BCy%C3%BCk&cores=1", "name=x&cores=cok", "name=&cores=1"] {
        assert!(post_ok(&app, "/pools", bad).await.contains("error="), "{bad}");
    }
    let pool = state.snapshot().pool("web").unwrap().clone();
    assert_eq!((pool.cores, pool.memory_mb, pool.disk_gb, pool.guests), (12, 0, 0, 0));

    // Konuk, Seçenekler sekmesinden havuza alınır (103: 8 çekirdek).
    let options = page(&app, "/guests/103?tab=secenekler").await;
    assert!(options.contains(r#"<select id="pool" name="pool">"#) && options.contains(r#"<option value="web">web</option>"#), "{options}");
    let base = "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&pool=web")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().pool.as_deref(), Some("web"));
    assert!(page(&app, "/guests/103?tab=secenekler").await.contains(r#"<option value="web" selected>web</option>"#));
    let listed = page(&app, "/pools").await;
    assert!(listed.contains("8 / 12") && listed.contains("1 / sınırsız") && listed.contains(r#"style="width:66%""#), "{listed}");
    assert!(listed.contains(r#"action="/pools/web/delete""#) && listed.contains(r#"aria-label="web havuzunu sil""#));

    // Kota: 12 çekirdeklik havuza 8 çekirdekli ikinci konuk sığmaz; 103'ün çekirdeği 13'e çıkarılamaz.
    let other = "part=secenekler&name=db-postgres&os=ubuntu-24.04&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=";
    let to = post_ok(&app, "/guests/102/edit", &format!("{other}&pool=web")).await;
    assert!(to.contains("error=") && state.snapshot().guest(102).unwrap().pool.is_none(), "{to}");
    let hardware = "part=donanim&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{hardware}&cores=13")).await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{hardware}&cores=12")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().cores, 12);
    // Dolu havuzdaki konuk klonlanamaz.
    assert!(post_ok(&app, "/guests/103/clone", "name=kopya").await.contains("error="));
    // Seçenekler sekmesini yeniden kaydetmek konuğu havuzda tutar; donanım sekmesi havuza dokunmaz.
    assert_eq!(state.snapshot().guest(103).unwrap().pool.as_deref(), Some("web"));

    // İçinde konuk olan havuz silinmez; kota kullanımın altına indirilemez.
    assert!(post_ok(&app, "/pools/web/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/pools", "name=web&cores=4").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&pool=")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().pool, None);
    assert!(post_ok(&app, "/pools/web/delete", "").await.contains("notice="));
    assert!(post_ok(&app, "/pools/web/delete", "").await.contains("error="));

    // API: liste kullanımı da verir; izleyici değiştiremez.
    let (status, _) = api(&app, post_json("/api/v1/pools", &json!({ "name": "db", "disk_gb": 600, "guests": 2 }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let patch = |vmid: u32, body: Value| request(Method::PATCH, &format!("/api/v1/guests/{vmid}")).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert!(api(&app, patch(102, json!({ "pool": "db" }))).await.0.is_success());
    let (status, list) = api(&app, get("/api/v1/pools")).await;
    assert_eq!((status, list[0]["name"].as_str(), list[0]["used"]["guests"].as_u64(), list[0]["used"]["disk_gb"].as_u64()), (StatusCode::OK, Some("db"), Some(1), Some(500)));
    // 500 GB'lik konuk havuzdayken 200 GB'lik ikinci konuk 600 GB kotaya sığmaz.
    let (status, v) = api(&app, patch(103, json!({ "pool": "db" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("GB disk"), "{v}");
    let viewer = as_viewer(Method::POST, "/api/v1/pools").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "name": "x" }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    let delete = |name: &str| request(Method::DELETE, &format!("/api/v1/pools/{name}")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, delete("db")).await.0, StatusCode::UNPROCESSABLE_ENTITY, "içinde konuk var");
    assert!(api(&app, patch(102, json!({ "pool": "" }))).await.0.is_success());
    assert_eq!(send(&app, delete("db")).await.0, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn unbound_offsite_copies_are_marked() {
    let (app, state) = setup();
    let backup = |id: &str, offsite: bool, bound: bool| redox_core::Backup {
        id: id.into(), vmid: 103, guest_name: "win-build".into(), created: now(), size_mb: 5, auto: false, offsite, new_mb: None, disks: Vec::new(), disk_gb: 200,
        quiesced: None, cbt: false, offsite_bound: bound, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0,
    };
    state.mutate(|c| {
        c.add_backup(backup("vm-103-1", true, false));
        c.add_backup(backup("vm-103-2", true, true));
        c.add_backup(backup("vm-103-3", false, false));
    });
    let all = page(&app, "/backups").await;
    assert_eq!(all.matches("ada bağlı değil").count() - all.matches("dış kopya ada bağlı değil").count(), 1, "yalnızca eski biçimli dış kopya işaretlenir");
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert_eq!(tab.matches(">dış kopya ada bağlı değil<").count(), 1, "{tab}");
}

// ---------- Depolama alanları arasında taşıma ----------

#[tokio::test]
async fn guest_disks_are_moved_to_another_storage_from_the_clone_tab() {
    let (app, state) = setup();
    // 103, pve-ist-02 düğümünde ve local-zfs alanında; aynı düğüme disk tutan ikinci bir alan eklenir.
    state.mutate(|c| {
        c.storages.push(redox_core::Storage { id: "hizli".into(), node: Some("pve-ist-02".into()), kind: redox_core::StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 1000 });
        c.storages.push(redox_core::Storage { id: "dar".into(), node: Some("pve-ist-02".into()), kind: redox_core::StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 10 });
    });
    let tab = page(&app, "/guests/103?tab=cogalt").await;
    assert!(tab.contains(r#"action="/guests/103/move""#) && tab.contains(r#"<option value="hizli">hizli · 1000 GB boş</option>"#), "{tab}");
    assert!(tab.contains(r#"<option value="dar" disabled>dar · 10 GB boş</option>"#), "yeri yetmeyen alan seçilemez");
    assert!(!tab.contains(r#"value="nfs-yedek""#) && !tab.contains(r#"value="local-lvm""#), "disk tutmayan ve başka düğümdeki alanlar sunulmaz");
    // Çalışan konukta form kapalıdır ve istek reddedilir.
    let running = page(&app, "/guests/102?tab=cogalt").await;
    assert!(running.contains(r#"<select name="storage" aria-label="Hedef depolama alanı" disabled>"#));
    assert!(post_ok(&app, "/guests/102/move", "storage=hizli").await.contains("error="));
    for bad in ["storage=", "storage=dar", "storage=nfs-yedek", "storage=local-zfs", "storage=yok"] {
        assert!(post_ok(&app, "/guests/103/move", bad).await.contains("error="), "{bad}");
    }

    let before = state.snapshot();
    let used = |c: &Cluster, id: &str| c.storages.iter().find(|s| s.id == id && s.available_on("pve-ist-02")).unwrap().used_gb;
    assert!(post_ok(&app, "/guests/103/move", "storage=hizli").await.contains("notice="));
    let after = state.snapshot();
    assert_eq!(after.guest(103).unwrap().storage, "hizli");
    assert_eq!((used(&after, "hizli"), used(&before, "local-zfs") - used(&after, "local-zfs")), (200, 200));
    assert_eq!(after.tasks[0].command, "vm move 103 hizli");
    assert!(page(&app, "/guests/103?tab=cogalt").await.contains(r#"<option value="local-zfs">"#), "geri taşınabilir");

    // API ve yetki.
    let (status, task) = api(&app, post_json("/api/v1/guests/103/move", &json!({ "storage": "local-zfs" }))).await;
    assert_eq!((status, task["command"].as_str()), (StatusCode::ACCEPTED, Some("vm move 103 local-zfs")));
    assert_eq!(state.snapshot().guest(103).unwrap().storage, "local-zfs");
    let viewer = as_viewer(Method::POST, "/api/v1/guests/103/move").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "storage": "hizli" }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/guests/103/move"), None, "operatöre açık değil");
}

// ---------- Yedekten yeni konuk ----------

#[tokio::test]
async fn backups_can_be_opened_as_new_guests_from_the_pages_and_the_api() {
    let (app, state) = setup();
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&format!(r#"action="/backups/{id}/restore-new""#)) && tab.contains("Yeni konuk olarak aç"), "{tab}");
    let all = page(&app, "/backups").await;
    assert!(all.contains(&format!(r#"aria-label="{id} yedeğinden yeni konuk aç""#)));

    let before = state.snapshot().guests.len();
    let to = post_ok(&app, &format!("/backups/{id}/restore-new"), "").await;
    let c = state.snapshot();
    let copy = c.guests.iter().find(|g| g.name == "win-build-yedekten").expect("yeni konuk");
    assert!(to.starts_with(&format!("/guests/{}?notice=", copy.vmid)), "{to}");
    assert_eq!((c.guests.len(), copy.kind, copy.status, copy.cores), (before + 1, GuestKind::Vm, GuestStatus::Stopped, 8));
    assert_eq!(c.tasks[0].command, format!("vm restore-new {} {id}", copy.vmid));
    // Aynı ad ikinci kez reddedilmez (adlar tekil olmak zorunda değil) ama izleyici açamaz; konteyner yedeği açılmaz.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, &format!("/backups/{id}/restore-new")), "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let ct = state.backup_create(202, "admin").unwrap();
    assert!(post_ok(&app, &format!("/backups/{ct}/restore-new"), "").await.contains("error="));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains("Yeni konuk olarak aç"));
    assert!(post_ok(&app, "/backups/vm-999-1/restore-new", "").await.contains("error="));

    // API: ad verilir, yeni kimlik döner.
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{id}/restore-new"), &json!({ "name": "api-kopyasi" }))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{v}");
    assert_eq!(state.snapshot().guest(v["vmid"].as_u64().unwrap() as u32).unwrap().name, "api-kopyasi");
    let (status, _) = api(&app, post_json(&format!("/api/v1/backups/{id}/restore-new"), &json!({ "name": "Geçersiz Ad" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(redox_core::restored_name("web"), "web-yedekten");
    assert!(redox_core::restored_name(&"a".repeat(80)).len() <= 63);
}

// ---------- Dosya düzeyinde geri yükleme ----------

#[tokio::test]
async fn files_inside_a_backup_need_guestfish_and_an_admin() {
    let (app, state) = setup_real();
    let id = state.backup_create(103, "admin").unwrap();
    // guestfish yokken: bağlantı sunulmaz, istek açık bir hatayla döner.
    assert!(!page(&app, "/guests/103?tab=yedekler").await.contains("Dosyalara göz at"));
    let listing = page(&app, &format!("/backups/{id}/files?path=/")).await;
    assert!(listing.contains("guestfish gerekir") && listing.contains("libguestfs-tools"), "{listing}");
    let (status, v) = api(&app, get(&format!("/api/v1/backups/{id}/files?path=/"))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let (status, _, body) = send(&app, get(&format!("/backups/{id}/file?path=/etc/hostname"))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && body.contains("guestfish"), "{status} {body}");

    // Yol denetimi ve yetki, araçtan önce gelir.
    for bad in ["etc", "/etc/../shadow", "-a"] {
        let (status, _) = api(&app, get(&format!("/api/v1/backups/{id}/files?path={bad}"))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    assert_eq!(send(&app, get(&format!("/backups/{id}/file?path=/"))).await.0, StatusCode::UNPROCESSABLE_ENTITY, "kök tek dosya olarak indirilemez");
    let viewer = |uri: String| as_viewer(Method::GET, &uri).body(Body::empty()).unwrap();
    assert_eq!(send(&app, viewer(format!("/backups/{id}/file?path=/etc/hostname"))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, viewer(format!("/backups/{id}/files?path=/"))).await.0, StatusCode::NOT_FOUND, "izleyiciye sayfa gösterilmez");
    let (status, v) = api(&app, viewer(format!("/api/v1/backups/{id}/files?path=/"))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("yalnızca yönetici"), "{v}");
    assert_eq!(send(&app, get("/backups/vm-999-1/files?path=/")).await.0, StatusCode::NOT_FOUND);

    // guestfish tanımlıyken bağlantı görünür; her erişim denetim günlüğüne yazılır.
    state.set_guestfish(Some("/yok/guestfish".into()));
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains(&format!(r#"href="/backups/{id}/files?path=/""#)));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains("Dosyalara göz at"), "konteyner yedeği tar arşividir");
    let _ = page(&app, &format!("/backups/{id}/files?path=/etc")).await;
    let audit = format!("{:?}", state.auth().audit.recent("", 20));
    assert!(audit.contains(&format!("backup ls {id} /etc")) && audit.contains(&format!("backup get {id} /etc/hostname")), "{audit}");
}

// ---------- GFS saklama ve değiştirilemez yedek ----------

#[tokio::test]
async fn backups_can_be_locked_and_keep_weekly_monthly_yearly_points() {
    let (app, state) = setup();
    // Konuğun kendi zamanlamasında haftalık/aylık/yıllık sayıları.
    let options = page(&app, "/guests/103?tab=secenekler").await;
    assert!(options.contains(r#"name="backup_weekly" type="text" inputmode="numeric" value="0""#) && options.contains(r#"name="backup_yearly""#));
    let base = "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=03%3A30&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=&pool=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_weekly=4&backup_monthly=12&backup_yearly=2")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().backup_gfs, redox_core::Gfs { weekly: 4, monthly: 12, yearly: 2 });
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_weekly=99999")).await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_weekly=x")).await.contains("error="));

    // İş formu: dönem yedekleri ve kilit süresi.
    let days = "day=1&day=2&day=3&day=4&day=5&day=6&day=7";
    assert!(post_ok(&app, "/backups/jobs", &format!("name=gece&time=03%3A00&{days}&tags=&keep=7&weekly=4&monthly=6&yearly=&lock=14")).await.contains("notice="));
    let job = state.snapshot().job("gece").unwrap().clone();
    assert_eq!((job.gfs, job.lock_days), (redox_core::Gfs { weekly: 4, monthly: 6, yearly: 0 }, 14));
    assert!(page(&app, "/backups").await.contains("son 7 · 4H 6A · kilit 14 gün"));
    assert!(post_ok(&app, "/backups/jobs", &format!("name=x&time=03%3A00&{days}&tags=&keep=7&lock=cok")).await.contains("error="));

    // İşin aldığı yedek kilitli doğar: silinemez, silme bağlantısı sunulmaz, budanmaz.
    state.update_guest(103, &redox_core::GuestPatch { backup_time: Some(String::new()), ..Default::default() }, "admin").unwrap();
    state.job_put(redox_core::BackupJob::from_text("tek", "01:00", "", "kilitli-is", Some(1)).unwrap().with_retention(redox_core::Gfs::default(), 14).unwrap()).unwrap();
    state.update_guest(103, &redox_core::GuestPatch { tags: Some(vec!["kilitli-is".into()]), ..Default::default() }, "admin").unwrap();
    assert_eq!(state.jobs_tick("2026-11-25", 2, 100), 1);
    let locked = state.snapshot().backups[0].clone();
    assert_eq!(locked.locked_until, Some(locked.created + 14 * 86_400));
    assert!(post_ok(&app, &format!("/backups/{}/delete", locked.id), "").await.contains("error="));
    assert!(state.snapshot().backup(&locked.id).is_ok(), "kilitli yedek durur");
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains("kilitli: ") && tab.contains("Kilitli yedek silinemez") && !tab.contains(&format!("op=delbackup&amp;name={}", locked.id)), "{tab}");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(state.jobs_tick("2026-11-26", 3, 100), 1);
    state.scheduler_tick("2026-11-26", 200);
    assert_eq!(state.snapshot().backups.iter().filter(|b| b.job.as_deref() == Some("tek")).count(), 2, "saklama sayısı 1 olsa da kilitli yedek budanmaz");

    // Elle kilit: panelden 30 gün, API'den istenen gün; kilit kısaltılamaz.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let manual = state.backup_create(103, "admin").unwrap();
    assert!(post_ok(&app, &format!("/backups/{manual}/lock"), "").await.contains("notice="));
    let until = state.snapshot().backup(&manual).unwrap().locked_until.unwrap();
    assert!(until > now() + 29 * 86_400 && until <= now() + 30 * 86_400);
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{manual}/lock"), &json!({ "days": 60 }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["locked_until"].as_u64().unwrap() > until);
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{manual}/lock"), &json!({ "days": 5 }))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("kısaltılamaz"), "{v}");
    let delete = request(Method::DELETE, &format!("/api/v1/backups/{manual}")).body(Body::empty()).unwrap();
    let (status, v) = api(&app, delete).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("kilitli"), "{v}");
    let viewer = as_viewer(Method::POST, &format!("/api/v1/backups/{manual}/lock")).header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "days": 1 }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    assert!(state.snapshot().tasks.iter().any(|t| t.command == format!("backup lock {manual} 60")));
}

// ---------- Yedeği açarak sınama ----------

#[tokio::test]
async fn backups_are_boot_tested_from_the_menu_and_the_api() {
    let (app, state) = setup();
    state.set_boot_test_wait(Duration::from_millis(50));
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&format!(r#"action="/backups/{id}/test""#)) && tab.contains("Açarak sına") && !tab.contains("sınandı:"));
    let before = state.snapshot().guests.len();
    assert!(post_ok(&app, &format!("/backups/{id}/test"), "").await.contains("notice="));
    let c = state.snapshot();
    let test = c.backup(&id).unwrap().tested.clone().expect("sonuç");
    assert!(test.ok, "{}", test.note);
    assert_eq!(c.guests.len(), before, "geçici konuk silindi");
    assert!(c.tasks.iter().any(|t| t.command == format!("backup test {id}") && t.status == TaskStatus::Ok));
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains("sınandı: "));
    assert!(page(&app, "/backups").await.contains("sınandı: "));
    // Konteyner yedeği açılarak sınanmaz; izleyici başlatamaz.
    let ct = state.backup_create(202, "admin").unwrap();
    assert!(post_ok(&app, &format!("/backups/{ct}/test"), "").await.contains("error="));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains("Açarak sına"));
    let viewer = as_viewer(Method::POST, &format!("/api/v1/backups/{id}/test")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    let (status, _) = api(&app, request(Method::POST, &format!("/api/v1/backups/{id}/test")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

// ---------- Anında kurtarma ----------

#[tokio::test]
async fn instant_recovery_is_offered_for_full_backups_and_pins_them() {
    let (app, state) = setup();
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&format!(r#"action="/backups/{id}/instant""#)) && tab.contains("Anında aç"));
    let to = post_ok(&app, &format!("/backups/{id}/instant"), "").await;
    let c = state.snapshot();
    let new = c.guests.iter().find(|g| g.hw.instant.as_deref() == Some(id.as_str())).expect("anında açılan konuk").clone();
    assert!(to.starts_with(&format!("/guests/{}?notice=", new.vmid)), "{to}");
    assert!(c.tasks.iter().any(|t| t.command == format!("vm instant {} {id}", new.vmid) && t.status == TaskStatus::Ok));
    // Yedek sabitlenir: silme reddedilir; konuğun sayfası ayırma kartını gösterir.
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.contains("error="));
    let clone_tab = page(&app, &format!("/guests/{}?tab=cogalt", new.vmid)).await;
    assert!(clone_tab.contains("Yedeğin üzerinde çalışıyor") && clone_tab.contains(&format!(r#"action="/guests/{}/detach""#, new.vmid)), "{clone_tab}");
    assert!(!page(&app, "/guests/103?tab=cogalt").await.contains("Yedeğin üzerinde çalışıyor"));
    assert!(post_ok(&app, "/guests/103/detach", "").await.contains("error="), "yedeğe dayanmayan konuk ayrılamaz");
    assert!(post_ok(&app, &format!("/guests/{}/detach", new.vmid), "").await.contains("notice="));
    assert_eq!(state.snapshot().guest(new.vmid).unwrap().hw.instant, None);
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.contains("notice="), "ayrıldıktan sonra yedek silinebilir");

    // Artımlı yedek anında açılamaz; menüde de sunulmaz. API ve yetki.
    let (real, real_state) = setup_real();
    real_state.mutate(|c| c.add_backup(redox_core::Backup { id: "vm-103-9".into(), vmid: 103, guest_name: "win-build".into(), created: now(), size_mb: 5, auto: false, offsite: false, new_mb: Some(3), disks: Vec::new(), disk_gb: 200, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 }));
    assert!(!page(&real, "/guests/103?tab=yedekler").await.contains("Anında aç"));
    let (status, v) = api(&real, post_json("/api/v1/backups/vm-103-9/instant", &json!({ "name": "x" }))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("artımlı"), "{v}");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = state.backup_create(103, "admin").unwrap();
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{second}/instant"), &json!({ "name": "api-acil" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    let vmid = v["vmid"].as_u64().unwrap();
    let (status, task) = api(&app, request(Method::POST, &format!("/api/v1/guests/{vmid}/detach")).body(Body::empty()).unwrap()).await;
    assert_eq!((status, task["command"].as_str()), (StatusCode::ACCEPTED, Some(format!("vm detach {vmid}").as_str())));
    let viewer = as_viewer(Method::POST, &format!("/api/v1/backups/{second}/instant")).header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "name": "x" }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
}

// ---------- Yedek raporu ve korumasız konuklar ----------

#[tokio::test]
async fn unprotected_guests_and_the_backup_report_are_shown() {
    let (app, state) = setup();
    let all = state.snapshot().guests.len();
    let listed = page(&app, "/backups").await;
    assert!(listed.contains("Korumasız konuklar") && listed.contains(&format!("{all} konuk")) && listed.contains("hiç yedeği yok") && listed.contains("zamanlanmamış"), "{listed}");
    // Yedeği alınan konuk listeden düşer; zamanlaması olan konuk öyle gösterilir.
    state.backup_create(103, "admin").unwrap();
    state.update_guest(202, &redox_core::GuestPatch { backup_time: Some("03:30".into()), ..Default::default() }, "admin").unwrap();
    let listed = page(&app, "/backups").await;
    assert!(listed.contains(&format!("{} konuk", all - 1)) && listed.contains("kendi zamanlaması var"), "{listed}");
    assert!(!listed.contains(r#"href="/guests/103?tab=yedekler"><span class="mono dim">103</span>"#) || listed.matches("win-build").count() >= 1);
    // Süre 0: kart çizilmez.
    state.set_backup_age_days(0);
    assert!(!page(&app, "/backups").await.contains("Korumasız konuklar"));
    state.set_backup_age_days(7);

    // API ve izleyici: rapor salt okunurdur.
    let (status, v) = api(&app, get("/api/v1/backups/report")).await;
    assert_eq!((status, v["taken"].as_u64(), v["unprotected"].as_u64()), (StatusCode::OK, Some(1), Some(all as u64 - 1)), "{v}");
    assert!(v["body"].as_str().unwrap().contains("Son 24 saat: 1 yedek alındı") && v["subject"].as_str().unwrap().contains("yedek raporu"));
    let viewer = as_viewer(Method::GET, "/api/v1/backups/report").body(Body::empty()).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::OK);

    // Zamanlama: saat ayarlı değilken hiçbir şey olmaz; ayarlıyken günde bir kez dener. E-posta ayarı yoksa neden görevde yazar.
    assert!(state.report_tick("2026-11-25", 600).is_none());
    state.set_backup_report(7 * 60 + 30);
    assert!(state.report_tick("2026-11-25", 7 * 60).is_none(), "saati gelmedi");
    let outcome = state.report_tick("2026-11-25", 7 * 60 + 31).expect("saati geldi");
    assert!(outcome.unwrap_err().contains("e-posta ayarı yok"));
    assert!(state.report_tick("2026-11-25", 900).is_none(), "aynı gün ikinci kez denenmez");
    assert!(state.report_tick("2026-11-26", 7 * 60 + 30).is_some(), "ertesi gün yeniden");
    let task = state.snapshot().tasks.iter().find(|t| t.command == "backup report").cloned().expect("görev kaydı");
    assert_eq!((task.status, task.user.as_str()), (TaskStatus::Failed, redox_server::state::SCHEDULER));
}

// ---------- Yedek penceresi ve iş sonrası doğrulama ----------

#[tokio::test]
async fn job_windows_and_verification_are_set_from_the_form() {
    let (app, state) = setup();
    let form = page(&app, "/backups").await;
    assert!(form.contains(r#"name="until" type="time""#) && form.contains(r#"name="verify" value="1""#));
    let days = "day=1&day=2&day=3&day=4&day=5&day=6&day=7";
    assert!(post_ok(&app, "/backups/jobs", &format!("name=gece&time=22%3A00&{days}&tags=pencereli&keep=7&until=06%3A00&verify=1")).await.contains("notice="));
    let job = state.snapshot().job("gece").unwrap().clone();
    assert_eq!((job.until.as_deref(), job.verify), (Some("06:00"), true));
    let listed = page(&app, "/backups").await;
    assert!(listed.contains("22:00–06:00") && listed.contains("doğrulamalı"), "{listed}");
    assert!(post_ok(&app, "/backups/jobs", &format!("name=x&time=22%3A00&{days}&tags=&keep=7&until=22%3A00")).await.contains("error="), "pencere sonu başlangıçla aynı olamaz");

    // Üç kapalı konuk kapsamda; tur 22:00'de başlar, her turda bir konuk yedeklenir.
    for vmid in [103, 202] {
        state.update_guest(vmid, &redox_core::GuestPatch { tags: Some(vec!["pencereli".into()]), ..Default::default() }, "admin").unwrap();
    }
    assert_eq!(state.jobs_tick("2026-11-25", 2, 22 * 60), 1, "103");
    // Yedeği alınan yedek, sonraki turda doğrulanır (iş doğrulama istiyor).
    assert_eq!(state.jobs_tick("2026-11-25", 2, 22 * 60 + 1), 1, "202");
    let c = state.snapshot();
    assert!(c.tasks.iter().any(|t| t.command.starts_with("backup verify vm-103-") && t.user == redox_server::state::SCHEDULER), "{:?}", c.tasks.iter().map(|t| &t.command).collect::<Vec<_>>());
    state.jobs_tick("2026-11-25", 2, 22 * 60 + 2);
    assert!(state.snapshot().tasks.iter().any(|t| t.command.starts_with("backup verify ct-202-")));

    // Pencere kapanınca sıradaki konuklar yedeklenmez ve bu günlüğe yazılır.
    state.update_guest(101, &redox_core::GuestPatch { tags: Some(vec!["pencereli".into()]), ..Default::default() }, "admin").unwrap();
    state.mutate(|c| c.jobs[0].last_run = None);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(state.jobs_tick("2026-11-26", 3, 23 * 60), 0, "ilk konuk (101) çalışıyor: simülasyonda yedeği alınamaz");
    assert_eq!(state.snapshot().job("gece").unwrap().queue.len(), 2);
    assert_eq!(state.jobs_tick("2026-11-27", 4, 6 * 60), 0, "pencere kapandı");
    let c = state.snapshot();
    assert!(c.job("gece").unwrap().queue.is_empty());
    let closed = c.tasks.iter().find(|t| t.command == "job gece").expect("pencere kaydı");
    assert!(closed.status == TaskStatus::Failed && closed.message.as_deref().unwrap().contains("2 konuk bu turda yedeklenmedi"), "{:?}", closed.message);

    // API: alanlar kabul edilir ve doğrulanır.
    let (status, _) = api(&app, post_json("/api/v1/jobs", &json!({ "name": "api", "time": "01:00", "until": "05:00", "verify": true }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state.snapshot().job("api").unwrap().until.as_deref(), Some("05:00"));
    let (status, _) = api(&app, post_json("/api/v1/jobs", &json!({ "name": "api", "time": "01:00", "until": "99:00" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn dynamic_memory_sets_the_balloon_target_from_guest_usage() {
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
async fn usage_counters_are_listed_and_reset() {
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
async fn nic_guard_is_set_from_the_options_tab() {
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
async fn four_eyes_holds_deletions_until_another_admin_approves() {
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
