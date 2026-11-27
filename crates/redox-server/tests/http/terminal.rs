//! Konteyner terminali: sayfa, yetki kuralları ve (Linux'ta) gerçek bir sözde uçbirimle WebSocket oturumu.
//!
//! Gerçek LXC gerekmez: sahte sürücü, `lxc-attach` yerine yerel `sh`'i LXC sürücüsünün kullandığı `script`
//! sarmalıyla açar.

use super::*;

use redox_server::lxc::pty_args;

#[derive(Clone)]
struct Shells;

impl Hypervisor for Shells {
    fn name(&self) -> &'static str {
        "kabuk-sinama"
    }

    fn is_real(&self) -> bool {
        true
    }

    fn supports(&self, _: GuestKind) -> bool {
        true
    }

    fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
        Ok(())
    }

    fn has_terminal(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Ct
    }

    fn terminal_command(&self, _: &Guest, cols: u16, rows: u16) -> Result<(String, Vec<String>), String> {
        // Tırnak ve boşluk içeren sözcükler de olduğu gibi geçmeli.
        let command = ["/bin/sh".to_string(), "-c".to_string(), "echo \"hazir 'tek tirnak'\"; exec /bin/sh -i".to_string()];
        Ok(("script".to_string(), pty_args(&command, cols, rows)))
    }
}

fn shells() -> (Router, AppState) {
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(Shells), Duration::ZERO, None, auth);
    (app(state.clone()), state)
}

const CT: u32 = 200;

#[tokio::test]
async fn only_the_terminal_page_loads_the_terminal_script_and_the_same_rules_as_the_console_apply() {
    let (app, _) = shells();
    let (status, headers, body) = send(&app, get(&format!("/guests/{CT}/terminal"))).await;
    assert_eq!(status, StatusCode::OK);
    let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("script-src 'self'") && !csp.contains("unsafe-eval") && !csp.contains("script-src 'self' 'unsafe-inline'"), "{csp}");
    assert_eq!(body.matches("<script").count(), 1);
    assert!(body.contains(r#"<script src="/assets/terminal.js?v="#) && body.contains(r#"data-ws="/guests/200/terminal/ws""#) && body.contains("<noscript>"), "{body}");
    let tab = send(&app, get(&format!("/guests/{CT}?tab=konsol"))).await;
    assert!(tab.2.contains(r#"href="/guests/200/terminal""#) && !tab.2.contains("<script"));
    assert!(tab.1.get("content-security-policy").unwrap().to_str().unwrap().contains("script-src 'none'"));
    let (status, headers, js) = send(&app, get("/assets/terminal.js")).await;
    assert_eq!((status, headers.get(CONTENT_TYPE).unwrap().to_str().unwrap()), (StatusCode::OK, "text/javascript; charset=utf-8"));
    assert!(js.contains("class Screen") && !js.contains("eval(") && !js.contains("new Function") && !js.contains("innerHTML") && !js.contains("document.write"));

    let upgrade = |builder: axum::http::request::Builder| builder.header("connection", "upgrade").header("upgrade", "websocket").header("sec-websocket-version", "13").header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==").body(Body::empty()).unwrap();
    let ws = format!("/guests/{CT}/terminal/ws");
    for uri in [format!("/guests/{CT}/terminal"), ws.clone()] {
        assert_eq!(send(&app, as_viewer(Method::GET, &uri).body(Body::empty()).unwrap()).await.0, StatusCode::FORBIDDEN, "{uri}");
    }
    assert_eq!(send(&app, upgrade(as_viewer(Method::GET, &ws))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, upgrade(request(Method::GET, &ws).header("origin", "https://evil.example"))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, upgrade(request(Method::GET, &ws).header("sec-fetch-site", "cross-site"))).await.0, StatusCode::FORBIDDEN);
    // Sanal makinede ve olmayan konukta terminal yoktur.
    assert_eq!(send(&app, upgrade(request(Method::GET, "/guests/100/terminal/ws"))).await.0, StatusCode::CONFLICT);
    assert_eq!(send(&app, upgrade(request(Method::GET, "/guests/999/terminal/ws"))).await.0, StatusCode::NOT_FOUND);
    assert!(page(&app, "/guests/100/terminal").await.contains("Terminal yalnızca çalışan konteynerde"));
}

#[test]
fn terminal_emulator_passes_its_node_tests() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/terminal.test.js");
    match std::process::Command::new("node").arg("--test").arg(&script).output() {
        Ok(out) => assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
        Err(e) => eprintln!("node bulunamadı, öykünücü testleri atlandı: {e}"),
    }
}

#[cfg(unix)]
mod live {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    async fn serve(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await });
        address
    }

    async fn connect(address: std::net::SocketAddr, cookie: &str, query: &str) -> Result<Socket, String> {
        let mut request = format!("ws://{address}/guests/{CT}/terminal/ws{query}").into_client_request().unwrap();
        request.headers_mut().insert("cookie", cookie.parse().unwrap());
        request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
        tokio_tungstenite::connect_async(request).await.map(|(socket, _)| socket).map_err(|e| e.to_string())
    }

    /// `wanted` görülene kadar çıktıyı toplar.
    async fn until(socket: &mut Socket, seen: &mut String, wanted: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !seen.contains(wanted) {
            let message = tokio::time::timeout_at(deadline, socket.next()).await.unwrap_or_else(|_| panic!("{wanted} gelmedi: {seen:?}"));
            match message {
                Some(Ok(Message::Binary(bytes))) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                Some(Ok(_)) => {}
                other => panic!("{wanted} beklenirken bağlantı kapandı ({other:?}): {seen:?}"),
            }
        }
    }

    async fn closes_within(socket: &mut Socket, seconds: u64) -> bool {
        tokio::time::timeout(Duration::from_secs(seconds), async {
            loop {
                match socket.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
        .is_ok()
    }

    async fn type_line(socket: &mut Socket, line: &str) {
        socket.send(Message::Binary(format!("{line}\r").into_bytes().into())).await.unwrap();
    }

    async fn alive_after(pid: &str, wait: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + wait;
        let status = format!("/proc/{pid}/status");
        loop {
            // Beklenen süreç gitmiş ya da yalnızca toplanmayı bekliyor (Z) olmalı.
            let running = std::fs::read_to_string(&status).is_ok_and(|s| !s.lines().any(|l| l.starts_with("State:") && l.contains('Z')));
            if !running || tokio::time::Instant::now() >= deadline {
                return running;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // Tek test: eşzamanlı oturum sınırı sunucu genelindedir; ayrı testler birbirinin sayacını görürdü.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_real_pty_session_works_and_ends_with_its_permission() {
        let (app, state) = shells();
        let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
        let address = serve(app.clone()).await;

        assert!(connect(address, "redox_session=sahte", "").await.is_err(), "oturumsuz bağlanılamaz");
        let mut socket = connect(address, &cookie, "?cols=100&rows=30").await.unwrap();
        let mut seen = String::new();
        until(&mut socket, &mut seen, "hazir 'tek tirnak'").await;
        // Bitiş işareti hesaplanır: yazılan satırın ekrandaki yankısında "SON-42" geçmez.
        type_line(&mut socket, "echo SONUC-$((40+2)); tty; stty size; echo TERM=$TERM; echo SON-$((6*7))").await;
        until(&mut socket, &mut seen, "SON-42").await;
        assert!(seen.contains("SONUC-42") && seen.contains("/dev/pts/"), "gerçek uçbirim: {seen}");
        assert!(seen.contains("30 100") && seen.contains("TERM=xterm"), "boyut ve tür: {seen}");
        // Ctrl+C süren komutu keser: uçbirim denetimi çalışıyor.
        type_line(&mut socket, "sleep 30; echo BITMEMELI").await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        socket.send(Message::Binary(vec![3u8].into())).await.unwrap();
        type_line(&mut socket, "echo SONRA-$((1+1))").await;
        seen.clear();
        until(&mut socket, &mut seen, "SONRA-2").await;
        type_line(&mut socket, "sleep 300 & echo ARKADA-$!-ARKADA; echo KABUK-$$-KABUK; echo SON-$((6*7))").await;
        seen.clear();
        until(&mut socket, &mut seen, "SON-42").await;
        let number = |tag: &str| seen.split(&format!("{tag}-")).filter_map(|p| p.split(&format!("-{tag}")).next()).find(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).unwrap_or_else(|| panic!("{seen}")).to_string();
        let (shell, background) = (number("KABUK"), number("ARKADA"));

        // Tarayıcı kapanınca kabuk ve ön planda çalışan komut kalmaz. `&` ile arka plana atılan iş, SSH'de olduğu
        // gibi sürer: kabuk onu ayrı bir süreç grubunda tutar.
        type_line(&mut socket, "sleep 271").await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        let foreground = || {
            std::fs::read_dir("/proc").unwrap().flatten().filter_map(|e| e.file_name().into_string().ok()).find(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c == b"sleep\x00271\x00"))
        };
        let running = foreground().expect("ön plandaki komut çalışıyor");
        socket.close(None).await.unwrap();
        drop(socket);
        assert!(!alive_after(&shell, Duration::from_secs(8)).await, "kabuk sürüyor");
        assert!(!alive_after(&running, Duration::from_secs(8)).await, "ön plandaki komut sürüyor");
        let _ = std::process::Command::new("kill").arg(&background).status();

        // Kabuktan çıkınca bağlantı kapanır.
        let mut socket = connect(address, &cookie, "").await.unwrap();
        seen.clear();
        until(&mut socket, &mut seen, "hazir").await;
        type_line(&mut socket, "exit").await;
        assert!(closes_within(&mut socket, 8).await);

        // Aynı anda en fazla sekiz oturum.
        let mut open = Vec::new();
        for _ in 0..8 {
            let mut socket = connect(address, &cookie, "").await.unwrap();
            let mut seen = String::new();
            until(&mut socket, &mut seen, "hazir").await;
            open.push(socket);
        }
        assert!(connect(address, &cookie, "").await.err().is_some_and(|e| e.contains("429")));

        // Konuk durunca ve oturum kapatılınca açık terminaller kapanır.
        state.mutate(|c| c.guest_mut_for_import(CT).unwrap().status = GuestStatus::Stopped);
        for socket in &mut open {
            assert!(closes_within(socket, 10).await, "duran konuğun terminali kapanır");
        }
        state.mutate(|c| c.guest_mut_for_import(CT).unwrap().status = GuestStatus::Running);
        // Yer, oturumun süreçleri sonlandırılınca boşalır.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut socket = loop {
            match connect(address, &cookie, "").await {
                Ok(socket) => break socket,
                Err(e) => assert!(tokio::time::Instant::now() < deadline, "sınırdaki yerler boşalmadı: {e}"),
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        let mut seen = String::new();
        until(&mut socket, &mut seen, "hazir").await;
        let token = cookie.split_once('=').unwrap().1.split(';').next().unwrap().to_string();
        state.auth().logout(&token);
        assert!(closes_within(&mut socket, 10).await, "çıkış yapılınca terminal kapanır");
        let log = state.auth().audit.recent("terminal", 50);
        assert!(log.len() >= 4, "açılış ve kapanış günlükte: {log:?}");
    }
}
