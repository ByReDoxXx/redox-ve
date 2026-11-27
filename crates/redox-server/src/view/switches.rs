//! Küme sayfasındaki dağıtık anahtar kartı.

use super::*;
use crate::state::vswitch::{PeerState, SwitchReport};
use crate::vswitch::status::Local;

fn local_pill(local: Option<Local>) -> Markup {
    match local {
        None => html! { span class="pill idle" { (tx("benzetim")) } },
        Some(Local::Ready) => html! { span class="pill ok" { (icon("check")) (tx(Local::Ready.label())) } },
        Some(Local::Missing) => html! { span class="pill warn" { (icon("alert")) (tx(Local::Missing.label())) } },
        Some(other) => html! { span class="pill err" { (icon("alert")) (tx(other.label())) } },
    }
}

fn peer_pill(name: &str, state: PeerState) -> Markup {
    let class = match state {
        PeerState::Same => "pill ok",
        PeerState::Unknown => "pill idle",
        PeerState::Different | PeerState::Missing => "pill warn",
    };
    html! { span class=(class) { (name) ": " (tx(state.label())) } }
}

pub(super) fn switch_card(p: &Page, switches: &[SwitchReport], has_peers: bool) -> Markup {
    // Eşi olmayan, anahtarı da tanımlı olmayan tek sunucuda yönetici olmayana gösterilecek bir şey yoktur.
    if switches.is_empty() && !(p.admin && has_peers) {
        return html! {};
    }
    html! {
        div class="card" {
            header {
                h2 { (tx("Dağıtık anahtarlar")) }
                (tip("Bütün sunucularda aynı adla bulunan ortak konuk ağı. Bir kez tanımlayıp eşlere gönderirsiniz; konuklarda ağ olarak bu adı seçersiniz."))
                div class="end" { span class="note" { (switches.len()) " / " (redox_core::vswitch::MAX_SWITCHES) } }
            }
            @if switches.is_empty() {
                p class="empty" { (tx("Dağıtık anahtar yok. Farklı sunuculardaki konukları aynı ağda buluşturmak için bir tane tanımlayın.")) }
            } @else {
                div class="tw" { table {
                    thead { tr {
                        th { (tx("Ad")) } th { "VNI / MTU" } th { (tx("Üye adresleri")) } th { (tx("Bu sunucu")) } th { (tx("Eşler")) } th { (tx("Konuklar")) }
                        @if p.admin { th { span class="sr" { (tx("İşlem")) } } }
                    } }
                    tbody { @for r in switches { tr {
                        td class="name mono" { (r.switch.name) }
                        td class="mono" { (r.switch.vni) " / " (r.switch.mtu) }
                        td class="mono dim" { @for member in &r.switch.members { div { (member) } } }
                        td { (local_pill(r.local)) }
                        td { @if r.peers.is_empty() { span class="dim" { "-" } } @for (name, state) in &r.peers { div { (peer_pill(name, *state)) } } }
                        td class="mono" { @if r.guests.is_empty() { span class="dim" { "-" } } @for vmid in &r.guests { a href={ "/guests/" (vmid) } { (vmid) } " " } }
                        @if p.admin { td class="act" {
                            @if has_peers {
                                form method="post" action="/cluster/switch" {
                                    input type="hidden" name="name" value=(r.switch.name);
                                    button class="btn sm" type="submit" name="op" value="push" { (icon("migrate")) (tx("Eşlere gönder")) }
                                }
                            }
                            details class="set" {
                                summary class="btn sm sq" aria-label=(tfmt(format!("{} silme seçenekleri", r.switch.name))) { (icon("trash")) }
                                div class="set-panel" {
                                    form class="danger-row" method="post" action="/cluster/switch" {
                                        input type="hidden" name="name" value=(r.switch.name);
                                        @if has_peers { label class="check" { input type="checkbox" name="everywhere" value="1"; (tx("Eşlerden de sil")) } }
                                        button class="btn sm danger" type="submit" name="op" value="remove" { (icon("trash")) (tx("Sil")) }
                                    }
                                }
                            }
                        } }
                    } } }
                } }
            }
            @if p.admin {
                form class="addrow vds" method="post" action="/cluster/switch" {
                    div class="field" { label for="vds-name" { (tx("Ad")) } input id="vds-name" name="name" type="text" placeholder="vds-web" required spellcheck="false"; }
                    div class="field" { label for="vds-vni" { "VNI" } input id="vds-vni" name="vni" type="text" inputmode="numeric" placeholder="100" required; }
                    div class="field" { label for="vds-mtu" { "MTU" } input id="vds-mtu" name="mtu" type="text" inputmode="numeric" value=(redox_core::vswitch::DEFAULT_MTU) required; }
                    div class="field" { label for="vds-members" { (tx("Üye adresleri (virgülle)")) } input id="vds-members" name="members" type="text" placeholder="10.0.0.1, 10.0.0.2" required spellcheck="false"; }
                    button class="btn" type="submit" name="op" value="save" { (icon("plus")) (tx("Kaydet")) }
                }
            }
            footer { (tx("Üye adreslerine bütün sunucuların birbirine ulaştığı adresleri yazın (bu sunucununki dahil). Ağı her sunucuda redox-net.service kurar. Tünel trafiği şifrelenmez (UDP 4789): yalnızca güvendiğiniz bir ağda kullanın.")) }
        }
    }
}
