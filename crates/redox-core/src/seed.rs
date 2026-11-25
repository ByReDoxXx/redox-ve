//! Simülasyon için örnek küme. Gerçek sürücüler bağlandığında bu veri
//! düğümlerden okunan durumla değiştirilecektir.

use crate::cluster::Cluster;
use crate::model::*;

const DAY: u64 = 86_400;
const HOUR: u64 = 3600;

fn gb(v: f64) -> u64 {
    (v * 1024.0).round() as u64
}

impl Cluster {
    /// Üç düğümlü, sekiz konuklu örnek küme. `now`, çalışma sürelerinin dayanağıdır.
    pub fn demo(now: u64) -> Cluster {
        let mut c = Cluster::new("istanbul-01");

        let node = |id: &str, cpu, used, total, cores, up, ip: &str| Node {
            id: id.to_string(),
            status: NodeStatus::Online,
            cores,
            cpu_pct: cpu,
            mem_used_mb: gb(used),
            mem_total_mb: gb(total),
            uptime_secs: up,
            kernel: "6.8.12".to_string(),
            ip: ip.to_string(),
        };
        c.nodes = vec![
            node("pve-ist-01", 34, 78.0, 128.0, 32, 42 * DAY + 7 * HOUR, "10.10.0.11"),
            node("pve-ist-02", 61, 101.0, 128.0, 32, 42 * DAY + 7 * HOUR, "10.10.0.12"),
            node("pve-ank-01", 12, 22.0, 64.0, 16, 9 * DAY + 3 * HOUR, "10.20.0.11"),
        ];

        #[allow(clippy::too_many_arguments)]
        let guest = |vmid,
                     name: &str,
                     kind,
                     node: &str,
                     status,
                     cpu,
                     used: f64,
                     total: f64,
                     cores,
                     disk,
                     storage: &str,
                     os: &str,
                     up: Option<u64>,
                     ip: Option<&str>| Guest {
            vmid,
            name: name.to_string(),
            kind,
            node: node.to_string(),
            status,
            cores,
            cpu_pct: cpu,
            mem_used_mb: gb(used),
            mem_total_mb: gb(total),
            disk_gb: disk,
            storage: storage.to_string(),
            os: os.to_string(),
            ip: ip.map(str::to_string),
            agent: None,
            bridge: "vmbr0".to_string(),
            onboot: true,
            started_at: up.map(|u| now.saturating_sub(u)),
            lock: None,
            iso: None,
            tags: Vec::new(),
            forwards: Vec::new(),
            snapshots: Vec::new(),
            backup_time: None,
            backup_keep: DEFAULT_KEEP,
            last_auto_backup: None,
            pool: None,
            backup_gfs: crate::retention::Gfs::default(),
            hw: Default::default(),
        };
        use GuestKind::{Ct, Vm};
        use GuestStatus::{Paused, Running, Stopped};
        c.guests = vec![
            guest(100, "web-prod-01", Vm, "pve-ist-01", Running, 23, 5.2, 8.0, 4, 80, "local-zfs", "debian-12", Some(12 * DAY + 4 * HOUR), Some("10.10.1.20")),
            guest(101, "web-prod-02", Vm, "pve-ist-01", Running, 19, 4.8, 8.0, 4, 80, "local-zfs", "debian-12", Some(12 * DAY + 4 * HOUR), Some("10.10.1.21")),
            guest(102, "db-postgres", Vm, "pve-ist-02", Running, 58, 27.1, 32.0, 8, 500, "local-zfs", "ubuntu-24.04", Some(30 * DAY + HOUR), Some("10.10.1.30")),
            guest(103, "win-build", Vm, "pve-ist-02", Stopped, 0, 0.0, 16.0, 8, 200, "local-zfs", "windows-2022", None, None),
            guest(104, "k8s-worker-1", Vm, "pve-ist-02", Paused, 0, 11.4, 16.0, 8, 120, "local-zfs", "talos-1.8", Some(3 * DAY + 9 * HOUR), Some("10.10.1.41")),
            guest(200, "dns-01", Ct, "pve-ist-01", Running, 2, 0.2, 0.5, 1, 4, "local-zfs", "alpine-3.20", Some(42 * DAY + 6 * HOUR), Some("10.10.0.53")),
            guest(201, "monitoring", Ct, "pve-ank-01", Running, 14, 2.9, 4.0, 2, 40, "local-lvm", "debian-12", Some(9 * DAY + 2 * HOUR), Some("10.20.1.10")),
            guest(202, "ci-runner", Ct, "pve-ank-01", Stopped, 0, 0.0, 4.0, 4, 60, "local-lvm", "ubuntu-24.04", None, None),
        ];

        let storage = |id: &str, node: Option<&str>, kind, content: &[&str], used, total| Storage {
            id: id.to_string(),
            node: node.map(str::to_string),
            kind,
            content: content.iter().map(|s| s.to_string()).collect(),
            used_gb: used,
            total_gb: total,
        };
        c.storages = vec![
            storage("local-zfs", Some("pve-ist-01"), StorageKind::Zfs, &["disk", "konteyner"], 612, 1800),
            storage("local-zfs", Some("pve-ist-02"), StorageKind::Zfs, &["disk", "konteyner"], 1390, 1800),
            storage("nfs-yedek", None, StorageKind::Nfs, &["yedek", "ISO"], 7400, 8000),
            storage("local-lvm", Some("pve-ank-01"), StorageKind::LvmThin, &["disk", "konteyner"], 210, 900),
        ];
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_internally_consistent() {
        let c = Cluster::demo(1_800_000_000);
        let mut ids: Vec<VmId> = c.guests.iter().map(|g| g.vmid).collect();
        let sorted = ids.clone();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, sorted, "kimlikler sıralı ve tekil olmalı");

        for g in &c.guests {
            assert!(c.node(&g.node).is_ok(), "{}: bilinmeyen düğüm", g.vmid);
            assert!(c.storage_for(&g.storage, &g.node).is_ok(), "{}: depolama düğümde yok", g.vmid);
            assert!(valid_name(&g.name), "{}", g.name);
            assert!(g.mem_used_mb <= g.mem_total_mb);
            assert!(g.lock.is_none());
            match g.status {
                GuestStatus::Stopped => {
                    assert_eq!((g.cpu_pct, g.mem_used_mb, g.started_at, g.ip.as_deref()), (0, 0, None, None))
                }
                _ => assert!(g.started_at.is_some() && g.ip.is_some()),
            }
        }
        for s in &c.storages {
            assert!(s.used_gb <= s.total_gb);
        }
        assert!(c.tasks.is_empty());
    }
}
