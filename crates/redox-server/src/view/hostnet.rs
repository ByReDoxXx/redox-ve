//! Düğüm sayfasındaki sunucu ağı kartı.

use super::*;
use crate::state::hostnet::HostNetReport;
use redox_core::hostnet::{BondMode, HostNet};

fn details(net: &HostNet) -> String {
    match net {
        HostNet::Bond { mode, ports, .. } => format!("{} · {}", mode.as_str(), ports.join(", ")),
        HostNet::Vlan { parent, id, .. } => format!("{parent} · VLAN {id}"),
        HostNet::Bridge { ports, vlan_aware, .. } => {
            let ports = if ports.is_empty() { "-".to_string() } else { ports.join(", ") };
            if *vlan_aware { format!("{ports} · VLAN") } else { ports }
        }
        HostNet::Guest { address, dhcp_start, dhcp_end, nat, .. } => {
            let dhcp = match (dhcp_start, dhcp_end) {
                (Some(a), Some(b)) => format!(" · DHCP {a}–{b}"),
                _ => String::new(),
            };
            format!("{address}{dhcp}{}", if *nat { " · NAT" } else { "" })
        }
    }
}

fn add_forms() -> Markup {
    html! {
        details class="set" {
            summary class="btn sm quiet" { (icon("plus")) (tx("Konuk ağı ekle")) }
            form class="addrow vds" method="post" action="/host/networks" {
                input type="hidden" name="kind" value="guest";
                div class="field" { label for="hn-g-name" { (tx("Ad")) } input id="hn-g-name" name="name" type="text" placeholder="vnet1" required spellcheck="false"; }
                div class="field" { label for="hn-g-address" { (tx("Adres/ön ek")) } input id="hn-g-address" name="address" type="text" placeholder="10.10.0.1/24" required spellcheck="false"; }
                div class="field" { label for="hn-g-dhcp" { (tx("DHCP aralığı")) } input id="hn-g-dhcp" name="dhcp" type="text" placeholder="10.10.0.50-10.10.0.200" spellcheck="false"; }
                label class="check" { input type="checkbox" name="nat" value="1" checked; "NAT" }
                button class="btn" type="submit" name="op" value="save" { (icon("plus")) (tx("Kaydet")) }
            }
        }
        details class="set" {
            summary class="btn sm quiet" { (icon("plus")) (tx("Köprü, bond ya da VLAN ekle")) }
            form class="addrow" method="post" action="/host/networks" {
                input type="hidden" name="kind" value="bridge";
                div class="field" { label for="hn-b-name" { (tx("Köprü adı")) } input id="hn-b-name" name="name" type="text" placeholder="vmbr1" required spellcheck="false"; }
                div class="field" { label for="hn-b-ports" { (tx("Arayüzler (virgülle)")) } input id="hn-b-ports" name="ports" type="text" placeholder="enp2s0" spellcheck="false"; }
                label class="check" { input type="checkbox" name="vlan_aware" value="1"; (tx("VLAN etiketine izin ver")) }
                button class="btn" type="submit" name="op" value="save" { (icon("plus")) (tx("Köprüyü kaydet")) }
            }
            form class="addrow" method="post" action="/host/networks" {
                input type="hidden" name="kind" value="bond";
                div class="field" { label for="hn-o-name" { (tx("Bond adı")) } input id="hn-o-name" name="name" type="text" placeholder="bond0" required spellcheck="false"; }
                div class="field" { label for="hn-o-mode" { (tx("Kip")) } select id="hn-o-mode" name="mode" { @for m in BondMode::ALL { option value=(m.as_str()) { (m.as_str()) } } } }
                div class="field" { label for="hn-o-ports" { (tx("Arayüzler (virgülle)")) } input id="hn-o-ports" name="ports" type="text" placeholder="enp2s0, enp3s0" required spellcheck="false"; }
                button class="btn" type="submit" name="op" value="save" { (icon("plus")) (tx("Bond kaydet")) }
            }
            form class="addrow" method="post" action="/host/networks" {
                input type="hidden" name="kind" value="vlan";
                div class="field" { label for="hn-v-parent" { (tx("Üst arayüz")) } input id="hn-v-parent" name="parent" type="text" placeholder="bond0" required spellcheck="false"; }
                div class="field" { label for="hn-v-id" { (tx("VLAN etiketi")) } input id="hn-v-id" name="id" type="text" inputmode="numeric" placeholder="20" required; }
                div {}
                button class="btn" type="submit" name="op" value="save" { (icon("plus")) (tx("VLAN kaydet")) }
            }
        }
    }
}

pub(super) fn hostnet_card(p: &Page, nets: &[HostNetReport]) -> Markup {
    if nets.is_empty() && !p.admin {
        return html! {};
    }
    html! {
        div class="card" {
            header {
                h2 { (tx("Sunucu ağı")) }
                (tip("Konuklar için köprü ve DHCP'li ağlar, fiziksel kartlar için bond ve VLAN. Ağı kök yetkili yardımcı kurar."))
                div class="end" { span class="note" { (nets.len()) " / " (redox_core::hostnet::MAX_HOST_NETS) } }
            }
            @if nets.is_empty() {
                p class="empty" { (tx("Panelden tanımlanmış ağ yok.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("Ad")) } th { (tx("Tür")) } th { (tx("Ayar")) } th { (tx("Durum")) } th { (tx("Konuklar")) } @if p.admin { th { span class="sr" { (tx("İşlem")) } } } } }
                    tbody { @for r in nets { tr {
                        td class="name mono" { (r.net.name()) }
                        td { (tx(r.net.kind_label())) }
                        td class="mono dim" { (details(&r.net)) }
                        td { @match r.present { Some(true) => span class="pill ok" { (icon("check")) (tx("kurulu")) }, Some(false) => span class="pill warn" { (icon("alert")) (tx("bekliyor")) }, None => span class="pill idle" { (tx("benzetim")) } } }
                        td class="mono" { @if r.guests.is_empty() { span class="dim" { "-" } } @for vmid in &r.guests { a href={ "/guests/" (vmid) } { (vmid) } " " } }
                        @if p.admin { td class="act" {
                            form method="post" action="/host/networks" {
                                input type="hidden" name="name" value=(r.net.name());
                                button class="btn sm sq danger" type="submit" name="op" value="remove" aria-label=(tfmt(format!("{} ağını sil", r.net.name()))) { (icon("trash")) }
                            }
                        } }
                    } } }
                } }
            }
            @if p.admin { (add_forms()) }
            footer { (tx("Üzerinde adres ya da varsayılan yol olan arayüz hiçbir yere bağlanmaz ve RedoX VE'nin kurmadığı aygıtlara dokunulmaz; bu yüzden sunucunun yönetim kartı panelden köprüye taşınamaz. Ağı sunucuda redox-hostnet.service kurar; DHCP için dnsmasq kurulu olmalıdır.")) }
        }
    }
}
