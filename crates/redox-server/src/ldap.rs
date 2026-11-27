//! LDAP / Active Directory ile parola doğrulama: kullanıcının kendi parolasıyla "basit bağlanma" (RFC 4511).
//!
//! RedoX VE dizinde arama yapmaz ve bir hizmet hesabı tutmaz. Kimlik dizine ait, yetki yereldir: yalnızca
//! `redox user ldap <ad> on` ile işaretlenmiş yerel kullanıcılar dizin parolasıyla girer; rol ve kapsam yerel
//! kayıttadır. Dizinden alınan tek bilgi "bu ad ve parola doğru mu" yanıtıdır.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REPLY: usize = 8 * 1024;
const MAX_PASSWORD: usize = 1024;
const INVALID_CREDENTIALS: u32 = 49;

#[derive(Clone)]
pub struct Config {
    host: String,
    port: u16,
    tls: Option<Arc<ClientConfig>>,
    /// Bağlanılacak ad; `{user}` yerine kullanıcı adı yazılır.
    template: String,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.describe())
    }
}

#[derive(Debug, PartialEq)]
pub enum BindError {
    /// Dizin adı ya da parolayı kabul etmedi.
    Refused,
    /// Dizine ulaşılamadı ya da yanıt anlaşılamadı.
    Unavailable(String),
}

impl Config {
    /// Ayar dosyası: `url = ldaps://dc.ornek.com`, `bind = uid={user},ou=people,dc=ornek,dc=com` (Active
    /// Directory'de `{user}@ornek.com`), isteğe bağlı `ca = /yol/kok.pem` (kurumun kendi sertifika yetkilisi).
    pub fn parse(text: &str) -> Result<Config, String> {
        let (mut url, mut template, mut ca) = (None, None, None);
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| format!("ldap dosyası, satır {}: \"anahtar = değer\" bekleniyordu", i + 1))?;
            let value = value.trim().to_string();
            match key.trim() {
                "url" => url = Some(value),
                "bind" => template = Some(value),
                "ca" => ca = Some(value),
                other => return Err(format!("ldap dosyası: bilinmeyen anahtar \"{other}\"")),
            }
        }
        let url = url.ok_or("ldap dosyası: \"url\" eksik")?;
        let template = template.filter(|t| t.matches("{user}").count() == 1).ok_or("ldap dosyası: \"bind\" satırında bir kez {user} geçmeli")?;
        let (secure, rest) = match url.split_once("://") {
            Some(("ldaps", rest)) => (true, rest),
            Some(("ldap", rest)) => (false, rest),
            _ => return Err("ldap dosyası: url ldaps:// ile başlamalı".to_string()),
        };
        let rest = rest.trim_end_matches('/');
        let default = if secure { 636 } else { 389 };
        let (host, port) = match rest.rsplit_once(':') {
            Some((host, port)) if !port.contains(']') => (host, port.parse().map_err(|_| "ldap dosyası: kapı numarası geçersiz".to_string())?),
            _ => (rest, default),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b)) {
            return Err("ldap dosyası: sunucu adı geçersiz".to_string());
        }
        // Parola şifresiz yalnızca aynı makinedeki sunucuya gider.
        if !secure && !matches!(host.as_str(), "127.0.0.1" | "::1" | "localhost") {
            return Err("ldap dosyası: şifresiz ldap:// yalnızca aynı makine içindir; ldaps:// kullanın".to_string());
        }
        let tls = if secure { Some(client_config(ca.as_deref().map(Path::new))?) } else { None };
        Ok(Config { host, port, tls, template })
    }

    pub fn describe(&self) -> String {
        format!("{}://{}:{} ({})", if self.tls.is_some() { "ldaps" } else { "ldap" }, self.host, self.port, self.template)
    }

    /// `name` ve `password` ile bağlanmayı dener.
    pub fn bind(&self, name: &str, password: &str) -> Result<(), BindError> {
        // Boş parolayla basit bağlanma "kimliksiz bağlanma"dır ve birçok sunucu onu kabul eder: asla gönderilmez.
        if password.is_empty() || password.len() > MAX_PASSWORD || name.is_empty() {
            return Err(BindError::Refused);
        }
        let dn = bind_name(&self.template, name);
        let unavailable = |e: String| BindError::Unavailable(e);
        let address = (self.host.as_str(), self.port).to_socket_addrs().map_err(|e| unavailable(format!("{} çözümlenemedi: {e}", self.host)))?.next().ok_or_else(|| unavailable(format!("{} çözümlenemedi", self.host)))?;
        let socket = TcpStream::connect_timeout(&address, TIMEOUT).map_err(|e| unavailable(format!("bağlanılamadı: {e}")))?;
        socket.set_read_timeout(Some(TIMEOUT)).and_then(|()| socket.set_write_timeout(Some(TIMEOUT))).map_err(|e| unavailable(e.to_string()))?;
        let request = bind_request(&dn, password);
        match &self.tls {
            None => exchange(socket, &request),
            Some(config) => {
                let server = ServerName::try_from(self.host.clone()).map_err(|e| unavailable(e.to_string()))?;
                let connection = ClientConnection::new(config.clone(), server).map_err(|e| unavailable(e.to_string()))?;
                exchange(StreamOwned::new(connection, socket), &request)
            }
        }
    }
}

fn client_config(ca: Option<&Path>) -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();
    match ca {
        None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
        Some(path) => {
            let bad = |e: String| format!("ldap dosyası: ca okunamadı ({}): {e}", path.display());
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

/// Kullanıcı adını kalıba yerleştirir. Ayırt edici ad (DN) kalıbında özel karakterler kaçırılır (RFC 4514):
/// ad, dizin ağacında başka bir yeri gösteremez.
pub fn bind_name(template: &str, user: &str) -> String {
    if !template.contains('=') {
        return template.replace("{user}", user);
    }
    let mut escaped = String::new();
    let last = user.chars().count().saturating_sub(1);
    for (i, c) in user.chars().enumerate() {
        match c {
            ',' | '+' | '"' | '\\' | '<' | '>' | ';' | '=' => {
                escaped.push('\\');
                escaped.push(c);
            }
            '#' if i == 0 => escaped.push_str("\\#"),
            ' ' if i == 0 || i == last => escaped.push_str("\\ "),
            '\0' => escaped.push_str("\\00"),
            other => escaped.push(other),
        }
    }
    template.replace("{user}", &escaped)
}

/// BER: etiket, uzunluk, içerik.
fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 128 {
        out.push(len as u8);
    } else {
        let bytes: Vec<u8> = len.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(content);
    out
}

/// `(etiket, içerik, geri kalan)`.
fn read_tlv(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, data) = data.split_first()?;
    let (&first, mut data) = data.split_first()?;
    let len = if first < 128 {
        first as usize
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 || data.len() < count {
            return None;
        }
        let len = data[..count].iter().fold(0usize, |acc, b| acc << 8 | *b as usize);
        data = &data[count..];
        len
    };
    (data.len() >= len).then(|| (tag, &data[..len], &data[len..]))
}

/// İleti 1: sürüm 3 basit bağlanma isteği.
fn bind_request(dn: &str, password: &str) -> Vec<u8> {
    let mut op = tlv(0x02, &[3]);
    op.extend(tlv(0x04, dn.as_bytes()));
    op.extend(tlv(0x80, password.as_bytes()));
    let mut message = tlv(0x02, &[1]);
    message.extend(tlv(0x60, &op));
    tlv(0x30, &message)
}

/// Bağlanma yanıtının sonuç kodu; yanıt tamamlanmadıysa `Ok(None)`.
fn bind_result(data: &[u8]) -> Result<Option<u32>, String> {
    let bad = || "anlaşılmayan yanıt".to_string();
    if data.first().is_some_and(|tag| *tag != 0x30) {
        return Err(bad());
    }
    let Some((_, message, _)) = read_tlv(data) else { return Ok(None) };
    let (id_tag, id, rest) = read_tlv(message).ok_or_else(bad)?;
    let (op_tag, op, _) = read_tlv(rest).ok_or_else(bad)?;
    if id_tag != 0x02 || id != [1] || op_tag != 0x61 {
        return Err(bad());
    }
    let (code_tag, code, _) = read_tlv(op).ok_or_else(bad)?;
    if code_tag != 0x0a || code.is_empty() || code.len() > 4 {
        return Err(bad());
    }
    Ok(Some(code.iter().fold(0u32, |acc, b| acc << 8 | u32::from(*b))))
}

fn exchange(mut stream: impl Read + Write, request: &[u8]) -> Result<(), BindError> {
    let unavailable = |e: String| BindError::Unavailable(e);
    stream.write_all(request).and_then(|()| stream.flush()).map_err(|e| unavailable(format!("istek gönderilemedi: {e}")))?;
    let mut reply = Vec::new();
    let mut buffer = [0u8; 1024];
    let code = loop {
        if let Some(code) = bind_result(&reply).map_err(unavailable)? {
            break code;
        }
        let n = stream.read(&mut buffer).map_err(|e| unavailable(format!("yanıt okunamadı: {e}")))?;
        if n == 0 || reply.len() + n > MAX_REPLY {
            return Err(unavailable("yanıt yarım kaldı".to_string()));
        }
        reply.extend_from_slice(&buffer[..n]);
    };
    // Oturumu kapatma (unbind); yanıtı yoktur.
    let mut unbind = tlv(0x02, &[2]);
    unbind.extend(tlv(0x42, &[]));
    let _ = stream.write_all(&tlv(0x30, &unbind));
    match code {
        0 => Ok(()),
        INVALID_CREDENTIALS => Err(BindError::Refused),
        other => Err(unavailable(format!("dizin bağlanmayı {other} koduyla reddetti"))),
    }
}

/// Testlerin ve yerel denemenin kullandığı, tek bir kullanıcıyı tanıyan küçük LDAP sunucusu.
#[doc(hidden)]
pub mod test_server {
    use super::*;

    /// Bir bağlanma isteğini okur; `(dn, parola)` döndürür.
    fn read_bind(stream: &mut impl Read) -> Option<(String, String)> {
        let mut data = Vec::new();
        let mut buffer = [0u8; 512];
        loop {
            if let Some((0x30, message, _)) = read_tlv(&data) {
                let (_, _, rest) = read_tlv(message)?;
                let (0x60, op, _) = read_tlv(rest)? else { return None };
                let (_, version, rest) = read_tlv(op)?;
                let (0x04, dn, rest) = read_tlv(rest)? else { return None };
                let (0x80, password, _) = read_tlv(rest)? else { return None };
                return (version == [3]).then(|| (String::from_utf8_lossy(dn).into_owned(), String::from_utf8_lossy(password).into_owned()));
            }
            let n = stream.read(&mut buffer).ok().filter(|n| *n > 0)?;
            data.extend_from_slice(&buffer[..n]);
        }
    }

    pub fn answer(stream: &mut (impl Read + Write), dn: &str, password: &str) -> Option<(String, String)> {
        let seen = read_bind(stream)?;
        let code = if seen.0 == dn && seen.1 == password { 0 } else { INVALID_CREDENTIALS as u8 };
        let mut result = tlv(0x0a, &[code]);
        result.extend(tlv(0x04, b""));
        result.extend(tlv(0x04, b""));
        let mut message = tlv(0x02, &[1]);
        message.extend(tlv(0x61, &result));
        stream.write_all(&tlv(0x30, &message)).ok()?;
        let _ = stream.flush();
        Some(seen)
    }

    /// Şifresiz sunucu; adresini döndürür.
    pub fn start(dn: &'static str, password: &'static str) -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bağlanma");
        let address = listener.local_addr().expect("adres");
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                std::thread::spawn(move || answer(&mut stream, dn, password));
            }
        });
        address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DN: &str = "uid=ali,ou=people,dc=ornek,dc=com";

    fn local(address: std::net::SocketAddr) -> Config {
        Config::parse(&format!("url = ldap://127.0.0.1:{}\nbind = uid={{user}},ou=people,dc=ornek,dc=com", address.port())).unwrap()
    }

    #[test]
    fn the_request_matches_the_protocol_bytes() {
        // RFC 4511: LDAPMessage { 1, BindRequest { 3, "cn=a", simple "pw" } }
        assert_eq!(bind_request("cn=a", "pw"), [0x30, 0x12, 0x02, 0x01, 0x01, 0x60, 0x0d, 0x02, 0x01, 0x03, 0x04, 0x04, b'c', b'n', b'=', b'a', 0x80, 0x02, b'p', b'w']);
        let long = bind_request("cn=a", &"p".repeat(300));
        assert_eq!(&long[..3], &[0x30, 0x82, 0x01], "uzun uzunluk biçimi");
        // OpenLDAP ve AD yanıtlarındaki uzun biçimli uzunluk da okunur.
        let reply = [0x30, 0x84, 0, 0, 0, 12, 0x02, 0x01, 0x01, 0x61, 0x07, 0x0a, 0x01, 0x31, 0x04, 0x00, 0x04, 0x00];
        assert_eq!(bind_result(&reply), Ok(Some(49)));
        assert_eq!(bind_result(&reply[..7]), Ok(None));
        assert!(bind_result(b"HTTP/1.1 400").is_err());
        assert!(bind_result(&[0x30, 0x03, 0x02, 0x01, 0x01]).is_err());
    }

    #[test]
    fn user_names_cannot_move_around_the_directory() {
        assert_eq!(bind_name("uid={user},dc=x", "ali,ou=admins"), "uid=ali\\,ou\\=admins,dc=x");
        assert_eq!(bind_name("cn={user},dc=x", "# a "), "cn=\\# a\\ ,dc=x");
        assert_eq!(bind_name("{user}@ornek.com", "ali"), "ali@ornek.com");
    }

    #[test]
    fn only_the_right_password_binds_and_empty_passwords_are_never_sent() {
        let config = local(test_server::start(DN, "dogru parola"));
        assert_eq!(config.bind("ali", "dogru parola"), Ok(()));
        assert_eq!(config.bind("ali", "yanlis"), Err(BindError::Refused));
        assert_eq!(config.bind("veli", "dogru parola"), Err(BindError::Refused));
        let closed = Config::parse("url = ldap://127.0.0.1:1\nbind = {user}@x").unwrap();
        assert_eq!(closed.bind("ali", ""), Err(BindError::Refused), "boş parola ağa hiç gitmez");
        assert!(matches!(closed.bind("ali", "p"), Err(BindError::Unavailable(_))));
    }

    #[test]
    fn unsafe_or_incomplete_settings_are_refused() {
        for (text, why) in [
            ("bind = {user}@x", "url"),
            ("url = ldaps://dc\nbind = sabit", "{user}"),
            ("url = ldaps://dc\nbind = {user}{user}", "{user}"),
            ("url = ldap://dc.ornek.com\nbind = {user}@x", "yalnızca aynı makine"),
            ("url = http://dc\nbind = {user}@x", "ldaps://"),
            ("url = ldaps://dc;rm\nbind = {user}@x", "sunucu adı"),
            ("url = ldaps://dc:99999\nbind = {user}@x", "kapı"),
            ("url = ldaps://dc\nbind = {user}@x\nparola = 1", "bilinmeyen anahtar"),
            ("url = ldaps://dc\nbind = {user}@x\nca = /yok.pem", "ca okunamadı"),
        ] {
            assert!(Config::parse(text).unwrap_err().contains(why), "{text}");
        }
        assert_eq!(Config::parse("url=ldaps://dc.ornek.com\nbind={user}@ornek.com").unwrap().describe(), "ldaps://dc.ornek.com:636 ({user}@ornek.com)");
    }

    #[cfg(unix)]
    #[test]
    fn ldaps_checks_the_certificate() {
        let dir = std::env::temp_dir().join(format!("redox-ldaps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (cert, key) = crate::tls::self_signed("openssl", "localhost", &dir).unwrap();
        let server = crate::tls::config(&cert, &key).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let mut server = server.as_ref().clone();
                server.alpn_protocols.clear();
                let connection = rustls::ServerConnection::new(Arc::new(server)).unwrap();
                test_server::answer(&mut StreamOwned::new(connection, socket), "ali@x", "gizli");
            }
        });
        let trusted = Config::parse(&format!("url = ldaps://localhost:{port}\nbind = {{user}}@x\nca = {}", cert.display())).unwrap();
        assert_eq!(trusted.bind("ali", "gizli"), Ok(()));
        assert_eq!(trusted.bind("ali", "yanlis"), Err(BindError::Refused));
        let untrusting = Config::parse(&format!("url = ldaps://localhost:{port}\nbind = {{user}}@x")).unwrap();
        assert!(matches!(untrusting.bind("ali", "gizli"), Err(BindError::Unavailable(_))), "güvenilmeyen sertifikaya parola gitmez");
        let _ = std::fs::remove_dir_all(dir);
    }
}
