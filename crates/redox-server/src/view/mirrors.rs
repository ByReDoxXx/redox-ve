//! Küme sayfasındaki disk yansıtma kartları: buradan eşlere yansıtılanlar ve eşler için burada tutulan kopyalar.

use super::*;
use redox_core::{mirror_blocker, Mirror, MirrorState, MirrorStore};

fn state_pill(state: MirrorState) -> Markup {
    let class = match state {
        MirrorState::InSync => "pill ok",
        MirrorState::Broken => "pill err",
        MirrorState::Syncing => "pill warn",
        MirrorState::Waiting => "pill idle",
    };
    html! { span class=(class) { (tx(state.label())) } }
}

fn when(at: Option<u64>) -> String {
    at.map_or_else(|| "-".to_string(), datetime)
}

fn mirror_row(p: &Page, m: &Mirror) -> Markup {
    let name = p.cluster.guest(m.vmid).map(|g| g.name.clone()).unwrap_or_default();
    let percent = (m.done * 100).checked_div(m.total).unwrap_or(0).min(100);
    html! {
        tr {
            td class="name" { a href={ "/guests/" (m.vmid) } { span class="mono dim" { (m.vmid) } " " (name) } }
            td class="mono" { (m.peer) }
            td {
                (state_pill(m.state))
                @if m.state == MirrorState::Syncing { " " span class="mono dim" { "%" (percent) } }
                @if let Some(e) = &m.error { div class="dim" { (tfmt(e.clone())) } }
            }
            td class="mono dim" { (when(m.last_in_sync)) }
            @if p.admin { td class="act" {
                details class="set" {
                    summary class="btn sm sq" aria-label=(tfmt(format!("{} yansıtmasını kapatma seçenekleri", m.vmid))) { (icon("trash")) }
                    div class="set-panel" {
                        form class="danger-row" method="post" action="/cluster/mirror" {
                            input type="hidden" name="vmid" value=(m.vmid);
                            label class="check" { input type="checkbox" name="delete_copy" value="1"; (tx("Eşteki kopyayı da sil")) }
                            button class="btn sm danger" type="submit" name="op" value="off" { (tx("Kapat")) }
                        }
                    }
                }
            } }
        }
    }
}

fn store_row(p: &Page, s: &MirrorStore) -> Markup {
    html! {
        tr {
            td class="name" { span class="mono" { (s.origin) " / " (s.vmid) } " " (s.guest.name) }
            td {
                @if s.complete.is_none() { span class="pill warn" { (tx("ilk kopya tamamlanmadı")) } }
                @else if s.in_sync { span class="pill ok" { (tx("kaynakla eşit")) } }
                @else { span class="pill warn" { (tx("kaynak eşit bildirmiyor")) } }
            }
            td class="mono dim" { (when(s.last_in_sync)) }
            @if p.admin { td class="act" {
                @if s.complete.is_some() {
                    details class="set" {
                        summary class="btn sm" { (icon("restart")) (tx("Devral")) }
                        div class="set-panel" {
                            form class="danger-row" method="post" action="/cluster/mirror" {
                                input type="hidden" name="origin" value=(s.origin); input type="hidden" name="vmid" value=(s.vmid);
                                span { (tx("Kaynakta konuğun kapalı olduğundan emin olun. Kopya, son eşit görülen andaki haliyle kapalı bir konuk olur.")) }
                                button class="btn sm primary" type="submit" name="op" value="promote" { (tx("Kopyadan konuk oluştur")) }
                            }
                        }
                    }
                }
                details class="set" {
                    summary class="btn sm sq" aria-label=(tfmt(format!("{} / {} kopyasını silme", s.origin, s.vmid))) { (icon("trash")) }
                    div class="set-panel" {
                        form class="danger-row" method="post" action="/cluster/mirror" {
                            input type="hidden" name="origin" value=(s.origin); input type="hidden" name="vmid" value=(s.vmid);
                            span { (tx("Kopyanın diskleri silinir.")) }
                            button class="btn sm danger" type="submit" name="op" value="delete" { (icon("trash")) (tx("Sil")) }
                        }
                    }
                }
            } }
        }
    }
}

pub(super) fn mirror_cards(p: &Page, peers: &[crate::peers::PeerView]) -> Markup {
    let c = p.cluster;
    let can_add = p.admin && p.real && !peers.is_empty();
    let candidates: Vec<&Guest> = c.guests.iter().filter(|g| mirror_blocker(g).is_none() && c.mirror(g.vmid).is_none()).collect();
    html! {
        @if can_add || !c.mirrors.is_empty() {
            div class="card" {
                header {
                    h2 { (tx("Disk yansıtma")) }
                    (tip("Çalışan konuğun her disk yazması aynı anda eş sunucudaki kopyaya da gider. Çoğaltmadan farkı: kopya geride kalmaz."))
                    div class="end" { span class="note" { (c.mirrors.len()) " / " (redox_core::mirrors::MAX_MIRRORS) } }
                }
                @if c.mirrors.is_empty() {
                    p class="empty" { (tx("Yansıtılan disk yok.")) }
                } @else {
                    div class="tw" { table {
                        thead { tr { th { (tx("Konuk")) } th { (tx("Hedef")) } th { (tx("Durum")) } th { (tx("Son eşit an")) } @if p.admin { th { span class="sr" { (tx("İşlem")) } } } } }
                        tbody { @for m in &c.mirrors { (mirror_row(p, m)) } }
                    } }
                }
                @if can_add && !candidates.is_empty() {
                    form class="addrow" method="post" action="/cluster/mirror" {
                        div class="field" { label for="mir-vmid" { (tx("Konuk")) } select id="mir-vmid" name="vmid" { @for g in &candidates { option value=(g.vmid) { (g.vmid) " " (g.name) } } } }
                        div class="field" { label for="mir-peer" { (tx("Eş sunucu")) } select id="mir-peer" name="peer" { @for v in peers { option value=(v.name) { (v.name) } } } }
                        div {}
                        button class="btn" type="submit" name="op" value="add" { (icon("plus")) (tx("Yansıt")) }
                    }
                }
                footer { (tx("Her yazma iki sunucuda da bitmeden tamamlanmaz: disk hızı aradaki ağa bağlıdır. Eş yanıt vermezse yansıtma birkaç saniye içinde durdurulur ve konuk çalışmayı sürdürür; kopya o andan sonra geridedir.")) }
            }
        }
        @if !c.mirror_stores.is_empty() {
            div class="card" {
                header { h2 { (tx("Tutulan kopyalar")) } (tip("Eş sunuculardaki konukların burada tutulan disk kopyaları.")) }
                div class="tw" { table {
                    thead { tr { th { (tx("Kaynak")) } th { (tx("Durum")) } th { (tx("Son eşit an")) } @if p.admin { th { span class="sr" { (tx("İşlem")) } } } } }
                    tbody { @for s in &c.mirror_stores { (store_row(p, s)) } }
                } }
                footer { (tx("Devralma elle yapılır: bu sunucu, kaynağın kapandığını aradaki bağlantının kopmasından ayıramaz. Aynı konuğu iki yerde açmak veriyi bozar.")) }
            }
        }
    }
}
