//! Düğüm sayfasındaki donanım sağlığı ve güncelleme kartı.

use super::*;
use redox_core::health::{problems, HostHealth, UpdateRun};

pub(super) fn health_card(p: &Page, health: &HostHealth, pending: bool, last: Option<&UpdateRun>) -> Markup {
    let found = problems(health);
    html! {
        div class="card" {
            header {
                h2 { (tx("Donanım sağlığı")) }
                (tip("Disklerin SMART durumu, donanım algılayıcıları ve bekleyen paket güncellemeleri. Kök yetkili yardımcı saatte bir okur."))
                div class="end" {
                    @if found.is_empty() { span class="pill ok" { (icon("check")) (tx("sorun yok")) } }
                    @else { span class="pill err" { (icon("alert")) (found.len()) " " (tx("uyarı")) } }
                }
            }
            @if !found.is_empty() {
                dl class="kv" { @for (_, _, message) in &found { dt { (icon("alert")) } dd { (tfmt(message.clone())) } } }
            }
            @if !health.disks.is_empty() {
                div class="tw" { table {
                    thead { tr { th { "Disk" } th { "Model" } th { "SMART" } th class="num" { "°C" } th class="num" { (tx("Çalışma saati")) } th class="num" { (tx("Bozuk sektör")) } } }
                    tbody { @for d in &health.disks { tr {
                        td class="name mono" { (d.device) }
                        td class="dim" { (d.model) }
                        td { @match d.passed { Some(true) => span class="pill ok" { (tx("geçti")) }, Some(false) => span class="pill err" { (tx("başarısız")) }, None => span class="dim" { "-" } } }
                        td class="num mono" { (d.temperature_c.map_or("-".to_string(), |v| v.to_string())) }
                        td class="num mono" { (d.power_on_hours.map_or("-".to_string(), |v| v.to_string())) }
                        td class="num mono" { (d.bad_sectors.map_or("-".to_string(), |v| v.to_string())) }
                    } } }
                } }
            }
            @if !health.sensors.is_empty() {
                details class="set" {
                    summary class="btn sm quiet" { (tx("Algılayıcılar")) " (" (health.sensors.len()) ")" }
                    div class="tw" { table { tbody { @for s in &health.sensors { tr { td { (s.name) } td class="mono" { (s.reading) } td class="mono dim" { (s.status) } } } } } }
                }
            }
            @if !health.updates.is_empty() {
                details class="set" {
                    summary class="btn sm quiet" { (tx("Bekleyen güncellemeler")) " (" (health.updates.len()) ")" }
                    div class="tw" { table { tbody { @for u in &health.updates { tr { td class="mono" { (u.name) } td class="mono dim" { (u.from) } td class="mono" { (u.to) } } } } } }
                }
                @if p.admin {
                    @if pending {
                        p class="empty" { (tx("Güncelleme istendi; yardımcı çalışınca sonuç burada görünür.")) }
                    } @else {
                        form class="inline" method="post" action="/host/update" {
                            button class="btn sm" type="submit" { (icon("restart")) (tx("Güncellemeleri kur")) }
                        }
                    }
                }
            }
            @if let Some(run) = last {
                details class="set" {
                    summary class="btn sm quiet" { (tx("Son güncelleme")) ": " (datetime(run.finished)) " · " @if run.ok { (tx("başarılı")) } @else { (tx("başarısız")) } }
                    pre class="mono dim" { (run.output) }
                }
            }
            footer {
                (tx("Okunma zamanı")) ": " (datetime(health.collected)) ". "
                @for note in &health.notes { (tfmt(note.clone())) ". " }
                (tx("Güncelleme kurmak için sunucuda redox-update.path birimi etkin olmalıdır. Çekirdek güncellemesinden sonra sunucuyu elle yeniden başlatın."))
            }
        }
    }
}
