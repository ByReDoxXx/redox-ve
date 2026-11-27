//! Çekirdek sabitleme, NUMA bağı ve büyük sayfaların gerçek QEMU ile sınanması.

use super::real_qemu::real;
use super::*;

/// QEMU sürecinin bütün iş parçacıklarının çalışabildiği çekirdek listeleri (tekrarsız).
fn thread_cpus(pid: &str) -> Vec<String> {
    let mut lists: Vec<String> = std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .flatten()
        .filter_map(|t| std::fs::read_to_string(t.path().join("status")).ok())
        .filter_map(|s| s.lines().find_map(|l| l.strip_prefix("Cpus_allowed_list:")).map(|v| v.trim().to_string()))
        .collect();
    lists.sort();
    lists.dedup();
    lists
}

#[test]
fn real_qemu_pins_threads_binds_memory_and_explains_missing_huge_pages() {
    let Some(r) = real() else { return };
    let mut g = Cluster::demo(0).guests.remove(0);
    (g.vmid, g.name, g.kind, g.bridge, g.cores, g.mem_total_mb, g.disk_gb) = (190, "perf".into(), GuestKind::Vm, "nat".into(), 2, 512, 1);
    g.iso = None;
    g.forwards.clear();
    g.hw = Default::default();
    g.hw.cpu_affinity = "0".into();
    let driver = QemuDriver::new(r.cfg.clone()).unwrap();
    driver.create(&g).unwrap();
    driver.apply(&g, Action::Start).unwrap();
    r.boot();
    let pid = || std::fs::read_to_string(r.cfg.pid_path(190)).unwrap().trim().to_string();
    assert_eq!(thread_cpus(&pid()), ["0"], "bütün iş parçacıkları tek çekirdekte");
    assert!(r.ask("grep -c ^processor /proc/cpuinfo").iter().any(|l| l == "2"), "konuk iki çekirdek görmeyi sürdürür");
    // Çalışırken değişir ve kaldırılır.
    g.hw.cpu_affinity = "1-2".into();
    driver.configure(&g).unwrap();
    assert_eq!(thread_cpus(&pid()), ["1-2"]);
    g.hw.cpu_affinity.clear();
    driver.configure(&g).unwrap();
    assert_eq!(thread_cpus(&pid()), thread_cpus(&std::process::id().to_string()), "sınamanın kendi çekirdek kümesine döner");
    driver.apply(&g, Action::Stop).unwrap();

    // NUMA bağı: konuğun belleği istenen düğüme "bind" ilkesiyle ayrılır.
    let host = redox_server::host::performance();
    if host.numa.iter().any(|n| n.node == 0) {
        g.hw.numa_node = Some(0);
        driver.apply(&g, Action::Start).unwrap();
        r.boot();
        let maps = std::fs::read_to_string(format!("/proc/{}/numa_maps", pid())).unwrap_or_default();
        assert!(maps.lines().any(|l| l.split_whitespace().nth(1) == Some("bind:0")), "bellek bağı: {}", maps.lines().take(5).collect::<Vec<_>>().join("\n"));
        assert_eq!(thread_cpus(&pid()), [host.numa[0].cpus.clone()], "çekirdek listesi yoksa düğümün çekirdekleri");
        driver.apply(&g, Action::Stop).unwrap();
        eprintln!("NUMA bağı: tamam");
    } else {
        eprintln!("ATLANDI: NUMA bilgisi yok");
    }
    g.hw.numa_node = Some(60);
    assert!(driver.apply(&g, Action::Start).unwrap_err().contains("NUMA düğümü yok"));
    assert_eq!(status(&driver, &g), GuestStatus::Stopped);

    g.hw.numa_node = None;
    g.hw.hugepages = true;
    let free = host.hugepages.map_or(0, |h| h.free * h.size_kb / 1024);
    if free < g.mem_total_mb {
        assert!(driver.apply(&g, Action::Start).unwrap_err().contains("boş büyük sayfa yok"));
        eprintln!("büyük sayfa: sunucuda ayrılmış sayfa yok; yalnızca açıklayıcı ret sınandı");
    } else {
        driver.apply(&g, Action::Start).unwrap();
        r.boot();
        driver.apply(&g, Action::Stop).unwrap();
        eprintln!("büyük sayfa: tamam");
    }
}
