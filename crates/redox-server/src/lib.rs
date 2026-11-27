//! RedoX VE sunucusu: JSON API ve sunucuda üretilen (JavaScript'siz) arayüz.
// Çeviri işlevi çağrıları hem maud şablonunda hem olağan ifadede geçerli olsun diye parantezlidir.
#![allow(unused_parens, clippy::double_parens)]

pub mod alerts;
pub mod api;
pub mod audit;
pub mod auth;
pub mod cli;
pub mod console;
pub mod firewall;
pub mod fonts;
pub mod guestfs;
pub mod health;
pub mod ldap;
pub mod tls;
pub mod ha;
pub mod history;
pub mod host;
pub mod hostdriver;
pub mod lxc;
pub mod mail;
pub mod media;
pub mod metrics;
pub mod net;
pub mod netlimit;
pub mod chunkstore;
pub mod offsite;
pub mod oidc;
pub mod ova;
pub mod peers;
pub mod qemu;
pub mod qga;
pub mod report;
pub mod state;
pub mod totp;
pub mod tty;
pub mod ui;
pub mod ui_admin;
pub mod ui_ops;
pub mod view;
pub mod view_admin;
pub mod view_ops;

use std::net::SocketAddr;

pub mod csv;
pub mod openapi;
pub mod vswitch;
pub mod vxlan;
pub mod delta;
pub mod i18n;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::{HeaderName, HeaderValue, AUTHORIZATION, CACHE_CONTROL, COOKIE, HOST, ORIGIN, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use auth::{Identity, SESSION_COOKIE};
pub use state::AppState;

/// Hiçbir betiğe izin vermez; stil, yazı tipi ve görseller yalnızca bu sunucudan yüklenir.
const CSP: &str = "default-src 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

/// İsteği yapan istemcinin adresi (giriş denemesi sınırlayıcısı ve günlük için).
#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(ui::datacenter))
        .route("/login", get(ui::login_form).post(ui::login))
        .route("/logout", post(ui::logout))
        .route("/login/oidc", get(ui::oidc_start))
        .route("/login/oidc/callback", get(ui::oidc_callback))
        .route("/healthz", get(|| async { "ok" }))
        .route("/nodes/{id}", get(ui::node))
        .route("/guests", get(ui::guests).post(ui::create))
        .route("/tasks", get(ui::tasks))
        .route("/backups", get(ui::backups))
        .route("/guests/new", get(ui::new_form))
        .route("/guests/{vmid}", get(ui::guest))
        .route("/guests/{vmid}/action", post(ui::action))
        .route("/guests/{vmid}/delete", get(ui::delete_confirm).post(ui::delete))
        .route("/guests/bulk", post(ui_ops::bulk))
        .route("/guests/{vmid}/edit", get(ui_ops::edit_form).post(ui_ops::edit))
        .route("/guests/{vmid}/resize", post(ui_ops::resize))
        .route("/guests/{vmid}/disks", post(ui_ops::disk_add))
        .route("/guests/{vmid}/nics", post(ui_ops::nic_add))
        .route("/guests/{vmid}/disks/{id}/delete", post(ui_ops::disk_remove))
        .route("/guests/{vmid}/schedules", post(ui_ops::schedule_add))
        .route("/guests/{vmid}/schedules/{index}/delete", post(ui_ops::schedule_delete))
        .route("/guests/{vmid}/clone", post(ui_ops::clone))
        .route("/guests/{vmid}/template", post(ui_ops::template))
        .route("/guests/{vmid}/suspend", post(ui_ops::suspend))
        .route("/guests/{vmid}/discard-suspend", post(ui_ops::discard_suspend))
        .route("/guests/{vmid}/snapshots", post(ui_ops::snapshot_create))
        .route("/guests/{vmid}/snapshots/{name}/{op}", post(ui_ops::snapshot_action))
        .route("/guests/{vmid}/backups", post(ui_ops::backup_create))
        .route("/backups/{id}/{op}", get(ui_ops::backup_get).post(ui_ops::backup_action))
        .route("/backups/jobs", post(ui_ops::job_save))
        .route("/backups/{id}/files", get(ui_ops::backup_files))
        .route("/backups/{id}/file", get(ui_ops::backup_file).post(ui_ops::backup_put))
        .route("/guests/{vmid}/balloon", post(ui_ops::balloon_set))
        .route("/guests/{vmid}/media", post(ui_ops::media_change))
        .route("/guests/{vmid}/cores", post(ui_ops::cores_add))
        .route("/guests/{vmid}/memory", post(ui_ops::memory_add))
        .route("/approvals/{id}/{op}", post(ui_ops::approval_action))
        .route("/lockdown/{op}", post(ui_ops::lockdown_action))
        .route("/maintenance/evacuate", post(ui_ops::maintenance_evacuate))
        .route("/maintenance/{op}", post(ui_ops::maintenance_action))
        .route("/guests/{vmid}/move", post(ui_ops::storage_move))
        .route("/guests/{vmid}/detach", post(ui_ops::guest_detach))
        .route("/backups/jobs/{name}/{op}", post(ui_ops::job_action))
        .route("/guests/{vmid}/confirm", get(ui_ops::confirm))
        .route("/guests/{vmid}/export", get(ui_ops::guest_export))
        .route("/guests/{vmid}/console", get(ui_ops::live_console))
        .route("/guests/{vmid}/vnc", get(ui_ops::vnc))
        .route("/guests/{vmid}/screen", get(ui_ops::screen))
        .route("/guests/{vmid}/screen.png", get(ui_ops::screen_png))
        .route("/guests/{vmid}/keys", post(ui_ops::keys))
        .route("/guests/{vmid}/exec", post(ui_ops::exec))
        .route("/storage", get(ui::storage))
        .route("/pools", get(ui::pools).post(ui_ops::pool_save))
        .route("/pools/{name}/delete", post(ui_ops::pool_delete))
        .route("/media/upload", post(ui_ops::media_upload).layer(axum::extract::DefaultBodyLimit::disable()))
        .route("/media/fetch", post(ui_ops::media_fetch))
        .route("/media/catalog/{key}", post(ui_ops::media_catalog))
        .route("/media/{name}/delete", post(ui_ops::media_delete))
        .route("/cluster", get(ui::peers))
        .route("/support", get(ui_admin::support_bundle))
        .route("/cluster/import", post(ui::peer_import))
        .route("/cluster/migrate", post(ui::peer_migrate))
        .route("/cluster/send", post(ui::peer_send))
        .route("/cluster/switch", post(ui_ops::switch_action))
        .route("/host/update", post(ui_ops::host_update))
        .route("/cluster/firewall", post(ui_ops::firewall_action))
        .route("/cluster/mirror", post(ui_ops::mirror_action))
        .route("/cluster/ft", post(ui_ops::ft_action))
        .route("/cluster/replica", post(ui::replica_action))
        .route("/help", get(ui::help))
        .route("/cmd", post(ui::command))
        .route("/theme", get(ui::theme))
        .route("/lang", get(ui::lang))
        .route("/usage.csv", get(ui_admin::usage_csv))
        .route("/audit.csv", get(ui_admin::audit_csv))
        .route("/assets/{file}", get(ui::asset))
        .route("/assets/fonts/{file}", get(ui_admin::font))
        .route("/metrics", get(ui_admin::metrics))
        .route("/audit", get(ui_admin::audit))
        .route("/users", get(ui_admin::users).post(ui_admin::user_add))
        .route("/users/{name}/role", post(ui_admin::user_role))
        .route("/users/{name}/scope", post(ui_admin::user_scope))
        .route("/users/{name}/password", post(ui_admin::user_password))
        .route("/users/{name}/delete", post(ui_admin::user_delete))
        .route("/users/{name}/totp-reset", post(ui_admin::user_totp_reset))
        .route("/users/{name}/tokens", post(ui_admin::user_token_create))
        .route("/users/{name}/tokens/{label}/revoke", post(ui_admin::user_token_revoke))
        .route("/account", get(ui_admin::account))
        .route("/account/password", post(ui_admin::account_password))
        .route("/account/totp/start", post(ui_admin::totp_start))
        .route("/account/totp/confirm", post(ui_admin::totp_confirm))
        .route("/account/totp/disable", post(ui_admin::totp_disable))
        .route("/account/tokens", post(ui_admin::account_token_create))
        .route("/account/tokens/{label}/revoke", post(ui_admin::account_token_revoke))
        .nest("/api/v1", api::routes())
        .fallback(ui::not_found)
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// İsteğin dili. Sayfalarda: `lang` çerezi, yoksa sunucunun varsayılanı (`--lang`). API'de istemci
/// `Accept-Language` ile de seçebilir (tarayıcının dil başlığı sayfaları kendiliğinden İngilizceye çevirmesin
/// diye sayfalarda bakılmaz).
fn request_lang(st: &AppState, req: &Request) -> i18n::Lang {
    if let Some(lang) = cookie(req.headers(), "lang").and_then(i18n::Lang::parse) {
        return lang;
    }
    let path = req.uri().path();
    if path.starts_with("/api/") {
        if let Some(lang) = req.headers().get(axum::http::header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()).and_then(i18n::Lang::from_accept) {
            return lang;
        }
    }
    st.default_lang()
}

/// Her isteğin geçtiği kapı: site dışı istekleri reddeder, kimliği doğrular,
/// rolü denetler ve yanıtlara güvenlik başlıklarını ekler.
async fn guard(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let outcome = authorize(&st, &mut req);
    // Durum değiştiren her kimlikli istek denetim günlüğüne düşer (gövde kaydedilmez).
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let trail = req.extensions().get::<Identity>().filter(|_| !safe).map(|me| {
        let ip = req.extensions().get::<ClientIp>().map(|c| c.0.clone()).unwrap_or_default();
        (me.user.clone(), ip, format!("{} {}", req.method(), req.uri().path()))
    });
    // İsteğin tamamı seçilen dilde işlenir: sayfalar, bildirimler ve hata iletileri.
    let lang = request_lang(&st, &req);
    let mut res = match outcome {
        Ok(()) => i18n::scope(lang, next.run(req)).await,
        Err(denied) => *denied,
    };
    if let Some((user, ip, action)) = trail {
        st.auth().audit.record(state::now(), &user, &ip, &action, res.status().as_u16() < 400);
    }
    let h = res.headers_mut();
    // Konsol çerçevesi kendi (aynı siteyle sınırlı) çerçeve ilkesini koyar; diğer her şey çerçevelenemez.
    for (name, value) in [("content-security-policy", CSP), ("x-frame-options", "DENY")] {
        if !h.contains_key(name) {
            h.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
        }
    }
    h.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    h.insert(HeaderName::from_static("referrer-policy"), HeaderValue::from_static("same-origin"));
    if !h.contains_key(CACHE_CONTROL) {
        // Kimlik gerektiren sayfalar ara belleklerde kalmasın.
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if st.auth().behind_tls_proxy {
        h.insert(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    res
}

fn authorize(st: &AppState, req: &mut Request) -> Result<(), Box<Response>> {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    // WebSocket açılışı GET'tir ama okuma değildir: başka bir sitenin sayfası, kullanıcının oturumuyla
    // konsola bağlanamamalı.
    let upgrade = req.headers().contains_key(axum::http::header::UPGRADE);
    if (!safe || upgrade) && is_cross_site(req) {
        return Err(Box::new((StatusCode::FORBIDDEN, "Başka bir siteden gelen istek reddedildi.").into_response()));
    }
    let ip = client_ip(req, st.auth().behind_tls_proxy);
    req.extensions_mut().insert(ClientIp(ip));

    let path = req.uri().path().to_string();
    // Dil, giriş yapmadan da değiştirilebilir (giriş sayfasının kendisi için).
    if path == "/lang" || path == "/login" || path == "/login/oidc" || path == "/login/oidc/callback" || path == "/healthz" || path.starts_with("/assets/") {
        return Ok(());
    }
    let is_api = path.starts_with("/api/") || path == "/metrics";
    let Some(me) = identify(st, req.headers()) else {
        return Err(Box::new(if is_api {
            (
                StatusCode::UNAUTHORIZED,
                [(WWW_AUTHENTICATE, "Bearer")],
                Json(json!({ "error": "kimlik doğrulanamadı; Authorization: Bearer <token> başlığı gerekli", "code": "unauthorized" })),
            )
                .into_response()
        } else if safe {
            let target = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
            Redirect::to(&format!("/login?next={}", view::enc(target))).into_response()
        } else {
            Redirect::to("/login").into_response()
        }));
    };
    // Reddedilen girişimler de günlüğe düşsün diye kimlik önce işlenir.
    req.extensions_mut().insert(me.clone());
    if is_api {
        if let Err(wait) = st.api_allowed(&me.user, state::now()) {
            return Err(Box::new(
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(axum::http::header::RETRY_AFTER, wait.to_string())],
                    Json(json!({ "error": format!("çok fazla istek; {wait} saniye sonra yeniden deneyin"), "code": "rate_limited" })),
                )
                    .into_response(),
            ));
        }
    }
    // Salt okunur hesap yalnızca çıkış yapabilir ve kendi hesabını yönetebilir.
    // Operatör yalnızca kapsamındaki konuklarda güç, konsol, anlık görüntü ve yedek işlemi yapabilir.
    let operated = me.role == auth::Role::Operator && operator_target(&path).and_then(|vmid| st.guest_tags(vmid)).is_some_and(|tags| me.can_operate(&tags));
    if !safe && !me.can_write() && !operated && path != "/logout" && !path.starts_with("/account") {
        let message = if me.role == auth::Role::Operator {
            "Bu işlem operatör yetkisini aşıyor ya da konuk yetki kapsamınızda değil."
        } else {
            "Bu işlem için yönetici yetkisi gerekir; hesabınız salt okunur."
        };
        return Err(Box::new(if is_api {
            (StatusCode::FORBIDDEN, Json(json!({ "error": message, "code": "forbidden" }))).into_response()
        } else {
            (StatusCode::FORBIDDEN, message).into_response()
        }));
    }
    // Kilit kipi: yetkisi ne olursa olsun kimse değişiklik yapamaz. Açık kalan yollar: kilidi kaldırmak (yukarıdaki
    // denetim onu zaten yöneticiye bırakır), çıkış ve kişinin kendi hesabı (parola, iki adımlı doğrulama).
    if !safe && st.in_lockdown() && !lockdown_exempt(&path) {
        let message = "Sunucu kilit kipinde: değişiklik yapılamaz. Bir yönetici kilidi kaldırmalı (redox lockdown off).";
        return Err(Box::new(if is_api {
            (StatusCode::LOCKED, Json(json!({ "error": message, "code": "lockdown" }))).into_response()
        } else {
            (StatusCode::LOCKED, message).into_response()
        }));
    }
    Ok(())
}

/// Kilit kipinde de açık kalan, durum değiştiren yollar.
pub fn lockdown_exempt(path: &str) -> bool {
    path == "/logout" || path == "/lockdown/off" || path == "/api/v1/lockdown" || path.starts_with("/account")
        // Kaynağın yansıtma bildirimi yönetim değişikliği değildir; kilitliyken kesilirse hedef kopyayı sahipsiz sanır.
        || (path.starts_with("/api/v1/mirror-stores/") && path.ends_with("/report"))
        // Hata toleransının sunucular arası kararları kilit kipinde de işlemelidir.
        || path == "/api/v1/ft/arbitrate"
        || path.starts_with("/api/v1/ft/standby/")
}

/// Operatöre açık işlemler: yolun hedeflediği konuğun kimliğini döndürür, değilse `None`.
/// Geri dönüş, silme, geri yükleme, ayar ve disk işlemleri bilerek dışarıda bırakılmıştır.
pub fn operator_target(path: &str) -> Option<redox_core::VmId> {
    let (api, rest) = match (path.strip_prefix("/api/v1/guests/"), path.strip_prefix("/guests/")) {
        (Some(rest), _) => (true, rest),
        (_, Some(rest)) => (false, rest),
        _ => return None,
    };
    let (id, op) = rest.split_once('/')?;
    let allowed = if api {
        matches!(op, "start" | "stop" | "reboot" | "pause" | "resume" | "suspend" | "snapshots" | "backups" | "exec")
    } else {
        matches!(op, "action" | "suspend" | "snapshots" | "backups" | "keys" | "exec")
    };
    if allowed { id.parse().ok() } else { None }
}

/// Önce API token'ına, yoksa oturum çerezine bakar.
pub fn identify(st: &AppState, headers: &HeaderMap) -> Option<Identity> {
    if let Some(value) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        return value.strip_prefix("Bearer ").and_then(|t| st.auth().bearer(t.trim()));
    }
    cookie(headers, SESSION_COOKIE).and_then(|t| st.auth().session(t, state::now()))
}

fn client_ip(req: &Request, trust_proxy: bool) -> String {
    if trust_proxy {
        // Vekil kendi gördüğü adresi listenin sonuna ekler; öncekiler istemcinin beyanıdır.
        let forwarded = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok());
        if let Some(last) = forwarded.and_then(|v| v.rsplit(',').next()).map(str::trim).filter(|s| !s.is_empty()) {
            return last.to_string();
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn is_cross_site(req: &Request) -> bool {
    let header = |name: &HeaderName| req.headers().get(name).and_then(|v| v.to_str().ok());
    if header(&HeaderName::from_static("sec-fetch-site")).is_some_and(|v| v == "cross-site") {
        return true;
    }
    match (header(&ORIGIN), header(&HOST)) {
        // Origin "şema://ana-makine[:port]" biçimindedir; Host ile aynı olmalı.
        (Some(origin), Some(host)) => origin.split_once("://").map(|(_, h)| h) != Some(host),
        (Some(_), None) => true,
        // Tarayıcı dışı istemciler (redox CLI, curl) Origin göndermez.
        (None, _) => false,
    }
}
