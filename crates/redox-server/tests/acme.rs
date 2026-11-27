//! ACME istemcisi, imzaları gerçekten doğrulayan sahte bir ACME sunucusuna karşı. Sunucu, sınama yanıtını
//! istemcinin açtığı HTTP kapısından kendisi okur ve gelen sertifika isteğini `openssl` ile doğrulayıp
//! imzalar. Gerçek Let's Encrypt (ya da Pebble) ile sınanmaz.
#![cfg(unix)]

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use redox_server::acme::{self, Settings};
use redox_server::oidc::{base64url, base64url_decode};
use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const TOKEN: &str = "sinama-belirteci_1";

#[derive(Default)]
struct Book {
    nonces: HashSet<String>,
    next: u32,
    /// Hesabın açık anahtarı (65 bayt) ve JWK özeti.
    account: Option<(Vec<u8>, String)>,
    authorized: bool,
    certificate: Option<String>,
    log: Vec<String>,
}

struct Server {
    port: u16,
    /// İstemcinin sınama yanıtlarını verdiği adres.
    challenge_at: String,
    dir: PathBuf,
    /// Üretilecek sertifikanın ömrü (gün).
    days: u32,
    /// İlk imzalı isteğe bir kez "sayı eskimiş" der.
    stale_once: Mutex<bool>,
    /// Sınama yanıtı beklenenle karşılaştırılırken bozulsun mu?
    refuse_challenge: bool,
    book: Mutex<Book>,
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rdx-acme-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn openssl(args: &[&str], dir: &Path) -> String {
    let out = Command::new("openssl").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "openssl {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn http_get(address: &str, path: &str) -> String {
    let mut socket = TcpStream::connect(address).unwrap();
    write!(socket, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut reply = String::new();
    socket.read_to_string(&mut reply).unwrap();
    reply
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn nonce(&self, book: &mut Book) -> String {
        book.next += 1;
        let nonce = format!("sayi-{}", book.next);
        book.nonces.insert(nonce.clone());
        nonce
    }

    /// İmzayı, sayıyı ve adresi denetler; yükü döndürür (POST-as-GET'te `Null`).
    fn verify(&self, book: &mut Book, path: &str, body: &[u8]) -> Result<Value, (u16, &'static str)> {
        let jws: Value = serde_json::from_slice(body).map_err(|_| (400, "malformed"))?;
        let part = |name: &str| jws[name].as_str().map(str::to_string).ok_or((400, "malformed"));
        let (protected, payload, signature) = (part("protected")?, part("payload")?, part("signature")?);
        let header: Value = serde_json::from_slice(&base64url_decode(&protected).ok_or((400, "malformed"))?).map_err(|_| (400, "malformed"))?;
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["url"], self.url(path).as_str(), "imzalanan adres istek adresiyle aynı olmalı");
        if !book.nonces.remove(header["nonce"].as_str().unwrap_or_default()) {
            return Err((400, "badNonce"));
        }
        if std::mem::take(&mut *self.stale_once.lock().unwrap()) {
            return Err((400, "badNonce"));
        }
        if path == "/new-account" {
            let jwk = &header["jwk"];
            let (x, y) = (jwk["x"].as_str().unwrap(), jwk["y"].as_str().unwrap());
            let point = [vec![4], base64url_decode(x).unwrap(), base64url_decode(y).unwrap()].concat();
            let thumbprint = base64url(&Sha256::digest(format!("{{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"{x}\",\"y\":\"{y}\"}}").as_bytes()));
            book.account = Some((point, thumbprint));
        } else {
            assert_eq!(header["kid"], self.url("/account/1").as_str());
            assert!(header["jwk"].is_null());
        }
        let key = book.account.as_ref().ok_or((400, "accountDoesNotExist"))?.0.clone();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key)
            .verify(format!("{protected}.{payload}").as_bytes(), &base64url_decode(&signature).ok_or((400, "malformed"))?)
            .map_err(|_| (400, "badSignature"))?;
        if payload.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&base64url_decode(&payload).unwrap()).map_err(|_| (400, "malformed"))
    }

    fn order(&self, book: &Book) -> Value {
        let status = if book.certificate.is_some() { "valid" } else if book.authorized { "ready" } else { "pending" };
        json!({ "status": status, "authorizations": [self.url("/authz/1")], "finalize": self.url("/finalize/1"), "certificate": self.url("/cert/1") })
    }

    /// Sertifika isteğini doğrular ve deneme kök sertifikasıyla imzalar.
    fn sign(&self, csr: &[u8]) -> String {
        std::fs::write(self.dir.join("istek.der"), csr).unwrap();
        let text = openssl(&["req", "-inform", "DER", "-in", "istek.der", "-verify", "-noout", "-text"], &self.dir);
        assert!(text.contains("DNS:panel.ornek.net") && text.contains("DNS:www.ornek.net"), "{text}");
        std::fs::write(self.dir.join("san.cnf"), "subjectAltName=DNS:panel.ornek.net,DNS:www.ornek.net\n").unwrap();
        openssl(&["req", "-inform", "DER", "-in", "istek.der", "-out", "istek.pem"], &self.dir);
        openssl(&["x509", "-req", "-in", "istek.pem", "-CA", "ca.crt", "-CAkey", "ca.key", "-CAcreateserial", "-days", &self.days.to_string(), "-extfile", "san.cnf", "-out", "yaprak.crt"], &self.dir);
        let read = |f: &str| std::fs::read_to_string(self.dir.join(f)).unwrap();
        format!("{}{}", read("yaprak.crt"), read("ca.crt"))
    }

    /// `(durum, ek başlıklar, gövde)`.
    fn answer(&self, method: &str, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
        let mut book = self.book.lock().unwrap();
        book.log.push(format!("{method} {path}"));
        if path == "/directory" {
            let directory = json!({ "newNonce": self.url("/new-nonce"), "newAccount": self.url("/new-account"), "newOrder": self.url("/new-order") });
            return (200, String::new(), directory.to_string().into_bytes());
        }
        if path == "/new-nonce" {
            let nonce = self.nonce(&mut book);
            return (200, format!("Replay-Nonce: {nonce}\r\n"), Vec::new());
        }
        let payload = self.verify(&mut book, path, body);
        let nonce = self.nonce(&mut book);
        let mut headers = format!("Replay-Nonce: {nonce}\r\n");
        let payload = match payload {
            Ok(payload) => payload,
            Err((code, kind)) => return (code, headers, json!({ "type": format!("urn:ietf:params:acme:error:{kind}"), "detail": kind }).to_string().into_bytes()),
        };
        let reply = match path {
            "/new-account" => {
                assert_eq!(payload["termsOfServiceAgreed"], true);
                headers.push_str(&format!("Location: {}\r\n", self.url("/account/1")));
                json!({ "status": "valid" }).to_string()
            }
            "/new-order" => {
                headers.push_str(&format!("Location: {}\r\n", self.url("/order/1")));
                self.order(&book).to_string()
            }
            "/order/1" => self.order(&book).to_string(),
            "/authz/1" => {
                let mut challenge = json!({ "type": "http-01", "url": self.url("/chall/1"), "token": TOKEN });
                let status = if book.authorized { "valid" } else if self.refuse_challenge && book.log.iter().any(|l| l == "POST /chall/1") { "invalid" } else { "pending" };
                if status == "invalid" {
                    challenge["error"] = json!({ "detail": "alan adı bu sunucuya gelmiyor" });
                }
                json!({ "status": status, "challenges": [{ "type": "dns-01", "url": self.url("/chall/2"), "token": "x" }, challenge] }).to_string()
            }
            "/chall/1" => {
                // Gerçek sunucu gibi: yanıtı istemcinin 80 numaralı kapısından okur.
                let expected = format!("{TOKEN}.{}", book.account.as_ref().unwrap().1);
                let got = http_get(&self.challenge_at, &format!("/.well-known/acme-challenge/{TOKEN}"));
                assert!(got.starts_with("HTTP/1.1 200") && got.ends_with(&expected), "{got}");
                assert!(http_get(&self.challenge_at, "/.well-known/acme-challenge/baska").starts_with("HTTP/1.1 404"));
                assert!(http_get(&self.challenge_at, "/etc/passwd").starts_with("HTTP/1.1 404"));
                book.authorized = !self.refuse_challenge;
                json!({ "status": "processing" }).to_string()
            }
            "/finalize/1" => {
                assert!(book.authorized);
                let csr = base64url_decode(payload["csr"].as_str().unwrap()).unwrap();
                book.certificate = Some(self.sign(&csr));
                self.order(&book).to_string()
            }
            "/cert/1" => return (200, headers, book.certificate.clone().unwrap().into_bytes()),
            _ => return (404, headers, Vec::new()),
        };
        (200, headers, reply.into_bytes())
    }
}

fn start(name: &str, days: u32, refuse_challenge: bool) -> (Arc<Server>, Settings, PathBuf) {
    let dir = temp(name);
    openssl(&["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "3650", "-subj", "/CN=Deneme Kok", "-keyout", "ca.key", "-out", "ca.crt"], &dir);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let challenge_at = format!("127.0.0.1:{}", free_port());
    let server = Arc::new(Server { port: listener.local_addr().unwrap().port(), challenge_at: challenge_at.clone(), dir: dir.clone(), days, stale_once: Mutex::new(true), refuse_challenge, book: Default::default() });
    let shared = server.clone();
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            let server = shared.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    return;
                }
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let mut words = first.split_whitespace();
                let (method, path) = (words.next().unwrap(), words.next().unwrap());
                let (code, headers, reply) = server.answer(method, path, &body);
                let mut socket = socket;
                let _ = write!(socket, "HTTP/1.1 {code} X\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
                if method != "HEAD" {
                    let _ = socket.write_all(&reply);
                }
            });
        }
    });
    let settings = Settings { names: vec!["panel.ornek.net".into(), "www.ornek.net".into()], directory: server.url("/directory"), email: "yonetici@ornek.net".into(), listen: challenge_at };
    (server, settings, dir.join("sertifika"))
}

#[test]
fn a_certificate_is_issued_usable_and_renewed_only_near_its_end() {
    let (server, settings, out) = start("al", 90, false);
    let (cert, key) = acme::issue(&settings, &out).unwrap_or_else(|e| panic!("{e}"));
    // Yerleşik TLS dinleyicisi dosyaları kabul eder (anahtar sertifikayla eşleşir) ve zincir köke bağlanır.
    redox_server::tls::config(&cert, &key).unwrap();
    openssl(&["verify", "-CAfile", server.dir.join("ca.crt").to_str().unwrap(), cert.to_str().unwrap()], &server.dir);
    openssl(&["pkey", "-in", key.to_str().unwrap(), "-noout"], &server.dir);
    use std::os::unix::fs::PermissionsExt;
    for file in [&key, &out.join("acme-account.key")] {
        assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o077, 0, "{}", file.display());
    }
    let now = redox_server::state::now();
    assert!((88..=90).contains(&acme::days_left(&cert, now).unwrap()));
    let before = std::fs::read(&cert).unwrap();
    // Henüz erken: sunucuya hiç gidilmez.
    let asked = server.book.lock().unwrap().log.len();
    assert!(acme::renew(&out, now).unwrap().is_none());
    assert_eq!(server.book.lock().unwrap().log.len(), asked);
    // 61 gün sonra 29 gün kalır: yenilenir; hesap anahtarı aynı, sertifika anahtarı yenidir.
    let (account, old_key) = (std::fs::read(out.join("acme-account.key")).unwrap(), std::fs::read(&key).unwrap());
    {
        let mut book = server.book.lock().unwrap();
        (book.authorized, book.certificate) = (false, None);
    }
    // Sınama kapısı ilk işlemden sonra bırakılmış olmalı.
    assert!(acme::renew(&out, now + 61 * 86_400).unwrap().is_some());
    assert_eq!(std::fs::read(out.join("acme-account.key")).unwrap(), account);
    assert_ne!(std::fs::read(&key).unwrap(), old_key);
    assert_ne!(std::fs::read(&cert).unwrap(), before);
    redox_server::tls::config(&cert, &key).unwrap();
    let _ = std::fs::remove_dir_all(&server.dir);
}

#[test]
fn a_failed_validation_leaves_the_old_certificate_in_place() {
    let (server, settings, out) = start("red", 90, true);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("tls.crt"), "eski sertifika").unwrap();
    let error = acme::issue(&settings, &out).unwrap_err();
    assert!(error.contains("alan adı bu sunucuya gelmiyor"), "{error}");
    assert_eq!(std::fs::read_to_string(out.join("tls.crt")).unwrap(), "eski sertifika");
    assert!(!out.join("tls.key").exists() && !out.join("acme.json").exists() && !out.join("tls.crt.yeni").exists());
    let _ = std::fs::remove_dir_all(&server.dir);
}

#[test]
fn bad_names_plain_directories_and_missing_consent_are_refused_before_any_request() {
    let out = temp("ayar");
    let settings = |names: &[&str], directory: &str| Settings { names: names.iter().map(|n| n.to_string()).collect(), directory: directory.into(), email: String::new(), listen: "127.0.0.1:0".into() };
    for bad in ["localhost", "10.0.0.1", "*.ornek.net", "Ornek.net", "a..net", "-a.net", "a.net/x"] {
        assert!(acme::issue(&settings(&[bad], "https://x.invalid/d"), &out).unwrap_err().contains("geçersiz alan adı"), "{bad}");
    }
    assert!(acme::issue(&settings(&[], "https://x.invalid/d"), &out).is_err());
    assert!(acme::issue(&settings(&["a.ornek.net"], "http://ornek.net/directory"), &out).unwrap_err().contains("https://"));
    let error = acme::command(&["issue", "--domain", "a.ornek.net", "--dir", out.to_str().unwrap()]).unwrap_err();
    assert!(error.contains("--agree-tos"), "{error}");
    assert!(acme::command(&["renew", "--dir", out.to_str().unwrap()]).unwrap_err().contains("önce redox acme issue"));
    assert!(acme::command(&["sil", "--dir", "x"]).is_err() && acme::command(&[]).is_err());
    let _ = std::fs::remove_dir_all(&out);
}
