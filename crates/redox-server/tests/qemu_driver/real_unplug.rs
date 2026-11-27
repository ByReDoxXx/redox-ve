//! Çalışan konuktan çekirdek ve bellek çıkarma, sürücünün kendi koduyla gerçek QEMU'da: konuk kaynağı gerçekten
//! bırakır, askıdan dönüşte makine yeni haliyle kurulur.

use std::time::Duration;

use super::real_qemu::real;
use super::*;

#[test]
fn real_qemu_removes_cores_and_memory_the_guest_lets_go_of() {
    let Some(real) = real() else { return };
    let d = QemuDriver::new(real.cfg.clone()).unwrap();
    let mut g = guest(&real.cfg, "gercek-cikarma");
    (g.cores, g.mem_total_mb, g.hw.max_cores, g.hw.max_mem_mb) = (2, 1024, 4, 4096);
    d.create(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();
    real.boot();
    let boot_kb = real.mem_kb();

    // İki çekirdek ekle, birini çıkar.
    for _ in 0..2 {
        d.hot_add_core(&g).unwrap();
        g.cores += 1;
    }
    std::thread::sleep(Duration::from_secs(2));
    real.ask("for c in /sys/devices/system/cpu/cpu[0-9]*/online; do echo 1 > $c; done");
    assert_eq!(real.cpus().1, "0-3");
    d.hot_remove_core(&g).unwrap_or_else(|e| panic!("{e}"));
    g.cores = 3;
    assert_eq!(d.hotplug_inventory(&g), Some((3, 0)));
    assert_eq!(real.cpus(), ("0-2".to_string(), "0-2".to_string()), "konukta çekirdek gerçekten gitmeli");
    // Açılışta takılı olan çekirdek de çıkarılabilir; ilki kalır.
    d.hot_remove_core(&g).unwrap_or_else(|e| panic!("{e}"));
    d.hot_remove_core(&g).unwrap_or_else(|e| panic!("açılış çekirdeği: {e}"));
    g.cores = 1;
    assert_eq!(real.cpus().0, "0");
    assert!(d.hot_remove_core(&g).unwrap_err().contains("son çekirdek"));
    // Çıkarılan yuva yeniden doldurulabilir.
    d.hot_add_core(&g).unwrap();
    g.cores = 2;
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(real.cpus().0, "0-1");
    real.ask("for c in /sys/devices/system/cpu/cpu[0-9]*/online; do echo 1 > $c; done");

    // İki parça bellek ekle. Linux belleği ancak "taşınabilir" etkinleştirilmişse güvenle geri verir.
    let policy = real.ask("cat /sys/devices/system/memory/auto_online_blocks").pop().unwrap_or_default();
    eprintln!("konukta otomatik açma: {policy}");
    real.ask("echo online_movable > /sys/devices/system/memory/auto_online_blocks");
    let online_movable = "for m in /sys/devices/system/memory/memory*/state; do grep -q offline $m && echo online_movable > $m; done; true";
    for mb in [512, 256] {
        d.hot_add_memory(&g, mb).unwrap();
        g.mem_total_mb += mb;
        g.hw.dimms.push(mb);
        std::thread::sleep(Duration::from_secs(3));
        real.ask(online_movable);
    }
    assert_eq!(real.mem_kb(), boot_kb + 768 * 1024);
    d.hot_remove_memory(&g).unwrap_or_else(|e| panic!("{e}"));
    g.mem_total_mb -= 256;
    g.hw.dimms.pop();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(real.mem_kb(), boot_kb + 512 * 1024, "konukta bellek gerçekten azalmalı");
    assert_eq!(d.hotplug_inventory(&g), Some((2, 1)));

    // Askıdan dönüş yeni makineyle olur: kayıt gerçekle uyuşmasaydı bellek durumu yüklenemezdi.
    real.ask("echo isaret-9021 > /tmp/isaret");
    d.suspend(&g).unwrap();
    d.apply(&g, Action::Start).unwrap_or_else(|e| panic!("askıdan dönüş: {e}"));
    assert_eq!(real.ask("cat /tmp/isaret"), ["isaret-9021"]);
    assert_eq!((real.mem_kb(), real.cpus().0), (boot_kb + 512 * 1024, "0-1".to_string()));
    // Döndükten sonra da çıkarılır ve yeniden eklenir.
    d.hot_remove_memory(&g).unwrap_or_else(|e| panic!("dönüşten sonra: {e}"));
    g.mem_total_mb -= 512;
    g.hw.dimms.pop();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(real.mem_kb(), boot_kb);
    assert!(d.hot_remove_memory(&g).unwrap_err().contains("çıkarılabilecek bellek parçası yok"));
    d.hot_add_memory(&g, 128).unwrap();
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(&real.dir);
}
