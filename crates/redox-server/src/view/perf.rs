//! Düğüm sayfasındaki performans donanımı kartı (yalnızca gerçek sunucuda).

use super::*;
use crate::qemu::perf::HostPerformance;

pub(super) fn perf_card(host: &HostPerformance) -> Markup {
    html! {
        div class="card" {
            header { h2 { (tx("Performans donanımı")) } (tip("Çekirdek sabitleme, NUMA bağı ve büyük sayfalar için sunucunun durumu.")) }
            dl class="kv" {
                dt { (tx("NUMA düğümleri")) }
                dd { @if host.numa.is_empty() { span class="muted" { "-" } } @for n in &host.numa { div class="mono" { (n.node) ": " (n.cpus) } } }
                dt { (tx("Büyük sayfalar")) }
                dd {
                    @match host.hugepages {
                        Some(h) if h.total > 0 => span class="mono" { (h.free) " / " (h.total) " × " (h.size_kb / 1024) " MB " (tx("boş")) },
                        _ => span class="muted" { (tx("Ayrılmamış (vm.nr_hugepages)")) },
                    }
                }
                dt { (tx("Bellek paylaşımı (KSM)")) }
                dd {
                    @match host.ksm {
                        Some(k) if k.running => span class="mono" { (tx("açık")) " · " (k.saved_mb) " MB " (tx("kazanıldı")) },
                        Some(_) => span class="muted" { (tx("Kapalı. Açmak için sunucuda redox-ksm.service birimini etkinleştirin.")) },
                        None => span class="muted" { (tx("Bu çekirdekte yok.")) },
                    }
                }
            }
        }
    }
}
