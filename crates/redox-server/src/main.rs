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

const DEFAULT_USERS: &str = "redox-users.json";
const REFRESH: Duration = Duration::from_secs(5);

struct ServeOptions {
    addr: String,
    users: PathBuf,
    state_file: Option<PathBuf>,
    task_delay: Duration,
    behind_tls_proxy: bool,
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
    alert_webhook: Option<String>,
    alert_email: Option<PathBuf>,
    peers: Option<PathBuf>,
    offsite: Option<PathBuf>,
    oidc: Option<PathBuf>,
    guestfish_bin: Option<String>,
    backup_test_secs: Option<u64>,
    backup_report: Option<String>,
    offsite_copies: Vec<PathBuf>,
    backup_age_days: Option<u64>,
    four_eyes: bool,
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
        alert_webhook: None,
        alert_email: None,
        peers: None,
        offsite: None,
        oidc: None,
        guestfish_bin: None,
        backup_test_secs: None,
        backup_report: None,
        offsite_copies: Vec::new(),
        backup_age_days: None,
        four_eyes: false,
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
            "--allow-plain-http" => opts.allow_plain_http = true,
            "--no-lxc" => opts.no_lxc = true,
            "--no-autostart" => opts.no_autostart = true,
            "--four-eyes" => opts.four_eyes = true,
            "--storage" => opts.storages.push(host::parse_storage(value()?)?),
            "--firewall" => opts.firewall = true,
            "--incremental-backups" => opts.incremental_backups = true,
            "--changed-block-tracking" => opts.cbt = true,
            "--backup-mbps" => opts.backup_mbps = number(value()?)?,
            "--alert-webhook" => opts.alert_webhook = Some(value()?.clone()),
            "--alert-email" => opts.alert_email = Some(PathBuf::from(value()?)),
            "--peers" => opts.peers = Some(PathBuf::from(value()?)),
            "--offsite" => opts.offsite = Some(PathBuf::from(value()?)),
            "--oidc" => opts.oidc = Some(PathBuf::from(value()?)),
            "--guestfish-bin" => opts.guestfish_bin = Some(value()?.clone()),
            "--backup-test-secs" => opts.backup_test_secs = Some(number(value()?)?),
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
            let qemu = QemuDriver::new(cfg)?;
            // Konteyner desteği isteğe bağlıdır: LXC yoksa yalnızca sanal makineler çalışır.
            let containers = if opts.no_lxc {
                None
            } else {
                let built = lxc::detect_idmap().and_then(|idmap| {
                    let mut c = LxcConfig::new(opts.data_dir.join("lxc"));
                    c.idmap = idmap;
                    c.backups_dir = opts.data_dir.join("backups");
                    c.stop_timeout = opts.shutdown_timeout;
                    LxcDriver::new(c)
                });
                match built {
                    Ok(d) => {
                        if d.config().idmap.is_none() {
                            eprintln!("UYARI: hizmet kök olarak çalışıyor; konteynerler yetkili (kimlik eşlemesiz) olur.");
                        }
                        Some(d)
                    }
                    Err(e) => {
                        eprintln!("Konteyner desteği kapalı: {e}");
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
                eprintln!("UYARI: /dev/kvm kullanılamıyor; konuklar yazılım öykünmesiyle (TCG) çok yavaş çalışır.");
                eprintln!("       Hizmet kullanıcısını kvm grubuna ekleyin: sudo usermod -aG kvm <kullanıcı>");
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
    let auth = Auth::new(users, Some(opts.users.clone()), opts.behind_tls_proxy, Cost::Production);

    let (driver, mut cluster, state_file, images) = prepare(&opts)?;
    let recovered = recover(&mut cluster, now());
    if recovered > 0 {
        eprintln!("{recovered} yarım kalmış görev başarısız olarak kapatıldı.");
    }
    let state = AppState::new(cluster, driver, opts.task_delay, state_file.clone(), auth);
    state.load_metrics();

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("{addr} adresi dinlenemedi: {e}"))?;
    println!("RedoX VE http://{addr} adresinde çalışıyor (sürücü: {}).", state.driver_name());
    println!("Kullanıcılar: {}", opts.users.display());
    match &state_file {
        Some(p) => println!("Durum {} dosyasında saklanıyor.", p.display()),
        None => println!("Durum yalnızca bellekte; kalıcı olması için --state DOSYA verin."),
    }
    if let Some(url) = &opts.alert_webhook {
        state.alerts().set_webhook(url)?;
        println!("Uyarılar webhook'a gönderilecek.");
    }
    if state.driver().firewall() {
        match state.sync_firewall() {
            Ok(()) => println!("Konuk güvenlik duvarı açık (nftables, bridge ailesi, tablo: redox)."),
            Err(e) => eprintln!("UYARI: {e}\n       Hizmetin CAP_NET_ADMIN yetkisi var mı? (AmbientCapabilities=CAP_NET_ADMIN)"),
        }
    }
    if let Some(path) = &opts.oidc {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let config = redox_server::oidc::Config::parse(&text)?;
        println!("Tek oturum açma açık: {} (eşleşen alan: {}).", config.issuer, config.claim);
        state.set_oidc(config);
    }
    if opts.four_eyes {
        state.set_four_eyes(true);
        println!("Dört göz ilkesi açık: yedek ve konuk silme ikinci bir yöneticinin onayını bekler.");
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
        println!("Yedek raporu her gün {time} saatinde e-postayla gönderilecek.");
    }
    if let Some(secs) = opts.backup_test_secs {
        state.set_boot_test_wait(Duration::from_secs(secs.clamp(5, 3600)));
    }
    // Dosya düzeyinde geri yükleme: guestfish kuruluysa açılır; açıkça verilen yol bulunamazsa hata.
    let guestfish = opts.guestfish_bin.clone().unwrap_or_else(|| "guestfish".to_string());
    if redox_server::guestfs::detect(&guestfish) {
        println!("Yedeklerin içindeki dosyalar okunabilir (guestfish: {guestfish}).");
        state.set_guestfish(Some(guestfish));
    } else if opts.guestfish_bin.is_some() {
        return Err(format!("{guestfish} çalıştırılamadı; --guestfish-bin yolunu denetleyin (sudo apt install libguestfs-tools)"));
    }
    if let Some(path) = &opts.offsite {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let config = redox_server::offsite::Config::parse(&text)?;
        println!("Yedekler sunucu dışına da kopyalanacak: {}", config.describe());
        state.set_offsite(config);
    }
    if !opts.offsite_copies.is_empty() && opts.offsite.is_none() {
        return Err("--offsite-copy için önce asıl dış hedef gerekir (--offsite DOSYA)".to_string());
    }
    for path in &opts.offsite_copies {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let config = redox_server::offsite::Config::parse(&text)?;
        println!("Yedeklerin ek kopyası: {}", config.describe());
        state.add_offsite_copy(config);
    }
    if let Some(path) = &opts.peers {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        let peers = redox_server::peers::parse_file(&text)?;
        println!("Birleşik görünüm: {} eş sunucu ({}).", peers.len(), peers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
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
        println!("Uyarılar e-postayla gönderilecek: {}", config.describe());
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
                    eprintln!("ağ kartı korumaları yüklenemedi: {e}");
                }
                let events = st.alerts_tick();
                for e in &events {
                    println!("uyarı: {}", e.text(&st.snapshot().name));
                }
                if let Err(e) = st.alerts().deliver(&st.snapshot().name, &events) {
                    eprintln!("{e}");
                }
                if let Err(e) = st.alerts().deliver_email(&st.snapshot().name, &events) {
                    eprintln!("uyarı e-postası gönderilemedi: {e}");
                }
            })
            .await;
            tokio::time::sleep(Duration::from_secs(15)).await;
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
                if let Some(Err(e)) = st.report_tick(&today, minute) {
                    eprintln!("yedek raporu gönderilemedi: {e}");
                }
                st.scheduler_tick(&today, minute)
            })
            .await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    if let Some(images) = images {
        println!("Veri dizini: {} (disk imajları images/, kurulum kalıpları iso/)", opts.data_dir.display());
        if opts.incremental_backups {
            println!("Artımlı yedek açık: yeni sanal makine yedekleri parça deposuna yazılır (backups/chunks/).");
        }
        // Konukların, düğümün ve diskin gerçek durumunu düzenli olarak oku.
        // Sunucu yeniden açıldıysa (hizmetin yeniden başlaması değil) "açılışta başlat" işaretli konuklar sırayla başlatılır.
        let mut boot = !opts.no_autostart && host::booted_since_last_run(&opts.data_dir.join("boot-id"), host::boot_id().as_deref());
        for (id, dir) in &opts.storages {
            println!("Ek depolama: {id} → {}", dir.display());
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
                            println!("Sunucu açılışı: {started} konuk başlatıldı.");
                        }
                    });
                }
                tokio::time::sleep(REFRESH).await;
            }
        });
    }
    if opts.behind_tls_proxy {
        println!("TLS vekili arkasında: çerezler Secure, HSTS açık, X-Forwarded-For'a güveniliyor.");
    } else if !addr.ip().is_loopback() {
        eprintln!("UYARI: şifresiz HTTP ağa açık; parolalar ve oturumlar dinlenebilir.");
    }

    let closing = state.clone();
    axum::serve(listener, app(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| format!("sunucu hatası: {e}"))?;
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
                println!("{} içinde kullanıcı yok.", path.display());
            }
            for u in &file.users {
                let tokens: Vec<&str> = u.tokens.iter().map(|t| t.name.as_str()).collect();
                println!("{:<20} {:<9} token: {}", u.name, u.role.label(), if tokens.is_empty() { "-".to_string() } else { tokens.join(", ") });
            }
            return Ok(());
        }
        ["user", "add", name] => file.add_user(name, &read_new_password()?, Role::Admin, Cost::Production)?,
        ["user", "add", name, "--role", role] => file.add_user(name, &read_new_password()?, role.parse()?, Cost::Production)?,
        ["user", "role", name, role] => file.set_role(name, role.parse()?)?,
        ["user", "scope", name] => file.set_scope(name, Vec::new())?,
        ["user", "scope", name, tags] => file.set_scope(name, redox_core::parse_tags(tags))?,
        ["user", "sso", name, subject] => file.set_sso(name, subject)?,
        ["user", "passwd", name] => file.set_password(name, &read_new_password()?, Cost::Production)?,
        ["user", "remove", name] => file.remove_user(name)?,
        ["user", "totp-reset", name] => file.disable_totp(name)?,
        ["token", "create", user, label] => {
            let plain = file.create_token(user, label)?;
            file.save(&path)?;
            println!("{plain}");
            eprintln!("Bu token bir daha gösterilmez; şimdi güvenli bir yere kaydedin.");
            return Ok(());
        }
        ["token", "revoke", user, label] => file.revoke_token(user, label)?,
        _ => return Err(format!("anlaşılmayan komut: {}\n\n{}", rest.join(" "), cli::USAGE)),
    }
    file.save(&path)?;
    println!("Tamam: {} güncellendi.", path.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            print!("{}", cli::USAGE);
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
        Some(_) => cli::run(&args).map(|out| {
            use std::io::Write;
            // Çıktı `head` gibi erken kapanan bir boruya gidiyorsa hata vermeden biter.
            let _ = writeln!(std::io::stdout(), "{out}");
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
