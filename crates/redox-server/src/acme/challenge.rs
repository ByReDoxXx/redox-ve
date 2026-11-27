//! HTTP-01 sınaması: sertifika alınırken 80 numaralı kapıda yalnızca sınama yanıtlarını veren küçük sunucu.
//! Başka hiçbir adrese yanıt vermez, dosya sistemine dokunmaz ve işlem bitince kapanır.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PREFIX: &str = "/.well-known/acme-challenge/";

pub struct Responder {
    answers: Arc<Mutex<HashMap<String, String>>>,
    stop: Arc<AtomicBool>,
    address: std::net::SocketAddr,
}

impl Responder {
    pub fn start(listen: &str) -> Result<Responder, String> {
        let listener = TcpListener::bind(listen).map_err(|e| format!("{listen} dinlenemiyor: {e} (80 numaralı kapı yönetici yetkisi ister ve boş olmalıdır)"))?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let answers: Arc<Mutex<HashMap<String, String>>> = Default::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (known, stopping) = (answers.clone(), stop.clone());
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let known = known.clone();
                std::thread::spawn(move || answer(socket, &known));
            }
        });
        Ok(Responder { answers, stop, address })
    }

    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    pub fn publish(&self, token: &str, content: String) {
        self.answers.lock().unwrap_or_else(|e| e.into_inner()).insert(token.to_string(), content);
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Bekleyen kabulü uyandırır; dinleyici kapanır.
        let mut address = self.address;
        if address.ip().is_unspecified() {
            address.set_ip(if address.is_ipv4() { std::net::Ipv4Addr::LOCALHOST.into() } else { std::net::Ipv6Addr::LOCALHOST.into() });
        }
        let _ = TcpStream::connect_timeout(&address, Duration::from_secs(1));
    }
}

fn answer(mut socket: TcpStream, known: &Mutex<HashMap<String, String>>) {
    let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = socket.set_write_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    // İstek satırı sınırlı okunur: ağa açık kapıda büyük girdi bellek tüketemez.
    let Ok(reading) = socket.try_clone() else { return };
    let mut reader = BufReader::new(reading.take(8192));
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|n| n > 0) && !header.trim().is_empty() {
        header.clear();
    }
    let mut parts = line.split_whitespace();
    let found = match (parts.next(), parts.next()) {
        (Some("GET"), Some(path)) => path.strip_prefix(PREFIX).and_then(|token| known.lock().unwrap_or_else(|e| e.into_inner()).get(token).cloned()),
        _ => None,
    };
    let (status, body) = match found {
        Some(body) => ("200 OK", body),
        None => ("404 Not Found", String::new()),
    };
    let _ = write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}
