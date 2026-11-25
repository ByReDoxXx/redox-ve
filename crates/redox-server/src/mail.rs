//! Uyarıları e-postayla gönderen küçük SMTP istemcisi.
//!
//! Bağlantı ya baştan TLS'lidir (465. kapı, "SMTPS") ya da düz başlayıp STARTTLS ile şifrelenir (587. kapı).
//! Şifresiz bağlantı yalnızca aynı makinedeki bir aktarıcıya (loopback) yapılır. Kimlik bilgisi ve ileti
//! hiçbir zaman şifreleme kurulmadan gönderilmez.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, PartialEq)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub from: String,
    pub to: Vec<String>,
    user: Option<String>,
    password: Option<String>,
    mode: Mode,
}

/// Bağlantının nasıl şifreleneceği.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// Baştan TLS (465).
    Tls,
    /// Düz başlar, `STARTTLS` ile şifrelenir (587).
    StartTls,
    /// Şifresiz: yalnızca aynı makinedeki aktarıcı.
    Plain,
}

/// Parola hata ayıklama çıktısına yazılmaz.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Config({})", self.describe())
    }
}

/// Zarf adresi: `ad@alan`. Açılı ayraç, boşluk ve satır sonu başlık kaçırmaya yol açacağı için reddedilir.
pub fn valid_address(addr: &str) -> bool {
    let Some((local, domain)) = addr.split_once('@') else { return false };
    let ok = |part: &str, extra: &[u8]| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b));
    addr.len() <= 254 && ok(local, b".-_+") && ok(domain, b".-") && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

impl Config {
    /// Yapılandırma dosyası: satır başına `anahtar = değer`; `to` virgülle birden çok alıcı alır.
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut map = std::collections::HashMap::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| format!("e-posta dosyası, satır {}: \"anahtar = değer\" bekleniyordu", i + 1))?;
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
        let mut take = |key: &str| map.remove(key).filter(|v| !v.is_empty());
        let need = |value: Option<String>, key: &str| value.ok_or_else(|| format!("e-posta dosyası: \"{key}\" eksik"));
        let host = need(take("host"), "host")?;
        if !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b)) {
            return Err(format!("e-posta dosyası: geçersiz sunucu adı \"{host}\""));
        }
        let port = match take("port") {
            Some(p) => p.parse().map_err(|_| format!("e-posta dosyası: kapı bir sayı olmalı: {p}"))?,
            None => 465,
        };
        let from = need(take("from"), "from")?;
        let to: Vec<String> = need(take("to"), "to")?.split(',').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
        if let Some(bad) = std::iter::once(&from).chain(&to).find(|a| !valid_address(a)) {
            return Err(format!("e-posta dosyası: geçersiz adres \"{bad}\""));
        }
        if to.is_empty() || to.len() > 10 {
            return Err("e-posta dosyası: 1 ile 10 arasında alıcı yazın".to_string());
        }
        let (user, password) = (take("user"), take("password"));
        if user.is_some() != password.is_some() {
            return Err("e-posta dosyası: \"user\" ve \"password\" birlikte verilmeli".to_string());
        }
        // Kip yazılmazsa kapıdan anlaşılır: 587 STARTTLS, diğerleri baştan TLS; aynı makinedeki aktarıcı şifresiz.
        let mode = match take("mode").as_deref() {
            Some("tls") => Mode::Tls,
            Some("starttls") => Mode::StartTls,
            Some(other) => return Err(format!("e-posta dosyası: mode \"tls\" ya da \"starttls\" olmalı: {other}")),
            None if loopback(&host) => Mode::Plain,
            None if port == 587 => Mode::StartTls,
            None => Mode::Tls,
        };
        if let Some(unknown) = map.keys().next() {
            return Err(format!("e-posta dosyası: bilinmeyen anahtar \"{unknown}\""));
        }
        Ok(Config { host, port, from, to, user, password, mode })
    }

    pub fn describe(&self) -> String {
        let mode = match self.mode {
            Mode::Tls => "TLS",
            Mode::StartTls => "STARTTLS",
            Mode::Plain => "şifresiz, yerel",
        };
        format!("{} → {} ({}:{}, {mode})", self.from, self.to.join(", "), self.host, self.port)
    }

    /// Tek bir ileti gönderir. Gövde düz metindir (UTF-8).
    pub fn send(&self, subject: &str, body: &str) -> Result<(), String> {
        let address = format!("{}:{}", self.host, self.port);
        let tcp = std::net::ToSocketAddrs::to_socket_addrs(&address)
            .ok()
            .and_then(|mut list| list.next())
            .ok_or_else(|| format!("e-posta sunucusu çözümlenemedi: {address}"))
            .and_then(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| format!("e-posta sunucusuna bağlanılamadı ({address}): {e}")))?;
        tcp.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
        match self.mode {
            Mode::Plain => self.converse(tcp, subject, body),
            Mode::Tls => self.converse(self.encrypt(tcp)?, subject, body),
            Mode::StartTls => {
                // Şifreleme kurulana kadar yalnızca karşılama, EHLO ve STARTTLS konuşulur.
                let mut plain = Session { reader: BufReader::new(tcp) };
                plain.expect(220, "karşılama")?;
                plain.command("EHLO redox.localdomain", 250)?;
                plain.command("STARTTLS", 220).map_err(|e| format!("e-posta sunucusu şifrelemeyi (STARTTLS) kabul etmedi: {e}"))?;
                // Yanıttan sonra tamponda veri kalmışsa araya girilmiş olabilir: şifreli oturuma taşınmaz.
                if !plain.reader.buffer().is_empty() {
                    return Err("e-posta sunucusu STARTTLS yanıtından sonra beklenmedik veri gönderdi".to_string());
                }
                let tls = self.encrypt(plain.reader.into_inner())?;
                self.deliver(Session { reader: BufReader::new(tls) }, subject, body).map_err(|e| format!("{e} (STARTTLS sonrası)"))
            }
        }
    }

    /// Bağlantıyı TLS ile sarar; sunucunun sertifikası genel kök sertifikalarla ve sunucu adıyla doğrulanır.
    fn encrypt(&self, tcp: TcpStream) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, String> {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let tls = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(self.host.clone()).map_err(|_| format!("geçersiz e-posta sunucusu adı: {}", self.host))?;
        let connection = rustls::ClientConnection::new(Arc::new(tls), name).map_err(|e| format!("TLS başlatılamadı: {e}"))?;
        Ok(rustls::StreamOwned::new(connection, tcp))
    }

    fn converse<S: Read + Write>(&self, stream: S, subject: &str, body: &str) -> Result<(), String> {
        let mut smtp = Session { reader: BufReader::new(stream) };
        smtp.expect(220, "karşılama")?;
        self.deliver(smtp, subject, body)
    }

    /// Karşılamadan sonraki konuşma: tanışma, kimlik, zarf ve ileti.
    fn deliver<S: Read + Write>(&self, mut smtp: Session<S>, subject: &str, body: &str) -> Result<(), String> {
        smtp.command("EHLO redox.localdomain", 250)?;
        if let (Some(user), Some(password)) = (&self.user, &self.password) {
            let token = base64(format!("\0{user}\0{password}").as_bytes());
            smtp.command(&format!("AUTH PLAIN {token}"), 235).map_err(|e| format!("e-posta sunucusu kimliği reddetti: {e}"))?;
        }
        smtp.command(&format!("MAIL FROM:<{}>", self.from), 250)?;
        for to in &self.to {
            smtp.command(&format!("RCPT TO:<{to}>"), 250)?;
        }
        smtp.command("DATA", 354)?;
        smtp.write(&self.message(subject, body))?;
        smtp.expect(250, "ileti")?;
        // Sunucu iletiyi kabul etti; kapanış yanıtı beklenmez.
        let _ = smtp.write("QUIT\r\n");
        Ok(())
    }

    /// Başlıklar ve base64 gövde. Konu RFC 2047 ile kodlanır; gövde base64 olduğu için
    /// satır başındaki nokta ve uzun satır sorunları oluşmaz.
    fn message(&self, subject: &str, body: &str) -> String {
        let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S +0000");
        let mut id = [0u8; 12];
        let _ = getrandom::getrandom(&mut id);
        let id: String = id.iter().map(|b| format!("{b:02x}")).collect();
        let domain = self.from.split_once('@').map(|(_, d)| d).unwrap_or("localhost");
        let encoded: Vec<String> = base64(body.as_bytes()).as_bytes().chunks(76).map(|c| String::from_utf8_lossy(c).to_string()).collect();
        format!(
            "From: RedoX VE <{}>\r\nTo: {}\r\nSubject: =?UTF-8?B?{}?=\r\nDate: {date}\r\nMessage-ID: <{id}@{domain}>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n.\r\n",
            self.from,
            self.to.join(", "),
            base64(subject.as_bytes()),
            encoded.join("\r\n"),
        )
    }
}

struct Session<S: Read + Write> {
    reader: BufReader<S>,
}

impl<S: Read + Write> Session<S> {
    fn write(&mut self, text: &str) -> Result<(), String> {
        let stream = self.reader.get_mut();
        stream.write_all(text.as_bytes()).and_then(|()| stream.flush()).map_err(|e| format!("e-posta sunucusuna yazılamadı: {e}"))
    }

    fn command(&mut self, line: &str, code: u16) -> Result<(), String> {
        self.write(&format!("{line}\r\n"))?;
        // Kimlik satırı hata iletisine yazılmaz.
        self.expect(code, line.split(' ').next().unwrap_or(line))
    }

    /// Yanıtı okur (çok satırlı yanıtlar `250-…` ile sürer, `250 …` ile biter).
    fn expect(&mut self, code: u16, what: &str) -> Result<(), String> {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).map_err(|e| format!("e-posta sunucusu yanıt vermedi ({what}): {e}"))?;
            if n == 0 {
                return Err(format!("e-posta sunucusu bağlantıyı kapattı ({what})"));
            }
            let line = line.trim_end();
            if line.len() >= 4 && line.as_bytes()[3] == b'-' {
                continue;
            }
            return if line.starts_with(&code.to_string()) { Ok(()) } else { Err(format!("e-posta sunucusu {what} adımını reddetti: {line}")) };
        }
    }
}

/// Standart base64 (dolgulu).
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= chunk.len() { TABLE[(n >> shift & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unbase64(text: &str) -> Vec<u8> {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let values: Vec<u32> = text.bytes().filter_map(|b| TABLE.iter().position(|t| *t == b).map(|p| p as u32)).collect();
        values.chunks(4).flat_map(|c| {
            let n = c.iter().enumerate().fold(0u32, |n, (i, v)| n | v << (18 - 6 * i));
            (0..c.len().saturating_sub(1)).map(move |i| (n >> (16 - 8 * i)) as u8).collect::<Vec<_>>()
        }).collect()
    }

    #[test]
    fn base64_matches_known_vectors() {
        for (plain, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(plain.as_bytes()), encoded);
            assert_eq!(unbase64(encoded), plain.as_bytes());
        }
    }

    #[test]
    fn config_is_validated() {
        let cfg = Config::parse("# uyarılar\nhost = smtp.example.com\nfrom = redox@example.com\nto = ben@example.com, nobet@example.org\nuser = redox\npassword = gizli-parola\n").unwrap();
        assert_eq!((cfg.port, cfg.to.len()), (465, 2));
        assert!(!format!("{cfg:?}").contains("gizli"), "parola çıktıya yazılmaz");
        for (text, needle) in [
            ("from = a@b.co\nto = c@d.co", "\"host\" eksik"),
            ("host = smtp.example.com\nto = c@d.co", "\"from\" eksik"),
            ("host = smtp.example.com\nfrom = a@b.co\nto = ", "\"to\" eksik"),
            ("host = smtp.example.com\nfrom = a@b.co\nto = c@d.co\nuser = x", "birlikte verilmeli"),
            ("host = smtp.example.com\nfrom = a@b.co\nto = c@d.co\nport = smtp", "kapı bir sayı"),
            ("host = smtp.example.com\nfrom = a@b.co\nto = c@d.co\nstarttls = on", "bilinmeyen anahtar"),
            ("host = smtp.example.com\nfrom = a@b.co\nto = c@d.co\nmode = duz", "mode \"tls\" ya da \"starttls\""),
            ("host = smtp.example.com\nfrom = a@b.co\nto = c@d.co>\\r\\nBCC: x@y.co", "geçersiz adres"),
            ("host = smtp.example.com/x\nfrom = a@b.co\nto = c@d.co", "geçersiz sunucu adı"),
        ] {
            let err = Config::parse(text).unwrap_err();
            assert!(err.contains(needle), "{text:?}: {err}");
        }
        for ok in ["ben@example.com", "ad.soyad+uyari@posta.example.org"] {
            assert!(valid_address(ok), "{ok}");
        }
        for bad in ["", "ben", "ben@", "@example.com", "ben@localhost", "ben@example.com\r\nBcc: x@y.co", "ben <ben@example.com>", "ben@.example.com"] {
            assert!(!valid_address(bad), "{bad:?}");
        }
    }

    #[test]
    fn encryption_mode_follows_the_port_unless_stated() {
        let cfg = |extra: &str| Config::parse(&format!("host = smtp.example.com\nfrom = a@b.co\nto = c@d.co\n{extra}")).unwrap();
        assert_eq!((cfg("").mode, cfg("port = 587").mode, cfg("port = 2525").mode), (Mode::Tls, Mode::StartTls, Mode::Tls));
        assert_eq!((cfg("port = 587\nmode = tls").mode, cfg("port = 465\nmode = starttls").mode), (Mode::Tls, Mode::StartTls));
        assert!(cfg("port = 587").describe().ends_with("(smtp.example.com:587, STARTTLS)"));
        let local = |extra: &str| Config::parse(&format!("host = 127.0.0.1\nport = 587\nfrom = a@b.co\nto = c@d.co\n{extra}")).unwrap();
        assert_eq!((local("").mode, local("mode = starttls").mode), (Mode::Plain, Mode::StartTls), "yerel aktarıcı şifresizdir; istenirse STARTTLS");
    }

    /// STARTTLS: kimlik ve ileti, şifreleme kurulmadan hiçbir koşulda gönderilmez.
    #[test]
    fn starttls_never_sends_credentials_before_encryption() {
        let starttls = |port: u16| Config::parse(&format!("host = 127.0.0.1\nport = {port}\nmode = starttls\nfrom = redox@example.com\nto = ben@example.com\nuser = redox\npassword = gizli-parola\n")).unwrap();
        // Sunucu STARTTLS'i bilmiyor: gönderim durur.
        let (port, server) = fake_smtp(true);
        let err = starttls(port).send("konu", "gövde").unwrap_err();
        assert!(err.contains("STARTTLS") && err.contains("500"), "{err}");
        let log = server.join().unwrap();
        assert_eq!(log, "EHLO redox.localdomain\r\nSTARTTLS\r\n", "yalnızca tanışma ve STARTTLS konuşuldu");
        // Sunucu kabul ediyor ama TLS konuşmuyor (ya da sertifikası doğrulanamıyor): yine hiçbir şey sızmaz.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.write_all(b"220 sahte ESMTP\r\n").unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 512];
            let mut replied = 0;
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            while let Ok(n) = socket.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
                let lines = seen.iter().filter(|b| **b == b'\n').count();
                if replied == 0 && lines >= 1 {
                    socket.write_all(b"250 sahte\r\n").unwrap();
                    replied = 1;
                } else if replied == 1 && lines >= 2 {
                    socket.write_all(b"220 hazir\r\n").unwrap();
                    replied = 2;
                } else if replied == 2 {
                    // TLS el sıkışması yerine düz metin: istemci bağlantıyı bırakmalı.
                    let _ = socket.write_all(b"250 bu TLS degil\r\n");
                    break;
                }
            }
            String::from_utf8_lossy(&seen).to_string()
        });
        let err = starttls(port).send("konu", "gizli gövde").unwrap_err();
        assert!(!err.contains("gizli"), "{err}");
        let seen = server.join().unwrap();
        assert!(seen.starts_with("EHLO redox.localdomain\r\nSTARTTLS\r\n"), "{seen:?}");
        assert!(!seen.contains("AUTH") && !seen.contains("MAIL FROM") && !seen.contains("gizli"), "şifreleme kurulmadan kimlik ve ileti gitmez");
        // STARTTLS yanıtının ardına eklenmiş veri (araya girme denemesi) oturuma taşınmaz.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket);
            reader.get_mut().write_all(b"220 sahte ESMTP\r\n").unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            reader.get_mut().write_all(b"250 sahte\r\n").unwrap();
            reader.read_line(&mut line).unwrap();
            reader.get_mut().write_all(b"220 hazir\r\n250 araya eklenmis yanit\r\n").unwrap();
            let mut rest = String::new();
            let _ = reader.get_mut().set_read_timeout(Some(Duration::from_secs(2)));
            let _ = reader.read_line(&mut rest);
            rest
        });
        let err = starttls(port).send("konu", "gövde").unwrap_err();
        let rest = server.join().unwrap();
        assert!(!rest.contains("AUTH") && !rest.contains("MAIL"), "{rest:?}");
        assert!(err.contains("beklenmedik veri") || err.contains("STARTTLS sonrası") || err.contains("TLS"), "{err}");
    }

    /// Tek bağlantı kabul eden sahte SMTP sunucusu; konuşmayı kaydeder.
    fn fake_smtp(auth_ok: bool) -> (u16, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket);
            let mut log = String::new();
            let reply = |reader: &mut BufReader<TcpStream>, text: &str| reader.get_mut().write_all(text.as_bytes()).unwrap();
            reply(&mut reader, "220 sahte ESMTP\r\n");
            let mut in_data = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                log.push_str(&line);
                let line = line.trim_end();
                if in_data {
                    if line == "." {
                        in_data = false;
                        reply(&mut reader, "250 kabul edildi\r\n");
                    }
                    continue;
                }
                match line.split(' ').next().unwrap_or("") {
                    "EHLO" => reply(&mut reader, "250-sahte\r\n250-AUTH PLAIN\r\n250 SIZE 1000000\r\n"),
                    "AUTH" => reply(&mut reader, if auth_ok { "235 tamam\r\n" } else { "535 5.7.8 kimlik doğrulanamadı\r\n" }),
                    "MAIL" | "RCPT" => reply(&mut reader, "250 ok\r\n"),
                    "DATA" => {
                        in_data = true;
                        reply(&mut reader, "354 gönderin\r\n");
                    }
                    "QUIT" => break,
                    _ => reply(&mut reader, "500 anlaşılmadı\r\n"),
                }
            }
            log
        });
        (port, server)
    }

    fn local(port: u16) -> Config {
        Config::parse(&format!("host = 127.0.0.1\nport = {port}\nfrom = redox@example.com\nto = ben@example.com, nobet@example.org\nuser = redox\npassword = gizli-parola\n")).unwrap()
    }

    #[test]
    fn sends_a_message_over_smtp() {
        let (port, server) = fake_smtp(true);
        let body = "[istanbul-01] KRİTİK: depolama dolmak üzere\n.tek nokta ile başlayan satır\n";
        local(port).send("RedoX VE: 1 uyarı (ğüşiöç)", body).unwrap();
        let log = server.join().unwrap();

        assert!(log.contains("EHLO redox.localdomain\r\n"));
        let auth = log.lines().find(|l| l.starts_with("AUTH PLAIN ")).unwrap();
        assert_eq!(unbase64(&auth["AUTH PLAIN ".len()..]), b"\0redox\0gizli-parola");
        assert!(log.contains("MAIL FROM:<redox@example.com>\r\n") && log.contains("RCPT TO:<ben@example.com>\r\n") && log.contains("RCPT TO:<nobet@example.org>\r\n"));
        assert!(log.contains("To: ben@example.com, nobet@example.org\r\n") && log.contains("Content-Transfer-Encoding: base64\r\n"));
        let subject = log.lines().find(|l| l.starts_with("Subject: ")).unwrap();
        let encoded = subject.trim_start_matches("Subject: =?UTF-8?B?").trim_end_matches("?=");
        assert_eq!(String::from_utf8(unbase64(encoded)).unwrap(), "RedoX VE: 1 uyarı (ğüşiöç)");
        // Gövde: boş satırdan sonra, tek noktalı satıra kadar.
        let data = log.split("\r\n\r\n").nth(1).unwrap().split("\r\n.\r\n").next().unwrap();
        assert!(data.lines().all(|l| l.len() <= 76 && !l.starts_with('.')), "satırlar kısa ve nokta ile başlamaz");
        assert_eq!(String::from_utf8(unbase64(data)).unwrap(), body);
    }

    #[test]
    fn rejected_login_and_closed_port_are_reported() {
        let (port, server) = fake_smtp(false);
        let err = local(port).send("konu", "gövde").unwrap_err();
        assert!(err.contains("kimliği reddetti") && err.contains("535") && !err.contains("gizli"), "{err}");
        drop(server);
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(local(closed).send("konu", "gövde").unwrap_err().contains("bağlanılamadı"));
    }
}
