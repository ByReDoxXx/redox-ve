//! Yerleşik TLS: vekil kullanmadan HTTPS dinlemek (`--tls-cert`, `--tls-key`).
//!
//! Her bağlantı için TLS burada sonlandırılır ve açık veri bellekteki bir borudan web sunucusuna verilir; böylece
//! istemcinin gerçek adresi korunur. Sertifika dosyası değiştiğinde (yenilendiğinde) sunucu yeniden başlatılmadan
//! yeni sertifika kullanılır.

use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ServerConfig, ServerConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// El sıkışma bu sürede bitmezse bağlantı bırakılır.
const HANDSHAKE: Duration = Duration::from_secs(10);
const CHUNK: usize = 16 * 1024;

fn load(cert: &Path, key: &Path) -> Result<Arc<CertifiedKey>, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| format!("sertifika okunamadı ({}): {e}", cert.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("sertifika okunamadı ({}): {e}", cert.display()))?;
    if chain.is_empty() {
        return Err(format!("{} içinde sertifika yok (PEM bekleniyor)", cert.display()));
    }
    let private = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("özel anahtar okunamadı ({}): {e}", key.display()))?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&private).map_err(|e| format!("özel anahtar kullanılamıyor: {e}"))?;
    let certified = CertifiedKey::new(chain, signer);
    certified.keys_match().map_err(|e| format!("sertifika ile özel anahtar eşleşmiyor: {e}"))?;
    Ok(Arc::new(certified))
}

type Stamp = (Option<SystemTime>, Option<SystemTime>);

#[derive(Debug)]
struct Reloading {
    cert: PathBuf,
    key: PathBuf,
    current: Mutex<(Stamp, Arc<CertifiedKey>)>,
}

impl Reloading {
    fn stamp(cert: &Path, key: &Path) -> Stamp {
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        (modified(cert), modified(key))
    }

    /// Dosyalar değiştiyse yeniden okur. Yeni dosyalar bozuksa (yarım yazılmış olabilir) eskisiyle sürer.
    fn get(&self) -> Arc<CertifiedKey> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let stamp = Self::stamp(&self.cert, &self.key);
        if stamp != current.0 {
            if let Ok(fresh) = load(&self.cert, &self.key) {
                *current = (stamp, fresh);
            }
        }
        current.1.clone()
    }
}

impl ResolvesServerCert for Reloading {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.get())
    }
}

pub fn config(cert: &Path, key: &Path) -> Result<Arc<ServerConfig>, String> {
    let first = load(cert, key)?;
    let resolver = Reloading { cert: cert.to_path_buf(), key: key.to_path_buf(), current: Mutex::new((Reloading::stamp(cert, key), first)) };
    let mut config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS kurulamadı: {e}"))?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn tls_error(e: rustls::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Bekleyen TLS kayıtlarını ağa yazar.
async fn flush(tls: &mut ServerConnection, socket: &mut (impl AsyncWriteExt + Unpin)) -> io::Result<()> {
    while tls.wants_write() {
        let mut out = Vec::new();
        tls.write_tls(&mut out)?;
        socket.write_all(&out).await?;
    }
    Ok(())
}

/// Ağdan gelen şifreli baytları işler; çıkan açık veriyi döndürür. İkinci değer: karşı taraf TLS'i kapattı mı?
fn decrypt(tls: &mut ServerConnection, mut input: &[u8]) -> io::Result<(Vec<u8>, bool)> {
    let mut plain = Vec::new();
    let mut closed = false;
    while !input.is_empty() {
        if tls.read_tls(&mut input)? == 0 {
            break;
        }
        let state = tls.process_new_packets().map_err(tls_error)?;
        let waiting = state.plaintext_bytes_to_read();
        if waiting > 0 {
            let start = plain.len();
            plain.resize(start + waiting, 0);
            tls.reader().read_exact(&mut plain[start..])?;
        }
        closed |= state.peer_has_closed();
    }
    Ok((plain, closed))
}

/// El sıkışmayı yapar; istemci erken veri gönderdiyse onu döndürür.
async fn handshake(tls: &mut ServerConnection, socket: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut early = Vec::new();
    let mut buffer = vec![0u8; CHUNK];
    while tls.is_handshaking() {
        flush(tls, socket).await?;
        if !tls.is_handshaking() {
            break;
        }
        let n = socket.read(&mut buffer).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        match decrypt(tls, &buffer[..n]) {
            Ok((plain, _)) => early.extend(plain),
            Err(e) => {
                // Karşı tarafa nedeni bildiren TLS uyarısı gönderilir.
                let _ = flush(tls, socket).await;
                return Err(e);
            }
        }
    }
    flush(tls, socket).await?;
    Ok(early)
}

/// Şifreli soket ile web sunucusuna giden boru arasında veri taşır.
async fn pump(mut tls: ServerConnection, socket: TcpStream, inner: DuplexStream, early: Vec<u8>) -> io::Result<()> {
    let (mut net_in, mut net_out) = socket.into_split();
    let (mut app_in, mut app_out) = tokio::io::split(inner);
    // Okunan baytlar kanallardan geçer: böylece TLS durumunu tek görev tutar ve iki yön birbirini bekletmez.
    let (to_core, mut from_net) = mpsc::channel::<Vec<u8>>(4);
    let (to_app, mut for_app) = mpsc::channel::<Vec<u8>>(4);
    let reader = tokio::spawn(async move {
        let mut buffer = vec![0u8; CHUNK];
        while let Ok(n @ 1..) = net_in.read(&mut buffer).await {
            if to_core.send(buffer[..n].to_vec()).await.is_err() {
                break;
            }
        }
    });
    let writer = tokio::spawn(async move {
        while let Some(data) = for_app.recv().await {
            if app_out.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = app_out.shutdown().await;
    });
    let mut to_app = Some(to_app);
    if !early.is_empty() {
        if let Some(sender) = &to_app {
            let _ = sender.send(early).await;
        }
    }
    let mut buffer = vec![0u8; CHUNK];
    let outcome = loop {
        tokio::select! {
            incoming = from_net.recv(), if to_app.is_some() => match incoming {
                None => to_app = None,
                Some(data) => {
                    let (plain, closed) = match decrypt(&mut tls, &data) {
                        Ok(result) => result,
                        Err(e) => break Err(e),
                    };
                    if !plain.is_empty() {
                        if let Some(sender) = &to_app {
                            let _ = sender.send(plain).await;
                        }
                    }
                    if closed {
                        to_app = None;
                    }
                    if let Err(e) = flush(&mut tls, &mut net_out).await {
                        break Err(e);
                    }
                }
            },
            outgoing = app_in.read(&mut buffer) => match outgoing {
                Ok(0) | Err(_) => break Ok(()),
                Ok(n) => {
                    if let Err(e) = tls.writer().write_all(&buffer[..n]) {
                        break Err(e);
                    }
                    if let Err(e) = flush(&mut tls, &mut net_out).await {
                        break Err(e);
                    }
                }
            },
        }
    };
    tls.send_close_notify();
    let _ = flush(&mut tls, &mut net_out).await;
    let _ = net_out.shutdown().await;
    reader.abort();
    writer.abort();
    outcome
}

/// HTTPS bağlantılarını kabul eden dinleyici. El sıkışmalar ayrı görevlerde yapılır: yavaş bir istemci
/// ötekileri bekletmez.
pub struct TlsListener {
    ready: mpsc::Receiver<(DuplexStream, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> io::Result<TlsListener> {
        let local = tcp.local_addr()?;
        let (send, ready) = mpsc::channel(32);
        tokio::spawn(async move {
            loop {
                let (mut socket, peer) = tokio::select! {
                    accepted = tcp.accept() => match accepted {
                        Ok(pair) => pair,
                        Err(_) => {
                            // Örn. dosya tanıtıcısı kalmadı: kısa bir aradan sonra yeniden denenir.
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    },
                    () = send.closed() => break,
                };
                let (send, config) = (send.clone(), config.clone());
                tokio::spawn(async move {
                    let Ok(mut tls) = ServerConnection::new(config) else { return };
                    let Ok(Ok(early)) = tokio::time::timeout(HANDSHAKE, handshake(&mut tls, &mut socket)).await else { return };
                    let (outer, inner) = tokio::io::duplex(4 * CHUNK);
                    if send.send((outer, peer)).await.is_ok() {
                        let _ = pump(tls, socket, inner, early).await;
                    }
                });
            }
        });
        Ok(TlsListener { ready, local })
    }
}

/// TLS bağlantısının istemci adresi.
#[derive(Debug, Clone, Copy)]
pub struct TlsPeer(pub SocketAddr);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TlsListener>> for TlsPeer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        TlsPeer(*stream.remote_addr())
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = DuplexStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(pair) => pair,
            // Kabul görevi yalnızca dinleyici bırakıldığında biter; buraya düşülmez.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Uygulamayı HTTPS dinleyicisinde sunar. İstemci adresi, vekilsiz HTTP'deki gibi `ConnectInfo<SocketAddr>`
/// olarak verilir (giriş deneme sınırı ve denetim günlüğü bunu kullanır).
pub async fn serve(listener: TlsListener, app: axum::Router, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> io::Result<()> {
    use axum::extract::{ConnectInfo, Request};
    let app = app.layer(axum::middleware::map_request(|mut request: Request| async move {
        if let Some(ConnectInfo(TlsPeer(peer))) = request.extensions().get::<ConnectInfo<TlsPeer>>().copied() {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        request
    }));
    axum::serve(listener, app.into_make_service_with_connect_info::<TlsPeer>()).with_graceful_shutdown(shutdown).await
}

/// `openssl` ile kendinden imzalı sertifika üretir (`<dizin>/tls.crt`, `tls.key`).
pub fn self_signed(openssl: &str, name: &str, dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let is_ip = name.parse::<std::net::IpAddr>().is_ok();
    if name.is_empty() || name.len() > 253 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b)) {
        return Err(format!("geçersiz sunucu adı: {name}"));
    }
    let (cert, key) = (dir.join("tls.crt"), dir.join("tls.key"));
    if cert.exists() || key.exists() {
        return Err(format!("{} ya da {} zaten var; üzerine yazılmaz", cert.display(), key.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    let alt = format!("subjectAltName={}:{name}", if is_ip { "IP" } else { "DNS" });
    let out = std::process::Command::new(openssl)
        .args(["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "825", "-subj"])
        .arg(format!("/CN={name}"))
        .args(["-addext", &alt, "-addext", "basicConstraints=critical,CA:FALSE", "-addext", "extendedKeyUsage=serverAuth", "-keyout"])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .output()
        .map_err(|e| format!("{openssl} çalıştırılamadı: {e}"))?;
    if !out.status.success() {
        return Err(format!("sertifika üretilemedi: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("bilinmeyen hata")));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok((cert, key))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("redox-tls-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Verilen sertifikaya güvenen bir istemciyle `GET path` yapar; yanıtı döndürür.
    fn https_get(addr: SocketAddr, trusted: &Path, path: &str) -> Result<String, String> {
        let mut roots = RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(trusted).unwrap() {
            roots.add(cert.unwrap()).unwrap();
        }
        let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth();
        let mut client = ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
        let mut socket = std::net::TcpStream::connect(addr).map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut stream = rustls::Stream::new(&mut client, &mut socket);
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut reply = Vec::new();
        match stream.read_to_end(&mut reply) {
            Ok(_) => {}
            Err(e) if !reply.is_empty() && e.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(e) => return Err(e.to_string()),
        }
        Ok(String::from_utf8_lossy(&reply).into_owned())
    }

    #[test]
    fn https_serves_large_answers_keeps_the_client_address_and_reloads_a_renewed_certificate() {
        let (first, second) = (dir("a"), dir("b"));
        let (cert, key) = self_signed("openssl", "localhost", &first).unwrap();
        assert!(self_signed("openssl", "localhost", &first).unwrap_err().contains("zaten var"));
        assert!(self_signed("openssl", "a b", &second).is_err());
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let addr = runtime.block_on(async {
            let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let listener = TlsListener::new(tcp, config(&cert, &key).unwrap()).unwrap();
            let addr = axum::serve::Listener::local_addr(&listener).unwrap();
            let app = axum::Router::new()
                .route("/who", axum::routing::get(|axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>| async move { format!("istemci {}", peer.ip()) }))
                .route("/big", axum::routing::get(|| async { "x".repeat(3_000_000) }));
            tokio::spawn(serve(listener, app, std::future::pending()));
            addr
        });
        assert!(https_get(addr, &cert, "/who").unwrap().ends_with("istemci 127.0.0.1"));
        let big = https_get(addr, &cert, "/big").unwrap();
        assert_eq!(big.split("\r\n\r\n").nth(1).map(str::len), Some(3_000_000), "büyük yanıt eksiksiz");
        // Düz HTTP ve el sıkışmayı bitirmeyen istemci sunucuyu engellemez.
        let idle = std::net::TcpStream::connect(addr).unwrap();
        let mut plain = std::net::TcpStream::connect(addr).unwrap();
        plain.write_all(b"GET /who HTTP/1.1\r\n\r\n").unwrap();
        let mut answer = Vec::new();
        let _ = plain.read_to_end(&mut answer);
        assert!(!String::from_utf8_lossy(&answer).contains("istemci"));
        assert!(https_get(addr, &cert, "/who").is_ok());
        drop(idle);
        // Başka sertifikaya güvenen istemci reddeder; dosya yenilenince sunucu yenisini sunar.
        let (new_cert, new_key) = self_signed("openssl", "localhost", &second).unwrap();
        assert!(https_get(addr, &new_cert, "/who").is_err());
        std::thread::sleep(Duration::from_millis(50));
        std::fs::copy(&new_key, &key).unwrap();
        std::fs::copy(&new_cert, &cert).unwrap();
        assert!(https_get(addr, &new_cert, "/who").is_ok(), "yenilenen sertifika yeniden başlatmadan kullanılır");
        // Anahtar sertifikayla eşleşmiyorsa açılışta söylenir.
        let third = dir("c");
        let (_, stray) = self_signed("openssl", "localhost", &third).unwrap();
        assert!(config(&new_cert, &stray).unwrap_err().contains("eşleşmiyor"));
        assert!(config(&stray, &new_cert).is_err());
        for d in [first, second, third] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
