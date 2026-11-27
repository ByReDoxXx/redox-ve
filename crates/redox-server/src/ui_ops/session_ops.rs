//! Oturum kapatma istekleri.

use super::*;
use crate::auth::SESSION_COOKIE;
use crate::i18n::tfmt;

pub async fn session_close(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Redirect {
    if st.auth().close_session(&me.user, &id, now()) {
        flash("/account", "notice", tx("Oturum kapatıldı."))
    } else {
        flash("/account", "error", tx("Oturum bulunamadı."))
    }
}

pub async fn sessions_close_others(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap) -> Redirect {
    // Hangi oturumun korunacağı bilinmeden "diğerleri" kapatılamaz.
    let Some(token) = crate::cookie(&headers, SESSION_COOKIE) else {
        return flash("/account", "error", tx("Oturum bulunamadı."));
    };
    let closed = st.auth().close_other_sessions(&me.user, token, now());
    flash("/account", "notice", &tfmt(format!("{closed} oturum kapatıldı.")))
}

/// Yönetici bir kullanıcının bütün oturumlarını kapatır (kayıp cihaz, şüpheli giriş).
pub async fn user_sessions_close(State(st): State<AppState>, Path(name): Path<String>) -> Redirect {
    let closed = st.auth().close_all_sessions(&name, now());
    flash("/users", "notice", &tfmt(format!("{name} için {closed} oturum kapatıldı.")))
}
