//! Küme sayfasındaki ortak depo kartı.

use super::*;
use crate::state::shared::SharedItem;

pub(super) fn shared_card(p: &Page, items: &[SharedItem], enabled: bool, peers: &[crate::peers::PeerView]) -> Markup {
    if !enabled {
        return html! {};
    }
    html! {
        div class="card" {
            header {
                h2 { (tx("Ortak depo")) }
                (tip("Birden çok sunucunun gördüğü depodaki konuklar. Her konuğun disklerinin tek sahibi vardır."))
                div class="end" { span class="note" { (items.len()) } }
            }
            @if items.is_empty() {
                p class="empty" { (tx("Ortak depoda konuk yok.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("Konuk")) } th { (tx("Depo")) } th { (tx("Sahibi")) } th { (tx("Durum")) } @if p.admin { th { span class="sr" { (tx("İşlem")) } } } } }
                    tbody { @for i in items { tr {
                        td class="name" { @if i.here { a href={ "/guests/" (i.vmid) } { span class="mono dim" { (i.vmid) } " " (i.name) } } @else { span class="mono dim" { (i.vmid) } " " (i.name) } }
                        td class="mono" { (i.storage) }
                        td class="mono" { (i.owner) }
                        td {
                            @if i.mine && i.here { span class="pill ok" { (icon("check")) (tx("burada")) } }
                            @else if i.mine { span class="pill warn" { (icon("alert")) (tx("kaydı eksik")) } }
                            @else if i.quiet { span class="pill err" { (icon("alert")) (tx("sahibi susmuş")) } }
                            @else { span class="pill idle" { (tx("başka sunucuda")) } }
                        }
                        @if p.admin { td class="act" {
                            @if (i.mine && !i.here) || i.quiet {
                                details class="set" {
                                    summary class="btn sm" { (icon("restart")) (tx("Devral")) }
                                    div class="set-panel" {
                                        form class="danger-row" method="post" action="/cluster/shared" {
                                            input type="hidden" name="storage" value=(i.storage); input type="hidden" name="vmid" value=(i.vmid);
                                            span { (tx("Sahibi olan sunucunun kapalı olduğundan emin olun. Ağı kopmuş ama paylaşıma erişemeyen bir sunucu hâlâ çalışıyor olabilir.")) }
                                            button class="btn sm primary" type="submit" name="op" value="adopt" { (tx("Konuğu devral")) }
                                        }
                                    }
                                }
                            }
                        } }
                    } } }
                } }
            }
            @if p.admin && !peers.is_empty() {
                form class="addrow" method="post" action="/cluster/shared" {
                    div class="field" { label for="sh-peer" { (tx("Eş sunucu")) } select id="sh-peer" name="peer" { @for v in peers { option value=(v.name) { (v.name) } } } }
                    div class="field" { label for="sh-vmid" { (tx("Kapalı konuğun kimliği")) } input id="sh-vmid" name="vmid" type="text" inputmode="numeric" required; }
                    div {}
                    button class="btn" type="submit" name="op" value="take" { (icon("migrate")) (tx("Disk kopyalamadan al")) }
                }
            }
            footer { (tx("Diskleri ortak depodaki çalışan sanal makine canlı taşınırken yalnızca belleği gider. UEFI, TPM, şablon, bağlı klon ve askıdaki konuklar devredilemez; yedekler bulundukları sunucuda kalır.")) }
        }
    }
}
