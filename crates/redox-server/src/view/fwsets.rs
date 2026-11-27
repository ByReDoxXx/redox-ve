//! Küme sayfasındaki güvenlik grupları ve adres kümeleri kartı.

use super::*;
use redox_core::hw::FwRule;

fn lines<'a>(rules: &'a [FwRule], outbound: bool) -> impl Iterator<Item = String> + 'a {
    rules.iter().map(move |r| if outbound { r.outbound() } else { r.to_string() })
}

fn remove_button(kind: &str, name: &str) -> Markup {
    html! {
        form method="post" action="/cluster/firewall" {
            input type="hidden" name="name" value=(name);
            button class="btn sm sq danger" type="submit" name="op" value={ "remove-" (kind) } aria-label=(tfmt(format!("{name} tanımını sil"))) { (icon("trash")) }
        }
    }
}

pub(super) fn fw_card(p: &Page) -> Markup {
    let c = p.cluster;
    if c.ip_sets.is_empty() && c.fw_groups.is_empty() && !p.admin {
        return html! {};
    }
    html! {
        div class="card" {
            header {
                h2 { (tx("Güvenlik grupları ve adres kümeleri")) }
                (tip("Bir kez tanımlayıp birçok konukta kullandığınız güvenlik duvarı kuralları ve adres listeleri."))
                div class="end" { span class="note" { (c.fw_groups.len()) " + " (c.ip_sets.len()) } }
            }
            @if c.ip_sets.is_empty() && c.fw_groups.is_empty() {
                p class="empty" { (tx("Tanım yok.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("Ad")) } th { (tx("İçerik")) } th { (tx("Kullanan")) } @if p.admin { th { span class="sr" { (tx("İşlem")) } } } } }
                    tbody {
                        @for s in &c.ip_sets {
                            @let (guests, groups) = c.ip_set_users(&s.name);
                            tr {
                                td class="name mono" { "@" (s.name) }
                                td class="mono dim" { (s.entries.join(", ")) }
                                td class="mono" { @for vmid in &guests { a href={ "/guests/" (vmid) } { (vmid) } " " } (groups.join(", ")) @if guests.is_empty() && groups.is_empty() { span class="dim" { "-" } } }
                                @if p.admin { td class="act" { (remove_button("set", &s.name)) } }
                            }
                        }
                        @for g in &c.fw_groups {
                            @let users = c.fw_group_users(&g.name);
                            tr {
                                td class="name mono" { (g.name) }
                                td class="mono dim" {
                                    @if !g.rules.is_empty() { div { (tx("gelen")) ": " (lines(&g.rules, false).collect::<Vec<_>>().join(", ")) } }
                                    @if !g.out.is_empty() { div { (tx("giden")) ": " (lines(&g.out, true).collect::<Vec<_>>().join(", ")) } }
                                }
                                td class="mono" { @for vmid in &users { a href={ "/guests/" (vmid) } { (vmid) } " " } @if users.is_empty() { span class="dim" { "-" } } }
                                @if p.admin { td class="act" { (remove_button("group", &g.name)) } }
                            }
                        }
                    }
                } }
            }
            @if p.admin {
                form class="addrow vds" method="post" action="/cluster/firewall" {
                    div class="field" { label for="set-name" { (tx("Küme adı (adres)")) } input id="set-name" name="name" type="text" placeholder="ofis" required spellcheck="false"; }
                    div class="field wide" { label for="set-entries" { (tx("Adresler (virgülle)")) } input id="set-entries" name="entries" type="text" placeholder="10.0.0.0/24, 192.168.1.5" required spellcheck="false"; }
                    button class="btn" type="submit" name="op" value="save-set" { (icon("plus")) (tx("Kümeyi kaydet")) }
                }
                form class="addrow" method="post" action="/cluster/firewall" {
                    div class="field" { label for="grp-name" { (tx("Grup adı")) } input id="grp-name" name="name" type="text" placeholder="web" required spellcheck="false"; }
                    div class="field" { label for="grp-rules" { (tx("Gelen kurallar")) } input id="grp-rules" name="rules" type="text" placeholder="tcp/443, tcp/22 from @ofis" spellcheck="false"; }
                    div class="field" { label for="grp-out" { (tx("Giden kurallar")) } input id="grp-out" name="out" type="text" placeholder="udp/53" spellcheck="false"; }
                    button class="btn" type="submit" name="op" value="save-group" { (icon("plus")) (tx("Grubu kaydet")) }
                }
            }
            footer { (tx("Aynı adla kaydetmek tanımı değiştirir ve kullanan konukların kuralları hemen yeniden yüklenir. Tanımlar eş sunuculara kendiliğinden gönderilmez.")) }
        }
    }
}
