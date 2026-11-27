//! QEMU sürücüsünün Linux üzerindeki gerçek süreç ve soket davranışı.
//! Gerçek QEMU yerine `fixtures/fake-qemu.py` kullanılır (python3 gerekir).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use redox_core::{Action, Cluster, Error, Guest, GuestKind, GuestSpec, GuestStatus, Hypervisor, SnapshotOp, TaskStatus};
use redox_server::auth::{Auth, Cost, UserFile};
use redox_server::host;
use redox_server::qemu::{QemuConfig, QemuDriver};
use redox_server::AppState;

mod agent_clones;
mod backup_features;
mod incremental;
mod lifecycle;
mod live;
mod live_migrate;
mod machine_move;
mod media_ova;
mod offsite_machine;
mod peers;
mod platform;
mod real_compat;
mod real_crypt;
mod real_firewall;
mod real_ft;
mod real_mirror;
mod real_perf;
mod real_qemu;
mod real_sound;
mod suspend_hotplug_net;

#[allow(unused_imports)]
pub use agent_clones::*;
#[allow(unused_imports)]
pub use backup_features::*;
#[allow(unused_imports)]
pub use incremental::*;
#[allow(unused_imports)]
pub use lifecycle::*;
#[allow(unused_imports)]
pub use live::*;
#[allow(unused_imports)]
pub use live_migrate::*;
#[allow(unused_imports)]
pub use machine_move::*;
#[allow(unused_imports)]
pub use media_ova::*;
#[allow(unused_imports)]
pub use offsite_machine::*;
#[allow(unused_imports)]
pub use peers::*;
#[allow(unused_imports)]
pub use platform::*;
#[allow(unused_imports)]
pub use real_qemu::*;
#[allow(unused_imports)]
pub use suspend_hotplug_net::*;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Sahte qemu-img: dosyaları gerçekten oluşturur/kopyalar, anlık görüntüleri yan dosyada tutar.
///   create -f qcow2 YOL BOYUT | snapshot -c|-a|-d AD YOL | resize YOL BOYUT | convert [...] KAYNAK HEDEF | check [...] YOL
const FAKE_IMG: &str = r#"case "$1" in
  create) if [ "$4" = "-F" ]; then printf 'bagli:%s' "$7" > "$8"; else printf 'disk' > "$4"; fi ;;
  snapshot)
    case "$2" in
      -c) echo "$3" >> "$4.snaps" ;;
      -a) grep -qx "$3" "$4.snaps" || { echo "qemu-img: snapshot '$3' yok" >&2; exit 1; } ;;
      -d) grep -vx "$3" "$4.snaps" > "$4.tmp"; mv "$4.tmp" "$4.snaps" ;;
    esac ;;
  resize) echo "$3" > "$2.size" ;;
  convert) for a; do src=$dst; dst=$a; done; cp "$src" "$dst" ;;
  rebase) for a; do f=$a; done; b=$(sed 's/^bagli://' "$f"); cp "$b" "$f" ;;
  check) for a; do f=$a; done; if grep -q bozuk "$f"; then echo "qemu-img: ERROR küme bozuk" >&2; exit 2; fi ;;
  info) for a; do f=$a; done; if grep -q arkalikli "$f"; then echo '{"format": "qcow2", "backing-filename": "/etc/shadow"}'; elif grep -q '^bagli:' "$f"; then echo '{"format": "qcow2", "backing-filename": "yedek"}'; else echo '{"format": "qcow2"}'; fi ;;
  *) echo "qemu-img: bilinmeyen komut $1" >&2; exit 1 ;;
esac"#;

fn python_available() -> bool {
    let ok = std::process::Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("ATLANDI: python3 yok; sahte QEMU çalıştırılamıyor");
    }
    ok
}


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

fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    write_script(&path, &format!("#!/bin/sh\n{body}\n"));
    path.display().to_string()
}

/// Kısa yollu (soket sınırı) geçici veri dizini ve sahte ikililerle yapılandırma.
fn config() -> (QemuConfig, PathBuf) {
    let dir = PathBuf::from(format!("/tmp/rdx-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-qemu.py");
    let mut cfg = QemuConfig::new(dir.join("data"));
    cfg.qemu_bin = script(&dir, "qemu", &format!("exec python3 '{}' \"$@\"", fake.display()));
    // qemu-img create -f qcow2 YOL BOYUT  →  $4 = YOL
    cfg.qemu_img_bin = script(&dir, "qemu-img", FAKE_IMG);
    cfg.shutdown_timeout = Duration::from_secs(10);
    (cfg, dir)
}

fn spec(name: &str, node: &str) -> GuestSpec {
    GuestSpec {
        vmid: None,
        name: name.into(),
        kind: GuestKind::Vm,
        node: node.into(),
        cores: 1,
        memory_mb: 256,
        disk_gb: 1,
        storage: host::LOCAL_STORAGE.into(),
        os: "linux".into(),
        bridge: "nat".into(),
        onboot: false,
        iso: None,
        hw: Default::default(),
    }
}

fn local_cluster(cfg: &QemuConfig) -> Cluster {
    let node = host::local_node();
    let mut storage = host::local_storage(&node.id, &cfg.data_dir);
    storage.total_gb = storage.total_gb.max(storage.used_gb + 100);
    Cluster::local(node, storage)
}

fn guest(cfg: &QemuConfig, name: &str) -> Guest {
    let mut c = local_cluster(cfg);
    let node = c.nodes[0].id.clone();
    let id = c.create_guest(&spec(name, &node), "u", 0).unwrap();
    c.guest(id).unwrap().clone()
}

fn status(d: &QemuDriver, g: &Guest) -> GuestStatus {
    d.probe(g).expect("gerçek sürücü her zaman gözlem döndürür").status
}
