//! Sürücünün gözlediği durumun kayda işlenmesi ve küme özeti.

use super::*;

impl Cluster {
    /// Sürücünün gözlediği gerçek durumu işler (ör. konuk kendi kendine kapandıysa).
    /// Süren bir görev varken dokunmaz: sonucu o görev belirler. Durum değiştiyse `true`.
    pub fn observe_guest(&mut self, vmid: VmId, probe: &crate::driver::Probe, now: u64) -> bool {
        let Ok(g) = self.guest_mut(vmid) else { return false };
        if g.lock.is_some() {
            return false;
        }
        let changed = g.status != probe.status;
        g.status = probe.status;
        match probe.status {
            GuestStatus::Stopped => {
                g.cpu_pct = 0;
                g.mem_used_mb = 0;
                g.started_at = None;
            }
            GuestStatus::Running | GuestStatus::Paused => {
                if g.started_at.is_none() {
                    g.started_at = Some(now);
                }
                if let Some(cpu) = probe.cpu_pct {
                    g.cpu_pct = if probe.status == GuestStatus::Paused { 0 } else { cpu.min(100) };
                }
                if let Some(mem) = probe.mem_used_mb {
                    g.mem_used_mb = mem.min(g.mem_total_mb);
                }
            }
        }
        changed
    }

    /// Sürücünün bulduğu ağ adresini işler (bulunamadıysa `None`). Değiştiyse `true`.
    /// Konuk ajanının bildirdiği işletim sistemini (ya da ajanın yokluğunu) kaydeder; değiştiyse `true`.
    pub fn observe_agent(&mut self, vmid: VmId, agent: Option<String>) -> bool {
        match self.guest_mut(vmid) {
            Ok(g) if g.agent != agent => {
                g.agent = agent;
                true
            }
            _ => false,
        }
    }

    pub fn observe_ip(&mut self, vmid: VmId, ip: Option<String>) -> bool {
        match self.guest_mut(vmid) {
            Ok(g) if g.ip != ip => {
                g.ip = ip;
                true
            }
            _ => false,
        }
    }

    /// Düğümün ölçülen kullanımını işler.
    pub fn observe_node(&mut self, id: &str, cpu_pct: u8, mem_used_mb: u64, mem_total_mb: u64, uptime_secs: u64) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.cpu_pct = cpu_pct.min(100);
            n.mem_total_mb = mem_total_mb;
            n.mem_used_mb = mem_used_mb.min(mem_total_mb);
            n.uptime_secs = uptime_secs;
        }
    }

    /// Depolamanın ölçülen doluluğunu işler.
    pub fn observe_storage(&mut self, id: &str, node: &str, used_gb: u64, total_gb: u64) {
        if let Ok(si) = self.storage_index(id, node) {
            self.storages[si].total_gb = total_gb;
            self.storages[si].used_gb = used_gb.min(total_gb);
        }
    }

    pub fn summary(&self) -> ClusterSummary {
        let online: Vec<&Node> = self
            .nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Online)
            .collect();
        let cores: u32 = online.iter().map(|n| n.cores).sum();
        let weighted: u64 = online.iter().map(|n| n.cpu_pct as u64 * n.cores as u64).sum();
        let mem_used_mb: u64 = online.iter().map(|n| n.mem_used_mb).sum();
        let mem_total_mb: u64 = online.iter().map(|n| n.mem_total_mb).sum();
        let storage_used_gb: u64 = self.storages.iter().map(|s| s.used_gb).sum();
        let storage_total_gb: u64 = self.storages.iter().map(|s| s.total_gb).sum();
        ClusterSummary {
            name: self.name.clone(),
            nodes_online: online.len(),
            nodes_total: self.nodes.len(),
            quorate: online.len() * 2 > self.nodes.len(),
            cores,
            cpu_pct: if cores == 0 { 0 } else { (weighted as f64 / cores as f64).round() as u8 },
            mem_used_mb,
            mem_total_mb,
            mem_pct: pct(mem_used_mb, mem_total_mb),
            storage_used_gb,
            storage_total_gb,
            storage_pct: pct(storage_used_gb, storage_total_gb),
            guests_running: self
                .guests
                .iter()
                .filter(|g| g.status == GuestStatus::Running)
                .count(),
            guests_total: self.guests.len(),
        }
    }
}
