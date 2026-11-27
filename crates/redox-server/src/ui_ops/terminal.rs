//! Konteyner terminali: tarayıcıdaki öykünücü ile konteynerdeki kabuk arasında WebSocket köprüsü.
//!
//! Kabuk, konteynerde **kök** yetkisiyle çalışır; kurallar canlı konsolunkiyle aynıdır (yönetici ya da konuk
//! kapsamındaki operatör, yalnızca bu sitenin sayfasından). Oturum açıkken yetki düzenli aralıkla yeniden
//! denetlenir: çıkış yapılınca, rol ya da kapsam geri alınınca terminal kapanır. Bu denetim web oturumunun
//! hareketsizlik süresini uzatmaz.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Aynı anda açık olabilecek terminal sayısı.
const MAX_SESSIONS: usize = 8;
/// Bu kadar süre tuş gelmeyen terminal kapanır.
const IDLE_SECS: u64 = 30 * 60;
/// Yetkinin yeniden denetlenme aralığı (saniye).
const RECHECK_SECS: u64 = 2;

static OPEN: AtomicUsize = AtomicUsize::new(0);

struct Slot;

impl Slot {
    fn take() -> Option<Slot> {
        let mut open = OPEN.load(Ordering::SeqCst);
        while open < MAX_SESSIONS {
            match OPEN.compare_exchange(open, open + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Slot),
                Err(now) => open = now,
            }
        }
        None
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

pub async fn terminal_page(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Path(raw): Path<String>) -> Response {
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Some(g) = raw.parse::<VmId>().ok().and_then(|id| ctx.cluster.guest(id).ok()) else {
        return ctx.not_found(&tfmt(format!("“{raw}” kimlikli bir konuk yok.")));
    };
    let available = g.kind == redox_core::GuestKind::Ct && g.status == redox_core::GuestStatus::Running && st.driver().has_terminal(g.kind);
    let page = ctx.page(Current::Guest(g.vmid), format!("/guests/{}/terminal", g.vmid));
    ([(HeaderName::from_static("content-security-policy"), CONSOLE_CSP)], Html(view_ops::terminal_page(&page, g, available))).into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Size {
    cols: Option<u16>,
    rows: Option<u16>,
}

/// İsteği yapan hâlâ bu konuğun terminalini kullanabilir mi?
fn still_allowed(st: &AppState, headers: &HeaderMap, raw: &str) -> bool {
    crate::identify_without_touching(st, headers).is_some_and(|me| console_allowed(st, &me, raw).is_ok())
}

pub async fn terminal_socket(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    axum::extract::Query(size): axum::extract::Query<Size>,
    ws: Result<axum::extract::ws::WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    // Yetki, bağlantı yükseltilmeden önce denetlenir.
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let Some(guest) = raw.parse::<VmId>().ok().and_then(|id| st.snapshot().guest(id).ok().cloned()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if guest.kind != redox_core::GuestKind::Ct || guest.status != redox_core::GuestStatus::Running {
        return (StatusCode::CONFLICT, tx("Terminal yalnızca çalışan konteynerde açılır.")).into_response();
    }
    let (cols, rows) = (size.cols.unwrap_or(80).clamp(20, 300), size.rows.unwrap_or(24).clamp(5, 120));
    let (program, args) = match st.driver().terminal_command(&guest, cols, rows) {
        Ok(command) => command,
        Err(e) => return (StatusCode::CONFLICT, capitalize(&e)).into_response(),
    };
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    let Some(slot) = Slot::take() else {
        return (StatusCode::TOO_MANY_REQUESTS, tx("Çok fazla terminal açık; birini kapatıp yeniden deneyin.")).into_response();
    };
    st.auth().audit.record(now(), &me.user, &ip, &format!("terminal {}", guest.vmid), true);
    let vmid = guest.vmid;
    // Dil isteğe bağlıdır: ileti, bağlantı yükseltilmeden önce çevrilir.
    let failure = tx("Terminal programı çalıştırılamadı").to_string();
    ws.max_message_size(64 * 1024).on_upgrade(move |client| async move {
        let _slot = slot;
        let allowed = {
            let (st, headers, raw) = (st.clone(), headers.clone(), raw.clone());
            move || still_allowed(&st, &headers, &raw) && st.snapshot().guest(vmid).is_ok_and(|g| g.status == redox_core::GuestStatus::Running)
        };
        relay(client, program, args, failure, allowed).await;
        st.auth().audit.record(now(), &me.user, &ip, &format!("terminal kapandı {vmid}"), true);
    })
}

#[cfg(unix)]
async fn relay(client: axum::extract::ws::WebSocket, program: String, args: Vec<String>, failure: String, allowed: impl Fn() -> bool + Send + 'static) {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};
    use std::io::{Read, Write};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let (mut to_browser, mut from_browser) = client.split();
    // Kendi süreç grubunda: kapanışta kabukla birlikte altındakiler de sonlandırılır.
    let spawned = Command::new(&program).args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let _ = to_browser.send(Message::Binary(format!("{failure} ({program}): {e}\r\n").into_bytes().into())).await;
            return;
        }
    };
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else { return };
    // Engelleyen okuma ve yazma kendi iş parçacıklarındadır.
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 16 * 1024];
        while let Ok(n @ 1..) = stdout.read(&mut buffer) {
            if output_tx.blocking_send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let (input_tx, input_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        while let Ok(bytes) = input_rx.recv() {
            if stdin.write_all(&bytes).and_then(|()| stdin.flush()).is_err() {
                break;
            }
        }
    });

    let started = tokio::time::Instant::now();
    let mut last_input = started;
    let mut check = tokio::time::interval(std::time::Duration::from_secs(RECHECK_SECS));
    loop {
        tokio::select! {
            output = output_rx.recv() => match output {
                Some(bytes) => {
                    if to_browser.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            message = from_browser.next() => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    last_input = tokio::time::Instant::now();
                    // Kabuk girdiyi tüketmiyorsa sıra dolar: oturum bırakılır.
                    if input_tx.try_send(bytes.to_vec()).is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = check.tick() => {
                if !allowed() || last_input.elapsed().as_secs() >= IDLE_SECS || started.elapsed() >= CONSOLE_SESSION_LIMIT {
                    break;
                }
            }
        }
    }
    let _ = to_browser.send(Message::Close(None)).await;
    // Grubun tamamı: önce kibarca, sonra zorla.
    let group = format!("-{}", child.id());
    let signal = |name: &str| {
        let _ = Command::new("kill").args([name, "--", &group]).stderr(Stdio::null()).status();
    };
    signal("-HUP");
    for _ in 0..20 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    signal("-KILL");
    let _ = child.kill();
    let _ = tokio::task::spawn_blocking(move || child.wait()).await;
}

#[cfg(not(unix))]
async fn relay(_client: axum::extract::ws::WebSocket, _program: String, _args: Vec<String>, _failure: String, _allowed: impl Fn() -> bool + Send + 'static) {
    let _ = (RECHECK_SECS, IDLE_SECS);
}
