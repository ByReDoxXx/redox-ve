use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use redox_core::{Cluster, Hypervisor, SimDriver};
use redox_server::auth::{Auth, Cost, Role, UserFile};
use redox_server::host::{self, HostReader};
use redox_server::hostdriver::HostDriver;
use redox_server::lxc::{self, LxcConfig, LxcDriver};
use redox_server::qemu::{QemuConfig, QemuDriver};
use redox_server::state::{load_state, now, recover};
use redox_server::{app, cli, AppState};

/// Komut satırı çıktısı: `REDOX_LANG=en` ya da `--lang en` ile İngilizce.
macro_rules! say {
    ($($arg:tt)*) => { println!("{}", redox_server::i18n::cli_text(&format!($($arg)*))) };
}
macro_rules! complain {
    ($($arg:tt)*) => { eprintln!("{}", redox_server::i18n::cli_text(&format!($($arg)*))) };
}

const DEFAULT_USERS: &str = "redox-users.json";
const REFRESH: Duration = Duration::from_secs(5);

struct ServeOptions {
    addr: String,
    users: PathBuf,
    state_file: Option<PathBuf>,
    task_delay: Duration,
    behind_tls_proxy: bool,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    health_dir: Option<PathBuf>,
    ldap: Option<PathBuf>,
    allow_plain_http: bool,
    driver: String,
    data_dir: PathBuf,
    qemu_bin: Option<String>,
    ovmf_dir: Option<PathBuf>,
    swtpm_bin: Option<String>,
    shutdown_timeout: Duration,
    no_lxc: bool,
    no_autostart: bool,
    storages: Vec<(String, PathBuf)>,
    firewall: bool,
    incremental_backups: bool,
    cbt: bool,
    backup_mbps: u64,
    backup_passphrase_file: Option<PathBuf>,
    alert_webhook: Option<String>,
    alert_ntfy: Option<String>,
    api_rate: Option<u64>,
    lang: Option<String>,
    alert_telegram: Option<PathBuf>,
    alert_email: Option<PathBuf>,
    peers: Option<PathBuf>,
    offsite: Option<PathBuf>,
    oidc: Option<PathBuf>,
    guestfish_bin: Option<String>,
    backup_test_secs: Option<u64>,
    backup_test_every: Option<String>,
    backup_report: Option<String>,
    offsite_copies: Vec<PathBuf>,
    backup_age_days: Option<u64>,
    four_eyes: bool,
    move_precopy: bool,
    migrate: redox_server::state::MigrateSettings,
}

fn default_users_path() -> PathBuf {
    PathBuf::from(std::env::var("REDOX_USERS").unwrap_or_else(|_| DEFAULT_USERS.to_string()))
}

fn serve_options(args: &[String]) -> Result<ServeOptions, String> {
    let mut opts = ServeOptions {
        addr: cli::DEFAULT_ADDR.to_string(),
        users: default_users_path(),
        state_file: None,
        task_delay: Duration::from_millis(1500),
        behind_tls_proxy: false,
        tls_cert: None,
        tls_key: None,
        health_dir: None,
        ldap: None,
        allow_plain_http: false,
        driver: "sim".to_string(),
        data_dir: PathBuf::from("redox-data"),
        qemu_bin: None,
        ovmf_dir: None,
        swtpm_bin: None,
        shutdown_timeout: Duration::from_secs(60),
        no_lxc: false,
        no_autostart: false,
        storages: Vec::new(),
        firewall: false,
        incremental_backups: false,
        cbt: false,
        backup_mbps: 0,
        backup_passphrase_file: None,
        alert_webhook: None,
        alert_ntfy: None,
        api_rate: None,
        lang: None,
        alert_telegram: None,
        alert_email: None,
        peers: None,
        offsite: None,
        oidc: None,
        guestfish_bin: None,
        backup_test_secs: None,
        backup_test_every: None,
        backup_report: None,
        offsite_copies: Vec::new(),
        backup_age_days: None,
        four_eyes: false,
        move_precopy: false,
        migrate: Default::default(),
    };
    let mut delay_given = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} bir değer ister"));
        let number = |raw: &String| raw.parse::<u64>().map_err(|_| format!("{flag} bir sayı olmalı: {raw}"));
        match flag.as_str() {
            "--addr" => opts.addr = value()?.clone(),
            "--users" => opts.users = PathBuf::from(value()?),
            "--state" => opts.state_file = Some(PathBuf::from(value()?)),
            "--behind-tls-proxy" => opts.behind_tls_proxy = true,
            "--tls-cert" => opts.tls_cert = Some(PathBuf::from(value()?)),
            "--tls-key" => opts.tls_key = Some(PathBuf::from(value()?)),
            "--health-dir" => opts.health_dir = Some(PathBuf::from(value()?)),
            "--allow-plain-http" => opts.allow_plain_http = true,
            "--no-lxc" => opts.no_lxc = true,
            "--no-autostart" => opts.no_autostart = true,
            "--four-eyes" => opts.four_eyes = true,
            "--storage" => opts.storages.push(host::parse_storage(value()?)?),
            "--firewall" => opts.firewall = true,
            "--incremental-backups" => opts.incremental_backups = true,
            "--changed-block-tracking" => opts.cbt = true,
            "--backup-mbps" => opts.backup_mbps = number(value()?)?,
            "--move-precopy" => opts.move_precopy = true,
            "--migrate-ports" => opts.migrate.ports = redox_server::state::parse_migrate_ports(value()?)?,
            "--migrate-host" => {
                let host = value()?.clone();
                if !redox_core::valid_migrate_host(&host) {
                    return Err(format!("geçersiz --migrate-host adresi: {host}"));
                }
                opts.migrate.host = Some(host);
            }
            "--migrate-bind" => {
                let bind = value()?.clone();
                if !redox_core::valid_migrate_host(&bind) {
                    return Err(format!("geçersiz --migrate-bind adresi: {bind}"));
                }
                opts.migrate.bind = bind;
            }
            "--backup-passphrase-file" => opts.backup_passphrase_file = Some(PathBuf::from(value()?)),
            "--alert-webhook" => opts.alert_webhook = Some(value()?.clone()),
            "--alert-ntfy" => opts.alert_ntfy = Some(value()?.clone()),
            "--api-rate" => opts.api_rate = Some(number(value()?)?),
            "--lang" => opts.lang = Some(value()?.clone()),
            "--alert-telegram" => opts.alert_telegram = Some(PathBuf::from(value()?)),
            "--alert-email" => opts.alert_email = Some(PathBuf::from(value()?)),
            "--peers" => opts.peers = Some(PathBuf::from(value()?)),
            "--offsite" => opts.offsite = Some(PathBuf::from(value()?)),
            "--oidc" => opts.oidc = Some(PathBuf::from(value()?)),
            "--ldap" => opts.ldap = Some(PathBuf::from(value()?)),
            "--guestfish-bin" => opts.guestfish_bin = Some(value()?.clone()),
            "--backup-test-secs" => opts.backup_test_secs = Some(number(value()?)?),
            "--backup-test-every" => opts.backup_test_every = Some(value()?.clone()),
            "--backup-report" => opts.backup_report = Some(value()?.clone()),
            "--offsite-copy" => opts.offsite_copies.push(PathBuf::from(value()?)),
            "--backup-age-days" => opts.backup_age_days = Some(number(value()?)?),
            "--driver" => opts.driver = value()?.clone(),
            "--data-dir" => opts.data_dir = PathBuf::from(value()?),
            "--qemu-bin" => opts.qemu_bin = Some(value()?.clone()),
            "--ovmf-dir" => opts.ovmf_dir = Some(PathBuf::from(value()?)),
            "--swtpm-bin" => opts.swtpm_bin = Some(value()?.clone()),
            "--shutdown-timeout-secs" => opts.shutdown_timeout = Duration::from_secs(number(value()?)?),
            "--task-delay-ms" => {
                opts.task_delay = Duration::from_millis(number(value()?)?);
                delay_given = true;
            }
            other => return Err(format!("bilinmeyen seçenek: {other}\n\n{}", cli::USAGE)),
        }
    }
    if opts.driver == "qemu" && !delay_given {
        // Gerçek sürücüde yapay bekleme gerekmez; görevler yine de arka planda çalışır.
        opts.task_delay = Duration::from_millis(1);
    }
    if opts.cbt && !opts.incremental_backups {
        return Err("--changed-block-tracking, --incremental-backups ile birlikte kullanılır".to_string());
    }
    if opts.backup_passphrase_file.is_some() && !opts.incremental_backups {
        return Err("--backup-passphrase-file, --incremental-backups ile birlikte kullanılır".to_string());
    }
    if opts.incremental_backups && opts.driver != "qemu" {
        return Err("--incremental-backups yalnızca qemu sürücüsüyle kullanılır (--driver qemu)".to_string());
    }
    if opts.driver == "qemu" && opts.task_delay.is_zero() {
        return Err("--task-delay-ms 0, qemu sürücüsüyle kullanılamaz: kapatma beklerken istekler kilitlenir".to_string());
    }
    Ok(opts)
}

/// Şifresiz HTTP'nin ağa açılmasını engeller: parola ve oturum çerezi dinlenebilir.
fn check_exposure(addr: &SocketAddr, opts: &ServeOptions) -> Result<(), String> {
    if addr.ip().is_loopback() || opts.allow_plain_http {
        return Ok(());
    }
    Err(format!(
        "{addr} bu makinenin dışından erişilebilir bir adres ve sunucu TLS sonlandırmaz.\n\
         Önerilen: 127.0.0.1 üzerinde dinleyin ve önüne TLS'li bir ters vekil (Caddy, nginx) koyun:\n\
         \x20   redox serve --addr 127.0.0.1:8006 --behind-tls-proxy\n\
         Şifresiz HTTP'yi bilerek açmak için (yalnızca yalıtılmış ağda): --allow-plain-http"
    ))
}

fn check_binary(bin: &str) -> Result<(), String> {
    match std::process::Command::new(bin).arg("--version").output() {
        Ok(out) if out.status.success() => Ok(()),
        _ => Err(format!(
            "\"{bin}\" çalıştırılamadı. QEMU kurulu mu?\n    Debian/Ubuntu: sudo apt install qemu-system-x86 qemu-utils"
        )),
    }
}

fn kvm_available() -> bool {
    std::fs::OpenOptions::new().read(true).write(true).open("/dev/kvm").is_ok()
}

/// Seçilen sürücüyü, başlangıç kümesini ve durum dosyasını hazırlar.
#[allow(clippy::type_complexity)]
fn prepare(opts: &ServeOptions) -> Result<(Box<dyn Hypervisor>, Cluster, Option<PathBuf>, Option<PathBuf>), String> {
    match opts.driver.as_str() {
        "sim" => {
            let cluster = match &opts.state_file {
                Some(path) => load_state(path)?.unwrap_or_else(|| Cluster::demo(now())),
                None => Cluster::demo(now()),
            };
            Ok((Box::new(SimDriver::default()), cluster, opts.state_file.clone(), None))
        }
        "qemu" => {
            let mut cfg = QemuConfig::new(&opts.data_dir);
            if let Some(bin) = &opts.qemu_bin {
                cfg.qemu_bin = bin.clone();
            }
            cfg.shutdown_timeout = opts.shutdown_timeout;
            cfg.kvm = kvm_available();
            cfg.incremental_backups = opts.incremental_backups;
            cfg.cbt = opts.cbt;
            cfg.backup_mbps = opts.backup_mbps.min(100_000);
            if let Some(path) = &opts.backup_passphrase_file {
                let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
                cfg.backup_passphrase = Some(text.trim().to_string());
            }
            cfg.storages = opts.storages.clone();
            if let Some(dir) = &opts.ovmf_dir {
                cfg.ovmf_dir = dir.clone();
            }
            if let Some(bin) = &opts.swtpm_bin {
                cfg.swtpm_bin = bin.clone();
            }
            check_binary(&cfg.qemu_bin)?;
            check_binary(&cfg.qemu_img_bin)?;
            let images = cfg.images_dir();
            let backup_passphrase = cfg.backup_passphrase.clone();
            let qemu = QemuDriver::new(cfg)?;
            // Konteyner desteği isteğe bağlıdır: LXC yoksa yalnızca sanal makineler çalışır.
            let containers = if opts.no_lxc {
                None
            } else {
                let built = lxc::detect_idmap().and_then(|idmap| {
                    let mut c = LxcConfig::new(opts.data_dir.join("lxc"));
                    c.idmap = idmap;
                    c.backups_dir = opts.data_dir.join("backups");
                    c.backup_passphrase = backup_passphrase.clone();
                    c.stop_timeout = opts.shutdown_timeout;
                    LxcDriver::new(c)
                });
                match built {
                    Ok(d) => {
                        if d.config().idmap.is_none() {
                            complain!("UYARI: hizmet kök olarak çalışıyor; konteynerler yetkili (kimlik eşlemesiz) olur.");
                        }
                        Some(d)
                    }
                    Err(e) => {
                        complain!("Konteyner desteği kapalı: {e}");
                        None
                    }
                }
            };
            let mut driver = HostDriver::new(qemu, containers);
            if opts.firewall {
                if std::process::Command::new("nft").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
                    return Err("--firewall için nftables gerekli: sudo apt install nftables".to_string());
                }
                driver = driver.with_firewall("nft");
            }
            if !driver.qemu().config().kvm {
                complain!("UYARI: /dev/kvm kullanılamıyor; konuklar yazılım öykünmesiyle (TCG) çok yavaş çalışır.");
                complain!("       Hizmet kullanıcısını kvm grubuna ekleyin: sudo usermod -aG kvm <kullanıcı>");
            }
            // Gerçek konuk kayıtları kaybolmamalı: durum dosyası zorunlu.
            let state_file = opts.state_file.clone().unwrap_or_else(|| opts.data_dir.join("state.json"));
            let mut cluster = match load_state(&state_file)? {
                Some(c) => c,
                None => {
                    let node = host::local_node();
                    let storage = host::local_storage(&node.id, &images);
                    Cluster::local(node, storage)
                }
            };
            host::sync_storages(&mut cluster, &opts.storages)?;
            host::detect_storage_kinds(&mut cluster, &images, &opts.storages);
            Ok((Box::new(driver), cluster, Some(state_file), Some(images)))
        }
        other => Err(format!("bilinmeyen sürücü \"{other}\"; \"sim\" ya da \"qemu\" olmalı")),
    }
}

async fn serve(opts: ServeOptions) -> Result<(), String> {
    let addr: SocketAddr = opts
        .addr
        .parse()
        .map_err(|_| format!("geçersiz adres \"{}\"; örnek: 127.0.0.1:8006", opts.addr))?;
    check_exposure(&addr, &opts)?;

    let users = UserFile::load(&opts.users)?;
    if users.users.is_empty() {
        return Err(format!(
            "{} içinde kullanıcı yok. Önce bir yönetici oluşturun:\n    redox user add admin --users {}",
            opts.users.display(),
            opts.users.display()
        ));
    }
    let tls = match (&opts.tls_cert, &opts.tls_key) {
        (None, None) => None,
        (Some(_), Some(_)) if opts.behind_tls_proxy => return Err("--tls-cert ile --behind-tls-proxy birlikte kullanılmaz: TLS'i ya sunucu ya da vekil sonlandırır".to_string()),
        (Some(cert), Some(key)) => Some(redox_server::tls::config(cert, key)?),
        _ => return Err("--tls-cert ve --tls-key birlikte verilmelidir".to_string()),
    };
    let mut auth = Auth::new(users, Some(opts.users.clone()), opts.behind_tls_proxy, Cost::Production);
    auth.direct_tls = tls.is_some();
    if let Some(path) = &opts.ldap {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let directory = redox_server::ldap::Config::parse(&text)?;
        say!("LDAP girişi açık: {}", directory.describe());
        auth.ldap = Some(directory);
    }

    let (driver, mut cluster, state_file, images) = prepare(&opts)?;
    let recovered = recover(&mut cluster, now());
    if recovered > 0 {
        complain!("{recovered} yarım kalmış görev başarısız olarak kapatıldı.");
    }
    let state = AppState::new(cluster, driver, opts.task_delay, state_file.clone(), auth);
    state.load_metrics();

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("{addr} adresi dinlenemedi: {e}"))?;
    say!("RedoX VE http://{addr} adresinde çalışıyor (sürücü: {}).", state.driver_name());
    say!("Kullanıcılar: {}", opts.users.display());
    match &state_file {
        Some(p) => say!("Durum {} dosyasında saklanıyor.", p.display()),
        None => say!("Durum yalnızca bellekte; kalıcı olması için --state DOSYA verin."),
    }
    if let Some(url) = &opts.alert_webhook {
        state.alerts().set_webhook(url)?;
        say!("Uyarılar webhook'a gönderilecek.");
    }
    if let Some(code) = &opts.lang {
        let lang = redox_server::i18n::Lang::parse(code).ok_or_else(|| format!("--lang tr ya da en olmalı: {code}"))?;
        state.set_default_lang(lang);
        redox_server::i18n::set_background(lang);
    }
    if let Some(rate) = opts.api_rate.filter(|r| *r > 0) {
        state.set_api_rate(rate.min(u32::MAX as u64) as u32);
        say!("API hız sınırı: kullanıcı başına dakikada {rate} istek.");
    }
    if let Some(url) = &opts.alert_ntfy {
        let channel = redox_server::alerts::Channel::ntfy(url)?;
        say!("Uyarılar gönderilecek: {}", channel.describe());
        state.alerts().add_channel(channel);
    }
    if let Some(path) = &opts.alert_telegram {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let channel = redox_server::alerts::Channel::telegram(&text)?;
        say!("Uyarılar gönderilecek: {}", channel.describe());
        state.alerts().add_channel(channel);
    }
    if state.driver().is_real() {
        // Dağıtık anahtar tanımları: ağı kuran root birimi (redox-net.service) bu dosyayı okur.
        if let Err(e) = state.set_vswitch_file(opts.data_dir.join("vswitch.json")) {
            complain!("UYARI: dağıtık anahtar dosyası yazılamadı: {e}");
        }
    }
    // Disk sağlığı ve güncelleme durumu: kök yardımcısı yazar, sunucu okur.
    if state.driver().is_real() || opts.health_dir.is_some() {
        let results = opts.health_dir.clone().unwrap_or_else(|| PathBuf::from(redox_server::health::HEALTH_DIR));
        state.set_health_files(redox_server::state::HealthFiles { results, request: opts.data_dir.join("update.request") });
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let worker = st.clone();
                let _ = tokio::task::spawn_blocking(move || worker.health_tick()).await;
            }
        });
    }
    if state.driver().firewall() {
        match state.sync_firewall() {
            Ok(()) => say!("Konuk güvenlik duvarı açık (nftables, bridge ailesi, tablo: redox)."),
            Err(e) => complain!("UYARI: {e}\n       Hizmetin CAP_NET_ADMIN yetkisi var mı? (AmbientCapabilities=CAP_NET_ADMIN)"),
        }
    }
    if let Some(path) = &opts.oidc {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let config = redox_server::oidc::Config::parse(&text)?;
        say!("Tek oturum açma açık: {} (eşleşen alan: {}).", config.issuer, config.claim);
        state.set_oidc(config);
    }
    state.set_move_precopy(opts.move_precopy);
    state.set_migrate(opts.migrate.clone());
    if opts.four_eyes {
        state.set_four_eyes(true);
        say!("Dört göz ilkesi açık: yedek ve konuk silme ikinci bir yöneticinin onayını bekler.");
    }
    if let Some(days) = opts.backup_age_days {
        state.set_backup_age_days(days.min(3650) as u32);
    }
    if let Some(time) = &opts.backup_report {
        let minute = redox_core::parse_time(time).ok_or_else(|| format!("--backup-report SS:DD biçiminde olmalı (örn. 07:30): {time}"))?;
        if opts.alert_email.is_none() {
            return Err("--backup-report için e-posta ayarı gerekir (--alert-email DOSYA)".to_string());
        }
        state.set_backup_report(minute);
        say!("Yedek raporu her gün {time} saatinde e-postayla gönderilecek.");
    }
    if let Some(text) = &opts.backup_test_every {
        let (days, time) = text.split_once('@').unwrap_or((text, "03:00"));
        let days: u32 = days.parse().ok().filter(|d| (1..=365).contains(d)).ok_or_else(|| format!("--backup-test-every GÜN[@SS:DD] biçiminde olmalı (örn. 7@03:00): {text}"))?;
        let minute = redox_core::parse_time(time).ok_or_else(|| format!("--backup-test-every GÜN[@SS:DD] biçiminde olmalı (örn. 7@03:00): {text}"))?;
        state.set_auto_test(days, minute);
        say!("Her sanal makinenin en yeni yedeği {days} günde bir, {time} saatinden başlayarak açılıp sınanacak.");
    }
    if let Some(secs) = opts.backup_test_secs {
        state.set_boot_test_wait(Duration::from_secs(secs.clamp(5, 3600)));
    }
    // Dosya düzeyinde geri yükleme: guestfish kuruluysa açılır; açıkça verilen yol bulunamazsa hata.
    let guestfish = opts.guestfish_bin.clone().unwrap_or_else(|| "guestfish".to_string());
    if redox_server::guestfs::detect(&guestfish) {
        say!("Yedeklerin içindeki dosyalar okunabilir (guestfish: {guestfish}).");
        state.set_guestfish(Some(guestfish));
    } else if opts.guestfish_bin.is_some() {
        return Err(format!("{guestfish} çalıştırılamadı; --guestfish-bin yolunu denetleyin (sudo apt install libguestfs-tools)"));
    }
    if let Some(path) = &opts.offsite {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let config = redox_server::offsite::Config::parse(&text)?;
        say!("Yedekler sunucu dışına da kopyalanacak: {}", config.describe());
        state.set_offsite(config);
    }
    if !opts.offsite_copies.is_empty() && opts.offsite.is_none() {
        return Err("--offsite-copy için önce asıl dış hedef gerekir (--offsite DOSYA)".to_string());
    }
    for path in &opts.offsite_copies {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let config = redox_server::offsite::Config::parse(&text)?;
        say!("Yedeklerin ek kopyası: {}", config.describe());
        state.add_offsite_copy(config);
    }
    if let Some(path) = &opts.peers {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let peers = redox_server::peers::parse_file(&text)?;
        say!("Birleşik görünüm: {} eş sunucu ({}).", peers.len(), peers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
        state.peers().configure(peers);
        // Eşler: 15 saniyede bir düğüm ve konuk listelerini oku.
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let st2 = st.clone();
                let _ = tokio::task::spawn_blocking(move || st2.peers().poll(now())).await;
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });
    }
    if let Some(path) = &opts.alert_email {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let config = redox_server::mail::Config::parse(&text)?;
        say!("Uyarılar e-postayla gönderilecek: {}", config.describe());
        state.alerts().set_email(config);
    }
    // Uyarılar: 15 saniyede bir değişen koşulları ve olayları değerlendir.
    let watcher = state.clone();
    tokio::spawn(async move {
        loop {
            let st = watcher.clone();
            let _ = tokio::task::spawn_blocking(move || {
                // Önce yeniden başlatma: uyarı turu aynı kapanışı "beklenmedik kapanma" olarak yine bildirir.
                st.ha_tick();
                st.dynamic_memory_tick();
                st.archive_tick();
                if let Err(e) = st.guards_tick() {
                    complain!("ağ kartı korumaları yüklenemedi: {e}");
                }
                let events = st.alerts_tick();
                for e in &events {
                    say!("uyarı: {}", e.text(&st.snapshot().name));
                }
                if let Err(e) = st.alerts().deliver(&st.snapshot().name, &events) {
                    complain!("{e}");
                }
                if let Err(e) = st.alerts().deliver_channels(&st.snapshot().name, &events) {
                    complain!("{e}");
                }
                if let Err(e) = st.alerts().deliver_email(&st.snapshot().name, &events) {
                    complain!("uyarı e-postası gönderilemedi: {e}");
                }
            })
            .await;
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
    // Disk yansıtma bekçisi: donan bir kopya konuğun yazmalarını beklettiği için saniyede bir ve ayrı bir döngüde.
    let mirrors = state.clone();
    tokio::spawn(async move {
        loop {
            let st = mirrors.clone();
            let _ = tokio::task::spawn_blocking(move || {
                st.mirror_tick();
                st.ft_tick();
            })
            .await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    // Zamanlanmış yedekler ve güç işlemleri: yarım dakikada bir saati gelenleri başlat, eski yedekleri buda.
    let scheduler = state.clone();
    tokio::spawn(async move {
        loop {
            let st = scheduler.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let (today, weekday, minute) = redox_server::state::local_now();
                st.power_tick(&today, weekday, minute);
                st.jobs_tick(&today, weekday, minute);
                st.auto_test_tick(minute);
                st.guest_fs_tick();
                st.replica_tick();
                st.failover_tick();
                if let Some(Err(e)) = st.report_tick(&today, minute) {
                    complain!("yedek raporu gönderilemedi: {e}");
                }
                st.scheduler_tick(&today, minute)
            })
            .await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    if let Some(images) = images {
        say!("Veri dizini: {} (disk imajları images/, kurulum kalıpları iso/)", opts.data_dir.display());
        if opts.incremental_backups {
            say!("Artımlı yedek açık: yeni sanal makine yedekleri parça deposuna yazılır (backups/chunks/).");
        }
        // Konukların, düğümün ve diskin gerçek durumunu düzenli olarak oku.
        // Sunucu yeniden açıldıysa (hizmetin yeniden başlaması değil) "açılışta başlat" işaretli konuklar sırayla başlatılır.
        let mut boot = !opts.no_autostart && host::booted_since_last_run(&opts.data_dir.join("boot-id"), host::boot_id().as_deref());
        for (id, dir) in &opts.storages {
            say!("Ek depolama: {id} → {}", dir.display());
        }
        let extras = opts.storages.clone();
        let (st, reader) = (state.clone(), Arc::new(HostReader::default()));
        tokio::spawn(async move {
            loop {
                let (worker, reader, images, extras) = (st.clone(), reader.clone(), images.clone(), extras.clone());
                let _ = tokio::task::spawn_blocking(move || {
                    let st = worker;
                    let sample = reader.sample();
                    st.refresh(sample.as_ref(), host::disk_usage_gb(&images));
                    let usage: Vec<(String, u64, u64)> = extras.iter().filter_map(|(id, dir)| host::disk_usage_gb(dir).map(|(used, total)| (id.clone(), used, total))).collect();
                    st.observe_storages(&usage);
                })
                .await;
                // İlk okumadan sonra: konukların gerçek durumu artık biliniyor.
                if std::mem::take(&mut boot) {
                    let st = st.clone();
                    tokio::spawn(async move {
                        let started = tokio::task::spawn_blocking(move || st.autostart(&|d| std::thread::sleep(d))).await.unwrap_or(0);
                        if started > 0 {
                            say!("Sunucu açılışı: {started} konuk başlatıldı.");
                        }
                    });
                }
                tokio::time::sleep(REFRESH).await;
            }
        });
    }
    if opts.behind_tls_proxy {
        say!("TLS vekili arkasında: çerezler Secure, HSTS açık, X-Forwarded-For'a güveniliyor.");
    } else if tls.is_some() {
        say!("TLS açık: panel https ile sunuluyor.");
    } else if !addr.ip().is_loopback() {
        complain!("UYARI: şifresiz HTTP ağa açık; parolalar ve oturumlar dinlenebilir.");
    }

    let closing = state.clone();
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let served = match tls {
        Some(config) => {
            let secure = redox_server::tls::TlsListener::new(listener, config).map_err(|e| format!("TLS dinleyicisi kurulamadı: {e}"))?;
            redox_server::tls::serve(secure, app(state), stop).await
        }
        None => axum::serve(listener, app(state).into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(stop).await,
    };
    served.map_err(|e| format!("sunucu hatası: {e}"))?;
    // Düzgün kapanışta son ölçümler diske yazılır.
    closing.save_metrics();
    Ok(())
}

/// `--users DOSYA` seçeneğini ayıklar; kalan argümanları döndürür.
fn split_users_flag(args: &[String]) -> Result<(PathBuf, Vec<&str>), String> {
    let mut path = default_users_path();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--users" {
            path = PathBuf::from(it.next().ok_or("--users bir dosya yolu ister")?);
        } else {
            rest.push(a.as_str());
        }
    }
    Ok((path, rest))
}

/// Parolayı ekrana yansıtmadan iki kez sorar. Betikler için REDOX_PASSWORD kullanılabilir.
fn read_new_password() -> Result<String, String> {
    if let Ok(p) = std::env::var("REDOX_PASSWORD") {
        return Ok(p);
    }
    let first = rpassword::prompt_password("Yeni parola (en az 12 karakter): ").map_err(|e| format!("parola okunamadı: {e}"))?;
    let second = rpassword::prompt_password("Parola (yeniden): ").map_err(|e| format!("parola okunamadı: {e}"))?;
    if first != second {
        return Err("parolalar eşleşmiyor".to_string());
    }
    Ok(first)
}

fn manage_users(args: &[String]) -> Result<(), String> {
    let (path, rest) = split_users_flag(args)?;
    let mut file = UserFile::load(&path)?;
    match rest.as_slice() {
        ["user", "list"] => {
            if file.users.is_empty() {
                say!("{} içinde kullanıcı yok.", path.display());
            }
            for u in &file.users {
                let tokens: Vec<&str> = u.tokens.iter().map(|t| t.name.as_str()).collect();
                say!("{:<20} {:<9} token: {}", u.name, u.role.label(), if tokens.is_empty() { "-".to_string() } else { tokens.join(", ") });
            }
            return Ok(());
        }
        ["user", "add", name] => file.add_user(name, &read_new_password()?, Role::Admin, Cost::Production)?,
        ["user", "add", name, "--role", role] => file.add_user(name, &read_new_password()?, role.parse()?, Cost::Production)?,
        ["user", "role", name, role] => file.set_role(name, role.parse()?)?,
        ["user", "scope", name] => file.set_scope(name, Vec::new())?,
        ["user", "scope", name, tags] => file.set_scope(name, redox_core::parse_tags(tags))?,
        ["user", "sso", name, subject] => file.set_sso(name, subject)?,
        ["user", "ldap", name, state @ ("on" | "off")] => file.set_ldap(name, *state == "on")?,
        ["user", "passwd", name] => file.set_password(name, &read_new_password()?, Cost::Production)?,
        ["user", "remove", name] => file.remove_user(name)?,
        ["user", "totp-reset", name] => file.disable_totp(name)?,
        ["token", "create", user, label, "--read-only"] => {
            let plain = file.create_token_as(user, label, true)?;
            file.save(&path)?;
            say!("{plain}");
            complain!("Salt okunur token. Bir daha gösterilmez; şimdi güvenli bir yere kaydedin.");
            return Ok(());
        }
        ["token", "create", user, label] => {
            let plain = file.create_token(user, label)?;
            file.save(&path)?;
            say!("{plain}");
            complain!("Bu token bir daha gösterilmez; şimdi güvenli bir yere kaydedin.");
            return Ok(());
        }
        ["token", "revoke", user, label] => file.revoke_token(user, label)?,
        _ => return Err(format!("anlaşılmayan komut: {}\n\n{}", rest.join(" "), cli::USAGE)),
    }
    file.save(&path)?;
    say!("Tamam: {} güncellendi.", path.display());
    Ok(())
}

/// Dağıtık anahtarların ağını kurar (root olarak çalışır; `redox-net.service`).
fn net_sync(file: &str, rest: &[&str]) -> Result<(), String> {
    let (mut dry_run, mut bridge_conf) = (false, None);
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--dry-run" => dry_run = true,
            "--bridge-conf" => bridge_conf = Some(*rest.next().ok_or("--bridge-conf bir dosya yolu ister")?),
            other => return Err(format!("bilinmeyen seçenek: {other}")),
        }
    }
    let switches = redox_server::vswitch::read_spec(std::path::Path::new(file))?;
    let (done, errors) = redox_server::vswitch::sync::sync(&switches, dry_run)?;
    for line in &done {
        say!("{line}");
    }
    if done.is_empty() && errors.is_empty() {
        say!("ağ zaten tanımlara uygun");
    }
    if let Some(path) = bridge_conf.filter(|_| !dry_run) {
        redox_server::vswitch::write_bridge_conf(std::path::Path::new(path), &switches)?;
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
}

/// Ayar dosyasını bir kullanıcıyla dener. Parola `REDOX_PASSWORD` değişkeninden ya da gizli istemden okunur.
fn ldap_test(file: &str, user: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{file} okunamadı: {e}"))?;
    let directory = redox_server::ldap::Config::parse(&text)?;
    let password = match std::env::var("REDOX_PASSWORD") {
        Ok(p) => p,
        Err(_) => rpassword::prompt_password("Parola: ").map_err(|e| format!("parola okunamadı: {e}"))?,
    };
    match directory.bind(user, &password) {
        Ok(()) => {
            println!("bağlanma başarılı: {}", directory.describe());
            Ok(())
        }
        Err(redox_server::ldap::BindError::Refused) => Err("dizin bu kullanıcı adı ve parolayı kabul etmedi".to_string()),
        Err(redox_server::ldap::BindError::Unavailable(why)) => Err(format!("LDAP: {why}")),
    }
}

/// Kök olarak çalışan kısa ömürlü yardımcılar (redox-health.service, redox-update.service).
fn host_helper(args: &[&str]) -> Result<(), String> {
    use redox_server::health::{apply_updates, collect, write_json, Tools, HEALTH_DIR};
    let usage = || "kullanım: redox host health [--refresh] [--out <dosya>]\n          redox host update-apply <istek-dosyası> [--out-dir <dizin>]".to_string();
    let now = redox_server::state::now();
    match args {
        ["host", "health", rest @ ..] => {
            let (mut refresh, mut out) = (false, None);
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                match *arg {
                    "--refresh" => refresh = true,
                    "--out" => out = Some(PathBuf::from(rest.next().ok_or_else(usage)?)),
                    _ => return Err(usage()),
                }
            }
            let health = collect(&Tools::default(), refresh, now);
            match out {
                Some(path) => write_json(&path, &health),
                None => serde_json::to_string_pretty(&health).map(|text| println!("{text}")).map_err(|e| e.to_string()),
            }
        }
        ["host", "update-apply", request, rest @ ..] => {
            let dir = match rest {
                [] => PathBuf::from(HEALTH_DIR),
                ["--out-dir", dir] => PathBuf::from(dir),
                _ => return Err(usage()),
            };
            // İstek dosyasının yalnızca varlığına bakılır ve hemen silinir (bağlantıysa yalnızca bağlantı silinir).
            std::fs::remove_file(request).map_err(|e| format!("güncelleme isteği yok: {e}"))?;
            let run = apply_updates(&Tools::default(), redox_server::state::now());
            write_json(&dir.join("update.json"), &run)?;
            write_json(&dir.join("health.json"), &collect(&Tools::default(), false, now))?;
            if run.ok { Ok(()) } else { Err(format!("güncelleme başarısız:\n{}", run.output)) }
        }
        _ => Err(usage()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Komut satırının dili: REDOX_LANG ortam değişkeni; `serve --lang en` sunucu çıktılarını da İngilizce yapar.
    let asked = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).cloned().or_else(|| std::env::var("REDOX_LANG").ok());
    if let Some(lang) = asked.as_deref().and_then(redox_server::i18n::Lang::parse) {
        redox_server::i18n::set_cli(lang);
    }
    let result = match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            print!("{}", cli::usage());
            Ok(())
        }
        Some("serve") => serve_options(&args[1..]).and_then(|opts| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("çalışma zamanı başlatılamadı: {e}"))?
                .block_on(serve(opts))
        }),
        Some("user" | "token") => manage_users(&args),
        Some("tty") => redox_server::tty::run(&args[1..]),
        Some("ldap") => match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["ldap", "test", file, user] => ldap_test(file, user),
            _ => Err("kullanım: redox ldap test <ayar-dosyası> <kullanıcı>".to_string()),
        },
        Some("host") => host_helper(&args.iter().map(String::as_str).collect::<Vec<_>>()),
        Some("tls") => match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["tls", "self-signed", name, "--dir", dir] => redox_server::tls::self_signed("openssl", name, std::path::Path::new(dir)).map(|(cert, key)| println!("{}\n{}", cert.display(), key.display())),
            _ => Err("kullanım: redox tls self-signed <sunucu-adı-ya-da-adresi> --dir <dizin>".to_string()),
        },
        Some("net") => match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["net", "vxlan", bridge, vni, local, peers] => redox_server::vxlan::plan(bridge, vni, local, peers).map(|script| print!("{}", redox_server::i18n::cli_text(&script))),
            ["net", "vxlan", bridge, vni, local, peers, "--unit"] => redox_server::vxlan::unit(bridge, vni, local, peers).map(|unit| print!("{}", redox_server::i18n::cli_text(&unit))),
            ["net", "sync", file, rest @ ..] => net_sync(file, rest),
            _ => Err("kullanım: redox net vxlan <köprü> <vni> <yerel-adres> <eş-adres>[,<eş-adres>...] [--unit]\n          redox net sync <tanım-dosyası> [--dry-run] [--bridge-conf <dosya>]".to_string()),
        },
        Some(_) => cli::run(&args).map(|out| {
            use std::io::Write;
            // Çıktı `head` gibi erken kapanan bir boruya gidiyorsa hata vermeden biter.
            let _ = writeln!(std::io::stdout(), "{}", redox_server::i18n::cli_text(&out));
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // Hata iletisine eklenen yardım metni de komut satırının dilinde gösterilir.
            complain!("{}", message.replace(cli::USAGE, cli::usage()));
            ExitCode::FAILURE
        }
    }
}
