//! Küme sayfasındaki hata toleransı kartı: korunan konuklar ve burada duran ikizler.

use super::*;
use redox_core::{ft_blocker, FtState};

fn state_pill(state: FtState) -> Markup {
    let class = match state {
        FtState::Protected => "pill ok",
        FtState::Unprotected | FtState::Suspended | FtState::HandedOver => "pill err",
        FtState::Preparing => "pill warn",
        FtState::Waiting => "pill idle",
    };
    html! { span class=(class) { (tx(state.label())) } }
}

pub(super) fn ft_card(p: &Page, peers: &[crate::peers::PeerView]) -> Markup {
    let c = p.cluster;
    // Hata toleransı en az iki eş ister: ikizi çalıştıran ve hakem.
    let can_add = p.admin && p.real && peers.len() >= 2;
    let candidates: Vec<&Guest> = c.guests.iter().filter(|g| g.status == GuestStatus::Stopped && g.lock.is_none() && ft_blocker(g).is_none() && c.protection(g.vmid).is_none() && c.mirror(g.vmid).is_none()).collect();
    if !can_add && c.protections.is_empty() && c.standbys.is_empty() {
        return html! {};
    }
    html! {
        div class="card" {
            header {
                h2 { (tx("Hata toleransı")) }
                (tip("Konuğun eş sunucuda eşzamanlı çalışan bir ikizi olur. Sunucu ölürse konuk yeniden başlamadan ikizde sürer."))
                div class="end" { span class="note" { (c.protections.len()) " / " (redox_core::ft::MAX_PROTECTED) } }
            }
            @if c.protections.is_empty() {
                p class="empty" { (tx("Korunan konuk yok.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("Konuk")) } th { (tx("İkiz sunucusu")) } th { (tx("Hakem")) } th { (tx("Durum")) } @if p.admin { th { span class="sr" { (tx("İşlem")) } } } } }
                    tbody { @for r in &c.protections { tr {
                        td class="name" { a href={ "/guests/" (r.vmid) } { span class="mono dim" { (r.vmid) } " " (c.guest(r.vmid).map(|g| g.name.clone()).unwrap_or_default()) } }
                        td class="mono" { (r.peer) }
                        td class="mono" { (r.witness) }
                        td { (state_pill(r.state)) @if let Some(e) = &r.error { div class="dim" { (tfmt(e.clone())) } } }
                        @if p.admin { td class="act" {
                            form method="post" action="/cluster/ft" {
                                input type="hidden" name="vmid" value=(r.vmid);
                                button class="btn sm danger" type="submit" name="op" value="off" { (tx("Kapat")) }
                            }
                        } }
                    } } }
                } }
            }
            @if !c.standbys.is_empty() {
                div class="tw" { table {
                    thead { tr { th { (tx("Buradaki ikiz")) } th { (tx("Birincil")) } th { (tx("Hakem")) } } }
                    tbody { @for s in &c.standbys { tr {
                        td class="name" { a href={ "/guests/" (s.vmid) } { span class="mono dim" { (s.vmid) } " " (c.guest(s.vmid).map(|g| g.name.clone()).unwrap_or_default()) } }
                        td class="mono" { (s.origin) " / " (s.source) }
                        td class="mono" { (s.witness) }
                    } } }
                } }
            }
            @if can_add {
                @if candidates.is_empty() {
                    p class="empty" { (tx("Korumaya uygun kapalı konuk yok: tek diskli, köprüye bağlı tek kartlı ve ek donanımı olmayan bir sanal makine gerekir.")) }
                } @else {
                    form class="addrow" method="post" action="/cluster/ft" {
                        div class="field" { label for="ft-vmid" { (tx("Konuk")) } select id="ft-vmid" name="vmid" { @for g in &candidates { option value=(g.vmid) { (g.vmid) " " (g.name) } } } }
                        div class="field" { label for="ft-peer" { (tx("İkiz sunucusu")) } select id="ft-peer" name="peer" { @for v in peers { option value=(v.name) { (v.name) } } } }
                        div class="field" { label for="ft-witness" { (tx("Hakem")) } select id="ft-witness" name="witness" { @for (i, v) in peers.iter().enumerate() { option value=(v.name) selected[i == 1] { (v.name) } } } }
                        button class="btn" type="submit" name="op" value="on" { (icon("plus")) (tx("Koru")) }
                    }
                }
            }
            footer { (tx("Korunurken konuk yedeklenemez, anlık görüntüsü alınamaz ve taşınamaz; konuğun ağdan gönderdiği bazı paketler birkaç saniye gecikebilir. İki sunucu aynı işlemciye sahip olmalı; paket karşılaştırma bağlantıları şifresizdir: ayrılmış, güvenilen bir ağ kullanın.")) }
        }
    }
}
