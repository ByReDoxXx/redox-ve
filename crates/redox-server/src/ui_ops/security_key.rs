//! Güvenlik anahtarı istekleri: girişin ikinci adımı, anahtar ekleme ve kaldırma.

use axum::http::header::{LOCATION, SET_COOKIE};

use super::*;
use crate::auth::{KeyChallenge, KeyResponse, LoginError};
use crate::view::{key_page, KeyStep};
use crate::ClientIp;

/// Ara sayfa: bu sunucunun betiğini yükleyen az sayıdaki sayfadan biri.
pub fn key_step_page(theme: &str, step: KeyStep, challenge: &KeyChallenge) -> Response {
    ([(HeaderName::from_static("content-security-policy"), CONSOLE_CSP)], Html(key_page(theme, step, challenge))).into_response()
}

fn account(result: Result<String, String>) -> Redirect {
    match result {
        Ok(notice) => flash("/account", "notice", &notice),
        // Çekirdek iletileri büyük harf ve nokta eklenmeden önce çevrilir (çevrilmiş metin değişmez).
        Err(e) => flash("/account", "error", &(capitalize(&tfmt(e)) + ".")),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct KeyLoginForm {
    #[serde(flatten)]
    response: KeyResponse,
    next: String,
}

/// Girişin ikinci adımı: kimlik doğrulaması burada tamamlanır (oturum henüz yoktur).
pub async fn login_key(State(st): State<AppState>, headers: HeaderMap, Extension(ClientIp(ip)): Extension<ClientIp>, Form(form): Form<KeyLoginForm>) -> Response {
    let next = crate::ui::safe_back(&form.next).to_string();
    match st.auth().finish_key_login(&ip, &form.response, now()) {
        Ok((token, name)) => {
            println!("giriş (güvenlik anahtarı): {name} ({ip})");
            st.auth().audit.record(now(), &name, &ip, "giriş (güvenlik anahtarı)", true);
            (StatusCode::SEE_OTHER, [(SET_COOKIE, st.auth().session_cookie(&token)), (LOCATION, next)]).into_response()
        }
        Err((name, error, why)) => {
            let name = name.unwrap_or_default();
            println!("başarısız güvenlik anahtarı denemesi: {name:?} ({ip}): {why}");
            st.auth().audit.record(now(), &name, &ip, "giriş (güvenlik anahtarı reddedildi)", false);
            let (status, message) = match error {
                LoginError::Locked(secs) => (StatusCode::TOO_MANY_REQUESTS, tfmt(format!("Çok fazla hatalı deneme. {} dakika sonra yeniden deneyin.", secs.div_ceil(60)))),
                _ => (StatusCode::UNAUTHORIZED, (tx("Güvenlik anahtarı doğrulanamadı; yeniden giriş yapın.")).to_string()),
            };
            (status, Html(crate::view::login(crate::ui::theme_of(&headers), &next, &name, Some(&message), st.oidc().is_some()))).into_response()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct KeyAccountForm {
    label: String,
    id: String,
    current: String,
}

/// Anahtar eklemek ve kaldırmak parola ister: açık bırakılmış bir oturum ikinci adımı değiştiremesin.
pub async fn key_add_start(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Form(f): Form<KeyAccountForm>) -> Response {
    let (worker, user) = (st.clone(), me.user.clone());
    let started = tokio::task::spawn_blocking(move || {
        if !worker.auth().check_password(&user, &f.current) {
            return Err((tx("parola hatalı")).to_string());
        }
        worker.auth().begin_key_registration(&user, &f.label, now())
    })
    .await;
    match started {
        Ok(Ok(challenge)) => key_step_page(crate::ui::theme_of(&headers), KeyStep::Register, &challenge),
        Ok(Err(e)) => account(Err(e)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn key_add_finish(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<KeyResponse>) -> Redirect {
    account(st.auth().finish_key_registration(&me.user, &f, now()).map(|name| tfmt(format!("{name} adlı güvenlik anahtarı eklendi."))))
}

pub async fn key_remove(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<KeyAccountForm>) -> Redirect {
    if !st.auth().check_password(&me.user, &f.current) {
        return account(Err((tx("parola hatalı")).to_string()));
    }
    account(st.auth().edit(|u| u.remove_key(&me.user, &f.id)).map(|name| tfmt(format!("{name} adlı güvenlik anahtarı kaldırıldı."))))
}
