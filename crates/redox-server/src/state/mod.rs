use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redox_core::{Action, Backup, Cluster, Error, Guest, GuestPatch, GuestSpec, Hypervisor, SnapshotOp, TaskId, TaskStatus, VmId};

use crate::auth::{write_private, Auth};
use crate::host::HostSample;

mod approvals;
mod maintenance;
mod ratelimit;
mod backup;
mod guest;
mod hardware;
mod netguard;
mod observe;
mod remote;
mod replica;
mod schedule;
mod snapshots;
mod transfer;

/// Grafiklerde tutulan örnek sayısı (5 sn aralıkla 10 dakika).
pub const HISTORY_LEN: usize = 120;

pub type History = HashMap<String, Vec<f64>>;

/// Zamanlayıcının görev günlüğündeki adı.
pub const SCHEDULER: &str = "zamanlayıcı";
/// Sunucu açılışında konukları başlatan iç kullanıcı.
pub const BOOT: &str = "açılış";
/// Beklenmedik biçimde kapanan konukları yeniden başlatan iç kullanıcı.
pub const HA: &str = "gözcü";

/// Sunucunun yerel saatiyle bugünün tarihi (`YYYY-AA-GG`) ve gece yarısından itibaren dakika.
pub fn local_day_minute() -> (String, u16) {
    use chrono::Timelike;
    let t = chrono::Local::now();
    (t.format("%Y-%m-%d").to_string(), (t.hour() * 60 + t.minute()) as u16)
}

/// Yerel gün, haftanın günü (0 Pazartesi … 6 Pazar) ve gece yarısından beri geçen dakika.
pub fn local_now() -> (String, u8, u16) {
    use chrono::{Datelike, Timelike};
    let t = chrono::Local::now();
    (t.format("%Y-%m-%d").to_string(), t.weekday().num_days_from_monday() as u8, (t.hour() * 60 + t.minute()) as u16)
}

/// Yedeği sırayla her hedeften geri yüklemeyi dener: asıl hedefe erişilemiyorsa ya da kopya orada bozuksa
/// kopya hedeflerine geçilir. Hedef yoksa yalnızca yerel dosyalar kullanılır.
fn restore_from_any(d: &dyn Hypervisor, g: &Guest, backup: &Backup, disks: &[u8], remotes: &[Arc<crate::offsite::Config>]) -> Result<(), String> {
    let Some((first, rest)) = remotes.split_first() else {
        return restore_work(d, g, backup, disks, None);
    };
    let mut outcome = restore_work(d, g, backup, disks, Some(first));
    for remote in rest {
        if outcome.is_ok() {
            break;
        }
        outcome = restore_work(d, g, backup, disks, Some(remote));
    }
    outcome
}

/// Geri yüklemenin sürücü tarafı: yerelde eksik dosyalar dış kopyadan iner, sonra diskler ve makine durumu
/// konuğa yazılır. Hem konuğun üstüne geri yüklemede hem yedekten yeni konuk açmada kullanılır.
fn restore_work(d: &dyn Hypervisor, g: &Guest, backup: &Backup, disks: &[u8], remote: Option<&Arc<crate::offsite::Config>>) -> Result<(), String> {
    let id1 = backup.id.as_str();
    // Yerel dosya yoksa (disk değişmiş olabilir) dış kopya önce geri indirilir.
    if let Some(config) = remote {
        let fetch = |disk: u8| match d.backup_disk_file(id1, disk) {
            Some(path) if !path.exists() => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // Bu sürümle gönderilmiş kopya adına bağlıdır: hedefte başka bir yedeğin dosyasıyla
                // değiştirilmişse geri yükleme reddedilir.
                config.pull_bound(&path, backup.offsite_bound)
            }
            _ => Ok(()),
        };
        if backup.new_mb.is_some() {
            // Parçalı dış kopya: yalnızca yerelde eksik parçalar iner (bütün diskler tek dizindedir).
            if !d.backup_present(id1) && !d.pull_backup(id1, &**config)? {
                fetch(0)?;
            }
        } else {
            // Tam yedek: yerelde eksik olan her disk dosyası indirilir.
            for disk in std::iter::once(0).chain(disks.iter().copied()) {
                fetch(disk)?;
            }
        }
    }
    // Makine durumu arşivi yerelde yoksa o da dış kopyadan iner.
    if let (true, Some(config), Some(path)) = (backup.machine, remote, d.backup_machine_file(id1)) {
        if !path.exists() {
            config.pull_bound(&path, backup.offsite_bound)?;
        }
    }
    match (d.restore(g, id1, disks), remote) {
        // Yerel parça bozuk ya da eksikse (geri yükleme hiçbir diske dokunmadan durmuştur) dış kopyadan
        // onarılıp bir kez daha denenir.
        (Err(first), Some(config)) if backup.new_mb.is_some() => match d.pull_backup(id1, &**config) {
            Ok(true) => d.restore(g, id1, disks),
            _ => Err(first),
        },
        (outcome, _) => outcome,
    }
}

/// `export_backup`'ın bu çağrı için ürettiği geçici kopyayı ve (boşsa) dizinini siler.
pub fn discard_export(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// Yedek sınamasının kapı denetimi: `127.0.0.1:port` adresine (konuğun kapısına yönlendirilmiştir) bağlanır
/// ve hizmetten **veri gelmesini** bekler. QEMU'nun kullanıcı kipindeki ağı sunucu tarafındaki bağlantıyı
/// konuk yanıt vermeden de kabul ettiği için bağlanabilmek tek başına kanıt değildir. Hizmet kendiliğinden
/// konuşmuyorsa (HTTP gibi) kısa bir istek gönderilir. `patience`: hizmetin ayağa kalkması için tanınan süre.
pub fn probe_tcp(port: u16, patience: Duration) -> Result<(), String> {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = std::time::Instant::now() + patience;
    loop {
        let attempt = (|| -> Result<(), String> {
            let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("bağlanılamadı: {e}"))?;
            let mut byte = [0u8; 1];
            for (step, wait) in [(0, Duration::from_secs(2)), (1, Duration::from_secs(5))] {
                if step == 1 {
                    stream.write_all(b"HEAD / HTTP/1.0\r\n\r\n").map_err(|e| format!("bağlantı koptu: {e}"))?;
                }
                stream.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
                match stream.read(&mut byte) {
                    Ok(0) => return Err("bağlantı veri gelmeden kapandı (kapı kapalı ya da hizmet çalışmıyor)".to_string()),
                    Ok(_) => return Ok(()),
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(e) => return Err(format!("bağlantı koptu: {e}")),
                }
            }
            Err("hizmet veri göndermedi (konuk yanıt vermiyor olabilir; veri göndermeyen hizmetler için sınama komutunu kullanın)".to_string())
        })();
        let Err(why) = attempt else { return Ok(()) };
        if std::time::Instant::now() >= deadline {
            return Err(why);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sunucunun paylaşılan durumu: küme, sürücü, kimlik doğrulama ve kalıcılık ayarı.
/// API hız sınırı durumu: (dakikadaki sınır, kullanıcı → (dakika, o dakikadaki istek sayısı)).
type ApiRate = (u32, HashMap<String, (u64, u32)>);

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    cluster: Mutex<Cluster>,
    driver: Box<dyn Hypervisor>,
    task_delay: Duration,
    state_file: Option<PathBuf>,
    auth: Auth,
    alerts: crate::alerts::Alerts,
    ha: Mutex<crate::ha::Watch>,
    peers: crate::peers::Peers,
    /// Yedeklerin sunucu dışı hedefi; yapılandırılmadıysa `None`.
    offsite: Mutex<Option<Arc<crate::offsite::Config>>>,
    /// Yedeklerin kopyalandığı ek dış hedefler (3-2-1: ikinci, üçüncü kopya).
    offsite_copies: Mutex<Vec<Arc<crate::offsite::Config>>>,
    /// Tek oturum açma; yapılandırılmadıysa `None`.
    oidc: Mutex<Option<Arc<crate::oidc::Oidc>>>,
    /// Yedeklerin içindeki dosyaları okuyan `guestfish` programı; yoksa dosya düzeyinde geri yükleme kapalıdır.
    guestfish: Mutex<Option<String>>,
    /// Yedek sınamasında geçici konuğun çalışır halde bekletildiği süre.
    boot_test_wait: Mutex<Duration>,
    /// Zamanlanmış yedek sınaması: (kaç günde bir, başlama saati gece yarısından dakika); 0 gün kapalı.
    auto_test: Mutex<(u32, u16)>,
    /// Bu kadar gündür yedeği olmayan konuk "korumasız" sayılır; 0 kapalı.
    backup_age_days: Mutex<u32>,
    /// Dört göz ilkesi: yedek ve konuk silme ikinci bir yöneticinin onayını bekler.
    four_eyes: std::sync::atomic::AtomicBool,
    /// API hız sınırı: (dakikadaki istek sınırı; 0 kapalı, kullanıcı → (dakika, o dakikadaki istek sayısı)).
    api_rate: Mutex<ApiRate>,
    /// Günlük yedek raporunun saati (gece yarısından dakika) ve son gönderildiği gün.
    report: Mutex<(Option<u16>, Option<String>)>,
    /// Gerçek sürücüden toplanan ölçüm geçmişi (anahtar: `node:cpu`, `guest:100:mem` …).
    history: Mutex<HashMap<String, VecDeque<f64>>>,
    /// Dakikalık ve saatlik ortalamalar (diske yazılır).
    long: Mutex<crate::history::Long>,
}

impl AppState {
    pub fn new(
        cluster: Cluster,
        driver: Box<dyn Hypervisor>,
        task_delay: Duration,
        state_file: Option<PathBuf>,
        auth: Auth,
    ) -> Self {
        AppState(Arc::new(Inner {
            cluster: Mutex::new(cluster),
            driver,
            task_delay,
            state_file,
            auth,
            alerts: crate::alerts::Alerts::default(),
            ha: Mutex::new(Default::default()),
            peers: crate::peers::Peers::default(),
            offsite: Mutex::new(None),
            oidc: Mutex::new(None),
            guestfish: Mutex::new(None),
            offsite_copies: Mutex::new(Vec::new()),
            boot_test_wait: Mutex::new(Duration::from_secs(60)),
            auto_test: Mutex::new((0, 180)),
            backup_age_days: Mutex::new(7),
            four_eyes: std::sync::atomic::AtomicBool::new(false),
            api_rate: Mutex::new((0, HashMap::new())),
            report: Mutex::new((None, None)),
            history: Mutex::new(HashMap::new()),
            long: Mutex::new(Default::default()),
        }))
    }

    pub fn auth(&self) -> &Auth {
        &self.0.auth
    }

    pub fn alerts(&self) -> &crate::alerts::Alerts {
        &self.0.alerts
    }

    pub fn set_oidc(&self, config: crate::oidc::Config) {
        *self.0.oidc.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(crate::oidc::Oidc::new(config)));
    }

    pub fn oidc(&self) -> Option<Arc<crate::oidc::Oidc>> {
        self.0.oidc.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_guestfish(&self, bin: Option<String>) {
        *self.0.guestfish.lock().unwrap_or_else(|e| e.into_inner()) = bin;
    }

    pub fn guestfish(&self) -> Option<String> {
        self.0.guestfish.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn driver(&self) -> &dyn Hypervisor {
        self.0.driver.as_ref()
    }

    pub fn driver_name(&self) -> &'static str {
        self.0.driver.name()
    }

    fn lock(&self) -> MutexGuard<'_, Cluster> {
        // Zehirlenmiş kilit: veri hâlâ tutarlıdır (her değişiklik tek metotta biter).
        self.0.cluster.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> Cluster {
        self.lock().clone()
    }

    /// Konuğun etiketleri (yetki denetimi için; bütün kümeyi kopyalamadan).
    pub fn guest_tags(&self, vmid: VmId) -> Option<Vec<String>> {
        self.lock().guest(vmid).ok().map(|g| g.tags.clone())
    }

    /// Kümeyi değiştirir ve ardından diske yazar.
    pub fn mutate<R>(&self, f: impl FnOnce(&mut Cluster) -> R) -> R {
        let mut cluster = self.lock();
        let result = f(&mut cluster);
        self.persist(&cluster);
        result
    }

    fn persist(&self, cluster: &Cluster) {
        let Some(path) = &self.0.state_file else { return };
        if let Err(e) = write_atomically(path, cluster) {
            eprintln!("uyarı: durum {} dosyasına yazılamadı: {e}", path.display());
        }
    }
}

fn write_atomically(path: &Path, cluster: &Cluster) -> std::io::Result<()> {
    // Durum dosyası yalnızca hizmet kullanıcısınca okunabilir (0600).
    write_private(path, &serde_json::to_vec_pretty(cluster)?)
}

/// Durum dosyasını okur. Dosya yoksa `Ok(None)`; bozuksa hata döner (üzerine yazılmaz).
pub fn load_state(path: &Path) -> Result<Option<Cluster>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{} okunamadı: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} açılamadı: {e}", path.display())),
    }
}

/// Önceki süreçten yarım kalan görevleri başarısız sayar ve konukların kilidini açar.
/// Döndürdüğü sayı, kapatılan görev sayısıdır.
pub fn recover(cluster: &mut Cluster, now: u64) -> usize {
    let stale: Vec<TaskId> = cluster
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Running)
        .map(|t| t.id)
        .collect();
    for id in &stale {
        let _ = cluster.complete_task(*id, Err("sunucu yeniden başlatıldı; görev yarıda kaldı".into()), now);
    }
    stale.len()
}

#[cfg(test)]
mod tests;

/// Yaşam döngüsü dışındaki konuk işlemleri: kurallar çekirdekte, dosya işleri sürücüde.
impl AppState {
    /// Uzun süren bir işlemi görev olarak yürütür: konuğu kilitler, sürücüyü kilit dışında
    /// çalıştırır, sonucu görev günlüğüne yazar. `done` başarıda kümeye değişikliği işler.
    #[allow(clippy::too_many_arguments)]
    fn run_op<T: Send + 'static>(
        &self,
        vmid: VmId,
        user: &str,
        command: String,
        need_stopped: Option<&'static str>,
        check: impl FnOnce(&Cluster) -> Result<(), Error>,
        work: impl FnOnce(&dyn Hypervisor, &Guest) -> Result<T, String> + Send + 'static,
        done: impl FnOnce(&mut Cluster, T) + Send + 'static,
    ) -> Result<TaskId, Error> {
        let (task, guest) = self.mutate(|c| {
            check(c)?;
            let task = c.begin_op(vmid, command, user, now(), need_stopped)?;
            Ok::<_, Error>((task, c.guest(vmid)?.clone()))
        })?;
        let me = self.clone();
        let job = move || {
            let result = work(me.driver(), &guest);
            me.mutate(|c| match result {
                Ok(value) => {
                    let _ = c.complete_task(task, Ok(()), now());
                    done(c, value);
                }
                Err(message) => {
                    let _ = c.complete_task(task, Err(message), now());
                }
            });
        };
        self.dispatch(job);
        Ok(task)
    }

    /// İşi hemen (testler) ya da arka planda, bloklayan iş parçacığında çalıştırır.
    fn dispatch(&self, job: impl FnOnce() + Send + 'static) {
        if self.0.task_delay.is_zero() {
            job();
        } else {
            let delay = self.0.task_delay;
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = tokio::task::spawn_blocking(job).await;
            });
        }
    }

    /// Konuk üzerindeki işlem bitene kadar bekler ve konuğun son halini döndürür.
    fn wait_unlocked(&self, vmid: VmId, limit: Duration) -> Result<Guest, String> {
        let deadline = std::time::Instant::now() + limit;
        loop {
            let guest = self.snapshot().guest(vmid).map_err(|e| e.to_string())?.clone();
            if guest.lock.is_none() {
                return Ok(guest);
            }
            if std::time::Instant::now() >= deadline {
                return Err("işlem zamanında bitmedi".to_string());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Konuğun en son başarısız görevinin iletisi.
    fn last_failure(&self, vmid: VmId) -> String {
        self.snapshot().tasks.iter().find(|t| t.vmid == Some(vmid) && t.status == TaskStatus::Failed).and_then(|t| t.message.clone()).unwrap_or_else(|| "neden bilinmiyor".to_string())
    }

    /// Sürücü canlı disk işlemi yapabiliyorsa konuğun durdurulması gerekmez.
    fn stopped_unless_live(&self, kind: redox_core::GuestKind, what: &'static str) -> Option<&'static str> {
        if self.driver().live_disk_ops(kind) { None } else { Some(what) }
    }

    fn kind_of(&self, vmid: VmId) -> Result<redox_core::GuestKind, Error> {
        Ok(self.lock().guest(vmid)?.kind)
    }
}

#[cfg(test)]
mod op_tests;
