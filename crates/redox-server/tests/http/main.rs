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

mod actions;
mod api;
mod assets_metrics;
mod auth;
mod backup_features;
mod console;
mod ft;
mod guest_ops;
mod ha_shares;
mod hardware;
mod health;
mod jobs;
mod mirrors;
mod network;
mod pages;
mod platform;
mod pools_storage;
mod power;
mod real_driver;
mod scheduling;
mod switches;
mod ui_v3;
mod users;

#[allow(unused_imports)]
pub use actions::*;
#[allow(unused_imports)]
pub use api::*;
#[allow(unused_imports)]
pub use assets_metrics::*;
#[allow(unused_imports)]
pub use auth::*;
#[allow(unused_imports)]
pub use backup_features::*;
#[allow(unused_imports)]
pub use console::*;
#[allow(unused_imports)]
pub use guest_ops::*;
#[allow(unused_imports)]
pub use ha_shares::*;
#[allow(unused_imports)]
pub use hardware::*;
#[allow(unused_imports)]
pub use jobs::*;
#[allow(unused_imports)]
pub use network::*;
#[allow(unused_imports)]
pub use pages::*;
#[allow(unused_imports)]
pub use platform::*;
#[allow(unused_imports)]
pub use pools_storage::*;
#[allow(unused_imports)]
pub use power::*;
#[allow(unused_imports)]
pub use real_driver::*;
#[allow(unused_imports)]
pub use scheduling::*;
#[allow(unused_imports)]
pub use ui_v3::*;
#[allow(unused_imports)]
pub use users::*;

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
