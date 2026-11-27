//! Denetim kayıtlarını dışarıdaki bir syslog sunucusuna gönderir (RFC 5424; TCP ve TLS'te RFC 6587 uzunluk
//! önekli çerçeve). Amaç: sunucu ele geçirilip yerel günlük silinse de kayıtların bir kopyası dışarıda kalsın.
//!
//! Gönderim ayrı bir iş parçacığındadır; syslog sunucusu yavaşsa ya da kapalıysa panel beklemez. Bekleyen
//! kayıtlar sınırlı bir sırada tutulur; sıra dolarsa yeni kayıtlar düşer ve sayısı sonradan bildirilir.
//! TCP ve TLS'te bağlantı kopunca yeniden kurulur. Teslim **garantisi yoktur**: UDP'de hiç onay yoktur,
//! TCP'de de karşı tarafın kaydı işlediği bilinemez. Yerel `audit.log` asıl kayıt olmayı sürdürür.

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::audit::AuditEntry;

const QUEUE: usize = 10_000;
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// `authpriv` tesisi (10); başarılı işlem "bilgi" (6), reddedilen "uyarı" (4).
const FACILITY: u8 = 10 * 8;

#[derive(Debug)]
enum Transport {
    Udp,
    Tcp,
    Tls(Arc<ClientConfig>, ServerName<'static>),
}

#[derive(Debug)]
pub struct Target {
    transport: Transport,
    address: String,
    description: String,
}

impl Target {
    /// `udp://ad:kapı`, `tcp://ad:kapı` ya da `tls://ad:kapı`; `ca`: TLS'te güvenilecek sertifika dosyası.
    pub fn parse(text: &str, ca: Option<&Path>) -> Result<Target, String> {
        let bad = || format!("geçersiz syslog hedefi: {text} (örn. tls://log.ornek.net:6514)");
        let (scheme, address) = text.split_once("://").ok_or_else(bad)?;
        let (host, port) = address.rsplit_once(':').ok_or_else(bad)?;
        let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
        let valid_host = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c));
        if !valid_host || port.parse::<u16>().map_or(true, |p| p == 0) {
            return Err(bad());
        }
        let transport = match scheme {
            "udp" => Transport::Udp,
            "tcp" => Transport::Tcp,
            "tls" => Transport::Tls(tls_config(ca)?, ServerName::try_from(host.to_string()).map_err(|_| bad())?),
            _ => return Err(bad()),
        };
        if ca.is_some() && !matches!(transport, Transport::Tls(..)) {
            return Err("--syslog-ca yalnızca tls:// hedefiyle kullanılır".to_string());
        }
        Ok(Target { transport, address: address.to_string(), description: text.to_string() })
    }

    pub fn describe(&self) -> &str {
        &self.description
    }

    /// Şifresiz mi gidiyor? Başka makineye düz metin gönderiliyorsa sunucu açılışta uyarır.
    pub fn is_plain_to_another_machine(&self) -> bool {
        !matches!(self.transport, Transport::Tls(..)) && !["127.0.0.1:", "localhost:", "[::1]:"].iter().any(|p| self.address.starts_with(p))
    }
}

fn tls_config(ca: Option<&Path>) -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();
    match ca {
        None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
        Some(path) => {
            let bad = |e: String| format!("syslog sertifikası okunamadı ({}): {e}", path.display());
            for cert in CertificateDer::pem_file_iter(path).map_err(|e| bad(e.to_string()))? {
                roots.add(cert.map_err(|e| bad(e.to_string()))?).map_err(|e| bad(e.to_string()))?;
            }
            if roots.is_empty() {
                return Err(bad("sertifika yok".to_string()));
            }
        }
    }
    let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS kurulamadı: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// RFC 5424 iletisi. Gövde JSON'dur: kayıttaki satır sonları kaçışlanır, bir alan ikinci bir kayıt uyduramaz.
pub fn message(entry: &AuditEntry, host: &str, pid: u32) -> String {
    let severity = if entry.ok { 6 } else { 4 };
    let time = chrono::DateTime::from_timestamp(entry.ts as i64, 0).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_else(|| "-".to_string());
    let host: String = host.chars().filter(|c| c.is_ascii_graphic()).take(255).collect();
    let host = if host.is_empty() { "-".to_string() } else { host };
    format!("<{}>1 {time} {host} redox {pid} audit - {}", FACILITY + severity, serde_json::to_string(entry).unwrap_or_default())
}

enum Link {
    Udp(UdpSocket),
    Tcp(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

fn connect(target: &Target) -> std::io::Result<Link> {
    let address = target.address.to_socket_addrs()?.next().ok_or_else(|| std::io::Error::other("adres çözülemedi"))?;
    if let Transport::Udp = target.transport {
        let socket = UdpSocket::bind(if address.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" })?;
        socket.connect(address)?;
        return Ok(Link::Udp(socket));
    }
    let stream = TcpStream::connect_timeout(&address, TIMEOUT)?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    match &target.transport {
        Transport::Tls(config, name) => {
            let mut connection = ClientConnection::new(config.clone(), name.clone()).map_err(std::io::Error::other)?;
            let mut stream = stream;
            // El sıkışma burada tamamlanır: güvenilmeyen sertifikaya hiçbir kayıt yazılmaz.
            while connection.is_handshaking() {
                connection.complete_io(&mut stream)?;
            }
            Ok(Link::Tls(Box::new(StreamOwned::new(connection, stream))))
        }
        _ => Ok(Link::Tcp(stream)),
    }
}

/// Karşı taraf bağlantıyı kapattıysa ilk yazma yine "başarılı" görünür; yazmadan önce bakılır.
fn closed(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let mut byte = [0u8; 1];
    let gone = match stream.peek(&mut byte) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => e.kind() != std::io::ErrorKind::WouldBlock,
    };
    stream.set_nonblocking(false).is_err() || gone
}

fn send(link: &mut Link, text: &str) -> std::io::Result<()> {
    let framed = || format!("{} {text}", text.len());
    match link {
        Link::Udp(socket) => socket.send(text.as_bytes()).map(|_| ()),
        Link::Tcp(stream) => {
            if closed(stream) {
                return Err(std::io::Error::other("bağlantı kapanmış"));
            }
            stream.write_all(framed().as_bytes())
        }
        Link::Tls(stream) => {
            if closed(&stream.sock) {
                return Err(std::io::Error::other("bağlantı kapanmış"));
            }
            stream.write_all(framed().as_bytes())?;
            stream.flush()
        }
    }
}

/// Kayıtların gönderildiği uç.
pub struct Forwarder {
    queue: SyncSender<AuditEntry>,
    dropped: Arc<AtomicU64>,
}

impl Forwarder {
    pub fn start(target: Target, host: String) -> Forwarder {
        let (queue, waiting) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let lost = dropped.clone();
        std::thread::spawn(move || run(target, host, waiting, lost));
        Forwarder { queue, dropped }
    }

    /// Hiç beklemez; sıra doluysa kayıt sayılarak düşürülür.
    pub fn push(&self, entry: &AuditEntry) {
        if self.queue.try_send(entry.clone()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn run(target: Target, host: String, waiting: Receiver<AuditEntry>, dropped: Arc<AtomicU64>) {
    let pid = std::process::id();
    let mut link: Option<Link> = None;
    let mut backoff = Duration::from_secs(1);
    let mut failing = false;
    while let Ok(entry) = waiting.recv() {
        let mut pending = vec![message(&entry, &host, pid)];
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost > 0 {
            let note = AuditEntry { ts: entry.ts, user: "-".into(), ip: "-".into(), action: format!("syslog: {lost} kayıt gönderilemeden düştü"), ok: false };
            pending.push(message(&note, &host, pid));
        }
        for text in pending {
            // Gönderilene kadar yeniden denenir; bu sırada yeni kayıtlar sırada birikir.
            loop {
                let outcome = match &mut link {
                    Some(open) => send(open, &text),
                    None => connect(&target).and_then(|mut fresh| {
                        let sent = send(&mut fresh, &text);
                        link = Some(fresh);
                        sent
                    }),
                };
                match outcome {
                    Ok(()) => {
                        if failing {
                            eprintln!("syslog: {} yeniden erişilebilir", target.description);
                        }
                        (failing, backoff) = (false, Duration::from_secs(1));
                        break;
                    }
                    Err(e) => {
                        link = None;
                        if !failing {
                            eprintln!("uyarı: syslog: {} hedefine gönderilemiyor: {e}", target.description);
                            // Kopan bağlantı hemen bir kez yeniden denenir.
                            failing = true;
                            continue;
                        }
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn entry(action: &str, ok: bool) -> AuditEntry {
        AuditEntry { ts: 1_800_000_000, user: "admin".into(), ip: "10.0.0.1".into(), action: action.into(), ok }
    }

    /// Uzunluk önekli bir çerçeve okur.
    fn frame(input: &mut impl BufRead) -> String {
        let mut length = Vec::new();
        input.read_until(b' ', &mut length).unwrap();
        let length: usize = std::str::from_utf8(&length).unwrap().trim().parse().unwrap();
        let mut body = vec![0; length];
        input.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    }

    #[test]
    fn messages_follow_rfc_5424_and_fields_cannot_forge_records() {
        let text = message(&entry("giriş: a\n<86>1 sahte kayıt \"x\"", false), "sunucu 1\n", 42);
        assert!(text.starts_with("<84>1 2027-01-15T08:00:00Z sunucu1 redox 42 audit - {"), "{text}");
        assert!(!text.contains('\n'), "{text}");
        assert!(message(&entry("x", true), "", 1).starts_with("<86>1 ") && message(&entry("x", true), "", 1).contains(" - redox 1 "));
    }

    #[test]
    fn targets_are_validated_and_plain_text_to_another_machine_is_flagged() {
        for bad in ["log:514", "http://log:514", "udp://log", "udp://:514", "udp://log:0", "udp://log:70000", "udp://a b:514", "udp://log/x:514"] {
            assert!(Target::parse(bad, None).is_err(), "{bad}");
        }
        assert!(Target::parse("udp://log.ornek.net:514", None).unwrap().is_plain_to_another_machine());
        assert!(!Target::parse("tcp://127.0.0.1:514", None).unwrap().is_plain_to_another_machine());
        assert!(!Target::parse("tls://log.ornek.net:6514", None).unwrap().is_plain_to_another_machine());
        assert!(Target::parse("tcp://log:1", Some(Path::new("/yok.pem"))).is_err());
        assert!(Target::parse("tls://log:1", Some(Path::new("/yok.pem"))).unwrap_err().contains("okunamadı"));
    }

    #[test]
    fn udp_datagrams_arrive() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let target = Target::parse(&format!("udp://127.0.0.1:{}", server.local_addr().unwrap().port()), None).unwrap();
        let forwarder = Forwarder::start(target, "ev".into());
        forwarder.push(&entry("POST /guests/103/action", true));
        let mut buffer = [0u8; 2048];
        let n = server.recv(&mut buffer).unwrap();
        assert!(String::from_utf8_lossy(&buffer[..n]).contains(r#""action":"POST /guests/103/action""#));
    }

    #[test]
    fn tcp_frames_arrive_and_a_dropped_connection_is_reopened() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = Target::parse(&format!("tcp://{}", listener.local_addr().unwrap()), None).unwrap();
        let forwarder = Forwarder::start(target, "ev".into());
        forwarder.push(&entry("bir", true));
        let (first, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(first);
        assert!(frame(&mut reader).contains(r#""action":"bir""#));
        // Sunucu bağlantıyı kapatır: sıradaki kayıt yeni bağlantıdan gelir, kaybolmaz.
        drop(reader);
        std::thread::sleep(Duration::from_millis(200));
        forwarder.push(&entry("iki", true));
        let (second, _) = listener.accept().unwrap();
        second.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        assert!(frame(&mut BufReader::new(second)).contains(r#""action":"iki""#));
    }

    #[test]
    fn an_unreachable_server_never_blocks_the_caller() {
        let target = Target::parse("tcp://127.0.0.1:1", None).unwrap();
        let forwarder = Forwarder::start(target, "ev".into());
        let started = std::time::Instant::now();
        for _ in 0..QUEUE + 500 {
            forwarder.push(&entry("x", true));
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(forwarder.dropped.load(Ordering::Relaxed) > 0, "sıra dolunca kayıtlar sayılarak düşer");
    }

    #[cfg(unix)]
    #[test]
    fn tls_sends_only_to_a_trusted_certificate() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("redox-syslog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (cert, key) = crate::tls::self_signed("openssl", "localhost", &dir).unwrap();
        let server = crate::tls::config(&cert, &key).unwrap();
        let serve = |listener: TcpListener| {
            let server = server.clone();
            let (got, received) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for socket in listener.incoming().flatten() {
                    let mut config = server.as_ref().clone();
                    config.alpn_protocols.clear();
                    let mut stream = StreamOwned::new(rustls::ServerConnection::new(Arc::new(config)).unwrap(), socket);
                    let mut buffer = [0u8; 2048];
                    if let Ok(n) = stream.read(&mut buffer) {
                        let _ = got.send(String::from_utf8_lossy(&buffer[..n]).into_owned());
                    }
                }
            });
            received
        };
        for (ca, trusted) in [(Some(cert.as_path()), true), (None, false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let received = serve(listener);
            let forwarder = Forwarder::start(Target::parse(&format!("tls://localhost:{port}"), ca).unwrap(), "ev".into());
            forwarder.push(&entry("gizli-kayit", true));
            let got = received.recv_timeout(Duration::from_secs(if trusted { 10 } else { 3 }));
            assert_eq!(got.as_ref().is_ok_and(|t| t.contains("gizli-kayit")), trusted, "{got:?}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
