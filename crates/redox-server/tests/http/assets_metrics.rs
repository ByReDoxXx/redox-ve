//! Gömülü yazı tipleri ve ölçümler.

use super::*;

pub(super) async fn send_bytes(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    (parts.status, parts.headers, body.collect().await.unwrap().to_bytes().to_vec())
}

#[tokio::test]
pub(super) async fn fonts_are_served_locally_and_nothing_loads_from_third_parties() {
    let (app, _) = setup();
    let (status, headers, css) = send(&app, anon(Method::GET, "/assets/fonts/fonts.css").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "yazı tipleri giriş sayfasında da gerekir; kimlik istemez");
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "text/css; charset=utf-8");
    assert!(css.contains("font-family:'Chakra Petch'") && css.contains("font-family:'JetBrains Mono'") && css.contains("U+0100-02BA") && css.contains("U+0131"), "Türkçe harfler (Ğ, Ş, İ: latin-ext aralığı; ı) dahil");
    assert!(!css.contains("http"), "stil dosyası dış adres içermemeli");

    let files: Vec<&str> = css.split("url(/assets/fonts/").skip(1).map(|part| part.split(')').next().unwrap()).collect();
    assert!(files.len() >= 10, "{}", files.len());
    for file in files {
        let (status, headers, bytes) = send_bytes(&app, anon(Method::GET, &format!("/assets/fonts/{file}")).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "font/woff2", "{file}");
        assert_eq!(&bytes[..4], b"wOF2", "{file}");
        assert!(headers.get(CACHE_CONTROL).unwrap().to_str().unwrap().contains("immutable"));
    }
    assert_eq!(send_bytes(&app, get("/assets/fonts/yok.woff2")).await.0, StatusCode::NOT_FOUND);

    let cookie = login_cookie(&app, "admin", ADMIN_PW).await;
    for uri in ["/", "/guests/102", "/help", "/account"] {
        let request = if uri == "/account" { anon(Method::GET, uri).header(COOKIE, &cookie).body(Body::empty()).unwrap() } else { get(uri) };
        let (_, headers, body) = send(&app, request).await;
        assert!(!body.contains("googleapis") && !body.contains("gstatic") && !body.contains("http://") && !body.contains("https://"), "{uri}: dış adres var");
        assert!(body.contains("/assets/fonts/fonts.css?v="), "{uri}");
        let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("font-src 'self'") && !csp.contains("google"), "{csp}");
    }
    let (_, _, login) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert!(!login.contains("googleapis") && login.contains("/assets/fonts/fonts.css?v="));
}

#[tokio::test]
pub(super) async fn prometheus_metrics_need_a_token() {
    let (app, _) = setup();
    let (status, headers, _) = send(&app, anon(Method::GET, "/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "kazıyıcı giriş sayfasına yönlenmez, 401 alır");
    assert_eq!(headers.get("www-authenticate").unwrap(), "Bearer");

    let (status, headers, body) = send(&app, as_viewer(Method::GET, "/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "salt okunur token yeterli");
    assert!(headers.get(CONTENT_TYPE).unwrap().to_str().unwrap().starts_with("text/plain; version=0.0.4"));
    assert!(body.contains("redox_node_up{node=\"pve-ist-01\"} 1") && body.contains("redox_guest_running{vmid=\"103\""));
}

// ---------- Kullanıcı yönetimi, hesap, iki adımlı doğrulama, denetim ----------
