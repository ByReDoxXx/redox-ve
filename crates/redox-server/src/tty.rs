//! Sunucunun fiziksel konsolundaki durum ekranı (`redox tty`).
//!
//! Sunucunun başına geçen kişi, oturum açmadan şunları görür: sunucunun adı, ağ adresleri, panelin adresi,
//! yönetim hizmetinin çalışıp çalışmadığı ve kaç konuğun açık olduğu. Ekran birkaç saniyede bir yenilenir;
//! Enter'a basınca olağan oturum açma (`login`) başlar. Ekran hiçbir işlem yapmaz ve gizli bilgi göstermez:
//! sunucuyu yönetmek için yine oturum açmak gerekir.
//!
//! Bilgiler yerelden okunur (durum dosyası, `/proc`, `ip`); yönetim hizmeti çalışmıyorken de ekran açılır ve
//! bunu söyler. Ayrıştırma ve çizim saf işlevlerdir.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Ekranda gösterilen anlık durum.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Screen {
    pub hostname: String,
    pub version: String,
    /// `(arayüz, adres)`; yalnızca dışarıdan erişilebilen IPv4 adresleri.
    pub addresses: Vec<(String, String)>,
    /// Panelin adresi; ağ adresi yoksa `None`.
    pub url: Option<String>,
    /// Yönetim hizmeti yanıt veriyor mu?
    pub service_up: bool,
    /// `(çalışan, toplam)` konuk; durum dosyası okunamadıysa `None`.
    pub guests: Option<(usize, usize)>,
    /// Yük ortalaması (1 dakika).
    pub load: Option<String>,
    /// `(kullanılan, toplam)` bellek, MB.
    pub mem_mb: Option<(u64, u64)>,
    pub uptime_secs: Option<u64>,
    /// İlk yönetici parolası hâlâ dosyada duruyor mu?
    pub first_password: bool,
}

/// `ip -4 -o addr show scope global` çıktısından `(arayüz, adres)` çiftleri. Konukların sanal arayüzleri
/// (TAP) ve köprülerin adressiz üyeleri bu listede zaten görünmez; yalnızca adresi olan arayüzler gelir.
pub fn parse_addresses(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.nth(1)?.trim_end_matches(':').to_string();
            let address = words.skip_while(|w| *w != "inet").nth(1)?.split('/').next()?.to_string();
            let safe = |s: &str| !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || ".-_@:".contains(c));
            (safe(&name) && safe(&address) && name != "lo").then_some((name, address))
        })
        .collect()
}

/// Çalışma süresi: `3 gün 4 sa`, `4 sa 12 dk`, `12 dk`.
fn uptime(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes} dk"),
        (0, _) => format!("{hours} sa {minutes} dk"),
        _ => format!("{days} gün {hours} sa"),
    }
}

/// Ekranın Türkçe metni.
pub fn render(s: &Screen) -> String {
    render_with(s, &|text| text.to_string())
}

/// Ekranın metni; her etiket ve ileti `tr` ile ekranın diline çevrilir (hizalama çeviriden sonra yapılır).
/// Çerçeve yalnızca ASCII'dir: Linux konsolunun yazı tipi her simgeyi çizemez.
pub fn render_with(s: &Screen, tr: &dyn Fn(&str) -> String) -> String {
    let width = 72;
    let rule = format!("  +{}+\n", "-".repeat(width));
    let row = |text: &str| {
        let shown: String = text.chars().take(width - 2).collect();
        format!("  | {shown}{} |\n", " ".repeat(width - 2 - shown.chars().count()))
    };
    let pair = |label: &str, value: &str| {
        let label = if label.is_empty() { String::new() } else { tr(label) };
        row(&format!("{label}{}{}", " ".repeat(20usize.saturating_sub(label.chars().count())), tr(value)))
    };
    let mut out = String::from("\n");
    out.push_str(&rule);
    out.push_str(&row(&format!("RedoX VE {}", s.version)));
    out.push_str(&row(""));
    out.push_str(&pair("Sunucu", &s.hostname));
    match &s.url {
        Some(url) => out.push_str(&pair("Panel", url)),
        None => out.push_str(&pair("Panel", "ağ adresi yok: kabloyu ve ağ ayarını denetleyin")),
    }
    for (i, (name, address)) in s.addresses.iter().take(6).enumerate() {
        out.push_str(&pair(if i == 0 { "Ağ" } else { "" }, &format!("{address}  ({name})")));
    }
    if s.addresses.len() > 6 {
        out.push_str(&pair("", &format!("... ve {} adres daha", s.addresses.len() - 6)));
    }
    out.push_str(&row(""));
    out.push_str(&pair("Yönetim hizmeti", if s.service_up { "çalışıyor" } else { "YANIT VERMİYOR (systemctl status redox)" }));
    match s.guests {
        Some((running, total)) => out.push_str(&pair("Konuklar", &format!("{running} çalışıyor / {total}"))),
        None => out.push_str(&pair("Konuklar", "durum dosyası okunamadı")),
    }
    if let Some((used, total)) = s.mem_mb {
        out.push_str(&pair("Bellek", &format!("{:.1} / {:.1} GB", used as f64 / 1024.0, total as f64 / 1024.0)));
    }
    if let Some(load) = &s.load {
        out.push_str(&pair("Yük", load));
    }
    if let Some(secs) = s.uptime_secs {
        out.push_str(&pair("Açık kalma", &uptime(secs)));
    }
    if s.first_password {
        out.push_str(&row(""));
        out.push_str(&row(&tr("İlk giriş: admin. Parola: /root/redox-ilk-parola.txt (kök okuyabilir).")));
        out.push_str(&row(&tr("İlk girişten sonra parolayı değiştirin ve dosyayı silin.")));
    }
    out.push_str(&rule);
    out.push_str(&format!("\n  {}\n", tr("Oturum açmak için Enter'a basın.")));
    out
}

/// `redox tty` seçenekleri.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Yönetim hizmetinin dinlediği yerel adres (sağlık denetimi için).
    pub listen: String,
    /// Panelin dışarıdan açıldığı şema ve kapı.
    pub scheme: String,
    pub port: u16,
    pub state: PathBuf,
    /// Panelin adresini tutan dosya (vekil kullanılıyorsa). Dosya yoksa adres şema ve kapıdan kurulur.
    pub url_file: Option<PathBuf>,
    /// Ekranı bir kez yazıp çık (betikler ve testler için).
    pub once: bool,
}

pub fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut o = Options { listen: "127.0.0.1:8006".into(), scheme: "https".into(), port: 8006, state: PathBuf::from("/var/lib/redox/state.json"), url_file: None, once: false };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} bir değer ister"));
        match arg.as_str() {
            "--once" => o.once = true,
            "--listen" => o.listen = value()?,
            "--scheme" => {
                o.scheme = value()?;
                if o.scheme != "http" && o.scheme != "https" {
                    return Err(format!("--scheme http ya da https olmalı: {}", o.scheme));
                }
            }
            "--port" => o.port = value()?.parse().map_err(|_| "--port bir kapı numarası olmalı".to_string())?,
            "--state" => o.state = PathBuf::from(value()?),
            "--url-file" => o.url_file = Some(PathBuf::from(value()?)),
            other => return Err(format!("bilinmeyen seçenek: {other}\nkullanım: redox tty [--once] [--listen 127.0.0.1:8006] [--scheme https] [--port 8006] [--state DOSYA] [--url-file DOSYA]")),
        }
    }
    Ok(o)
}

/// Yönetim hizmeti yanıt veriyor mu? (Kısa süre beklenir: ekran takılmasın.)
fn healthy(listen: &str) -> bool {
    ureq::AgentBuilder::new().timeout(Duration::from_millis(900)).build().get(&format!("http://{listen}/healthz")).call().is_ok()
}

/// Adres dosyasının ilk satırı; yalnızca düz bir http(s) adresi kabul edilir (ekrana olduğu gibi yazılır).
fn published_url(path: Option<&Path>) -> Option<String> {
    let text = std::fs::read_to_string(path?).ok()?;
    let line = text.lines().next()?.trim();
    let plain = (line.starts_with("https://") || line.starts_with("http://")) && line.len() <= 200 && line.chars().all(|c| c.is_ascii_graphic());
    plain.then(|| line.to_string())
}

/// Durum dosyasından `(çalışan, toplam)` konuk sayısı.
pub fn guest_counts(state: &Path) -> Option<(usize, usize)> {
    let cluster = crate::state::load_state(state).ok()??;
    Some((cluster.guests.iter().filter(|g| g.status == redox_core::GuestStatus::Running).count(), cluster.guests.len()))
}

/// Anlık durumu toplar. Adres, panel yalnızca bu makineden açılıyorsa (TLS'siz kurulum) yerel adrestir.
pub fn gather(o: &Options) -> Screen {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let addresses = std::process::Command::new("ip").args(["-4", "-o", "addr", "show", "scope", "global"]).output().ok().map(|out| parse_addresses(&String::from_utf8_lossy(&out.stdout))).unwrap_or_default();
    let local_only = o.scheme == "http" && o.listen.starts_with("127.");
    // Panel adresi, dışarıya giden arayüzün adresidir; bulunamazsa geri döngü dışındaki ilk adres.
    let primary = crate::host::primary_ip().filter(|ip| addresses.iter().any(|(_, a)| a == ip)).or_else(|| addresses.iter().find(|(name, _)| name != "lo").map(|(_, a)| a.clone()));
    let url = match published_url(o.url_file.as_deref()) {
        Some(url) => Some(url),
        None if local_only => Some(format!("http://{} (yalnızca bu sunucudan)", o.listen)),
        None => primary.map(|address| format!("{}://{address}:{}", o.scheme, o.port)),
    };
    Screen {
        hostname: crate::host::hostname(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        url,
        addresses,
        service_up: healthy(&o.listen),
        guests: guest_counts(&o.state),
        load: read("/proc/loadavg").and_then(|l| l.split_whitespace().next().map(str::to_string)),
        mem_mb: read("/proc/meminfo").and_then(|m| crate::host::parse_meminfo(&m)).map(|(total_kb, available_kb)| (total_kb.saturating_sub(available_kb) / 1024, total_kb / 1024)),
        uptime_secs: read("/proc/uptime").and_then(|u| crate::host::parse_uptime(&u)),
        first_password: Path::new("/root/redox-ilk-parola.txt").exists(),
    }
}

/// Ekranı gösterir. `--once` yoksa birkaç saniyede bir yeniler; Enter'a basılınca oturum açma başlar.
pub fn run(args: &[String]) -> Result<(), String> {
    let o = parse_options(args)?;
    let draw = |clear: bool| {
        let text = render_with(&gather(&o), &|text| crate::i18n::cli_text(text));
        // Ekranı temizle ve imleci başa al (yalnızca gerçek konsolda).
        print!("{}{text}", if clear { "\x1b[2J\x1b[H" } else { "" });
        use std::io::Write;
        let _ = std::io::stdout().flush();
    };
    if o.once {
        draw(false);
        return Ok(());
    }
    // Klavye ayrı bir iş parçacığında beklenir; ekran o sırada yenilenmeye devam eder.
    let (pressed, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = pressed.send(());
    });
    loop {
        draw(true);
        match wait.recv_timeout(Duration::from_secs(5)) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            _ => break,
        }
    }
    login()
}

/// Olağan oturum açmaya geçer; oturum bitince süreç de biter ve ekranı systemd yeniden başlatır.
fn login() -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        print!("\x1b[2J\x1b[H");
        let error = std::process::Command::new("/bin/login").exec();
        Err(format!("oturum açma başlatılamadı: {error}"))
    }
    #[cfg(not(unix))]
    {
        Err("konsol ekranı yalnızca Linux'ta kullanılır".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_read_from_ip_output_and_odd_values_are_dropped() {
        let text = "2: eno1    inet 192.168.1.20/24 brd 192.168.1.255 scope global dynamic eno1\\       valid_lft 86000sec preferred_lft 86000sec\n\
                    5: vmbr0    inet 10.10.0.2/16 scope global vmbr0\\       valid_lft forever preferred_lft forever\n\
                    7: kötü;ad    inet 10.0.0.9/8 scope global x\n\
                    bozuk satır\n";
        assert_eq!(parse_addresses(text), [("eno1".to_string(), "192.168.1.20".to_string()), ("vmbr0".to_string(), "10.10.0.2".to_string())]);
        assert!(parse_addresses("").is_empty());
    }

    #[test]
    fn the_screen_shows_where_the_panel_is_and_whether_the_service_answers() {
        let mut s = Screen {
            hostname: "sunucu-1".into(),
            version: "0.1.0".into(),
            addresses: vec![("eno1".into(), "192.168.1.20".into())],
            url: Some("https://192.168.1.20:8006".into()),
            service_up: true,
            guests: Some((3, 5)),
            load: Some("0.42".into()),
            mem_mb: Some((2048, 16384)),
            uptime_secs: Some(3 * 86400 + 4 * 3600),
            first_password: false,
        };
        let text = render(&s);
        for part in ["RedoX VE 0.1.0", "sunucu-1", "https://192.168.1.20:8006", "192.168.1.20  (eno1)", "çalışıyor", "3 çalışıyor / 5", "2.0 / 16.0 GB", "0.42", "3 gün 4 sa", "Enter"] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        assert!(!text.contains("redox-ilk-parola"), "parola dosyası yokken anılmaz");
        // Her satır aynı genişliktedir: çerçeve bozulmaz (Türkçe harfler de tek sütun sayılır).
        let boxed: Vec<usize> = text.lines().filter(|l| l.starts_with("  |") || l.starts_with("  +")).map(|l| l.chars().count()).collect();
        assert!(boxed.len() > 8 && boxed.iter().all(|w| *w == boxed[0]), "{boxed:?}");

        // Ağ yokken, hizmet durmuşken ve ilk parola dururken ekran bunları açıkça söyler; parolanın kendisi yazılmaz.
        (s.url, s.addresses, s.service_up, s.guests, s.first_password, s.uptime_secs) = (None, Vec::new(), false, None, true, Some(125));
        let text = render(&s);
        for part in ["ağ adresi yok", "YANIT VERMİYOR", "durum dosyası okunamadı", "/root/redox-ilk-parola.txt", "2 dk"] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        // Başka dilde de çerçeve bozulmaz: hizalama çeviriden sonra yapılır.
        let other = render_with(&s, &|text| format!("[{text}]"));
        let boxed: Vec<usize> = other.lines().filter(|l| l.starts_with("  |") || l.starts_with("  +")).map(|l| l.chars().count()).collect();
        assert!(other.contains("[Panel]") && boxed.iter().all(|w| *w == boxed[0]), "{other}");
        // Çok uzun değer çerçeveyi taşırmaz.
        s.hostname = "x".repeat(200);
        assert!(render(&s).lines().all(|l| l.chars().count() <= 78));
    }

    #[test]
    fn options_are_parsed_and_checked() {
        let args = |text: &str| text.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let o = parse_options(&args("--once --listen 127.0.0.1:8005 --scheme https --port 8006 --state /tmp/d.json")).unwrap();
        assert_eq!((o.once, o.listen.as_str(), o.scheme.as_str(), o.port, o.state.to_str()), (true, "127.0.0.1:8005", "https", 8006, Some("/tmp/d.json")));
        assert_eq!(parse_options(&[]).unwrap().port, 8006);
        for bad in ["--scheme ftp", "--port çok", "--listen", "--bilinmeyen"] {
            assert!(parse_options(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_published_address_is_shown_only_if_it_is_a_plain_url() {
        let file = std::env::temp_dir().join(format!("redox-adres-{}", std::process::id()));
        assert_eq!(published_url(Some(&file)), None, "dosya yok");
        assert_eq!(published_url(None), None);
        std::fs::write(&file, "https://sunucu.ag-adi.ts.net\n").unwrap();
        assert_eq!(published_url(Some(&file)).as_deref(), Some("https://sunucu.ag-adi.ts.net"));
        for bad in ["", "sunucu.ts.net", "https://x\u{1b}[2J", "https://a b"] {
            std::fs::write(&file, bad).unwrap();
            assert_eq!(published_url(Some(&file)), None, "{bad:?}");
        }
        std::fs::remove_file(&file).unwrap();
        assert_eq!(parse_options(&["--url-file".to_string(), "/etc/redox/panel.url".to_string()]).unwrap().url_file, Some(PathBuf::from("/etc/redox/panel.url")));
    }

    #[test]
    fn guest_counts_come_from_the_state_file() {
        let dir = std::env::temp_dir().join(format!("redox-tty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("state.json");
        assert_eq!(guest_counts(&file), None, "dosya yokken sayı uydurulmaz");
        let cluster = redox_core::Cluster::demo(1_700_000_000);
        let (running, total) = (cluster.guests.iter().filter(|g| g.status == redox_core::GuestStatus::Running).count(), cluster.guests.len());
        std::fs::write(&file, serde_json::to_vec(&cluster).unwrap()).unwrap();
        assert_eq!(guest_counts(&file), Some((running, total)));
        std::fs::write(&file, b"bozuk").unwrap();
        assert_eq!(guest_counts(&file), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
