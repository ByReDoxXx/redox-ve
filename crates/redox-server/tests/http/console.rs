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
