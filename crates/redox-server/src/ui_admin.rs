//! Yönetim işleyicileri: kullanıcılar, hesabım, denetim günlüğü, ölçümler ve gömülü yazı tipleri.

#[allow(unused_imports)]
use crate::i18n::{tfmt, tx};
use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use serde::Deserialize;

use crate::auth::{Identity, Role};
use crate::state::{now, AppState};
use crate::ui::{capitalize, flash, Ctx, PageQuery};
use crate::view::Current;
use crate::view_admin::{self, TotpSetup};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AdminForm {
    name: String,
    password: String,
    current: String,
    role: String,
    scope: String,
    label: String,
    code: String,
    confirm: Option<String>,
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, (tx("Bu sayfa için yönetici yetkisi gerekir."))).into_response()
}

/// Sonucu ilgili sayfaya bildirim olarak döndürür.
fn done(back: &str, result: Result<String, String>) -> Redirect {
    match result {
        Ok(notice) => flash(back, "notice", &notice),
        Err(e) => flash(back, "error", &(capitalize(&e) + ".")),
    }
}

pub async fn users(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    if !me.can_write() {
        return forbidden();
    }
    let ctx = Ctx::new(&st, &headers, q, &me);
    Html(view_admin::users_page(&ctx.page(Current::Other, "/users"), &st.auth().list())).into_response()
}

pub async fn user_add(State(st): State<AppState>, Form(f): Form<AdminForm>) -> Redirect {
    let role = match f.role.parse::<Role>() {
        Ok(r) => r,
        Err(e) => return done("/users", Err(e)),
    };
    let (name, cost) = (f.name.trim().to_string(), st.auth().cost());
    done("/users", st.auth().edit(|u| u.add_user(&name, &f.password, role, cost)).map(|()| format!("{name} eklendi.")))
}

pub async fn user_role(State(st): State<AppState>, Path(name): Path<String>, Form(f): Form<AdminForm>) -> Redirect {
    let result = f.role.parse::<Role>().and_then(|role| st.auth().edit(|u| u.set_role(&name, role)));
    done("/users", result.map(|()| tfmt(format!("{name} için rol değiştirildi."))))
}

pub async fn user_scope(State(st): State<AppState>, Path(name): Path<String>, Form(f): Form<AdminForm>) -> Redirect {
    let tags = redox_core::parse_tags(&f.scope);
    let summary = if tags.is_empty() { (tx("bütün konuklar")).to_string() } else { tags.join(", ") };
    done("/users", st.auth().edit(|u| u.set_scope(&name, tags)).map(|()| tfmt(format!("{name} için yetki kapsamı: {summary}."))))
}

/// Yol `/users/-/…` ise kullanıcı formdaki `name` alanından alınır (seçim kutulu formlar).
fn target(path_name: String, f: &AdminForm) -> String {
    if path_name == "-" { f.name.trim().to_string() } else { path_name }
}

pub async fn user_password(State(st): State<AppState>, Path(name): Path<String>, Form(f): Form<AdminForm>) -> Redirect {
    let (name, cost) = (target(name, &f), st.auth().cost());
    done("/users", st.auth().edit(|u| u.set_password(&name, &f.password, cost)).map(|()| tfmt(format!("{name} için parola sıfırlandı."))))
}

pub async fn user_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>, Form(f): Form<AdminForm>) -> Redirect {
    if name == me.user {
        return done("/users", Err((tx("kendi hesabınızı silemezsiniz")).to_string()));
    }
    if f.confirm.is_none() {
        return done("/users", Err((tx("silmek için “Eminim” kutusunu işaretleyin")).to_string()));
    }
    done(
        "/users",
        st.request_user_remove(&name, &me.user).map_err(|e| e.to_string()).map(|queued| match queued {
            Some(n) => tfmt(format!("{name} silinmedi: ikinci bir yöneticinin onayı bekleniyor (istek #{n}).")),
            None => format!("{name} silindi."),
        }),
    )
}

pub async fn user_totp_reset(State(st): State<AppState>, Path(name): Path<String>) -> Redirect {
    done("/users", st.auth().edit(|u| u.disable_totp(&name)).map(|()| tfmt(format!("{name} için iki adımlı doğrulama kapatıldı."))))
}

fn create_token(st: &AppState, me: &Identity, headers: &HeaderMap, user: &str, label: &str, back: &str) -> Response {
    match st.auth().edit(|u| u.create_token(user, label)) {
        Ok(token) => {
            let ctx = Ctx::new(st, headers, PageQuery::default(), me);
            Html(view_admin::token_created(&ctx.page(Current::Other, back), user, label, &token, back)).into_response()
        }
        Err(e) => done(back, Err(e)).into_response(),
    }
}

pub async fn user_token_create(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Form(f): Form<AdminForm>,
) -> Response {
    create_token(&st, &me, &headers, &target(name, &f), f.label.trim(), "/users")
}

pub async fn user_token_revoke(State(st): State<AppState>, Path((name, label)): Path<(String, String)>) -> Redirect {
    done("/users", st.auth().edit(|u| u.revoke_token(&name, &label)).map(|()| tfmt(format!("{label} token'ı iptal edildi."))))
}

fn qr_svg(data: &str) -> String {
    use qrcode::render::svg;
    qrcode::QrCode::new(data.as_bytes())
        .map(|code| code.render::<svg::Color>().min_dimensions(220, 220).dark_color(svg::Color("#000000")).light_color(svg::Color("#ffffff")).build())
        .unwrap_or_default()
}

pub async fn account(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    let ctx = Ctx::new(&st, &headers, q, &me);
    let Some(user) = st.auth().list().into_iter().find(|u| u.name == me.user) else {
        return ctx.not_found((tx("Hesabınız bulunamadı.")));
    };
    let setup = user.totp_pending.as_ref().filter(|_| user.totp_secret.is_none()).map(|secret| TotpSetup {
        secret: secret.clone(),
        qr_svg: qr_svg(&crate::totp::otpauth_uri("RedoX VE", &user.name, secret)),
    });
    Html(view_admin::account_page(&ctx.page(Current::Other, "/account"), &user, setup.as_ref())).into_response()
}

pub async fn account_password(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<AdminForm>) -> Response {
    if !st.auth().check_password(&me.user, &f.current) {
        return done("/account", Err((tx("şimdiki parola hatalı")).to_string())).into_response();
    }
    let cost = st.auth().cost();
    match st.auth().edit(|u| u.set_password(&me.user, &f.password, cost)) {
        // Parola özeti değiştiği için bütün oturumlar düşer; çerezi de temizle.
        Ok(()) => (StatusCode::SEE_OTHER, [(SET_COOKIE, st.auth().clear_cookie()), (LOCATION, "/login".to_string())]).into_response(),
        Err(e) => done("/account", Err(e)).into_response(),
    }
}

pub async fn totp_start(State(st): State<AppState>, Extension(me): Extension<Identity>) -> Redirect {
    done("/account", st.auth().edit(|u| u.begin_totp(&me.user)).map(|_| (tx("Kodu okutup uygulamadaki 6 haneli kodu girin.")).to_string()))
}

pub async fn totp_confirm(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<AdminForm>) -> Redirect {
    done("/account", st.auth().edit(|u| u.confirm_totp(&me.user, &f.code, now())).map(|()| (tx("İki adımlı doğrulama açıldı.")).to_string()))
}

pub async fn totp_disable(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<AdminForm>) -> Redirect {
    if !st.auth().check_password(&me.user, &f.current) {
        return done("/account", Err((tx("parola hatalı")).to_string()));
    }
    done("/account", st.auth().edit(|u| u.disable_totp(&me.user)).map(|()| (tx("İki adımlı doğrulama kapatıldı.")).to_string()))
}

pub async fn account_token_create(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Form(f): Form<AdminForm>) -> Response {
    create_token(&st, &me, &headers, &me.user, f.label.trim(), "/account")
}

pub async fn account_token_revoke(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(label): Path<String>) -> Redirect {
    done("/account", st.auth().edit(|u| u.revoke_token(&me.user, &label)).map(|()| tfmt(format!("{label} token'ı iptal edildi."))))
}

fn download(name: &str, body: String) -> Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        body,
    )
        .into_response()
}

/// Bütün konukların kaynak ölçümü, CSV olarak (oturumu açık herkes okuyabilir: sayfalarda da görünen veridir).
pub async fn usage_csv(State(st): State<AppState>) -> Response {
    download("redox-usage.csv", crate::csv::usage(&st.snapshot()))
}

/// Denetim günlüğü, CSV olarak (yalnızca yönetici). `q` sayfadaki süzgeçle aynıdır.
pub async fn audit_csv(State(st): State<AppState>, Extension(me): Extension<Identity>, Query(q): Query<PageQuery>) -> Response {
    if !me.can_write() {
        return forbidden();
    }
    download("redox-audit.csv", crate::csv::audit(&st.auth().audit.recent(q.q.trim(), 5000)))
}

pub async fn audit(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Query(q): Query<PageQuery>) -> Response {
    if !me.can_write() {
        return forbidden();
    }
    let ctx = Ctx::new(&st, &headers, q, &me);
    let page = ctx.page(Current::Other, "/audit");
    let entries = st.auth().audit.recent(page.q, 300);
    Html(view_admin::audit_page(&page, &entries, page.q)).into_response()
}

/// Prometheus ölçümleri (metin biçimi). Kimlik doğrulama gerekir; izleyici token'ı yeterlidir.
pub async fn metrics(State(st): State<AppState>) -> Response {
    ([(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], crate::metrics::render(&st.snapshot())).into_response()
}

pub async fn font(Path(file): Path<String>) -> Response {
    if file == "fonts.css" {
        return ([(CONTENT_TYPE, "text/css; charset=utf-8"), (CACHE_CONTROL, "public, max-age=3600")], crate::fonts::FONTS_CSS).into_response();
    }
    match crate::fonts::FONTS.iter().find(|(name, _)| *name == file) {
        // Dosya adları içerikle birlikte değişmez; uzun süre önbellekte kalabilir.
        Some((_, bytes)) => ([(CONTENT_TYPE, "font/woff2"), (CACHE_CONTROL, "public, max-age=31536000, immutable")], *bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
