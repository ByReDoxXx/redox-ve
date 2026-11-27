//! Eşzamanlı disk yansıtma, sürücünün kendi koduyla **gerçek QEMU**'da (koşullar `real_qemu.rs` ile aynı).

use super::real_qemu::{real, Real};
use super::*;

fn until(seconds: u64, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "zamanında olmadı: {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn pid_in(path: std::path::PathBuf) -> String {
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).trim().to_string()
}

fn signal(name: &str, pid: &str) {
    assert!(std::process::Command::new("kill").args([name, pid]).status().unwrap().success(), "kill {name} {pid}");
}

const DISKS: &str = "depmod -a 2>/dev/null; modprobe virtio_pci 2>/dev/null; modprobe virtio_blk 2>/dev/null; mount -t devtmpfs dev /dev 2>/dev/null; sleep 1";

fn write(vm: &Real, disk: &str, sector: u64, text: &str) {
    vm.ask(&format!("echo {text} | dd of=/dev/{disk} bs=512 seek={sector} conv=notrunc 2>/dev/null; sync"));
}

fn read(vm: &Real, disk: &str, sector: u64) -> String {
    vm.ask(&format!("dd if=/dev/{disk} bs=512 skip={sector} count=1 2>/dev/null | head -n 1")).pop().unwrap_or_default()
}

/// Yansıtılan konuk, kaynak QEMU aniden ölse de kopyadan **bütün yazdıklarıyla** açılır. Donan bir kopya
/// konuğun disk yazmalarını bekletir; yansıtma durdurulunca bekleyen yazma tamamlanır ve konuk çalışmayı sürdürür.
#[test]
pub(super) fn real_qemu_mirrors_disks_synchronously_and_survives_a_frozen_copy() {
    let (Some(a), Some(b)) = (real(), real()) else { return };
    let (da, db) = (QemuDriver::new(a.cfg.clone()).unwrap(), QemuDriver::new(b.cfg.clone()).unwrap());
    let mut g = guest(&a.cfg, "gercek-ayna");
    (g.cores, g.mem_total_mb) = (1, 768);
    g.hw.disks.push(redox_core::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 });
    da.create(&g).unwrap();
    da.add_disk(&g, 1, 1).unwrap();
    assert!(da.mirror_probe(&g).is_none() && da.mirror_disk_sizes(&g).is_err(), "kapalı konuk yansıtılmaz");
    da.apply(&g, Action::Start).unwrap();
    a.boot();
    a.ask(DISKS);
    write(&a, "vda", 10, "yansitma-oncesi-1");
    write(&a, "vdb", 11, "yansitma-oncesi-2");

    let sizes = da.mirror_disk_sizes(&g).unwrap();
    assert!(sizes.len() == 2 && sizes[1] == 1 << 30, "{sizes:?}");
    assert_eq!(da.mirror_probe(&g).map(|p| (p.disks, p.jobs, p.synced)), Some((2, 0, false)));
    let base = 49300 + (std::process::id() % 20) as u16 * 3;
    let key = "7c".repeat(32);
    db.replica_serve("kaynak-1-1", &sizes, "127.0.0.1", base, &key).unwrap();
    let refused = da.mirror_start(&g, "127.0.0.1", base, &"8d".repeat(32)).unwrap_err();
    assert!(refused.contains("bağlanılamadı"), "yanlış anahtar: {refused}");
    assert_eq!(da.mirror_probe(&g).unwrap().jobs, 0, "başarısız denemeden iz kalmaz");

    da.mirror_start(&g, "127.0.0.1", base, &key).unwrap();
    until(120, "ilk eşitleme", || da.mirror_probe(&g).is_some_and(|p| p.synced));
    write(&a, "vda", 2000, "yansitma-sonrasi-1");
    write(&a, "vdb", 3000, "yansitma-sonrasi-2");
    let probe = da.mirror_probe(&g).unwrap();
    assert!(probe.synced && probe.done == probe.total && probe.total > 0, "{probe:?}");
    // Yansıtma açıkken anlık görüntü ve yedek: sürücü yansıtmayı duraklatır ve işlem başarılı olur.
    da.snapshot(&g, redox_core::SnapshotOp::Create("aynali")).unwrap();
    assert_eq!(da.mirror_probe(&g).unwrap().jobs, 0, "anlık görüntü yansıtmayı duraklatır");
    let resync = |generation: &str| {
        db.replica_delete("kaynak-1-1").unwrap();
        db.replica_serve(generation, &sizes, "127.0.0.1", base, &key).unwrap();
        da.mirror_start(&g, "127.0.0.1", base, &key).unwrap();
        until(120, "yeniden eşitleme", || da.mirror_probe(&g).is_some_and(|p| p.synced));
    };
    resync("kaynak-1-1");
    da.backup(&g, "vm-aynali-1").unwrap();
    assert_eq!(da.mirror_probe(&g).unwrap().jobs, 0, "yedek yansıtmayı duraklatır");
    resync("kaynak-1-1");

    // Kaynak aniden ölür: QEMU öldürülür, hiçbir şey düzgün kapatılmaz.
    signal("-9", &pid_in(a.cfg.pid_path(g.vmid)));
    until(10, "kaynağın ölmesi", || da.mirror_probe(&g).is_none());
    // Devralma: hedefte aynı donanımla konuk oluşturulur, disklerinin yerine kopya konur.
    db.create(&g).unwrap();
    db.add_disk(&g, 1, 1).unwrap();
    db.replica_adopt("kaynak-1-1", &g).unwrap();
    assert!(!b.cfg.data_dir.join("mirrors/kaynak-1-1").exists(), "sahiplenilen kopyanın dizini kalkar");
    db.apply(&g, Action::Start).unwrap();
    b.boot();
    b.ask(DISKS);
    for (disk, sector, text) in [("vda", 10, "yansitma-oncesi-1"), ("vdb", 11, "yansitma-oncesi-2"), ("vda", 2000, "yansitma-sonrasi-1"), ("vdb", 3000, "yansitma-sonrasi-2")] {
        assert_eq!(read(&b, disk, sector), text, "{disk} {sector}");
    }

    // Ters yön ve donan kopya: tutucu süreç durdurulur (fişi çekilmiş sunucu gibi, bağlantı kapanmaz).
    let key = "9e".repeat(32);
    da.replica_serve("hedef-1-1", &sizes, "127.0.0.1", base + 1, &key).unwrap();
    db.mirror_start(&g, "127.0.0.1", base + 1, &key).unwrap();
    until(120, "ters yönde eşitleme", || db.mirror_probe(&g).is_some_and(|p| p.synced));
    let holder = pid_in(a.cfg.run_dir().join("k-hedef-1-1.pid"));
    signal("-STOP", &holder);
    b.ask("(echo donukken | dd of=/dev/vda bs=512 seek=4000 conv=notrunc 2>/dev/null; sync; echo tamam > /tmp/yazildi) >/dev/null 2>&1 & true");
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(b.ask("cat /tmp/yazildi 2>/dev/null; echo son"), ["son"], "kopya donukken yazma tamamlanmamalı");
    let stalled = db.mirror_probe(&g).unwrap();
    assert!(stalled.done < stalled.total, "takılma yerel hipervizörden görülebilmeli: {stalled:?}");
    let started = std::time::Instant::now();
    db.mirror_stop(&g).unwrap();
    let took = started.elapsed();
    eprintln!("donan kopyadan kurtarma: {} ms", took.as_millis());
    assert!(took < Duration::from_secs(10), "{took:?}");
    until(10, "bekleyen yazma", || b.ask("cat /tmp/yazildi 2>/dev/null; echo son") == ["tamam", "son"]);
    assert_eq!(db.mirror_probe(&g).unwrap().jobs, 0);
    write(&b, "vda", 4001, "kurtarma-sonrasi");
    assert_eq!((read(&b, "vda", 4000), read(&b, "vda", 4001)), ("donukken".to_string(), "kurtarma-sonrasi".to_string()));
    // Durdurulan yansıtma yeniden kurulabilir ve yetişmişken durdurmak da temiz biter.
    signal("-CONT", &holder);
    da.replica_delete("hedef-1-1").unwrap();
    da.replica_serve("hedef-1-2", &sizes, "127.0.0.1", base + 2, &key).unwrap();
    db.mirror_start(&g, "127.0.0.1", base + 2, &key).unwrap();
    until(120, "yeniden eşitleme", || db.mirror_probe(&g).is_some_and(|p| p.synced));
    db.mirror_stop(&g).unwrap();
    assert_eq!(db.mirror_probe(&g).unwrap().jobs, 0);
    da.replica_delete("hedef-1-2").unwrap();
    assert!(da.replica_serve("../disari", &sizes, "127.0.0.1", base + 2, &key).is_err(), "kopya adı dosya yolu olamaz");

    db.apply(&g, Action::Stop).unwrap();
    for dir in [&a.dir, &b.dir] {
        let _ = std::fs::remove_dir_all(dir);
    }
}
