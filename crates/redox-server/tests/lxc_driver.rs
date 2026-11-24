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

/// lxc-usernsexec: eşleme argümanlarını kaydeder ve komutu doğrudan çalıştırır.
const USERNS_TOOL: &str = r#"#!/bin/sh
MAPS=
while [ "$1" != "--" ]; do [ "$1" = "-m" ] || MAPS="$MAPS $1"; shift; done
shift
echo "$MAPS" > "$(dirname "$0")/last-userns"
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
    for (name, script) in [("lxc-snapshot", SNAPSHOT_TOOL), ("lxc-usernsexec", USERNS_TOOL)] {
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
    let g = guest(&dir, "kutu", "alpine-3.20");
    let config = cfg.lxc_path.join("redox-100/config");

    assert_eq!(status(&d, &g), GuestStatus::Stopped, "var olmayan konteyner durmuş görünür");
    assert!(d.apply(&g, Action::Start).unwrap_err().contains("diye bir konteyner yok"));

    d.create(&g).unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("download -d alpine -r 3.20 -a amd64"), "şablon argümanları: {text}");
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
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join("redox-100/last-stop")).unwrap().trim(), "t=7 r=1", "yeniden başlatma -r ve süre sınırıyla");

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

    let broken = guest(&dir, "bozuk", "alpine-3.20");
    d.create(&broken).unwrap();
    assert_eq!(d.apply(&broken, Action::Start).unwrap_err(), "lxc-start: konteyner başlatılamadı");
    assert_eq!(status(&d, &broken), GuestStatus::Stopped);

    for err in [d.resize_disk(&broken, 20).unwrap_err(), d.clone_disk(&broken, &broken).unwrap_err()] {
        assert!(err.contains("konteynerlerde desteklenmiyor"), "{err}");
    }
    assert!(d.supports(GuestKind::Ct) && !d.supports(GuestKind::Vm));
    assert!(d.templates().contains(&"alpine-3.20".to_string()));
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
    let mut probe_vm = guest(&dir, "vm", "alpine-3.20");
    probe_vm.kind = GuestKind::Vm;
    assert!(driver.add_disk(&probe_vm, 1, 1).is_err(), "qemu-img yokken disk eklemek hata vermeli");
    assert!(driver.backup_file("vm-1-1").is_some() && driver.backup_file("ct-1-1").is_some());
    // Canlı disk işlemi yeteneği alttaki sürücüden gelir: QEMU'da var, LXC'de yok.
    assert!(driver.live_disk_ops(GuestKind::Vm) && !driver.live_disk_ops(GuestKind::Ct));
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, auth);

    assert!(matches!(state.create_guest(&spec("kutu", &node, "windows-11"), "admin"), Err(Error::InvalidSpec(m)) if m.contains("bilinmeyen konteyner şablonu")));
    let id = state.create_guest(&spec("kutu", &node, "alpine-3.20"), "admin").unwrap();
    assert!(cfg.lxc_path.join(format!("redox-{id}/config")).is_file());

    state.run_action(id, Action::Start, "admin").unwrap();
    state.refresh(None, None);
    let g = state.snapshot().guest(id).unwrap().clone();
    assert_eq!((g.status, g.mem_used_mb, g.ip.as_deref()), (GuestStatus::Running, 50, Some("10.0.3.17")));
    assert_eq!(state.exec(id, "echo tamam", "admin").unwrap(), "tamam\n");
    assert_eq!(state.snapshot().tasks[0].command, format!("ct exec {id}"));

    // Çalışan konteynerde anlık görüntü ve yedek reddedilir (canlı işlem yalnızca sanal makinelerde).
    assert!(matches!(state.snapshot_create(id, "s1", "admin"), Err(Error::RequiresStopped { .. })));
    assert!(matches!(state.backup_create(id, "admin"), Err(Error::RequiresStopped { .. })));
    state.run_action(id, Action::Stop, "admin").unwrap();

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
    let ct = guest(&dir, "kutu", "alpine-3.20");
    assert_eq!(driver.apply(&ct, Action::Start).unwrap_err(), "bu sunucuda LXC kurulu değil");
    assert!(driver.probe(&ct).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn container_snapshots_map_our_names_to_lxc_names() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    let g = guest(&dir, "kutu", "alpine-3.20");
    d.create(&g).unwrap();
    let rootfs = cfg.lxc_path.join("redox-100/rootfs");
    let file = rootfs.join("veri.txt");

    std::fs::write(&file, "ilk").unwrap();
    d.snapshot(&g, SnapshotOp::Create("ilk-hali")).unwrap();
    std::fs::write(&file, "ikinci").unwrap();
    d.snapshot(&g, SnapshotOp::Create("ikinci-hali")).unwrap();
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join("redox-100/redox-snapshots")).unwrap(), "ilk-hali snap0\nikinci-hali snap1\n");
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
    assert_eq!(std::fs::read_to_string(cfg.lxc_path.join("redox-100/redox-snapshots")).unwrap(), "ikinci-hali snap1\nucuncu snap0\n");

    // Çalışırken araç reddeder; ileti aktarılır ve eşleme değişmez.
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(d.snapshot(&g, SnapshotOp::Create("canli")).unwrap_err(), "lxc-snapshot: konteyner çalışırken görüntü alınamaz");
    assert!(!std::fs::read_to_string(cfg.lxc_path.join("redox-100/redox-snapshots")).unwrap().contains("canli"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn container_backup_and_restore_round_trip() {
    let (cfg, dir) = setup();
    let d = LxcDriver::new(cfg.clone()).unwrap();
    let g = guest(&dir, "kutu", "alpine-3.20");
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
    let id0 = off.create_guest(&spec("kutu", &node, "alpine-3.20"), "admin").unwrap();
    let enable = redox_core::GuestPatch { firewall: Some(true), ..Default::default() };
    assert!(matches!(off.update_guest(id0, &enable, "admin"), Err(Error::InvalidSpec(m)) if m.contains("--firewall")));
    off.delete_guest(id0, "admin").unwrap();

    let driver = HostDriver::new(qemu, Some(LxcDriver::new(cfg.clone()).unwrap())).with_firewall(nft.display().to_string());
    assert!(driver.firewall());
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, auth);
    state.sync_firewall().unwrap();
    assert!(last().contains("table bridge redox {") && !last().contains("jump"), "başlangıçta boş tablo yüklenir");

    let id = state.create_guest(&spec("kutu", &node, "alpine-3.20"), "admin").unwrap();
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
    let mut vm = guest(&dir, "web", "alpine-3.20");
    vm.kind = GuestKind::Vm;
    vm.bridge = "vmbr0".into();
    vm.hw.firewall = true;
    vm.hw.fw_rules = redox_core::hw::parse_fw_rules("tcp/22 from 10.0.0.0/8, tcp/80-443, udp/53 from 192.168.1.10, udp, tcp, icmp").unwrap();
    let mut ct = guest(&dir, "kutu", "alpine-3.20");
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
