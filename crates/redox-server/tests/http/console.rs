//! Canlı konsol.

use super::*;

#[tokio::test]
pub(super) async fn live_console_is_the_only_page_with_a_script() {
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
pub(super) async fn live_console_needs_permission_and_a_same_site_page() {
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

#[tokio::test]
async fn the_sound_button_appears_only_for_a_guest_with_a_sound_card() {
    let (app, state) = setup_real();
    assert!(!page(&app, "/guests/102/console").await.contains("console-sound"));
    // Kart yalnızca konuk kapalıyken takılır.
    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/102").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert_eq!(send(&app, patch(json!({ "sound": true }))).await.0, StatusCode::CONFLICT);
    assert!(!state.snapshot().guest(102).unwrap().hw.sound);

    let base = "name=db&tags=&notes=&iso=&boot=disk&backup_interval=24h&keep=0&forwards=&hardware=1&cores=4&memory_mb=16384&bridge=nat";
    assert!(settings(&app, "/guests/103").await.contains(r#"name="sound" value="1""#));
    let done = post_ok(&app, "/guests/103/edit", &format!("{base}&sound=1")).await;
    assert!(done.contains("notice="), "{done}");
    assert!(state.snapshot().guest(103).unwrap().hw.sound);
    assert!(post_ok(&app, "/guests/103/edit", base).await.contains("notice="));
    assert!(!state.snapshot().guest(103).unwrap().hw.sound);

    // Çalışan konukta kart varsa konsol sayfası düğmeyi ve işareti taşır.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&sound=1")).await.contains("notice="));
    assert!(post_ok(&app, "/guests/103/action", "action=start").await.contains("notice="));
    for _ in 0..200 {
        if state.snapshot().guest(103).unwrap().status == redox_core::GuestStatus::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let console = page(&app, "/guests/103/console").await;
    assert!(console.contains(r#"id="console-sound""#) && console.contains(r#"data-sound="1""#) && console.contains("Sesi Aç"), "{console}");
}

#[test]
pub(super) fn console_client_logic_passes_its_node_tests() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/console.test.js");
    match std::process::Command::new("node").arg("--test").arg(&script).output() {
        Ok(out) => assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
        // Node yoksa istemci testleri atlanır; sunucu tarafı yine sınanır.
        Err(e) => eprintln!("node bulunamadı, istemci testleri atlandı: {e}"),
    }
}

// ---------- Kendiliğinden yeniden başlatma ----------

#[cfg(unix)]
mod live {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    /// Konsol soketi gerçekten açılan sahte sürücü.
    struct WithConsole(std::path::PathBuf);

    impl Hypervisor for WithConsole {
        fn name(&self) -> &'static str {
            "qemu-kvm"
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
        fn vnc_socket(&self, g: &Guest) -> Option<std::path::PathBuf> {
            (g.kind == GuestKind::Vm && g.status == GuestStatus::Running).then(|| self.0.clone())
        }
    }

    /// Açık bir konsol, oturumu kapatılan (ya da yetkisi alınan) kullanıcıya açık kalmaz.
    #[tokio::test]
    async fn an_open_console_closes_when_the_session_ends() {
        let dir = std::env::temp_dir().join(format!("redox-konsol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("102.vnc");
        let guest = tokio::net::UnixListener::bind(&socket).unwrap();
        // Konuk tarafı: gelen baytı geri yollar.
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut stream, _)) = guest.accept().await {
                tokio::spawn(async move {
                    let mut chunk = [0u8; 64];
                    while let Ok(n) = stream.read(&mut chunk).await {
                        if n == 0 || stream.write_all(&chunk[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        let auth = Auth::new(users(), None, false, Cost::Test);
        let state = AppState::new(Cluster::demo(now()), Box::new(WithConsole(socket)), Duration::ZERO, None, auth);
        let router = app(state.clone());
        let cookie = login_cookie(&router, "admin", ADMIN_PW).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await });

        let mut request = format!("ws://{address}/guests/102/vnc").into_client_request().unwrap();
        request.headers_mut().insert("cookie", cookie.parse().unwrap());
        request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
        let (mut console, _) = tokio_tungstenite::connect_async(request).await.unwrap();

        // Bağlantı çalışıyor ve yetki sürdükçe açık kalıyor (denetim aralığından uzun süre).
        console.send(Message::Binary(b"RFB".to_vec().into())).await.unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(10), console.next()).await.unwrap();
        assert!(matches!(echoed, Some(Ok(Message::Binary(bytes))) if &bytes[..] == b"RFB"));
        assert!(tokio::time::timeout(Duration::from_secs(5), console.next()).await.is_err(), "yetki sürerken konsol kapanmaz");

        let token = cookie.split_once('=').unwrap().1.split(';').next().unwrap().to_string();
        state.auth().logout(&token);
        let closed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match console.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "oturum kapanınca konsol da kapanır");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
