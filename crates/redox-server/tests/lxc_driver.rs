//! LXC sürücüsünün Linux üzerindeki davranışı. Gerçek `lxc-*` araçları yerine, aynı komut
//! satırını kabul eden ve durumu dosyada tutan sahte betikler kullanılır.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use redox_core::{Action, Cluster, Error, Guest, GuestKind, GuestPatch, GuestSpec, GuestStatus, Hypervisor, SnapshotOp, TaskStatus};
use redox_server::auth::{Auth, Cost, UserFile};
use redox_server::host;
use redox_server::hostdriver::HostDriver;
use redox_server::lxc::{LxcConfig, LxcDriver};
use redox_server::qemu::{QemuConfig, QemuDriver};
use redox_server::AppState;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Bütün sahte araçların ortak başı: `-P YOL -n AD` ve diğer seçenekleri ayıklar.
const HEAD: &str = r#"P=; N=; F=; R=; T=
while [ $# -gt 0 ]; do
  case "$1" in
    --version) echo 6.0.0; exit 0 ;;
    -P) P=$2; shift ;;
    -n) N=$2; shift ;;
    -f) F=$2; shift ;;
    -t) T=$2; shift ;;
    -r) R=1 ;;
    -H|--clear-env) ;;
    --) shift; break ;;
  esac
  shift
done
D="$P/$N"
need() { [ -d "$D" ] || { echo "$(basename "$0"): $N diye bir konteyner yok" >&2; exit 1; }; }
"#;

/// lxc-snapshot: görüntüleri snaps/snapN dizinleri olarak tutar (kök dosya sisteminin kopyası).
const SNAPSHOT_TOOL: &str = r#"#!/bin/sh
P=; N=; OP=create; S=
while [ $# -gt 0 ]; do
  case "$1" in
    -P) P=$2; shift ;;
    -n) N=$2; shift ;;
    -r) OP=restore; S=$2; shift ;;
    -d) OP=destroy; S=$2; shift ;;
  esac
  shift
done
D="$P/$N"
[ "$(cat "$D/state")" = STOPPED ] || { echo "lxc-snapshot: konteyner çalışırken görüntü alınamaz" >&2; exit 1; }
case $OP in
  create) i=0; while [ -e "$D/snaps/snap$i" ]; do i=$((i+1)); done; mkdir -p "$D/snaps/snap$i"; cp -a "$D/rootfs" "$D/snaps/snap$i/rootfs" ;;
  restore) [ -d "$D/snaps/$S" ] || { echo "lxc-snapshot: $S yok" >&2; exit 1; }; rm -rf "$D/rootfs"; cp -a "$D/snaps/$S/rootfs" "$D/rootfs" ;;
  destroy) rm -rf "$D/snaps/$S" ;;
esac
"#;

/// lxc-copy: kapalı konteynerin dizinini yeni adla kopyalar ve yapılandırmadaki adı değiştirir.
const COPY_TOOL: &str = r#"#!/bin/sh
P=; N=; NEW=
while [ $# -gt 0 ]; do
  case "$1" in
    -P) P=$2; shift ;;
    -n) N=$2; shift ;;
    -N) NEW=$2; shift ;;
  esac
  shift
done
[ -d "$P/$N" ] || { echo "lxc-copy: $N diye bir konteyner yok" >&2; exit 1; }
[ "$(cat "$P/$N/state")" = STOPPED ] || { echo "lxc-copy: konteyner çalışırken kopyalanamaz" >&2; exit 1; }
[ -e "$P/copy-fail" ] && { echo "lxc-copy: diskte yer kalmadı" >&2; exit 1; }
cp -a "$P/$N" "$P/$NEW"
sed -i "s/$N/$NEW/g" "$P/$NEW/config"
"#;

/// lxc-usernsexec: eşleme argümanlarını kaydeder ve komutu doğrudan çalıştırır.
const USERNS_TOOL: &str = r#"#!/bin/sh
MAPS=
while [ "$1" != "--" ]; do [ "$1" = "-m" ] || MAPS="$MAPS $1"; shift; done
shift
echo "$MAPS" > "$(dirname "$0")/last-userns"
[ -e "$(dirname "$0")/userns-fail" ] && { echo "cp: diskte yer kalmadı" >&2; exit 1; }
exec "$@"
"#;

const TOOLS: [(&str, &str); 8] = [
    ("lxc-create", r##"[ "$2" = alpine ] || [ "$2" = debian ] || { echo "lxc-create: $2 imajı indirilemedi" >&2; exit 1; }
mkdir -p "$D/rootfs"
{ echo "# Template used to create this container: download $*"; cat "$F"; echo "lxc.rootfs.path = dir:$D/rootfs"; } > "$D/config"
echo STOPPED > "$D/state""##),
    ("lxc-start", r#"need
grep -q "uts.name = bozuk" "$D/config" && { echo "lxc-start: konteyner başlatılamadı" >&2; exit 1; }
echo RUNNING > "$D/state""#),
    ("lxc-stop", r#"need
echo "t=$T r=$R" > "$D/last-stop"
[ -n "$R" ] || echo STOPPED > "$D/state""#),
    ("lxc-freeze", "need\necho FROZEN > \"$D/state\""),
    ("lxc-unfreeze", "need\necho RUNNING > \"$D/state\""),
    ("lxc-info", r#"need
S=$(cat "$D/state")
echo "Name:           $N"
echo "State:          $S"
if [ "$S" != STOPPED ]; then
  echo "PID:            4242"
  echo "IP:             10.0.3.17"
  echo "CPU use:        $(date +%s%N)"
  echo "Memory use:     52428800"
fi"#),
    ("lxc-destroy", "need\nrm -rf \"$D\""),
    // Komutu konteyner yerine burada çalıştırır: çıktı yakalama ve çıkış kodu sınanır.
    ("lxc-attach", "need\n[ \"$(cat \"$D/state\")\" = RUNNING ] || { echo \"lxc-attach: konteyner çalışmıyor\" >&2; exit 1; }\nexec \"$@\""),
];


/// Betiği bir alt süreçte yazar. Yazma tanıtıcısı bu süreçte hiç açılmadığı için, koşut çalışan bir
/// testin çatalladığı süreç onu devralıp betiğin çalıştırılmasını "dosya meşgul" hatasıyla engelleyemez.
fn write_script(path: &Path, content: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$0" && chmod 755 "$0""#)
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(content.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
}

fn setup() -> (LxcConfig, PathBuf) {
    let dir = PathBuf::from(format!("/tmp/rdx-lxc-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for (name, body) in TOOLS {
        let path = bin.join(name);
        write_script(&path, &format!("#!/bin/sh\n{HEAD}{body}\n"));
    }
    for (name, script) in [("lxc-snapshot", SNAPSHOT_TOOL), ("lxc-usernsexec", USERNS_TOOL), ("lxc-copy", COPY_TOOL)] {
        let path = bin.join(name);
        write_script(&path, script);
    }
    let mut cfg = LxcConfig::new(dir.join("data/lxc"));
    cfg.bin_dir = Some(bin);
    cfg.stop_timeout = Duration::from_secs(7);
    cfg.idmap = Some((100_000, 65_536, 100_000, 65_536));
    (cfg, dir)
}

fn spec(name: &str, node: &str, os: &str) -> GuestSpec {
    GuestSpec {
        vmid: None,
        name: name.into(),
        kind: GuestKind::Ct,
        node: node.into(),
        cores: 2,
        memory_mb: 256,
        disk_gb: 1,
        storage: host::LOCAL_STORAGE.into(),
        os: os.into(),
        bridge: "nat".into(),
        onboot: false,
        iso: None,
        hw: Default::default(),
    }
}

fn local_cluster(dir: &Path) -> Cluster {
    let node = host::local_node();
    let mut storage = host::local_storage(&node.id, dir);
    storage.total_gb = storage.total_gb.max(storage.used_gb + 100);
    Cluster::local(node, storage)
}

fn guest(dir: &Path, name: &str, os: &str) -> Guest {
    let mut c = local_cluster(dir);
    let node = c.nodes[0].id.clone();
    let id = c.create_guest(&spec(name, &node, os), "u", 0).unwrap();
    c.guest(id).unwrap().clone()
}

fn status(d: &LxcDriver, g: &Guest) -> GuestStatus {
    d.probe(g).unwrap().status
}

#[test]
fn container_lifecycle_against_fake_tools() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    let g = guest(&dir, "kutu", "alpine-3.24");
    let config = cfg.lxc_path.join("redox-100/config");

    assert_eq!(status(&d, &g), GuestStatus::Stopped, "var olmayan konteyner durmuş görünür");
    assert!(d.apply(&g, Action::Start).unwrap_err().contains("diye bir konteyner yok"));

    d.create(&g).unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("download -d alpine -r 3.24 -a amd64"), "şablon argümanları: {text}");
    assert!(text.contains("lxc.idmap = u 0 100000 65536") && text.contains("lxc.idmap = g 0 100000 65536"), "yetkisiz kimlik eşlemesi");
    assert!(text.contains("lxc.cgroup2.memory.max = 256M") && text.contains("lxc.cgroup2.cpu.max = 200000 100000"));
    assert!(text.contains("lxc.net.0.link = lxcbr0") && text.contains("lxc.net.0.hwaddr = 52:54:00:00:00:64"));
    assert!(text.contains(&format!("lxc.rootfs.path = dir:{}", cfg.lxc_path.join("redox-100/rootfs").display())), "LXC'nin yazdıkları korunur");
    assert!(std::fs::read_dir(&cfg.lxc_path).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".conf")), "geçici dosya silinir");
    assert!(d.create(&g).unwrap_err().contains("zaten var"));

    d.apply(&g, Action::Start).unwrap();
    let first = d.probe(&g).unwrap();
    assert_eq!((first.status, first.mem_used_mb, first.cpu_pct), (GuestStatus::Running, Some(50), None));
    assert_eq!(d.guest_ip(&g).as_deref(), Some("10.0.3.17"));
    assert!(d.probe(&g).unwrap().cpu_pct.is_some(), "ikinci örnekte CPU yüzdesi");

    d.apply(&g, Action::Pause).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Paused);
    d.apply(&g, Action::Resume).unwrap();
    d.apply(&g, Action::Reboot).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Running);
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join("redox-100/last-stop")).unwrap().trim(), "t=7 r=", "yeniden başlatma: süre sınırıyla durdur, sonra başlat (-r yetkisiz konteynerde çalışmaz)");

    // Ayar değişince yalnızca yönetilen blok yenilenir.
    let mut bigger = g.clone();
    bigger.mem_total_mb = 1024;
    bigger.bridge = "docker0".into();
    d.configure(&bigger).unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("memory.max = 1024M") && !text.contains("memory.max = 256M") && text.contains("lxc.net.0.link = docker0"));
    assert!(text.contains("lxc.idmap = u 0 100000 65536") && text.contains("lxc.rootfs.path"));

    d.apply(&g, Action::Stop).unwrap();
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join("redox-100/last-stop")).unwrap().trim(), "t=7 r=");
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    assert_eq!(d.guest_ip(&g), None, "duran konteynerin adresi unutulur");

    d.destroy(&g).unwrap();
    assert!(!cfg.lxc_path.join("redox-100").exists());
    d.destroy(&g).unwrap();
    d.configure(&g).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exec_captures_output_and_exit_status() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    let g = guest(&dir, "kutu", "debian-12");
    d.create(&g).unwrap();
    assert!(d.exec(&g, "echo x").unwrap().contains("konteyner çalışmıyor"), "duran konteynerde araç hatası çıktıda görünür");
    d.apply(&g, Action::Start).unwrap();

    assert_eq!(d.exec(&g, "echo merhaba; echo hata >&2").unwrap(), "merhaba\nhata\n", "stdout ve stderr birlikte");
    assert_eq!(d.exec(&g, "printf 'a b' | wc -c").unwrap().trim(), "3", "komut kabukta bütün olarak çalışır");
    let failed = d.exec(&g, "echo once; exit 3").unwrap();
    assert!(failed.starts_with("once\n") && failed.ends_with("[çıkış kodu 3]"), "{failed:?}");
    // Tırnak ve özel karakterler argüman olarak geçer; kabuk enjeksiyonu için ayrı bir katman yoktur.
    assert_eq!(d.exec(&g, "echo \"$(echo 'iç içe')\"").unwrap(), "iç içe\n");

    let big = d.exec(&g, "head -c 200000 /dev/zero | tr '\\0' 'x'").unwrap();
    assert!(big.len() < 70_000 && big.ends_with("[çıktı kesildi]"), "{}", big.len());
    // Çok çıktı üreten komut diske yazılmaz, belleğe de alınmaz: sınırın üstü okunup atılır.
    let flood = d.exec(&g, "head -c 50000000 /dev/zero").unwrap();
    assert!(flood.len() < 70_000 && flood.ends_with("[çıktı kesildi]"), "{}", flood.len());
    assert!(std::fs::read_dir(&cfg.lxc_path).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".exec")), "çıktı dosyası tutulmaz");
    assert!(d.exec(&g, "   ").unwrap_err().contains("boş"));
    assert!(d.exec(&g, &"x".repeat(3000)).unwrap_err().contains("en fazla"));
    assert!(std::fs::read_dir(&cfg.lxc_path).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".exec")), "çıktı dosyası silinir");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failures_and_unsupported_operations_are_explained() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    assert!(d.create(&guest(&dir, "kutu", "windows-11")).unwrap_err().contains("bilinmeyen şablon"));
    // Şablon tanınır ama indirme başarısız olur: aracın iletisi iletilir, kalıntı bırakılmaz.
    assert_eq!(d.create(&guest(&dir, "kutu", "ubuntu-24.04")).unwrap_err(), "lxc-create: ubuntu imajı indirilemedi");
    assert!(!cfg.lxc_path.join("redox-100/config").exists());

    let broken = guest(&dir, "bozuk", "alpine-3.24");
    d.create(&broken).unwrap();
    assert_eq!(d.apply(&broken, Action::Start).unwrap_err(), "lxc-start: konteyner başlatılamadı");
    assert_eq!(status(&d, &broken), GuestStatus::Stopped);

    for err in [d.resize_disk(&broken, 20).unwrap_err(), d.link_disk(&broken, &broken).unwrap_err()] {
        assert!(err.contains("konteynerlerde desteklenmiyor"), "{err}");
    }
    assert!(d.supports(GuestKind::Ct) && !d.supports(GuestKind::Vm));
    assert!(d.templates().contains(&"alpine-3.24".to_string()) && !d.templates().contains(&"alpine-3.20".to_string()), "kaldırılmış sürüm önerilmez");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn host_driver_routes_containers_and_vms() {
    let (cfg, dir) = setup();
    let qemu = QemuDriver::new(QemuConfig::new(dir.join("data"))).unwrap();
    let cluster = local_cluster(&dir);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let driver = HostDriver::new(qemu, Some(LxcDriver::new(cfg.clone()).unwrap()));
    assert!(driver.name().ends_with("+lxc") && driver.supports(GuestKind::Ct) && driver.supports(GuestKind::Vm));
    // Disk ekleme gibi isteğe bağlı işlemler de alttaki sürücüye iletilir (varsayılan "tamam" dönmemeli).
    let mut probe_vm = guest(&dir, "vm", "alpine-3.24");
    probe_vm.kind = GuestKind::Vm;
    assert!(driver.add_disk(&probe_vm, 1, 1).is_err(), "qemu-img yokken disk eklemek hata vermeli");
    assert!(driver.backup_file("vm-1-1").is_some() && driver.backup_file("ct-1-1").is_some());
    // Canlı disk işlemi yeteneği alttaki sürücüden gelir: QEMU'da var, LXC'de yok.
    assert!(driver.live_disk_ops(GuestKind::Vm) && !driver.live_disk_ops(GuestKind::Ct));
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, auth);

    assert!(matches!(state.create_guest(&spec("kutu", &node, "windows-11"), "admin"), Err(Error::InvalidSpec(m)) if m.contains("bilinmeyen konteyner şablonu")));
    let id = state.create_guest(&spec("kutu", &node, "alpine-3.24"), "admin").unwrap();
    assert!(cfg.lxc_path.join(format!("redox-{id}/config")).is_file());

    state.run_action(id, Action::Start, "admin").unwrap();
    state.refresh(None, None);
    let g = state.snapshot().guest(id).unwrap().clone();
    assert_eq!((g.status, g.mem_used_mb, g.ip.as_deref()), (GuestStatus::Running, 50, Some("10.0.3.17")));
    assert_eq!(state.exec(id, "echo tamam", "admin").unwrap(), "tamam\n");
    assert_eq!(state.snapshot().tasks[0].command, format!("ct exec {id}"));

    // Çalışan konteynerde anlık görüntü ve klon reddedilir; yedek alınır (konteyner o sırada dondurulur).
    assert!(matches!(state.snapshot_create(id, "s1", "admin"), Err(Error::RequiresStopped { .. })));
    assert!(matches!(state.clone_guest(id, "kopya", "admin"), Err(Error::RequiresStopped { .. })));
    let live = state.backup_create(id, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(cfg.backups_dir.join(format!("{live}.tar.gz")).is_file());
    state.refresh(None, None);
    assert_eq!(state.snapshot().guest(id).unwrap().status, GuestStatus::Running, "yedekten sonra çalışmayı sürdürür");
    state.backup_delete(&live, "admin").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    state.run_action(id, Action::Stop, "admin").unwrap();

    // Kapalı konteyner klonlanır: kopya kendi kimliğiyle ayrı bir konteynerdir.
    let copy = state.clone_guest(id, "kopya", "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(cfg.lxc_path.join(format!("redox-{copy}/config")).is_file());
    assert_eq!(state.snapshot().guest(copy).unwrap().kind, GuestKind::Ct);
    state.delete_guest(copy, "admin").unwrap();
    assert!(!cfg.lxc_path.join(format!("redox-{copy}")).exists());

    state.snapshot_create(id, "s1", "admin").unwrap();
    let backup = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    assert!(c.tasks.iter().take(2).all(|t| t.status == TaskStatus::Ok), "{:?}", &c.tasks[..2]);
    assert_eq!((c.guest(id).unwrap().snapshots.len(), c.backups[0].id.as_str()), (1, backup.as_str()));
    assert!(cfg.backups_dir.join(format!("{backup}.tar.gz")).is_file());
    state.backup_delete(&backup, "admin").unwrap();
    assert!(!cfg.backups_dir.join(format!("{backup}.tar.gz")).exists(), "birleşik sürücü konteyner yedeğini de siler");

    // Desteklenmeyen disk işlemi görev olarak başlar ve açıklamalı hatayla biter.
    state.resize_disk(id, 50, "admin").unwrap();
    let c = state.snapshot();
    assert_eq!(c.tasks[0].status, TaskStatus::Failed);
    assert!(c.tasks[0].message.as_deref().unwrap().contains("konteynerlerde desteklenmiyor"));
    assert_eq!(c.guest(id).unwrap().disk_gb, 1);

    // Bellek değişikliği LXC yapılandırmasına yazılır.
    state.update_guest(id, &GuestPatch { memory_mb: Some(512), ..Default::default() }, "admin").unwrap();
    assert!(std::fs::read_to_string(cfg.lxc_path.join(format!("redox-{id}/config"))).unwrap().contains("memory.max = 512M"));

    state.delete_guest(id, "admin").unwrap();
    assert!(!cfg.lxc_path.join(format!("redox-{id}")).exists() && state.snapshot().guests.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn without_lxc_only_vms_are_offered() {
    let (_, dir) = setup();
    let driver = HostDriver::new(QemuDriver::new(QemuConfig::new(dir.join("data"))).unwrap(), None);
    assert!(!driver.supports(GuestKind::Ct) && driver.supports(GuestKind::Vm) && driver.templates().is_empty());
    let ct = guest(&dir, "kutu", "alpine-3.24");
    assert_eq!(driver.apply(&ct, Action::Start).unwrap_err(), "bu sunucuda LXC kurulu değil");
    assert!(driver.probe(&ct).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn container_snapshots_map_our_names_to_lxc_names() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    let g = guest(&dir, "kutu", "alpine-3.24");
    d.create(&g).unwrap();
    let rootfs = cfg.lxc_path.join("redox-100/rootfs");
    let file = rootfs.join("veri.txt");

    std::fs::write(&file, "ilk").unwrap();
    d.snapshot(&g, SnapshotOp::Create("ilk-hali")).unwrap();
    std::fs::write(&file, "ikinci").unwrap();
    d.snapshot(&g, SnapshotOp::Create("ikinci-hali")).unwrap();
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join(".redox-100.snapshots")).unwrap(), "ilk-hali snap0\nikinci-hali snap1\n");
    assert!(d.snapshot(&g, SnapshotOp::Create("ilk-hali")).unwrap_err().contains("zaten var"));

    std::fs::write(&file, "bozuldu").unwrap();
    d.snapshot(&g, SnapshotOp::Rollback("ilk-hali")).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ilk", "kök dosya sistemi o ana döner");
    d.snapshot(&g, SnapshotOp::Rollback("ikinci-hali")).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ikinci");

    d.snapshot(&g, SnapshotOp::Delete("ilk-hali")).unwrap();
    assert!(!cfg.lxc_path.join("redox-100/snaps/snap0").exists());
    assert!(d.snapshot(&g, SnapshotOp::Rollback("ilk-hali")).unwrap_err().contains("bulunamadı"));
    assert!(d.snapshot(&g, SnapshotOp::Delete("yok")).unwrap_err().contains("bulunamadı"));
    assert!(d.snapshot(&g, SnapshotOp::Create("../x")).unwrap_err().contains("geçersiz"));
    // Silinen adın LXC numarası yeniden kullanılsa da eşleme doğru kalır.
    d.snapshot(&g, SnapshotOp::Create("ucuncu")).unwrap();
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join(".redox-100.snapshots")).unwrap(), "ikinci-hali snap1\nucuncu snap0\n");

    // Çalışırken araç reddeder; ileti aktarılır ve eşleme değişmez.
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(d.snapshot(&g, SnapshotOp::Create("canli")).unwrap_err(), "lxc-snapshot: konteyner çalışırken görüntü alınamaz");
    assert!(!std::fs::read_to_string(cfg.lxc_path.join(".redox-100.snapshots")).unwrap().contains("canli"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn container_backup_and_restore_round_trip() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    let g = guest(&dir, "kutu", "alpine-3.24");
    d.create(&g).unwrap();
    let rootfs = cfg.lxc_path.join("redox-100/rootfs");
    std::fs::create_dir_all(rootfs.join("etc")).unwrap();
    std::fs::write(rootfs.join("etc/hostname"), "kutu\n").unwrap();
    std::fs::write(rootfs.join(".gizli"), "x").unwrap();

    assert_eq!(d.backup(&g, "ct-100-1").unwrap().size_mb, 1, "boyut MB'a yukarı yuvarlanır");
    let archive = cfg.backups_dir.join("ct-100-1.tar.gz");
    assert!(archive.is_file());
    // Yetkisiz kurulumda arşivleme konteynerin kimlik eşlemesiyle yapılır.
    let maps = std::fs::read_to_string(cfg.bin_dir.as_ref().unwrap().join("last-userns")).unwrap();
    assert_eq!(maps.trim(), "u:0:100000:65536 g:0:100000:65536");
    assert!(d.backup(&g, "ct-100-1").unwrap_err().contains("zaten var"));
    for bad in ["../x", "a/b", ""] {
        assert!(d.backup(&g, bad).unwrap_err().contains("geçersiz yedek kimliği"), "{bad}");
    }

    // Geri yükleme: sonradan eklenen silinir, değişen eski haline döner, gizli dosyalar dahil.
    std::fs::write(rootfs.join("etc/hostname"), "bozuk\n").unwrap();
    std::fs::write(rootfs.join("sonradan.txt"), "silinmeli").unwrap();
    std::fs::remove_file(rootfs.join(".gizli")).unwrap();
    let config_before = std::fs::read_to_string(cfg.lxc_path.join("redox-100/config")).unwrap();
    d.restore(&g, "ct-100-1", &[]).unwrap();
    assert_eq!(std::fs::read_to_string(rootfs.join("etc/hostname")).unwrap(), "kutu\n");
    assert!(!rootfs.join("sonradan.txt").exists() && rootfs.join(".gizli").is_file());
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join("redox-100/config")).unwrap(), config_before, "yapılandırma değişmez");

    assert!(d.restore(&g, "ct-100-yok", &[]).unwrap_err().contains("bulunamadı"));
    d.delete_backup("ct-100-1").unwrap();
    assert!(!archive.exists());
    d.delete_backup("ct-100-1").unwrap();

    // Kök kurulumda (kimlik eşlemesi yok) komutlar doğrudan çalışır.
    let mut root_cfg = cfg.clone();
    root_cfg.idmap = None;
    let as_root = LxcDriver::new(root_cfg).unwrap();
    std::fs::remove_file(cfg.bin_dir.as_ref().unwrap().join("last-userns")).unwrap();
    assert_eq!(as_root.backup(&g, "ct-100-2").unwrap().size_mb, 1);
    assert!(!cfg.bin_dir.as_ref().unwrap().join("last-userns").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

/// Sahte nft: standart girdiyi dosyaya yazar; `fail` dosyası varsa hata verir.
const NFT_TOOL: &str = r#"#!/bin/sh
D=$(dirname "$0")
cat > "$D/nft-last"
[ -e "$D/nft-fail" ] && { echo "Error: Operation not permitted" >&2; exit 1; }
exit 0
"#;

#[test]
fn firewall_rules_follow_guest_changes() {
    let (cfg, dir) = setup();
    let bin = cfg.bin_dir.clone().unwrap();
    let nft = bin.join("nft");
    write_script(&nft, NFT_TOOL);
    let last = || std::fs::read_to_string(bin.join("nft-last")).unwrap_or_default();

    let qemu = QemuDriver::new(QemuConfig::new(dir.join("data"))).unwrap();
    let cluster = local_cluster(&dir);
    let node = cluster.nodes[0].id.clone();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);

    // --firewall verilmeden: gerçek sürücüde güvenlik duvarı açılamaz.
    let plain = HostDriver::new(QemuDriver::new(QemuConfig::new(dir.join("data2"))).unwrap(), Some(LxcDriver::new(cfg.clone()).unwrap()));
    let off = AppState::new(local_cluster(&dir), Box::new(plain), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id0 = off.create_guest(&spec("kutu", &node, "alpine-3.24"), "admin").unwrap();
    let enable = redox_core::GuestPatch { firewall: Some(true), ..Default::default() };
    assert!(matches!(off.update_guest(id0, &enable, "admin"), Err(Error::InvalidSpec(m)) if m.contains("--firewall")));
    off.delete_guest(id0, "admin").unwrap();

    let driver = HostDriver::new(qemu, Some(LxcDriver::new(cfg.clone()).unwrap())).with_firewall(nft.display().to_string());
    assert!(driver.firewall());
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, auth);
    state.sync_firewall().unwrap();
    assert!(last().contains("table bridge redox {") && !last().contains("jump"), "başlangıçta boş tablo yüklenir");

    let id = state.create_guest(&spec("kutu", &node, "alpine-3.24"), "admin").unwrap();
    let rules = redox_core::hw::parse_fw_rules("tcp/22 from 10.0.0.0/8, tcp/443").unwrap();
    state.update_guest(id, &redox_core::GuestPatch { firewall: Some(true), fw_rules: Some(rules), ..Default::default() }, "admin").unwrap();
    let script = last();
    let mac = redox_core::mac_for(id);
    assert!(script.contains(&format!("ether daddr {mac} jump g{id}")), "{script}");
    assert!(script.contains("ip saddr 10.0.0.0/8 tcp dport 22 accept") && script.contains("tcp dport 443 accept") && script.contains("drop"));

    // nft başarısız olursa kullanıcı hemen öğrenir ve günlüğe yazılır.
    std::fs::write(bin.join("nft-fail"), "").unwrap();
    let err = state.update_guest(id, &redox_core::GuestPatch { fw_rules: Some(Vec::new()), ..Default::default() }, "admin").unwrap_err();
    assert!(matches!(&err, Error::Driver(m) if m.contains("güvenlik duvarı kuralları yüklenemedi: Error: Operation not permitted")), "{err}");
    let c = state.snapshot();
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].status), ("firewall apply", TaskStatus::Failed));
    std::fs::remove_file(bin.join("nft-fail")).unwrap();

    // Silinen konuğun kuralları kalkar.
    state.delete_guest(id, "admin").unwrap();
    assert!(!last().contains("jump"), "{}", last());
    std::fs::remove_dir_all(dir).unwrap();
}

/// Üretilen betiği gerçek çekirdeğe yükler: ayrı bir kullanıcı ve ağ ad alanında, kök yetkisi olmadan.
#[test]
fn generated_ruleset_loads_into_a_real_kernel() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let usable = Command::new("unshare").args(["-Urn", "nft", "list", "ruleset"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !usable {
        eprintln!("ATLANDI: unshare/nft kullanılamıyor; betik gerçek çekirdekte denenemedi");
        return;
    }
    let (_, dir) = setup();
    let mut vm = guest(&dir, "web", "alpine-3.24");
    vm.kind = GuestKind::Vm;
    vm.bridge = "vmbr0".into();
    vm.hw.firewall = true;
    vm.hw.fw_rules = redox_core::hw::parse_fw_rules("tcp/22 from 10.0.0.0/8, tcp/80-443, udp/53 from 192.168.1.10, udp, tcp, icmp").unwrap();
    let mut ct = guest(&dir, "kutu", "alpine-3.24");
    ct.vmid = 201;
    ct.hw.firewall = true;
    let script = redox_server::firewall::ruleset(&[vm.clone(), ct]);

    // Aynı betik iki kez yüklenir: ikinci yükleme var olan tabloyu atomik olarak değiştirmelidir.
    let mut child = Command::new("unshare")
        .args(["-Urn", "sh", "-c", "cat > /tmp/redox-nft-$$ && nft -f /tmp/redox-nft-$$ && nft -f /tmp/redox-nft-$$ && nft list table bridge redox; rc=$?; rm -f /tmp/redox-nft-$$; exit $rc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let (listed, errors) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!errors.contains("syntax error"), "nft betiğin sözdizimini reddetti:\n{errors}\n--- betik ---\n{script}");
    if errors.contains("Operation not supported") {
        // Çekirdekte bridge ailesi yok (örn. WSL): ayrıştırıcı betiği kabul etti ama yüklenemedi.
        eprintln!("ATLANDI: çekirdek nftables bridge ailesini desteklemiyor; yalnızca sözdizimi doğrulandı");
        std::fs::remove_dir_all(dir).unwrap();
        return;
    }
    assert!(out.status.success(), "nft betiği reddetti:\n{errors}\n--- betik ---\n{script}");
    assert!(listed.contains(&format!("ether daddr {} jump g{}", redox_core::mac_for(vm.vmid), vm.vmid)), "{listed}");
    assert!(listed.contains("ip saddr 10.0.0.0/8 tcp dport 22 accept") && listed.contains("tcp dport 80-443 accept"), "{listed}");
    assert_eq!(listed.matches("drop").count(), 2, "her konuğun zinciri düşürmeyle biter:\n{listed}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn containers_are_cloned_with_their_own_identity() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    let source = guest(&dir, "kutu", "alpine-3.24");
    d.create(&source).unwrap();
    let rootfs = cfg.lxc_path.join("redox-100/rootfs");
    std::fs::create_dir_all(rootfs.join("etc")).unwrap();
    std::fs::write(rootfs.join("etc/veri"), "kaynak").unwrap();
    d.snapshot(&source, SnapshotOp::Create("once")).unwrap();

    let mut target = source.clone();
    (target.vmid, target.name, target.cores) = (101, "kutu-kopya".into(), 1);
    d.clone_disk(&source, &target).unwrap();
    let copy = cfg.lxc_path.join("redox-101");
    assert_eq!(std::fs::read_to_string(copy.join("rootfs/etc/veri")).unwrap(), "kaynak", "kök dosya sistemi kopyalanır");
    let config = std::fs::read_to_string(copy.join("config")).unwrap();
    assert!(config.contains("lxc.uts.name = kutu-kopya") && !config.contains("lxc.uts.name = kutu\n"), "kopya kendi adını alır: {config}");
    assert!(config.contains(&format!("lxc.net.0.hwaddr = {}", redox_core::mac_for(101))) && !config.contains(&redox_core::mac_for(100)), "kopya kendi ağ adresini alır");
    assert!(config.contains("lxc.cgroup2.cpu.max = 100000 100000"), "sınırlar hedef konuğun kaydına göre yazılır");
    assert!(!cfg.lxc_path.join(".redox-101.snapshots").exists() && !copy.join("redox-snapshots").exists(), "anlık görüntü eşlemesi kopyalanmaz");
    // Kaynak değişmez; kopya bağımsızdır.
    std::fs::write(copy.join("rootfs/etc/veri"), "kopya").unwrap();
    assert_eq!(std::fs::read_to_string(rootfs.join("etc/veri")).unwrap(), "kaynak");
    assert!(std::fs::read_to_string(cfg.lxc_path.join("redox-100/config")).unwrap().contains("lxc.uts.name = kutu\n"));
    d.apply(&target, Action::Start).unwrap();
    assert_eq!(status(&d, &target), GuestStatus::Running);

    // Var olan hedefin üstüne yazılmaz; çalışan kaynak kopyalanmaz; araç hatası olduğu gibi aktarılır.
    assert!(d.clone_disk(&source, &target).unwrap_err().contains("zaten var"));
    let mut third = source.clone();
    (third.vmid, third.name) = (102, "ucuncu".into());
    let fail = cfg.bin_dir.as_ref().unwrap().join("userns-fail");
    std::fs::write(&fail, "").unwrap();
    assert_eq!(d.clone_disk(&source, &third).unwrap_err(), "kök dosya sistemi kopyalanamadı: cp: diskte yer kalmadı");
    assert!(!cfg.lxc_path.join("redox-102").exists(), "yarım kopya bırakılmaz");
    std::fs::remove_file(&fail).unwrap();
    d.apply(&source, Action::Start).unwrap();
    assert_eq!(d.clone_disk(&source, &third).unwrap_err(), "konteyner çalışırken kopyalanamaz");
    assert!(!cfg.lxc_path.join("redox-102").exists());
    // Yetkili (kimlik eşlemesiz) kurulumda LXC'nin kendi aracı kullanılır.
    d.apply(&source, Action::Stop).unwrap();
    let mut privileged = cfg.clone();
    privileged.idmap = None;
    LxcDriver::new(privileged).unwrap().clone_disk(&source, &third).unwrap();
    assert!(cfg.lxc_path.join("redox-102/rootfs/etc/veri").is_file());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn running_containers_are_frozen_for_the_backup_and_thawed_after() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    assert!(d.live_backup(GuestKind::Ct) && !d.live_disk_ops(GuestKind::Ct), "yedek çalışırken alınır, anlık görüntü alınmaz");
    let g = guest(&dir, "kutu", "alpine-3.24");
    d.create(&g).unwrap();
    let home = cfg.lxc_path.join("redox-100");
    std::fs::create_dir_all(home.join("rootfs/etc")).unwrap();
    std::fs::write(home.join("rootfs/etc/veri"), "canli").unwrap();
    let bin = cfg.bin_dir.clone().unwrap();
    // Sahte dondurma ve çözme araçları çağrıldıklarını kaydeder; tar sırasında durum FROZEN olmalıdır.
    let log = dir.join("freeze-log");
    for (tool, state) in [("lxc-freeze", "FROZEN"), ("lxc-unfreeze", "RUNNING")] {
        write_script(&bin.join(tool), &format!("#!/bin/sh\n{HEAD}need\n[ -e \"$P/{tool}-fail\" ] && {{ echo \"{tool}: cgroup yazılamadı\" >&2; exit 1; }}\necho {state} > \"$D/state\"\necho {tool} >> '{}'\n", log.display()));
    }
    d.apply(&g, Action::Start).unwrap();
    d.backup(&g, "ct-100-1").unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "lxc-freeze\nlxc-unfreeze\n");
    assert_eq!(status(&d, &g), GuestStatus::Running, "yedekten sonra konteyner çalışmayı sürdürür");
    assert!(cfg.backups_dir.join("ct-100-1.tar.gz").is_file());

    // Kapalı ve zaten duraklatılmış konteynere dokunulmaz.
    std::fs::remove_file(&log).unwrap();
    d.apply(&g, Action::Pause).unwrap();
    std::fs::remove_file(&log).unwrap();
    d.backup(&g, "ct-100-2").unwrap();
    assert!(!log.exists() && status(&d, &g) == GuestStatus::Paused, "duraklatılmış konteyner duraklatılmış kalır");
    d.apply(&g, Action::Resume).unwrap();
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_file(&log);
    d.backup(&g, "ct-100-3").unwrap();
    assert!(!log.exists());

    // Dondurulamazsa yedek alınmaz; çözülemezse yedek bırakılmaz ve ne yapılacağı söylenir.
    d.apply(&g, Action::Start).unwrap();
    std::fs::write(cfg.lxc_path.join("lxc-freeze-fail"), "").unwrap();
    assert!(d.backup(&g, "ct-100-4").unwrap_err().contains("dondurulamadı"));
    assert!(!cfg.backups_dir.join("ct-100-4.tar.gz").exists());
    std::fs::remove_file(cfg.lxc_path.join("lxc-freeze-fail")).unwrap();
    std::fs::write(cfg.lxc_path.join("lxc-unfreeze-fail"), "").unwrap();
    let err = d.backup(&g, "ct-100-5").unwrap_err();
    assert!(err.contains("çözülemedi") && err.contains("lxc-unfreeze -n redox-100"), "{err}");
    assert!(!cfg.backups_dir.join("ct-100-5.tar.gz").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn containers_are_copied_and_replicated_between_servers() {
    // Eş sunucu: sahte LXC araçları, gerçek TCP kapısı.
    let (peer_cfg, peer_dir) = setup();
    let peer_cluster = local_cluster(&peer_dir);
    let node = peer_cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "t", "es-sunucu-yonetici-tokeni").unwrap();
    let peer_driver = HostDriver::new(QemuDriver::new(QemuConfig::new(peer_dir.join("data"))).unwrap(), Some(LxcDriver::new(peer_cfg.clone()).unwrap()));
    let peer = AppState::new(peer_cluster, Box::new(peer_driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let source = peer.create_guest(&spec("web", &node, "alpine-3.24"), "admin").unwrap();
    let peer_rootfs = peer_cfg.lxc_path.join(format!("redox-{source}/rootfs"));
    std::fs::create_dir_all(peer_rootfs.join("etc")).unwrap();
    std::fs::write(peer_rootfs.join("etc/hostname"), "birinci\n").unwrap();
    peer.update_guest(source, &GuestPatch { tags: Some(vec!["web".into()]), ..Default::default() }, "admin").unwrap();
    peer.run_action(source, Action::Start, "admin").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer_app = redox_server::app(peer.clone());
    tokio::spawn(async move { axum::serve(listener, peer_app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });

    // Bu sunucu.
    let (cfg, dir) = setup();
    let driver = HostDriver::new(QemuDriver::new(QemuConfig::new(dir.join("data"))).unwrap(), Some(LxcDriver::new(cfg.clone()).unwrap()));
    let hub = AppState::new(local_cluster(&dir), Box::new(driver), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    hub.peers().configure(redox_server::peers::parse_file(&format!("ankara http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\n")).unwrap());
    let hostname = |cfg: &LxcConfig, id: u32| std::fs::read_to_string(cfg.lxc_path.join(format!("redox-{id}/rootfs/etc/hostname"))).unwrap_or_default();

    // Kopyalama: kaynak çalışmaya devam eder, kopya kapalı gelir ve içeriği aynıdır.
    {
        let hub = hub.clone();
        tokio::task::spawn_blocking(move || hub.import_from_peer("ankara", source, "admin")).await.unwrap().unwrap();
    }
    let c = hub.snapshot();
    let task = c.tasks[0].clone();
    assert_eq!((task.command.as_str(), task.status), (format!("vm import ankara/{source}").as_str(), TaskStatus::Ok), "{:?}", task.message);
    let copy = c.guests[0].clone();
    assert_eq!((copy.kind, copy.name.as_str(), copy.status, copy.tags.clone()), (GuestKind::Ct, "web", GuestStatus::Stopped, vec!["web".to_string(), "tasindi-ankara".to_string()]));
    assert_eq!(hostname(&cfg, copy.vmid), "birinci\n");
    assert_eq!(peer.snapshot().guest(source).unwrap().status, GuestStatus::Running, "kaynak çalışmaya devam eder");
    assert!(peer.snapshot().backups.is_empty() && std::fs::read_dir(&cfg.backups_dir).map(|d| d.count()).unwrap_or(0) == 0, "geçici yedek iki tarafta da kalmaz");

    // Çoğaltma: yedek kopya oluşur; kaynak değişince tazelenir ve eski kopya silinir.
    hub.replica_put("ankara", source, 60, "admin").unwrap();
    {
        let hub = hub.clone();
        assert_eq!(tokio::task::spawn_blocking(move || hub.replica_tick()).await.unwrap(), 1);
    }
    let first = hub.snapshot().replicas[0].standby.expect("yedek kopya");
    assert_eq!(hub.snapshot().guest(first).unwrap().name, "web-kopya");
    std::fs::write(peer_rootfs.join("etc/hostname"), "ikinci\n").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    {
        let hub = hub.clone();
        tokio::task::spawn_blocking(move || hub.replica_run("ankara", source)).await.unwrap().unwrap();
    }
    let c = hub.snapshot();
    let second = c.replicas[0].standby.unwrap();
    assert!(c.guest(first).is_err() && second != first, "eski kopya silinir");
    assert_eq!(hostname(&cfg, second), "ikinci\n");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(peer_dir);
}

#[test]
fn container_backups_are_encrypted_on_disk_when_a_passphrase_is_set() {
    let (mut cfg, dir) = setup();
    cfg.backup_passphrase = Some("dogru-at-pil-zimba".into());
    let d = LxcDriver::new(cfg.clone()).unwrap();
    assert!(d.backups_sealed());
    let g = guest(&dir, "kutu", "alpine-3.24");
    d.create(&g).unwrap();
    let rootfs = cfg.lxc_path.join("redox-100/rootfs");
    std::fs::create_dir_all(rootfs.join("etc")).unwrap();
    std::fs::write(rootfs.join("etc/hostname"), "gizli-kutu\n").unwrap();
    d.backup(&g, "ct-100-1").unwrap();
    let archive = cfg.backups_dir.join("ct-100-1.tar.gz");
    let sealed = std::fs::read(&archive).unwrap();
    assert!(sealed.starts_with(b"RDXENC2\n"), "arşiv diske şifreli yazılır");
    assert!(d.verify_backup("ct-100-1", &[]).is_ok());
    std::fs::write(rootfs.join("etc/hostname"), "bozuk\n").unwrap();
    d.restore(&g, "ct-100-1", &[]).unwrap();
    assert_eq!(std::fs::read_to_string(rootfs.join("etc/hostname")).unwrap(), "gizli-kutu\n");

    // İndirme için açık kopya geçici bir dizine çözülür; gerçek bir gzip arşividir.
    let (plain, temporary) = d.export_backup("ct-100-1", 0).unwrap().unwrap();
    assert!(temporary && plain != archive && std::fs::read(&plain).unwrap().starts_with(&[0x1f, 0x8b]));
    std::fs::remove_dir_all(plain.parent().unwrap()).unwrap();

    // Başka bir yedeğin adına konan ya da değiştirilen arşiv açılmaz; parolasız sürücü nedenini söyler.
    std::fs::copy(&archive, cfg.backups_dir.join("ct-100-2.tar.gz")).unwrap();
    assert!(d.restore(&g, "ct-100-2", &[]).unwrap_err().contains("çözülemedi"));
    let mut tampered = sealed.clone();
    *tampered.last_mut().unwrap() ^= 1;
    std::fs::write(&archive, &tampered).unwrap();
    assert!(d.verify_backup("ct-100-1", &[]).is_err());
    std::fs::write(&archive, &sealed).unwrap();
    cfg.backup_passphrase = None;
    let open = LxcDriver::new(cfg.clone()).unwrap();
    assert!(open.restore(&g, "ct-100-1", &[]).unwrap_err().contains("--backup-passphrase-file"));
    // Parolasız alınmış eski arşivler şifreli kurulumda da okunur.
    open.backup(&g, "ct-100-3").unwrap();
    assert!(d.verify_backup("ct-100-3", &[]).is_ok() && !d.export_backup("ct-100-3", 0).unwrap().unwrap().1);
    let _ = std::fs::remove_dir_all(dir);
}

/// Eşteki konteyner yedeği üzerinden bu sunucuya kopyalanır: iki gerçek sunucu, aralarında HTTP.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_container_is_copied_from_a_peer_server_with_its_files() {
    let server = |name: &str| {
        let (cfg, dir) = setup();
        let qemu = QemuDriver::new(QemuConfig::new(dir.join("data"))).unwrap();
        let cluster = local_cluster(&dir);
        let node = cluster.nodes[0].id.clone();
        let mut users = UserFile::default();
        users.add_user("admin", "es-yonetici-parolasi", redox_server::auth::Role::Admin, Cost::Test).unwrap();
        users.add_token("admin", name, "es-sunucu-yonetici-tokeni").unwrap();
        let driver = HostDriver::new(qemu, Some(LxcDriver::new(cfg.clone()).unwrap()));
        let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
        (state, cfg, dir, node)
    };
    let (source, source_cfg, source_dir, source_node) = server("kaynak");
    let (target, target_cfg, target_dir, _) = server("hedef");
    let id = source.create_guest(&spec("tasinan", &source_node, "alpine-3.24"), "admin").unwrap();
    let rootfs = source_cfg.lxc_path.join(format!("redox-{id}/rootfs"));
    std::fs::create_dir_all(rootfs.join("srv/veri")).unwrap();
    std::fs::write(rootfs.join("srv/veri/kayit"), "kaynaktaki-veri").unwrap();
    std::os::unix::fs::symlink("veri/kayit", rootfs.join("srv/kisayol")).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = redox_server::app(source.clone());
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    target.peers().configure(redox_server::peers::parse_file(&format!("kaynak http://127.0.0.1:{port} es-sunucu-yonetici-tokeni\n")).unwrap());

    let worker = target.clone();
    tokio::task::spawn_blocking(move || worker.import_from_peer("kaynak", id, "admin")).await.unwrap().unwrap();
    let task = target.snapshot().tasks.iter().find(|t| t.command.starts_with("vm import")).cloned().expect("içe aktarma görevi");
    assert_eq!(task.status, TaskStatus::Ok, "{:?}", task.message);
    let copy = target.snapshot().guests.iter().find(|g| g.name == "tasinan").cloned().expect("kopya oluşmalı");
    assert_eq!((copy.kind, copy.status), (GuestKind::Ct, GuestStatus::Stopped), "kopya kapalı gelir");
    assert!(copy.tags.iter().any(|t| t == "tasindi-kaynak"), "{:?}", copy.tags);
    let arrived = target_cfg.lxc_path.join(format!("redox-{}/rootfs", copy.vmid));
    assert_eq!(std::fs::read_to_string(arrived.join("srv/veri/kayit")).unwrap(), "kaynaktaki-veri");
    assert_eq!(std::fs::read_link(arrived.join("srv/kisayol")).unwrap(), PathBuf::from("veri/kayit"));
    // Kaynak yerinde durur; geçici yedekler iki tarafta da silinir.
    assert!(source.snapshot().guest(id).is_ok() && source.snapshot().backups.is_empty());
    for cfg in [&source_cfg, &target_cfg] {
        assert!(std::fs::read_dir(&cfg.backups_dir).map_or(true, |mut d| d.next().is_none()), "{}", cfg.backups_dir.display());
    }
    for dir in [source_dir, target_dir] {
        std::fs::remove_dir_all(dir).unwrap();
    }
}
