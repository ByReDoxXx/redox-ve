//! Bu makinenin (düğümün) gerçek ölçümleri: `/proc` ve `df` çıktısından okunur.
//! Ayrıştırıcılar saf işlevlerdir; okuma yalnızca Linux'ta anlamlıdır.

use std::path::Path;
use std::sync::Mutex;

use redox_core::{Node, NodeStatus, Storage, StorageKind};

/// Bu makinenin NUMA, büyük sayfa ve KSM bilgisi (Linux dışında boştur).
pub fn performance() -> crate::qemu::perf::HostPerformance {
    crate::qemu::perf::read_performance(Path::new("/sys"), &std::fs::read_to_string("/proc/meminfo").unwrap_or_default())
}

/// `/proc/meminfo` → (toplam kB, kullanılabilir kB).
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        text.lines().find_map(|l| l.strip_prefix(name))?.split_whitespace().next()?.parse().ok()
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// `/proc/stat` ilk satırı → (boşta tık, toplam tık). Boşta = idle + iowait.
pub fn parse_cpu_line(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    Some((v[3] + v.get(4).copied().unwrap_or(0), v.iter().sum()))
}

/// İki `/proc/stat` örneği arasındaki CPU kullanım yüzdesi.
pub fn cpu_pct(previous: (u64, u64), current: (u64, u64)) -> u8 {
    let total = current.1.saturating_sub(previous.1);
    let idle = current.0.saturating_sub(previous.0);
    if total == 0 {
        return 0;
    }
    ((total.saturating_sub(idle)) as f64 / total as f64 * 100.0).round().clamp(0.0, 100.0) as u8
}

/// `/proc/uptime` → saniye.
pub fn parse_uptime(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse::<f64>().ok().map(|s| s as u64)
}

/// `df -Pk <dizin>` çıktısı → (dolu GB, toplam GB). Toplam = dolu + kullanılabilir
/// (kök için ayrılmış bloklar sayılmaz; kullanıcıya gerçekten yazılabilir alanı verir).
pub fn parse_df(text: &str) -> Option<(u64, u64)> {
    let fields: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let used: u64 = fields.get(2)?.parse().ok()?;
    let avail: u64 = fields.get(3)?.parse().ok()?;
    let gb = |kb: u64| kb / (1024 * 1024);
    Some((gb(used), gb(used + avail)))
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostSample {
    pub cpu_pct: u8,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub uptime_secs: u64,
}

/// Ardışık okumalar arasında CPU farkını tutar.
#[derive(Default)]
pub struct HostReader {
    previous: Mutex<Option<(u64, u64)>>,
}

impl HostReader {
    pub fn sample(&self) -> Option<HostSample> {
        let read = |p: &str| std::fs::read_to_string(p).ok();
        let (total_kb, avail_kb) = parse_meminfo(&read("/proc/meminfo")?)?;
        let cpu = parse_cpu_line(&read("/proc/stat")?)?;
        let mut previous = self.previous.lock().unwrap_or_else(|e| e.into_inner());
        let pct = previous.map(|p| cpu_pct(p, cpu)).unwrap_or(0);
        *previous = Some(cpu);
        Some(HostSample {
            cpu_pct: pct,
            mem_used_mb: total_kb.saturating_sub(avail_kb) / 1024,
            mem_total_mb: total_kb / 1024,
            uptime_secs: read("/proc/uptime").and_then(|t| parse_uptime(&t)).unwrap_or(0),
        })
    }
}

pub fn disk_usage_gb(dir: &Path) -> Option<(u64, u64)> {
    let out = std::process::Command::new("df").arg("-Pk").arg(dir).output().ok()?;
    out.status.success().then(|| parse_df(&String::from_utf8_lossy(&out.stdout)))?
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Düğüm kimliği olarak kullanılacak ana makine adı (adreslerde güvenli karakterlerle).
/// Çekirdeğin bu açılışa verdiği kimlik (Linux); başka sistemlerde `None`.
pub fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Sunucu, bu hizmetin son çalışmasından beri yeniden açıldı mı? Açılış kimliğini `marker` dosyasında tutar:
/// hizmet yeniden başlatıldığında (konuklar çalışmaya devam ederken) `false`, sunucu açıldığında `true` döner.
/// Kimlik okunamıyorsa (Linux değilse) `false`.
pub fn booted_since_last_run(marker: &Path, current: Option<&str>) -> bool {
    let Some(current) = current else { return false };
    if std::fs::read_to_string(marker).ok().as_deref().map(str::trim) == Some(current) {
        return false;
    }
    let _ = std::fs::write(marker, current);
    true
}

pub fn hostname() -> String {
    let raw = read_trimmed("/proc/sys/kernel/hostname")
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "local".to_string());
    let clean: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
        .collect();
    if clean.is_empty() { "local".to_string() } else { clean }
}

/// Dışarı giden arayüzün adresi. Paket gönderilmez; yalnızca yönlendirme sorulur.
fn local_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").and_then(|()| s.local_addr()))
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// Dışarı giden arayüzün adresi; ağ yoksa `None`.
pub fn primary_ip() -> Option<String> {
    Some(local_ip()).filter(|ip| ip != "127.0.0.1" && ip != "0.0.0.0")
}

/// Bu makineyi tek düğüm olarak tanımlar.
pub fn local_node() -> Node {
    Node {
        id: hostname(),
        status: NodeStatus::Online,
        cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
        cpu_pct: 0,
        mem_used_mb: 0,
        mem_total_mb: read_trimmed("/proc/meminfo")
            .and_then(|t| parse_meminfo(&t))
            .map(|(total, _)| total / 1024)
            .unwrap_or(1024),
        uptime_secs: 0,
        kernel: read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_else(|| "-".to_string()),
        ip: local_ip(),
    }
}

pub const LOCAL_STORAGE: &str = "local";

/// `/proc/self/mounts` içeriğinden `dir` dizinini taşıyan bağlama noktası: (aygıt, dosya sistemi türü).
fn mount_of<'a>(mounts: &'a str, dir: &Path) -> Option<(&'a str, &'a str)> {
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (device, point, kind) = (fields.next()?, fields.next()?, fields.next()?);
            dir.starts_with(point).then_some((point.len(), device, kind))
        })
        // En uzun bağlama noktası kazanır; eşitlikte sonradan bağlanan (üstte duran).
        .fold(None, |best: Option<(usize, &str, &str)>, m| if best.is_none_or(|b| m.0 >= b.0) { Some(m) } else { best })
        .map(|(_, device, kind)| (device, kind))
}

fn kind_from(mounts: &str, dir: &Path, dm_uuid: &dyn Fn(&str) -> Option<String>) -> StorageKind {
    match mount_of(mounts, dir) {
        Some((_, "zfs")) => StorageKind::Zfs,
        Some((_, kind)) if kind.starts_with("nfs") => StorageKind::Nfs,
        Some((device, _)) if device.starts_with("/dev/") && dm_uuid(device).is_some_and(|uuid| uuid.starts_with("LVM-")) => StorageKind::Lvm,
        _ => StorageKind::Dir,
    }
}

/// Dizinin durduğu depolamanın türü: ZFS veri kümesi, NFS paylaşımı, LVM birimi ya da düz dizin.
/// Yalnızca bilgi içindir; disk imajları her durumda dosya olarak durur.
pub fn storage_kind(dir: &Path) -> StorageKind {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else { return StorageKind::Dir };
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    kind_from(&mounts, &dir, &|device| {
        let real = std::fs::canonicalize(device).ok()?;
        read_trimmed(&format!("/sys/block/{}/dm/uuid", real.file_name()?.to_str()?))
    })
}

/// Depolama alanlarının türünü dizinlerine bakarak günceller (sunucu başlarken).
pub fn detect_storage_kinds(c: &mut redox_core::Cluster, images: &Path, extras: &[(String, std::path::PathBuf)]) {
    for s in c.storages.iter_mut() {
        let dir = if s.id == LOCAL_STORAGE { Some(images) } else { extras.iter().find(|(id, _)| *id == s.id).map(|(_, dir)| dir.as_path()) };
        if let Some(dir) = dir {
            s.kind = storage_kind(dir);
        }
    }
}

/// Disk imajlarının durduğu dizini tek depolama olarak tanımlar.
pub fn local_storage(node: &str, images_dir: &Path) -> Storage {
    let (used_gb, total_gb) = disk_usage_gb(images_dir).unwrap_or((0, 0));
    Storage {
        id: LOCAL_STORAGE.to_string(),
        node: Some(node.to_string()),
        kind: StorageKind::Dir,
        content: vec!["disk".to_string(), "ISO".to_string()],
        used_gb,
        total_gb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo() {
        let text = "MemTotal:       16303412 kB\nMemFree:         1000000 kB\nMemAvailable:    8151706 kB\nBuffers: 1 kB\n";
        assert_eq!(parse_meminfo(text), Some((16_303_412, 8_151_706)));
        assert_eq!(parse_meminfo("MemTotal: 5 kB\n"), None, "MemAvailable yoksa tahmin yürütülmez");
        assert_eq!(parse_meminfo(""), None);
    }

    #[test]
    fn cpu_usage_from_two_samples() {
        let a = parse_cpu_line("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(a, (850, 1000));
        let b = parse_cpu_line("cpu  160 0 90 900 50 0 0 0 0 0\n").unwrap();
        assert_eq!(b, (950, 1200));
        assert_eq!(cpu_pct(a, b), 50, "200 tıkın 100'ü boşta");
        assert_eq!(cpu_pct(a, a), 0, "fark yoksa sıfır");
        assert_eq!(cpu_pct(b, a), 0, "sayaç geri giderse taşma olmaz");
        assert_eq!(parse_cpu_line("cpu0 1 2 3 4\n"), None);
        assert_eq!(parse_cpu_line("cpu  1 2\n"), None);
    }

    #[test]
    fn uptime_and_df() {
        assert_eq!(parse_uptime("351735.47 2713908.44\n"), Some(351_735));
        assert_eq!(parse_uptime("x"), None);

        let df = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/sdd        1055762868  66060288 936000000       7% /\n";
        assert_eq!(parse_df(df), Some((63, 955)));
        assert_eq!(parse_df("Filesystem 1024-blocks Used Available Capacity Mounted on\n"), None);
        assert_eq!(parse_df("a\nb c\n"), None);
    }

    #[test]
    fn local_node_is_usable_as_cluster_seed() {
        let n = local_node();
        assert!(!n.id.is_empty() && n.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'), "{}", n.id);
        assert!(n.cores >= 1 && n.mem_total_mb >= 1);
        let s = local_storage(&n.id, &std::env::temp_dir());
        assert!(s.available_on(&n.id) && s.id == LOCAL_STORAGE);
    }
}

/// Depolama adı: küçük harf, rakam ve tire; 1-24 karakter. `local` veri dizinine ayrılmıştır.
pub fn valid_storage_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 24 && id != LOCAL_STORAGE && !id.starts_with('-') && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `--storage AD=DİZİN` seçeneğini ayrıştırır. Dizin mutlak olmalı ve var olmalıdır.
pub fn parse_storage(arg: &str) -> Result<(String, std::path::PathBuf), String> {
    let (id, dir) = arg.split_once('=').ok_or_else(|| format!("--storage AD=DİZİN biçiminde olmalı: {arg}"))?;
    if !valid_storage_id(id) {
        return Err(format!("geçersiz depolama adı \"{id}\": küçük harf, rakam ve tire; en çok 24 karakter; \"local\" kullanılamaz"));
    }
    let dir = std::path::PathBuf::from(dir);
    if !dir.is_absolute() {
        return Err(format!("{id} deposunun dizini mutlak yol olmalı: {}", dir.display()));
    }
    if !dir.is_dir() {
        return Err(format!("{id} deposunun dizini yok ya da dizin değil: {} (ağ paylaşımıysa bağlı mı?)", dir.display()));
    }
    Ok((id.to_string(), dir))
}

/// Kümedeki depolama listesini yapılandırmayla eşler: yeni alanlar eklenir, kullanılmayan eski alanlar
/// kaldırılır. Üzerinde konuk olan bir alan yapılandırmadan çıkarılmışsa sunucu başlamaz: diskleri bulunamaz.
pub fn sync_storages(c: &mut redox_core::Cluster, extras: &[(String, std::path::PathBuf)]) -> Result<(), String> {
    let Some(node) = c.nodes.first().map(|n| n.id.clone()) else { return Ok(()) };
    let mut seen = std::collections::HashSet::new();
    for (id, _) in extras {
        if !seen.insert(id.as_str()) {
            return Err(format!("{id} deposu iki kez tanımlanmış"));
        }
    }
    let orphans: Vec<String> = c.storages.iter().filter(|s| s.id != LOCAL_STORAGE && !extras.iter().any(|(id, _)| *id == s.id)).map(|s| s.id.clone()).collect();
    for id in orphans {
        let users: Vec<String> = c.guests.iter().filter(|g| g.storage == id).map(|g| g.vmid.to_string()).collect();
        if !users.is_empty() {
            return Err(format!("{id} deposu tanımlı değil ama konukların diski orada ({}); sunucuyu --storage {id}=DİZİN ile başlatın", users.join(", ")));
        }
        c.storages.retain(|s| s.id != id);
    }
    for (id, dir) in extras {
        let (used_gb, total_gb) = disk_usage_gb(dir).unwrap_or((0, 0));
        match c.storages.iter_mut().find(|s| s.id == *id) {
            Some(s) => (s.used_gb, s.total_gb) = (used_gb.min(total_gb), total_gb),
            None => c.storages.push(Storage { id: id.clone(), node: Some(node.clone()), kind: StorageKind::Dir, content: vec!["disk".to_string()], used_gb, total_gb }),
        }
    }
    Ok(())
}

#[cfg(test)]
mod storage_tests {
    use super::*;

    #[test]
    fn extra_storages_are_parsed_and_synced() {
        let dir = std::env::temp_dir().join(format!("redox-depo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let arg = format!("hizli={}", dir.display());
        assert_eq!(parse_storage(&arg).unwrap(), ("hizli".to_string(), dir.clone()));
        for bad in ["hizli", "=x", "local=/tmp", "Hızlı=/tmp", "-a=/tmp", "a_b=/tmp", "hizli=goreli/yol", &format!("hizli={}", dir.join("yok").display())] {
            assert!(parse_storage(bad).is_err(), "{bad}");
        }
        assert!(valid_storage_id("nvme-2") && !valid_storage_id(&"a".repeat(25)));

        let node = local_node();
        let mut c = redox_core::Cluster::local(node.clone(), local_storage(&node.id, &dir));
        let extras = vec![("hizli".to_string(), dir.clone())];
        sync_storages(&mut c, &extras).unwrap();
        sync_storages(&mut c, &extras).unwrap();
        assert_eq!(c.storages.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [LOCAL_STORAGE, "hizli"], "ikinci eşleme yinelemez");
        assert!(c.storages[1].available_on(&node.id) && c.storages[1].kind == StorageKind::Dir);
        assert!(sync_storages(&mut c, &[extras[0].clone(), extras[0].clone()]).unwrap_err().contains("iki kez"));

        // Kullanılmayan alan yapılandırmadan çıkınca kaldırılır; konuğu olan alan çıkarılamaz.
        sync_storages(&mut c, &[]).unwrap();
        assert_eq!(c.storages.len(), 1);
        sync_storages(&mut c, &extras).unwrap();
        c.storages[1].total_gb = c.storages[1].used_gb + 100;
        let spec = redox_core::GuestSpec { vmid: None, name: "depolu".into(), kind: redox_core::GuestKind::Vm, node: node.id.clone(), cores: 1, memory_mb: 256, disk_gb: 1, storage: "hizli".into(), os: "x".into(), bridge: "nat".into(), onboot: false, iso: None, hw: Default::default() };
        let id = c.create_guest(&spec, "u", 0).unwrap();
        let err = sync_storages(&mut c, &[]).unwrap_err();
        assert!(err.contains("hizli deposu tanımlı değil") && err.contains(&id.to_string()) && err.contains("--storage hizli=DİZİN"), "{err}");
        assert_eq!(c.storages.len(), 2, "hata durumunda liste değişmez");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod boot_tests {
    use super::*;

    #[test]
    fn boot_is_detected_once_per_boot_id() {
        let marker = std::env::temp_dir().join(format!("redox-boot-id-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        assert!(!booted_since_last_run(&marker, None), "kimlik okunamıyorsa açılış sayılmaz");
        assert!(!marker.exists());
        assert!(booted_since_last_run(&marker, Some("aaa")), "ilk çalıştırma açılış sayılır");
        assert!(!booted_since_last_run(&marker, Some("aaa")), "hizmetin yeniden başlaması açılış değildir");
        assert!(booted_since_last_run(&marker, Some("bbb")), "kimlik değişti: sunucu yeniden açıldı");
        assert!(!booted_since_last_run(&marker, Some("bbb")));
        std::fs::remove_file(&marker).unwrap();
    }

    #[test]
    fn storage_kind_follows_the_longest_mount_point() {
        const MOUNTS: &str = "/dev/sda2 / ext4 rw 0 0\ntank/vm /srv/vm zfs rw 0 0\n10.0.0.5:/yedek /srv/yedek nfs4 rw 0 0\n/dev/mapper/vg0-hizli /srv/hizli ext4 rw 0 0\n/dev/mapper/sifreli /srv/luks ext4 rw 0 0\n";
        let dm = |device: &str| match device {
            "/dev/mapper/vg0-hizli" => Some("LVM-abc".to_string()),
            "/dev/mapper/sifreli" => Some("CRYPT-LUKS2-abc".to_string()),
            _ => None,
        };
        let kind = |dir: &str| kind_from(MOUNTS, Path::new(dir), &dm);
        assert_eq!(kind("/srv/vm/images"), StorageKind::Zfs);
        assert_eq!(kind("/srv/yedek"), StorageKind::Nfs);
        assert_eq!(kind("/srv/hizli/a/b"), StorageKind::Lvm);
        assert_eq!(kind("/srv/luks"), StorageKind::Dir, "her aygıt eşleyici birimi LVM değildir");
        assert_eq!(kind("/var/lib/redox"), StorageKind::Dir);
        assert_eq!(kind("/srv/vmx"), StorageKind::Dir, "ad benzerliği bağlama noktası sayılmaz");
        assert_eq!(kind_from("", Path::new("/x"), &dm), StorageKind::Dir);
    }
}
