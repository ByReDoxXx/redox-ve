//! Prometheus metin biçiminde ölçüm çıktısı (`/metrics`).

use redox_core::{Cluster, GuestStatus, NodeStatus, TaskStatus};

/// Etiket değerini Prometheus kurallarına göre kaçırır.
fn label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

pub fn render(c: &Cluster) -> String {
    let mut out = String::new();
    let mut family = |name: &str, kind: &str, help: &str, lines: Vec<String>| {
        out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n"));
        for line in lines {
            out.push_str(&format!("{name}{line}\n"));
        }
    };
    let mb = 1024 * 1024u64;
    let node = |id: &str| format!("{{node=\"{}\"}}", label(id));
    family("redox_node_up", "gauge", "Düğüm çevrimiçi mi (1/0).",
        c.nodes.iter().map(|n| format!("{} {}", node(&n.id), u8::from(n.status == NodeStatus::Online))).collect());
    family("redox_node_cpu_percent", "gauge", "Düğüm CPU kullanımı, yüzde.",
        c.nodes.iter().map(|n| format!("{} {}", node(&n.id), n.cpu_pct)).collect());
    family("redox_node_memory_used_bytes", "gauge", "Düğümde kullanılan bellek.",
        c.nodes.iter().map(|n| format!("{} {}", node(&n.id), n.mem_used_mb * mb)).collect());
    family("redox_node_memory_total_bytes", "gauge", "Düğümün toplam belleği.",
        c.nodes.iter().map(|n| format!("{} {}", node(&n.id), n.mem_total_mb * mb)).collect());

    let storage = |s: &redox_core::Storage| format!("{{storage=\"{}\",node=\"{}\"}}", label(&s.id), label(s.node.as_deref().unwrap_or("")));
    family("redox_storage_used_bytes", "gauge", "Depolamada dolu alan.",
        c.storages.iter().map(|s| format!("{} {}", storage(s), s.used_gb * 1024 * mb)).collect());
    family("redox_storage_total_bytes", "gauge", "Depolamanın toplam alanı.",
        c.storages.iter().map(|s| format!("{} {}", storage(s), s.total_gb * 1024 * mb)).collect());

    let guest = |g: &redox_core::Guest| format!("vmid=\"{}\",name=\"{}\",kind=\"{}\",node=\"{}\"", g.vmid, label(&g.name), g.kind, label(&g.node));
    family("redox_guest_running", "gauge", "Konuk çalışıyor mu (1/0).",
        c.guests.iter().map(|g| format!("{{{}}} {}", guest(g), u8::from(g.status == GuestStatus::Running))).collect());
    family("redox_guest_cpu_percent", "gauge", "Konuk CPU kullanımı, ayrılan çekirdeklere göre yüzde.",
        c.guests.iter().map(|g| format!("{{{}}} {}", guest(g), g.cpu_pct)).collect());
    family("redox_guest_memory_used_bytes", "gauge", "Konuğun kullandığı bellek.",
        c.guests.iter().map(|g| format!("{{{}}} {}", guest(g), g.mem_used_mb * mb)).collect());
    family("redox_guest_memory_total_bytes", "gauge", "Konuğa ayrılan bellek.",
        c.guests.iter().map(|g| format!("{{{}}} {}", guest(g), g.mem_total_mb * mb)).collect());

    let running = c.tasks.iter().filter(|t| t.status == TaskStatus::Running).count();
    let failed = c.tasks.iter().filter(|t| t.status == TaskStatus::Failed).count();
    family("redox_tasks_running", "gauge", "Süren görev sayısı.", vec![format!(" {running}")]);
    family("redox_tasks_failed", "gauge", "Günlükteki başarısız görev sayısı.", vec![format!(" {failed}")]);
    family("redox_backups", "gauge", "Kayıtlı yedek sayısı.", vec![format!(" {}", c.backups.len())]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposition_format() {
        let mut c = Cluster::demo(0);
        c.guests[0].name = "tırnak\"ve\\ters".into();
        let text = render(&c);
        assert!(text.contains("# TYPE redox_node_cpu_percent gauge\nredox_node_cpu_percent{node=\"pve-ist-01\"} 34\n"), "{text}");
        assert!(text.contains("redox_guest_running{vmid=\"103\",name=\"win-build\",kind=\"vm\",node=\"pve-ist-02\"} 0\n"));
        assert!(text.contains("redox_guest_running{vmid=\"102\",name=\"db-postgres\",kind=\"vm\",node=\"pve-ist-02\"} 1\n"));
        assert!(text.contains("name=\"tırnak\\\"ve\\\\ters\""), "etiket değerleri kaçırılır");
        assert!(text.contains("redox_storage_total_bytes{storage=\"nfs-yedek\",node=\"\"} 8589934592000\n"));
        assert!(text.contains("redox_tasks_running 0\n") && text.contains("redox_backups 0\n"));
        // Her satır ya açıklama ya da "ad{etiketler} sayı" biçimindedir.
        for line in text.lines() {
            assert!(line.starts_with('#') || line.rsplit(' ').next().unwrap().parse::<f64>().is_ok(), "{line}");
        }
        assert!(text.ends_with('\n'));
    }
}
