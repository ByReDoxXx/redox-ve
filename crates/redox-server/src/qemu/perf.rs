//! Sunucunun performans donanımı: NUMA düğümleri, büyük sayfalar ve KSM. Ayrıştırıcılar saf işlevlerdir.

use std::path::Path;

use redox_core::hw::parse_cpu_list;
use redox_core::Guest;
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct NumaNode {
    pub node: u32,
    /// Düğümün çekirdekleri (`0-7`).
    pub cpus: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct HugePages {
    pub total: u64,
    pub free: u64,
    pub size_kb: u64,
}

/// Aynı içerikli bellek sayfalarını birleştiren çekirdek hizmeti.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Ksm {
    pub running: bool,
    /// Birleştirme sayesinde kazanılan bellek (MB).
    pub saved_mb: u64,
    /// Paylaşılan sayfa sayısı.
    pub pages_shared: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HostPerformance {
    pub numa: Vec<NumaNode>,
    pub hugepages: Option<HugePages>,
    pub ksm: Option<Ksm>,
}

/// `sys`: `/sys`, `meminfo`: `/proc/meminfo` içeriği.
pub fn read_performance(sys: &Path, meminfo: &str) -> HostPerformance {
    let text = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().map(|t| t.trim().to_string());
    let nodes = sys.join("devices/system/node");
    let mut numa: Vec<NumaNode> = std::fs::read_dir(&nodes)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("node")?.parse::<u32>().ok())
        .filter_map(|node| Some(NumaNode { node, cpus: text(nodes.join(format!("node{node}/cpulist")))? }))
        .collect();
    numa.sort_by_key(|n| n.node);
    let ksm_dir = sys.join("kernel/mm/ksm");
    let number = |name: &str| text(ksm_dir.join(name)).and_then(|t| t.parse::<u64>().ok());
    let ksm = number("run").map(|run| Ksm { running: run == 1, saved_mb: number("pages_sharing").unwrap_or(0) * 4 / 1024, pages_shared: number("pages_shared").unwrap_or(0) });
    HostPerformance { numa, hugepages: parse_hugepages(meminfo), ksm }
}

pub fn parse_hugepages(meminfo: &str) -> Option<HugePages> {
    let field = |name: &str| meminfo.lines().find_map(|l| l.strip_prefix(name))?.split_whitespace().next()?.parse::<u64>().ok();
    Some(HugePages { total: field("HugePages_Total:")?, free: field("HugePages_Free:")?, size_kb: field("Hugepagesize:")? })
}

/// Konuk bu sunucuda istenen performans ayarlarıyla açılabilir mi? Açılamıyorsa QEMU'nun anlaşılmaz hatasından
/// önce nedeni ve çözümü söylenir.
pub fn preflight(g: &Guest, host: &HostPerformance) -> Result<(), String> {
    if let Some(node) = g.hw.numa_node {
        if !host.numa.iter().any(|n| n.node == node) {
            let known = host.numa.iter().map(|n| n.node.to_string()).collect::<Vec<_>>().join(", ");
            return Err(format!("sunucuda {node} numaralı NUMA düğümü yok (olanlar: {known})"));
        }
    }
    if g.hw.hugepages {
        let pages = host.hugepages.filter(|h| h.size_kb > 0).ok_or("sunucunun büyük sayfa bilgisi okunamadı")?;
        let needed = (g.mem_total_mb * 1024).div_ceil(2048);
        let free = pages.free * pages.size_kb / 2048;
        if free < needed {
            return Err(format!("sunucuda yeterli boş büyük sayfa yok: {needed} sayfa gerekiyor, {free} boş. Sunucuda kök olarak ayırın: sysctl vm.nr_hugepages=<sayı>"));
        }
    }
    Ok(())
}

/// Konuğun çalışacağı çekirdekler: elle verilen liste, yoksa bağlı olduğu NUMA düğümününkiler.
pub fn wanted_cpus(g: &Guest, host: &HostPerformance) -> Option<Vec<u32>> {
    let list = if g.hw.cpu_affinity.is_empty() { host.numa.iter().find(|n| Some(n.node) == g.hw.numa_node)?.cpus.as_str() } else { g.hw.cpu_affinity.as_str() };
    parse_cpu_list(list).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::Cluster;

    const MEMINFO: &str = "MemTotal: 100 kB\nHugePages_Total:    1024\nHugePages_Free:      600\nHugepagesize:       2048 kB\n";

    #[test]
    fn host_information_is_read_from_sys_and_meminfo() {
        let dir = std::env::temp_dir().join(format!("redox-perf-{}", std::process::id()));
        for (path, text) in [("devices/system/node/node1/cpulist", "8-15\n"), ("devices/system/node/node0/cpulist", "0-7\n"), ("devices/system/node/possible", "0-1"), ("kernel/mm/ksm/run", "1\n"), ("kernel/mm/ksm/pages_sharing", "51200\n"), ("kernel/mm/ksm/pages_shared", "100\n")] {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        let host = read_performance(&dir, MEMINFO);
        assert_eq!(host.numa, [NumaNode { node: 0, cpus: "0-7".into() }, NumaNode { node: 1, cpus: "8-15".into() }]);
        assert_eq!(host.ksm, Some(Ksm { running: true, saved_mb: 200, pages_shared: 100 }));
        assert_eq!(host.hugepages, Some(HugePages { total: 1024, free: 600, size_kb: 2048 }));
        let empty = read_performance(&dir.join("yok"), "MemTotal: 1 kB");
        assert_eq!(empty, HostPerformance::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn preflight_explains_what_the_host_lacks_and_cpus_follow_the_node() {
        let host = HostPerformance { numa: vec![NumaNode { node: 0, cpus: "0-3".into() }], hugepages: parse_hugepages(MEMINFO), ksm: None };
        let mut g = Cluster::demo(0).guests[0].clone();
        assert!(preflight(&g, &host).is_ok() && wanted_cpus(&g, &host).is_none());
        g.hw.numa_node = Some(2);
        assert!(preflight(&g, &host).unwrap_err().contains("NUMA düğümü yok (olanlar: 0)"));
        g.hw.numa_node = Some(0);
        assert_eq!(wanted_cpus(&g, &host), Some(vec![0, 1, 2, 3]));
        g.hw.cpu_affinity = "2".into();
        assert_eq!(wanted_cpus(&g, &host), Some(vec![2]), "elle verilen liste önce gelir");
        (g.hw.hugepages, g.mem_total_mb) = (true, 2048);
        assert!(preflight(&g, &host).unwrap_err().contains("1024 sayfa gerekiyor, 600 boş"));
        g.mem_total_mb = 1200;
        assert!(preflight(&g, &host).is_ok());
    }
}
