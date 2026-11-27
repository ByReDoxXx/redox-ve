//! Düğüm sayfasındaki güncelleme isteği.

use super::*;

pub async fn host_update(State(st): State<AppState>, Extension(me): Extension<Identity>) -> Redirect {
    match st.request_update(&me.user) {
        Ok(()) => flash("/", "notice", tx("Güncelleme istendi.")),
        Err(e) => flash("/", "error", &(capitalize(&e.to_string()) + ".")),
    }
}
