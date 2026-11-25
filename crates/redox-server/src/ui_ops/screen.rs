//! Canlı konsol, ekran görüntüsü, tuş gönderme ve komut çalıştırma.

use super::*;

/// Konsol: yöneticiler ve konuk kapsamındaki operatörler.
pub(super) fn console_allowed(st: &AppState, me: &Identity, raw: &str) -> Result<(), Box<Response>> {
    let tags = raw.parse::<VmId>().ok().and_then(|vmid| st.guest_tags(vmid)).unwrap_or_default();
    if me.can_operate(&tags) {
        Ok(())
    } else {
        // Ekranda parola gibi gizli bilgiler görünebilir; salt okunur hesaplar konsolu göremez.
        Err(Box::new((StatusCode::FORBIDDEN, "Konsol için yönetici ya da bu konukta operatör yetkisi gerekir.").into_response()))
    }
}

/// Canlı konsol sayfasının içerik ilkesi: betik yalnızca bu sunucudan (konsol istemcisi), bağlantı yalnızca
/// bu sunucuya. Satır içi betik ve başka kaynak yine yasaktır. Uygulamanın geri kalanı betiksizdir.
pub const CONSOLE_CSP: &str = "default-src 'self'; script-src 'self'; connect-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

/// Canlı konsol sayfası: konuğun ekranı tuvalde, giriş WebSocket üzerinden.
pub async fn live_console(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Path(raw): Path<String>) -> Response {
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Some(g) = raw.parse::<VmId>().ok().and_then(|id| ctx.cluster.guest(id).ok()) else {
        return ctx.not_found(&format!("“{raw}” kimlikli bir konuk yok."));
    };
    let available = st.driver().vnc_socket(g).is_some();
    let page = ctx.page(Current::Guest(g.vmid), format!("/guests/{}/console", g.vmid));
    ([(HeaderName::from_static("content-security-policy"), CONSOLE_CSP)], Html(view_ops::live_console_page(&page, g, available))).into_response()
}

/// Canlı konsolun WebSocket ucu: tarayıcı ile konuğun yerel VNC soketi arasında baytları taşır.
pub async fn vnc(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(raw): Path<String>,
    ws: Result<axum::extract::ws::WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    // Yetki, bağlantı yükseltilmeden önce denetlenir.
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let Some(guest) = raw.parse::<VmId>().ok().and_then(|id| st.snapshot().guest(id).ok().cloned()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(socket) = st.driver().vnc_socket(&guest) else {
        return (StatusCode::CONFLICT, "Konuk çalışmıyor ya da canlı konsolu yok (bir kez kapatıp açın).").into_response();
    };
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    st.auth().audit.record(now(), &me.user, &ip, &format!("console {}", guest.vmid), true);
    ws.on_upgrade(move |client| bridge(client, socket))
}

/// Bir konsol oturumunun en uzun süresi; dolunca bağlantı kapanır (sayfa yenilenerek yeniden açılır).
pub(super) const CONSOLE_SESSION_LIMIT: std::time::Duration = std::time::Duration::from_secs(12 * 3600);

#[cfg(unix)]
pub(super) async fn bridge(client: axum::extract::ws::WebSocket, socket: std::path::PathBuf) {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(stream) = tokio::net::UnixStream::connect(&socket).await else { return };
    let (mut from_guest, mut to_guest) = stream.into_split();
    let (mut to_browser, mut from_browser) = client.split();
    let up = async {
        while let Some(Ok(message)) = from_browser.next().await {
            match message {
                Message::Binary(bytes) => {
                    if to_guest.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let down = async {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match from_guest.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if to_browser.send(Message::Binary(buffer[..n].to_vec().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    // Bir yön kapanınca öbürü de bırakılır: iki uç da düşer.
    let _ = tokio::time::timeout(CONSOLE_SESSION_LIMIT, async {
        tokio::select! {
            _ = up => {}
            _ = down => {}
        }
    })
    .await;
}

#[cfg(not(unix))]
pub(super) async fn bridge(_client: axum::extract::ws::WebSocket, _socket: std::path::PathBuf) {
    let _ = CONSOLE_SESSION_LIMIT;
}

/// Konsol çerçevesi: yalnızca aynı siteden çerçevelenebilir.
pub async fn screen(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Response {
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let Ok(vmid) = raw.parse::<VmId>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (HeaderName::from_static("x-frame-options"), "SAMEORIGIN"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'self'",
            ),
        ],
        Html(view_ops::console_frame(vmid, now())),
    )
        .into_response()
}

pub async fn screen_png(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Response {
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let Ok(vmid) = raw.parse::<VmId>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Ekran dökümü bloklayan G/Ç yapar (QMP + dosya).
    match tokio::task::spawn_blocking(move || st.screenshot(vmid)).await {
        Ok(Ok(png)) => ([(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")], png).into_response(),
        Ok(Err(Error::GuestNotFound(_))) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => (StatusCode::SERVICE_UNAVAILABLE, capitalize(&e.to_string())).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct KeysForm {
    text: String,
    enter: Option<String>,
    key: String,
}

pub async fn keys(State(st): State<AppState>, Extension(_me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<KeysForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=konsol");
    let chords = if !f.key.is_empty() {
        match console::special_key(&f.key) {
            Some(chord) => vec![chord],
            None => return flash(&back, "error", &format!("Bilinmeyen tuş: {}", f.key)),
        }
    } else {
        let mut chords = match console::text_to_chords(&f.text) {
            Ok(c) => c,
            Err(c) => return flash(&back, "error", &format!("“{c}” karakteri gönderilemiyor; yalnızca ABD klavyesindeki karakterler desteklenir.")),
        };
        if f.enter.is_some() {
            chords.push(vec!["ret".to_string()]);
        }
        chords
    };
    if chords.is_empty() {
        return Redirect::to(&back);
    }
    if chords.len() > console::MAX_CHORDS {
        return flash(&back, "error", &format!("Tek seferde en fazla {} karakter gönderilebilir.", console::MAX_CHORDS));
    }
    match tokio::task::spawn_blocking(move || st.send_keys(vmid, &chords)).await {
        Ok(Ok(())) => Redirect::to(&back),
        Ok(Err(e)) => failed(&back, &e),
        Err(_) => flash(&back, "error", "Tuşlar gönderilemedi."),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExecForm {
    command: String,
}

/// Konteynerde komut çalıştırır ve çıktısını gösterir.
pub async fn exec(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Form(f): Form<ExecForm>,
) -> Response {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r.into_response(),
    };
    let command = f.command.trim().to_string();
    if command.is_empty() {
        return flash(&format!("/guests/{vmid}?tab=konsol"), "error", "Bir komut yazın.").into_response();
    }
    let (state, user, cmd) = (st.clone(), me.user.clone(), command.clone());
    // Komut 20 saniyeye kadar sürebilir: bloklayan iş parçacığında çalışır.
    let result = match tokio::task::spawn_blocking(move || state.exec(vmid, &cmd, &user)).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(Error::GuestNotFound(_))) => return failed("/", &Error::GuestNotFound(vmid)).into_response(),
        Ok(Err(Error::Driver(message))) => Err(message),
        Ok(Err(e)) => return failed(&format!("/guests/{vmid}?tab=konsol"), &e).into_response(),
        Err(_) => Err("komut çalıştırılamadı".to_string()),
    };
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Ok(g) = ctx.cluster.guest(vmid) else {
        return ctx.not_found("Konuk bulunamadı.");
    };
    let page = ctx.page(Current::Guest(vmid), format!("/guests/{vmid}?tab=konsol"));
    Html(view_ops::exec_page(&page, g, &command, &result)).into_response()
}
