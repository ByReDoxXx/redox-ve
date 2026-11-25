//! LXC sürücüsü: konteynerleri `lxc-*` araçlarıyla yönetir.
//!
//! Konteynerler sunucunun veri dizinindeki kendi LXC yolunda (`-P`) durur; hizmet kök
//! değilse **yetkisiz** (kimlik eşlemeli) çalışır. Bu sürücü Windows'ta derlenir ama
//! yalnızca Linux'ta, `lxc` paketi kuruluyken çalışır.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use redox_core::{mac_for, valid_bridge, Action, BackupSize, Guest, GuestKind, GuestStatus, Hypervisor, Probe, SnapshotOp, VmId, NAT};

/// Sunulan şablonlar: (ad, dağıtım, sürüm). `download` şablonunun adlandırmasıyla.
pub const TEMPLATES: [(&str, &str, &str); 8] = [
    ("alpine-3.24", "alpine", "3.24"),
    ("debian-13", "debian", "trixie"),
    ("debian-12", "debian", "bookworm"),
    ("ubuntu-26.04", "ubuntu", "resolute"),
    ("ubuntu-24.04", "ubuntu", "noble"),
    ("rockylinux-10", "rockylinux", "10"),
    ("rockylinux-9", "rockylinux", "9"),
    ("alpine-3.20", "alpine", "3.20"),
];

/// İndirme sunucusundan kaldırılmış sürümler: var olan konteynerlerin etiketi geçerli kalsın diye tabloda
/// durur, ama yeni konteyner için önerilmez.
const RETIRED: [&str; 1] = ["alpine-3.20"];

/// Hizmetin süreçlerinin durduğu alt grup (systemd `DelegateSubgroup=`); birim dosyasındakiyle aynı olmalıdır.
pub const SUPERVISOR_SUBGROUP: &str = "supervisor";

/// `/proc/self/cgroup` içeriğinden, hizmete devredilmiş cgroup'un yolu: süreç `…/supervisor` alt grubundaysa
/// onun üstü. Başka her durumda (eski systemd, elle çalıştırma, cgroup v1) `None`.
pub fn delegated_cgroup(proc_self_cgroup: &str) -> Option<&str> {
    let own = proc_self_cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let parent = own.strip_suffix(SUPERVISOR_SUBGROUP)?.strip_suffix('/')?;
    (!parent.is_empty() && !parent.contains("..")).then_some(parent)
}

/// `lxc-start` günlüğünden asıl hata: genel kapanış satırları ve zararsız D-Bus uyarısı atlanır, ilk gerçek
/// hatanın açıklaması (kaynak dosya öneki olmadan) verilir.
pub fn start_failure(log: &str) -> Option<String> {
    const NOISE: [&str; 7] = ["Failed opening dbus connection", "The container failed to start", "To get more details", "Additional information can be obtained", "instead of \"RUNNING\"", "Failed to spawn container", "An error occurred in another process"];
    // Başarılı başlatmalarda da görülen satır: başka hata yoksa neden budur, varsa asıl neden o değildir.
    const MINOR: &str = "controllers in the unified cgroup";
    let errors: Vec<String> = log
        .lines()
        .filter(|line| line.contains(" ERROR "))
        .filter(|line| !NOISE.iter().any(|noise| line.contains(noise)))
        .filter_map(|line| line.rsplit_once(" - ").map(|(_, text)| text.trim().to_string()))
        .filter(|text| !text.is_empty())
        .collect();
    let real: Vec<&String> = errors.iter().filter(|text| !text.contains(MINOR)).collect();
    match real.as_slice() {
        [] => errors.first().cloned(),
        [only] => Some((*only).clone()),
        [first, second, ..] => Some(format!("{first}; {second}")),
    }
}

/// `lxc-create` hatasını açıklar: imaj sunucuda yoksa bunu söyler, yoksa aracın son satırını verir.
pub fn explain_create(os: &str, stdout: &[u8], stderr: &[u8]) -> String {
    let all = format!("{}\n{}", String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr));
    if all.contains("Couldn't find a matching image") {
        return format!("{os} imajı indirme sunucusunda yok (sürüm eskimiş olabilir); başka bir şablon seçin");
    }
    if all.contains("Failed to download") || all.contains("Unable to fetch") || all.contains("Temporary failure in name resolution") {
        return format!("{os} imajı indirilemedi; sunucunun internete çıkabildiğini denetleyin ({})", last_line(stderr));
    }
    last_line(if stderr.is_empty() { stdout } else { stderr })
}

/// LXC'nin kendi NAT köprüsü (`lxc-net` hizmeti kurar).
pub const LXC_NAT_BRIDGE: &str = "lxcbr0";
const BEGIN: &str = "# --- redox: bu blok RedoX VE tarafından yönetilir ---";
const END: &str = "# --- redox: blok sonu ---";
/// Konteynerde çalıştırılan komutun en uzun süresi ve gösterilen çıktı sınırı.
const EXEC_TIMEOUT: Duration = Duration::from_secs(20);
const EXEC_OUTPUT_LIMIT: usize = 64 * 1024;
pub const EXEC_COMMAND_LIMIT: usize = 2000;

#[derive(Debug, Clone)]
pub struct LxcConfig {
    /// Konteynerlerin durduğu dizin (`lxc-* -P`).
    pub lxc_path: PathBuf,
    /// Araçların bulunduğu dizin; `None` ise PATH'ten aranır (testler sahte araç dizini verir).
    pub bin_dir: Option<PathBuf>,
    /// "Kapat" isteğinde konteynerin kendi kapanması için beklenen süre.
    pub stop_timeout: Duration,
    /// Yetkisiz çalışma için kimlik eşlemesi: (uid başlangıcı, adet, gid başlangıcı, adet).
    pub idmap: Option<(u64, u64, u64, u64)>,
    /// Konteyner yedeklerinin (tar.gz) durduğu dizin.
    pub backups_dir: PathBuf,
    /// Verilirse yedek arşivleri diske bu parolayla şifreli yazılır.
    pub backup_passphrase: Option<String>,
}

impl LxcConfig {
    pub fn new(lxc_path: impl Into<PathBuf>) -> Self {
        let lxc_path = lxc_path.into();
        let backups_dir = lxc_path.parent().map(|p| p.join("backups")).unwrap_or_else(|| lxc_path.join("backups"));
        LxcConfig { lxc_path, bin_dir: None, stop_timeout: Duration::from_secs(30), idmap: None, backups_dir, backup_passphrase: None }
    }
}

pub fn container_name(vmid: VmId) -> String {
    format!("redox-{vmid}")
}

pub fn template(os: &str) -> Option<(&'static str, &'static str)> {
    TEMPLATES.iter().find(|(name, _, _)| *name == os).map(|(_, dist, release)| (*dist, *release))
}

/// Konteynerin sınırlarını ve ağını tanımlayan yönetilen yapılandırma bloğu.
pub fn managed_block(g: &Guest) -> String {
    let bridge = if g.bridge == NAT || !valid_bridge(&g.bridge) { LXC_NAT_BRIDGE } else { g.bridge.as_str() };
    let mut lines = vec![
        BEGIN.to_string(),
        format!("lxc.uts.name = {}", g.name),
        format!("lxc.cgroup2.memory.max = {}M", g.mem_total_mb),
        // Dönem 100 ms; kota çekirdek sayısı kadar dönem.
        format!("lxc.cgroup2.cpu.max = {} 100000", g.cores as u64 * 100_000),
        "lxc.net.0.type = veth".to_string(),
        format!("lxc.net.0.link = {bridge}"),
        "lxc.net.0.flags = up".to_string(),
        format!("lxc.net.0.hwaddr = {}", mac_for(g.vmid)),
        format!("lxc.start.auto = {}", u8::from(g.onboot)),
    ];
    for (i, network) in g.hw.nics.iter().enumerate().filter(|(_, n)| valid_bridge(n)) {
        let n = i + 1;
        let link = if network == NAT { LXC_NAT_BRIDGE } else { network.as_str() };
        lines.extend([
            format!("lxc.net.{n}.type = veth"),
            format!("lxc.net.{n}.link = {link}"),
            format!("lxc.net.{n}.flags = up"),
            format!("lxc.net.{n}.hwaddr = {}", redox_core::mac_for_nic(g.vmid, n as u8)),
        ]);
    }
    lines.push(END.to_string());
    lines.push(String::new());
    lines.join("\n")
}

/// Var olan yapılandırmadaki yönetilen bloğu yenisiyle değiştirir (yoksa sona ekler).
/// Blok dışındaki satırlara (LXC'nin ve kullanıcının yazdıkları) dokunulmaz.
pub fn merge_config(existing: &str, block: &str) -> String {
    let mut kept = Vec::new();
    let mut inside = false;
    for line in existing.lines() {
        match line.trim() {
            BEGIN => inside = true,
            END => inside = false,
            _ if !inside => kept.push(line),
            _ => {}
        }
    }
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block);
    out
}

/// Kimlik eşlemesi satırları (`lxc-create -f` ile verilen temel yapılandırma).
pub fn idmap_lines(idmap: Option<(u64, u64, u64, u64)>) -> String {
    match idmap {
        Some((uid, uids, gid, gids)) => format!("lxc.idmap = u 0 {uid} {uids}\nlxc.idmap = g 0 {gid} {gids}\n"),
        None => String::new(),
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Info {
    pub state: String,
    pub ip: Option<String>,
    pub cpu_ns: Option<u64>,
    pub mem_bytes: Option<u64>,
}

/// `lxc-info -H` çıktısı (anahtar: değer satırları; -H sayıları ham verir).
pub fn parse_info(text: &str) -> Info {
    let mut info = Info::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match key.trim() {
            "State" => info.state = value.to_string(),
            // Birden çok adres olabilir; ilk IPv4 alınır.
            "IP" if info.ip.is_none() && value.parse::<std::net::Ipv4Addr>().is_ok() => info.ip = Some(value.to_string()),
            "CPU use" => info.cpu_ns = value.split_whitespace().next().and_then(|v| v.parse().ok()),
            "Memory use" => info.mem_bytes = value.split_whitespace().next().and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    info
}

pub struct LxcDriver {
    cfg: LxcConfig,
    /// Son CPU örneği: (nanosaniye, an).
    samples: Mutex<HashMap<VmId, (u64, Instant)>>,
    /// Son gözlemde okunan adresler.
    ips: Mutex<HashMap<VmId, String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("(çıktı yok)").trim().to_string()
}

const UNSUPPORTED: &str = "konteynerlerde desteklenmiyor (yalnızca sanal makinelerde)";

impl LxcDriver {
    /// Araçların varlığını denetler ve konteyner dizinini oluşturur.
    pub fn new(cfg: LxcConfig) -> Result<Self, String> {
        std::fs::create_dir_all(&cfg.lxc_path).map_err(|e| format!("{} oluşturulamadı: {e}", cfg.lxc_path.display()))?;
        // Yetkisiz konteynerin kökü (eşlenmiş kimlik) kendi kök dosya sistemine ulaşmak için bu dizinden
        // geçebilmelidir; hizmetin UMask'ı dizini 0700 oluşturur ve konteyner başlamaz. Yalnızca geçiş izni
        // verilir: dizinin içeriği başkalarınca listelenemez.
        #[cfg(unix)]
        if cfg.idmap.is_some() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&cfg.lxc_path, std::fs::Permissions::from_mode(0o711));
        }
        let driver = LxcDriver { cfg, samples: Mutex::new(HashMap::new()), ips: Mutex::new(HashMap::new()) };
        let probe = driver.tool("lxc-create").arg("--version").output();
        if !probe.is_ok_and(|o| o.status.success()) {
            return Err("lxc-create çalıştırılamadı. LXC kurulu mu?\n    Debian/Ubuntu: sudo apt install lxc uidmap".to_string());
        }
        Ok(driver)
    }

    pub fn config(&self) -> &LxcConfig {
        &self.cfg
    }

    fn tool(&self, name: &str) -> Command {
        let mut command = match &self.cfg.bin_dir {
            Some(dir) => Command::new(dir.join(name)),
            None => Command::new(name),
        };
        command.arg("-P").arg(&self.cfg.lxc_path);
        command
    }

    /// `-P yol -n ad` ile bir aracı çalıştırır; başarısızsa hata çıktısının son satırını döndürür.
    fn run(&self, name: &str, vmid: VmId, extra: &[&str]) -> Result<Vec<u8>, String> {
        let out = self
            .tool(name)
            .args(["-n", &container_name(vmid)])
            .args(extra)
            .output()
            .map_err(|e| format!("{name} çalıştırılamadı: {e}"))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            // LXC araçları iletiyi çoğunlukla kendi adlarıyla başlatır; ad iki kez yazılmasın.
            let message = last_line(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
            Err(if message.starts_with(name) { message } else { format!("{name}: {message}") })
        }
    }

    /// Konteyneri başlatır. Yetkisiz LXC, konteynerin cgroup'larını kendi bulunduğu grubun altında kurar ve o
    /// grupta başka süreç olmamasını ister (cgroup v2 kuralı). Hizmetin kendi süreçleri de oradaysa konteyner
    /// başlamaz; bu yüzden `lxc-start`, hizmete devredilmiş cgroup'un altında yalnızca kendisinin durduğu ayrı
    /// bir gruptan çalıştırılır. Devredilmiş grup yoksa (bkz. `delegated_cgroup`) olağan yolla başlatılır.
    fn start(&self, vmid: VmId) -> Result<Vec<u8>, String> {
        // LXC'nin kendi hata iletisi geneldir ("ek bilgi için --logfile"); asıl neden günlükten okunur.
        let log = self.cfg.lxc_path.join(format!(".{}.start.log", container_name(vmid)));
        let _ = std::fs::remove_file(&log);
        let mut command = match self.launch_group(vmid) {
            Some(group) => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(r#"echo $$ > "$0/cgroup.procs" && exec "$@""#).arg(group).arg(match &self.cfg.bin_dir {
                    Some(dir) => dir.join("lxc-start"),
                    None => PathBuf::from("lxc-start"),
                });
                c.arg("-P").arg(&self.cfg.lxc_path);
                c
            }
            None => self.tool("lxc-start"),
        };
        let out = command
            .args(["-n", &container_name(vmid)])
            .arg("--logfile")
            .arg(&log)
            .args(["--logpriority", "ERROR"])
            .output()
            .map_err(|e| format!("lxc-start çalıştırılamadı: {e}"))?;
        let reason = std::fs::read_to_string(&log).ok().and_then(|text| start_failure(&text));
        let _ = std::fs::remove_file(&log);
        if out.status.success() {
            return Ok(out.stdout);
        }
        let message = reason.unwrap_or_else(|| last_line(if out.stderr.is_empty() { &out.stdout } else { &out.stderr }));
        Err(if message.starts_with("lxc-start") { message } else { format!("lxc-start: {message}") })
    }

    /// Bu konteynerin başlatılacağı cgroup dizini; kurulamıyorsa `None`.
    fn launch_group(&self, vmid: VmId) -> Option<PathBuf> {
        let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let base = PathBuf::from(format!("/sys/fs/cgroup{}", delegated_cgroup(&own)?));
        // Denetleyiciler alt gruplara açılır (zaten açıksa ya da desteklenmiyorsa hata yok sayılır).
        for controller in ["cpu", "memory", "pids", "io", "cpuset"] {
            let _ = std::fs::write(base.join("cgroup.subtree_control"), format!("+{controller}"));
        }
        let group = base.join(format!("ct-{vmid}"));
        std::fs::create_dir_all(&group).ok()?;
        // Hizmetin UMask'ı dizini 0700 oluşturur; konteynerin kökü (eşlenmiş kimlik) kendi cgroup ağacına
        // ulaşmak için buradan geçebilmelidir, yoksa konteynerde cgroup2 bağlanamaz.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&group, std::fs::Permissions::from_mode(0o755));
        }
        Some(group)
    }

    fn config_path(&self, vmid: VmId) -> PathBuf {
        self.cfg.lxc_path.join(container_name(vmid)).join("config")
    }

    fn exists(&self, vmid: VmId) -> bool {
        self.config_path(vmid).is_file()
    }

    fn info(&self, vmid: VmId) -> Option<Info> {
        self.run("lxc-info", vmid, &["-H"]).ok().map(|out| parse_info(&String::from_utf8_lossy(&out)))
    }

    fn write_managed(&self, g: &Guest) -> Result<(), String> {
        let path = self.config_path(g.vmid);
        let existing = std::fs::read_to_string(&path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
        std::fs::write(&path, merge_config(&existing, &managed_block(g))).map_err(|e| format!("{} yazılamadı: {e}", path.display()))
    }
}

impl Hypervisor for LxcDriver {
    fn name(&self) -> &'static str {
        "lxc"
    }

    fn is_real(&self) -> bool {
        true
    }

    fn supports(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Ct
    }

    fn templates(&self) -> Vec<String> {
        TEMPLATES.iter().map(|(name, _, _)| name.to_string()).filter(|name| !RETIRED.contains(&name.as_str())).collect()
    }

    fn create(&self, g: &Guest) -> Result<(), String> {
        let (dist, release) = template(&g.os).ok_or_else(|| format!("bilinmeyen şablon: {}", g.os))?;
        if self.cfg.lxc_path.join(container_name(g.vmid)).exists() {
            return Err(format!("{} adlı konteyner dizini zaten var", container_name(g.vmid)));
        }
        // Kimlik eşlemesi oluşturma anında gerekir: kök dosya sistemi eşlenmiş kimliklerle açılır.
        let base = self.cfg.lxc_path.join(format!(".{}.conf", container_name(g.vmid)));
        std::fs::write(&base, idmap_lines(self.cfg.idmap)).map_err(|e| format!("{} yazılamadı: {e}", base.display()))?;
        let base_arg = base.display().to_string();
        let created = self
            .tool("lxc-create")
            .args(["-n", &container_name(g.vmid), "-f", &base_arg, "-t", "download", "--", "-d", dist, "-r", release, "-a", "amd64"])
            .output()
            .map_err(|e| format!("lxc-create çalıştırılamadı: {e}"));
        let _ = std::fs::remove_file(&base);
        let created = created?;
        if !created.status.success() {
            // Yarım kalan dizin bir sonraki denemeyi engellemesin.
            let _ = self.run("lxc-destroy", g.vmid, &[]);
            let _ = std::fs::remove_dir_all(self.cfg.lxc_path.join(container_name(g.vmid)));
            let message = explain_create(&g.os, &created.stdout, &created.stderr);
            return Err(if message.starts_with("lxc-create") { message } else { format!("lxc-create: {message}") });
        }
        self.write_managed(g).inspect_err(|_| {
            let _ = self.run("lxc-destroy", g.vmid, &[]);
        })
    }

    fn configure(&self, g: &Guest) -> Result<(), String> {
        if self.exists(g.vmid) { self.write_managed(g) } else { Ok(()) }
    }

    fn destroy(&self, g: &Guest) -> Result<(), String> {
        lock(&self.samples).remove(&g.vmid);
        lock(&self.ips).remove(&g.vmid);
        if !self.cfg.lxc_path.join(container_name(g.vmid)).exists() {
            return Ok(());
        }
        self.run("lxc-destroy", g.vmid, &[]).map(|_| ())
    }

    fn apply(&self, g: &Guest, action: Action) -> Result<(), String> {
        let timeout = self.cfg.stop_timeout.as_secs().to_string();
        let result = match action {
            Action::Start => self.start(g.vmid),
            // Süre dolunca lxc-stop konteyneri zorla durdurur.
            Action::Stop => self.run("lxc-stop", g.vmid, &["-t", &timeout]),
            Action::Reboot => self.run("lxc-stop", g.vmid, &["-r", "-t", &timeout]),
            Action::Pause => self.run("lxc-freeze", g.vmid, &[]),
            Action::Resume => self.run("lxc-unfreeze", g.vmid, &[]),
        };
        if action == Action::Stop {
            lock(&self.samples).remove(&g.vmid);
            lock(&self.ips).remove(&g.vmid);
        }
        result.map(|_| ())
    }

    fn probe(&self, g: &Guest) -> Option<Probe> {
        let stopped = Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None };
        let Some(info) = self.info(g.vmid) else { return Some(stopped) };
        let status = match info.state.as_str() {
            "RUNNING" | "STARTING" | "STOPPING" => GuestStatus::Running,
            "FROZEN" | "FREEZING" | "THAWED" => GuestStatus::Paused,
            _ => {
                lock(&self.samples).remove(&g.vmid);
                lock(&self.ips).remove(&g.vmid);
                return Some(stopped);
            }
        };
        match info.ip {
            Some(ip) => lock(&self.ips).insert(g.vmid, ip),
            None => lock(&self.ips).remove(&g.vmid),
        };
        let now = Instant::now();
        let cpu_pct = info.cpu_ns.and_then(|ns| {
            let previous = lock(&self.samples).insert(g.vmid, (ns, now));
            previous.map(|(old, at)| {
                let secs = now.duration_since(at).as_secs_f64();
                if secs <= 0.0 || g.cores == 0 {
                    return 0;
                }
                let used = ns.saturating_sub(old) as f64 / 1e9;
                (used / secs / g.cores as f64 * 100.0).round().clamp(0.0, 100.0) as u8
            })
        });
        Some(Probe { status, cpu_pct, mem_used_mb: info.mem_bytes.map(|b| b / (1024 * 1024)) })
    }

    fn guest_ip(&self, g: &Guest) -> Option<String> {
        lock(&self.ips).get(&g.vmid).cloned()
    }

    fn exec(&self, g: &Guest, command: &str) -> Result<String, String> {
        if command.trim().is_empty() {
            return Err("komut boş".to_string());
        }
        if command.len() > EXEC_COMMAND_LIMIT {
            return Err(format!("komut en fazla {EXEC_COMMAND_LIMIT} bayt olabilir"));
        }
        // Çıktı dosyaya yazılır: boru dolup süreç kilitlenmesin, süre sınırı uygulanabilsin.
        let capture = self.cfg.lxc_path.join(format!(".{}.exec", container_name(g.vmid)));
        let file = std::fs::File::create(&capture).map_err(|e| format!("çıktı dosyası açılamadı: {e}"))?;
        let stderr = file.try_clone().map_err(|e| e.to_string())?;
        let mut child = self
            .tool("lxc-attach")
            .args(["-n", &container_name(g.vmid), "--clear-env", "--", "/bin/sh", "-c", command])
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(stderr)
            .spawn()
            .map_err(|e| format!("lxc-attach çalıştırılamadı: {e}"))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() >= EXEC_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("komut beklenemedi: {e}")),
            }
        };
        let bytes = std::fs::read(&capture).unwrap_or_default();
        let _ = std::fs::remove_file(&capture);
        let mut output = String::from_utf8_lossy(&bytes[..bytes.len().min(EXEC_OUTPUT_LIMIT)]).to_string();
        if bytes.len() > EXEC_OUTPUT_LIMIT {
            output.push_str("\n[çıktı kesildi]");
        }
        match status {
            None => Err(format!("komut {} saniyede bitmedi ve durduruldu\n{output}", EXEC_TIMEOUT.as_secs())),
            Some(s) if s.success() => Ok(output),
            Some(s) => Ok(format!("{output}\n[çıkış kodu {}]", s.code().map(|c| c.to_string()).unwrap_or_else(|| "sinyal".to_string()))),
        }
    }

    fn snapshot(&self, g: &Guest, op: SnapshotOp<'_>) -> Result<(), String> {
        let name = match op {
            SnapshotOp::Create(n) | SnapshotOp::Rollback(n) | SnapshotOp::Delete(n) => n,
        };
        if !redox_core::valid_snapshot_name(name) {
            return Err(format!("geçersiz anlık görüntü adı: {name}"));
        }
        // LXC görüntülere kendi adını verir (snap0, snap1 …); bizim adımızla eşlemesi yan dosyada tutulur.
        let mut map = self.read_snapshot_map(g.vmid);
        match op {
            SnapshotOp::Create(_) => {
                if map.iter().any(|(ours, _)| ours == name) {
                    return Err(format!("\"{name}\" adlı anlık görüntü zaten var"));
                }
                let before = self.lxc_snapshots(g.vmid);
                self.run("lxc-snapshot", g.vmid, &[])?;
                let created = self.lxc_snapshots(g.vmid).into_iter().find(|s| !before.contains(s)).ok_or("lxc-snapshot yeni görüntü oluşturmadı")?;
                map.push((name.to_string(), created));
            }
            SnapshotOp::Rollback(_) | SnapshotOp::Delete(_) => {
                let theirs = map.iter().find(|(ours, _)| ours == name).map(|(_, t)| t.clone()).ok_or_else(|| format!("anlık görüntü bulunamadı: {name}"))?;
                if matches!(op, SnapshotOp::Rollback(_)) {
                    return self.run("lxc-snapshot", g.vmid, &["-r", &theirs]).map(|_| ());
                }
                self.run("lxc-snapshot", g.vmid, &["-d", &theirs])?;
                map.retain(|(ours, _)| ours != name);
            }
        }
        self.write_snapshot_map(g.vmid, &map)
    }

    fn live_backup(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Ct
    }

    /// Kök dosya sisteminin sıkıştırılmış arşivi. Çalışan konteyner arşivleme süresince dondurulur
    /// (süreçleri durur, dosyalar değişmez) ve sonra çözülür; kapalı ya da zaten duraklatılmış konteynere dokunulmaz.
    fn backup(&self, g: &Guest, id: &str) -> Result<BackupSize, String> {
        let target = self.backup_path(id)?;
        std::fs::create_dir_all(&self.cfg.backups_dir).map_err(|e| format!("yedek dizini oluşturulamadı: {e}"))?;
        if target.exists() {
            return Err(format!("{} zaten var", target.display()));
        }
        let running = self.info(g.vmid).is_some_and(|i| i.state == "RUNNING");
        if running {
            self.run("lxc-freeze", g.vmid, &[]).map_err(|e| format!("konteyner yedek için dondurulamadı: {e}"))?;
        }
        let made = self.archive(g, &target);
        if running {
            if let Err(e) = self.run("lxc-unfreeze", g.vmid, &[]) {
                let _ = std::fs::remove_file(&target);
                return Err(format!("konteyner yedekten sonra çözülemedi ({e}); elle çözün: lxc-unfreeze -n {}", container_name(g.vmid)));
            }
        }
        made
    }

    /// Kök dosya sistemini boşaltıp arşivi yerine açar. Yapılandırma (sınırlar, ağ) değişmez.
    fn restore(&self, g: &Guest, id: &str, _disks: &[u8]) -> Result<(), String> {
        let source = self.backup_path(id)?;
        if !source.is_file() {
            return Err(format!("yedek dosyası bulunamadı: {}", source.display()));
        }
        let rootfs = self.rootfs(g.vmid)?;
        let step = |out: std::io::Result<std::process::Output>, what: &str| match out {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(format!("{what}: {}", last_line(&o.stderr))),
            Err(e) => Err(format!("{what}: {e}")),
        };
        step(self.userns().arg("find").arg(&rootfs).args(["-mindepth", "1", "-delete"]).output(), "eski dosyalar silinemedi")?;
        let mut unpack = self.userns();
        unpack.args(["tar", "--numeric-owner", "-C"]).arg(&rootfs).args(["-xzf", "-"]);
        step(self.feed(id, unpack), "yedek geri yüklenemedi")
    }

    fn delete_backup(&self, id: &str) -> Result<(), String> {
        let path = self.backup_path(id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{} silinemedi: {e}", path.display())),
        }
    }

    fn backup_file(&self, id: &str) -> Option<PathBuf> {
        self.backup_path(id).ok()
    }

    fn backups_sealed(&self) -> bool {
        self.cfg.backup_passphrase.is_some()
    }

    /// İndirme ve eş sunucuya aktarma için arşivin açık kopyası: şifreli arşiv geçici bir dizine çözülür.
    fn export_backup(&self, id: &str, _disk: u8) -> Result<Option<(PathBuf, bool)>, String> {
        let path = self.backup_path(id)?;
        if !self.sealed(&path) {
            return Ok(Some((path, false)));
        }
        let dir = self.cfg.backups_dir.join(format!(".acik-{id}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
        let plain = dir.join(format!("{id}.tar.gz"));
        let written = std::fs::File::create(&plain).map_err(|e| format!("{} oluşturulamadı: {e}", plain.display())).and_then(|mut out| self.pour(id, &mut out));
        match written {
            Ok(()) => Ok(Some((plain, true))),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    /// Arşivin sıkıştırma bütünlüğünü denetler.
    fn verify_backup(&self, id: &str, _disks: &[u8]) -> Result<String, String> {
        let path = self.backup_path(id)?;
        if !path.is_file() {
            return Err(format!("yedek dosyası bulunamadı: {}", path.display()));
        }
        let mut check = std::process::Command::new("gzip");
        check.arg("-t");
        match self.feed(id, check) {
            Ok(o) if o.status.success() => Ok("arşiv tutarlı".to_string()),
            Ok(o) => Err(format!("yedek bozuk: {}", last_line(&o.stderr))),
            Err(e) => Err(format!("gzip çalıştırılamadı: {e}")),
        }
    }

    fn resize_disk(&self, _: &Guest, _: u64) -> Result<(), String> {
        Err(format!("disk büyütme {UNSUPPORTED}"))
    }

    fn suspend(&self, _: &Guest) -> Result<(), String> {
        Err(format!("askıya alma {UNSUPPORTED}"))
    }

    fn link_disk(&self, _: &Guest, _: &Guest) -> Result<(), String> {
        Err(format!("bağlı klon {UNSUPPORTED}"))
    }

    /// Kapalı konteyneri yeni adla kopyalar (`lxc-copy`), sonra yönetilen ayarları (ad, sınırlar, ağ adresi)
    /// yeni konuğa göre yazar. Anlık görüntüler kopyalanmaz.
    fn clone_disk(&self, source: &Guest, target: &Guest) -> Result<(), String> {
        let new_name = container_name(target.vmid);
        if self.cfg.lxc_path.join(&new_name).exists() {
            return Err(format!("{new_name} adlı konteyner dizini zaten var"));
        }
        self.run("lxc-copy", source.vmid, &["-N", &new_name])?;
        let _ = std::fs::remove_file(self.snapshot_map_path(target.vmid));
        self.write_managed(target).inspect_err(|_| {
            let _ = self.run("lxc-destroy", target.vmid, &[]);
        })
    }
}

impl LxcDriver {
    /// Arşiv dosyası şifreli mi (başında şifreleme imi var mı)?
    fn sealed(&self, path: &std::path::Path) -> bool {
        use std::io::Read;
        let mut head = [0u8; 8];
        std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && crate::offsite::is_encrypted(&head)
    }

    /// Arşivin açık içeriğini `out` içine döker; şifreliyse çözer.
    fn pour(&self, id: &str, out: &mut dyn std::io::Write) -> Result<(), String> {
        let path = self.backup_path(id)?;
        let mut file = std::fs::File::open(&path).map_err(|_| format!("yedek dosyası bulunamadı: {}", path.display()))?;
        if !self.sealed(&path) {
            return std::io::copy(&mut file, out).map(|_| ()).map_err(|e| format!("yedek okunamadı: {e}"));
        }
        let passphrase = self.cfg.backup_passphrase.as_deref().ok_or("yedek şifreli: sunucuyu --backup-passphrase-file ile başlatın")?;
        crate::offsite::decrypt(&mut file, passphrase, out, id, true).map_err(|_| "yedek çözülemedi: yerel yedek parolası yanlış ya da dosya bozulmuş".to_string())
    }

    /// Komutu çalıştırır ve arşivin açık içeriğini standart girdisine verir.
    fn feed(&self, id: &str, mut command: Command) -> std::io::Result<std::process::Output> {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
        let mut stdin = child.stdin.take().expect("stdin borusu");
        let poured = self.pour(id, &mut stdin);
        drop(stdin);
        let output = child.wait_with_output()?;
        // Arşiv okunamadıysa ya da çözülemediyse asıl neden budur; komutun yakınması bunun sonucudur.
        poured.map(|()| output).map_err(std::io::Error::other)
    }

    /// Kök dosya sistemini `target` arşivine yazar; parola varsa arşiv diske şifreli yazılır.
    fn archive(&self, g: &Guest, target: &std::path::Path) -> Result<BackupSize, String> {
        let rootfs = self.rootfs(g.vmid)?;
        let mut file = std::fs::File::create(target).map_err(|e| format!("{} oluşturulamadı: {e}", target.display()))?;
        let mut pack = self.userns();
        pack.args(["tar", "--numeric-owner", "-C"]).arg(&rootfs).args(["-czf", "-", "."]).stdin(Stdio::null());
        let out = match &self.cfg.backup_passphrase {
            None => pack.stdout(file).output().map_err(|e| format!("tar çalıştırılamadı: {e}")),
            Some(passphrase) => (|| {
                let id = target.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".tar.gz")).ok_or("geçersiz yedek adı")?;
                let mut child = pack.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("tar çalıştırılamadı: {e}"))?;
                let stdout = child.stdout.take().expect("stdout borusu");
                let copied = crate::offsite::EncryptReader::new(stdout, passphrase, id).and_then(|mut sealed| std::io::copy(&mut sealed, &mut file).map_err(|e| format!("yedek yazılamadı: {e}")));
                let output = child.wait_with_output().map_err(|e| format!("tar çalıştırılamadı: {e}"))?;
                match copied {
                    Err(e) if output.status.success() => Err(e),
                    _ => Ok(output),
                }
            })(),
        };
        match out {
            Ok(o) if o.status.success() => Ok(BackupSize::full(std::fs::metadata(target).map(|m| m.len()).unwrap_or(0).div_ceil(1024 * 1024))),
            other => {
                let _ = std::fs::remove_file(target);
                Err(match other {
                    Ok(o) => format!("yedek alınamadı: {}", last_line(&o.stderr)),
                    Err(e) => e,
                })
            }
        }
    }

    fn backup_path(&self, id: &str) -> Result<PathBuf, String> {
        if !crate::qemu::valid_backup_id(id) {
            return Err(format!("geçersiz yedek kimliği: {id}"));
        }
        Ok(self.cfg.backups_dir.join(format!("{id}.tar.gz")))
    }

    fn rootfs(&self, vmid: VmId) -> Result<PathBuf, String> {
        let path = self.cfg.lxc_path.join(container_name(vmid)).join("rootfs");
        if path.is_dir() { Ok(path) } else { Err(format!("kök dosya sistemi bulunamadı: {}", path.display())) }
    }

    /// Kök dosya sistemindeki dosyalar eşlenmiş kimliklere aittir: yetkisiz kurulumda komutlar
    /// konteynerin kimlik eşlemesiyle (`lxc-usernsexec`) çalıştırılır; kök kurulumda doğrudan.
    fn userns(&self) -> Command {
        let Some((uid, uids, gid, gids)) = self.cfg.idmap else {
            return Command::new("env");
        };
        let mut command = match &self.cfg.bin_dir {
            Some(dir) => Command::new(dir.join("lxc-usernsexec")),
            None => Command::new("lxc-usernsexec"),
        };
        command.args(["-m", &format!("u:0:{uid}:{uids}"), "-m", &format!("g:0:{gid}:{gids}"), "--"]);
        command
    }

    /// LXC'nin kendi görüntü adları (`snaps/` altındaki dizinler).
    fn lxc_snapshots(&self, vmid: VmId) -> Vec<String> {
        std::fs::read_dir(self.cfg.lxc_path.join(container_name(vmid)).join("snaps"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }

    fn snapshot_map_path(&self, vmid: VmId) -> PathBuf {
        self.cfg.lxc_path.join(container_name(vmid)).join("redox-snapshots")
    }

    fn read_snapshot_map(&self, vmid: VmId) -> Vec<(String, String)> {
        std::fs::read_to_string(self.snapshot_map_path(vmid))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())))
            .collect()
    }

    fn write_snapshot_map(&self, vmid: VmId, map: &[(String, String)]) -> Result<(), String> {
        let text: String = map.iter().map(|(ours, theirs)| format!("{ours} {theirs}\n")).collect();
        std::fs::write(self.snapshot_map_path(vmid), text).map_err(|e| format!("anlık görüntü kaydı yazılamadı: {e}"))
    }
}

/// Hizmet kök değilse konteynerler için gereken alt kimlik aralığını okur.
/// Kök ise `None` döner (eşlemesiz, yetkili konteyner).
pub fn detect_idmap() -> Result<Option<(u64, u64, u64, u64)>, String> {
    let id = |flag: &str| -> Option<String> {
        let out = Command::new("id").arg(flag).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let (Some(uid), Some(user)) = (id("-u"), id("-un")) else {
        return Err("kullanıcı kimliği okunamadı (id komutu)".to_string());
    };
    if uid == "0" {
        return Ok(None);
    }
    let range = |file: &str| {
        std::fs::read_to_string(file).ok().and_then(|text| crate::net::subid_range(&text, &user, &uid)).ok_or_else(|| {
            format!("{file} içinde {user} için aralık yok. Ekleyin:\n    sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 {user}")
        })
    };
    let ((uid_start, uid_count), (gid_start, gid_count)) = (range("/etc/subuid")?, range("/etc/subgid")?);
    Ok(Some((uid_start, uid_count, gid_start, gid_count)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::{Cluster, GuestSpec};

    fn guest(bridge: &str) -> Guest {
        let mut c = Cluster::demo(0);
        let spec = GuestSpec {
            vmid: Some(250),
            name: "kutu".into(),
            kind: GuestKind::Ct,
            node: "pve-ist-01".into(),
            cores: 2,
            memory_mb: 512,
            disk_gb: 8,
            storage: "local-zfs".into(),
            os: "alpine-3.20".into(),
            bridge: bridge.into(),
            onboot: true,
            iso: None,
            hw: Default::default(),
        };
        c.create_guest(&spec, "u", 0).unwrap();
        c.guest(250).unwrap().clone()
    }

    #[test]
    fn extra_nics_get_their_own_addresses() {
        let mut g = guest("vmbr0");
        g.hw.nics = vec!["vmbr1".into(), "nat".into(), "bad name".into()];
        let block = managed_block(&g);
        for line in ["lxc.net.1.type = veth", "lxc.net.1.link = vmbr1", "lxc.net.1.hwaddr = 52:54:01:00:00:fa", "lxc.net.2.link = lxcbr0", "lxc.net.2.hwaddr = 52:54:02:00:00:fa"] {
            assert!(block.contains(line), "{line} yok:\n{block}");
        }
        assert!(!block.contains("lxc.net.3"), "geçersiz ağ adı yapılandırmaya girmez");
        assert!(block.trim_end().ends_with(END));
    }

    #[test]
    fn managed_block_sets_limits_and_network() {
        let block = managed_block(&guest("vmbr0"));
        for line in [
            "lxc.uts.name = kutu",
            "lxc.cgroup2.memory.max = 512M",
            "lxc.cgroup2.cpu.max = 200000 100000",
            "lxc.net.0.type = veth",
            "lxc.net.0.link = vmbr0",
            "lxc.net.0.hwaddr = 52:54:00:00:00:fa",
            "lxc.start.auto = 1",
        ] {
            assert!(block.lines().any(|l| l == line), "eksik: {line}\n{block}");
        }
        assert!(block.starts_with(BEGIN) && block.trim_end().ends_with(END));
        // NAT ve geçersiz köprü adı LXC'nin kendi köprüsüne düşer.
        assert!(managed_block(&guest("nat")).contains("lxc.net.0.link = lxcbr0\n"));
        let mut hostile = guest("nat");
        hostile.bridge = "br0\nlxc.hook.pre-start = /tmp/x".into();
        let block = managed_block(&hostile);
        assert!(block.contains("lxc.net.0.link = lxcbr0\n") && !block.contains("hook"), "ad üzerinden satır eklenemez");
    }

    #[test]
    fn merge_replaces_only_the_managed_block() {
        let lxc_wrote = "# Template used to create this container\nlxc.rootfs.path = dir:/x/rootfs\nlxc.idmap = u 0 100000 65536\n";
        let first = merge_config(lxc_wrote, &managed_block(&guest("nat")));
        assert!(first.starts_with(lxc_wrote.trim_end()) && first.contains("memory.max = 512M"));

        let mut bigger = guest("nat");
        bigger.mem_total_mb = 2048;
        let second = merge_config(&format!("{first}lxc.mount.entry = /srv srv none bind 0 0\n"), &managed_block(&bigger));
        assert!(second.contains("memory.max = 2048M") && !second.contains("memory.max = 512M"), "eski blok kalmaz");
        assert!(second.contains("lxc.rootfs.path = dir:/x/rootfs") && second.contains("lxc.mount.entry = /srv"), "blok dışı satırlar korunur");
        assert_eq!(second.matches(BEGIN).count(), 1);
        // Aynı blokla yeniden birleştirmek dosyayı değiştirmez.
        assert_eq!(merge_config(&second, &managed_block(&bigger)), second);
        assert_eq!(merge_config("", "x\n"), "x\n");
    }

    #[test]
    fn idmap_and_names() {
        assert_eq!(idmap_lines(Some((100_000, 65_536, 200_000, 1000))), "lxc.idmap = u 0 100000 65536\nlxc.idmap = g 0 200000 1000\n");
        assert_eq!(idmap_lines(None), "");
        assert_eq!(container_name(250), "redox-250");
        assert_eq!(template("debian-12"), Some(("debian", "bookworm")));
        assert_eq!(template("ubuntu-24.04"), Some(("ubuntu", "noble")));
        assert_eq!(template("windows-2022"), None);
        assert_eq!(template("../etc"), None);
    }

    #[test]
    fn info_parsing() {
        let text = "Name:           redox-250\nState:          RUNNING\nPID:            4242\nIP:             fe80::1\nIP:             10.0.3.17\nIP:             10.0.3.18\nCPU use:        1500000000\nMemory use:     52428800\nKMem use:       0\nLink:           vethX\n TX bytes:      1024\n";
        assert_eq!(
            parse_info(text),
            Info { state: "RUNNING".into(), ip: Some("10.0.3.17".into()), cpu_ns: Some(1_500_000_000), mem_bytes: Some(52_428_800) }
        );
        assert_eq!(parse_info("Name: x\nState: STOPPED\n"), Info { state: "STOPPED".into(), ..Info::default() });
        assert_eq!(parse_info("bozuk çıktı"), Info::default());
    }

    #[test]
    fn missing_tools_are_reported_with_install_hint() {
        let dir = std::env::temp_dir().join(format!("rdx-lxc-{}", std::process::id()));
        let mut cfg = LxcConfig::new(dir.join("lxc"));
        cfg.bin_dir = Some(dir.join("yok"));
        let err = LxcDriver::new(cfg).err().expect("araç yoksa sürücü kurulmaz");
        assert!(err.contains("apt install lxc"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_errors_name_a_missing_image_and_retired_templates_are_not_offered() {
        let missing = explain_create("alpine-3.20", b"Downloading the image index\nERROR: Couldn't find a matching image\n", b"lxc-create: x: Failed to create container x\n");
        assert!(missing.contains("alpine-3.20 imajı indirme sunucusunda yok"), "{missing}");
        assert!(explain_create("debian-13", b"", b"ERROR: Failed to download http://x\n").contains("internete"));
        assert_eq!(explain_create("debian-13", b"", b"a\nlxc-create: boom\n"), "lxc-create: boom");
        // Eski etiket çözülmeye devam eder, ama önerilenler arasında yoktur.
        assert_eq!(template("alpine-3.20"), Some(("alpine", "3.20")));
        assert_eq!(template("debian-13"), Some(("debian", "trixie")));
        assert!(RETIRED.iter().all(|name| template(name).is_some()));
    }

    #[test]
    fn delegated_cgroup_is_found_only_under_the_supervisor_subgroup() {
        assert_eq!(delegated_cgroup("0::/system.slice/redox.service/supervisor\n"), Some("/system.slice/redox.service"));
        assert_eq!(delegated_cgroup("1:name=systemd:/x\n0::/a/b/supervisor\n"), Some("/a/b"));
        for other in ["0::/system.slice/redox.service\n", "0::/user.slice/user-1000.slice/session-3.scope\n", "0::/supervisor\n", "0::/a/notsupervisor\n", "0::/a/../supervisor\n", "12:memory:/x\n", ""] {
            assert_eq!(delegated_cgroup(other), None, "{other}");
        }
    }

    #[test]
    fn start_failure_picks_the_first_real_error_from_the_log() {
        let log = "lxc-start redox-100 2026 WARN     apparmor - x.c:1 - Per-container AppArmor profiles are disabled
lxc-start redox-100 2026 ERROR    cgfsng - ../src/lxc/cgroups/cgfsng.c:unpriv_systemd_create_scope:1529 - Failed opening dbus connection
lxc-start redox-100 2026 ERROR    start - ../src/lxc/start.c:print_top_failing_dir:100 - Permission denied - Could not access /var/lib/redox/lxc. Please grant it x access
lxc-start redox-100 2026 ERROR    lxc_start - ../src/lxc/tools/lxc_start.c:lxc_start_main:307 - The container failed to start
";
        assert_eq!(start_failure(log).as_deref(), Some("Could not access /var/lib/redox/lxc. Please grant it x access"));
        let busy = "lxc-start x 2026 ERROR    cgfsng - c.c:1 - Device or resource busy - Could not enable \"+cpu\" controllers in the unified cgroup 11
";
        assert!(start_failure(busy).unwrap().contains("Could not enable"), "tek hata buysa o gösterilir");
        let two = format!("{busy}lxc-start x 2026 ERROR    conf - c.c:2 - No such file or directory - Failed to set \"memory.max\" to \"256M\"
lxc-start x 2026 ERROR    start - s.c:3 - Failed to setup cgroup limits for container \"x\"
");
        assert_eq!(start_failure(&two).as_deref(), Some("Failed to set \"memory.max\" to \"256M\"; Failed to setup cgroup limits for container \"x\""));
        assert_eq!(start_failure("lxc-start x 2026 ERROR    lxc_start - a.c:1 - The container failed to start
"), None);
        assert_eq!(start_failure(""), None);
    }
}
