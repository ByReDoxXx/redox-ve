//! Çoğaltma sekmesi ve onay sayfası.

use super::*;

/// Çoğalt ve Taşı sekmesi: klon, şablon ve dışa aktarma (yalnızca yönetici).
pub fn clone_tab(p: &Page, g: &Guest) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let stopped = g.status == GuestStatus::Stopped && g.lock.is_none();
    // Gerçek konteynerde yalnızca klon vardır: şablon, bağlı klon ve OVA sanal makinelere özgüdür.
    if !crate::view::disk_ops(p, g) {
        return html! {
            @if !stopped {
                div class="notice" role="status" { (icon("alert")) span { (tx("Klonlamak için önce konteyneri kapatın.")) } }
            }
            div class="card" {
                header { h2 { (tx("Klonla")) } }
                div class="op" {
                    div {
                        h3 { (tx("Bu konteynerin kopyası")) }
                        p { (tx("Kök dosya sistemi ve ayarlar kopyalanır; kopya kendi kimliğini, adını ve ağ adresini alır. Anlık görüntüler, yedek zamanlaması ve port yönlendirmeleri kopyalanmaz.")) }
                    }
                    form method="post" action={ (base) "/clone" } {
                        input id="clone" name="name" type="text" value={ (g.name) "-kopya" } autocomplete="off" aria-label=(tx("Yeni konteynerin adı")) disabled[!stopped];
                        button class="btn primary" type="submit" disabled[!stopped] { (icon("plus")) (tx("Klonla")) }
                    }
                }
            }
        };
    }
    let clones: Vec<&Guest> = p.cluster.guests.iter().filter(|c| c.hw.base == Some(g.vmid)).collect();
    html! {
        @if !stopped {
            div class="notice" role="status" { (icon("alert")) span { (tx("Klonlamak, şablona çevirmek ve dışa aktarmak için önce konuğu kapatın.")) } }
        }
        div class="stack" {
            div class="card" {
                header { h2 { (tx("Klonla")) } }
                div class="op" {
                    div {
                        h3 { @if g.hw.template { (tx("Bu şablondan yeni konuk")) } @else { (tx("Bu konuğun kopyası")) } }
                        p {
                            (tx("Disk ve ayarlar kopyalanır; anlık görüntüler ve port yönlendirmeleri kopyalanmaz."))
                            @if g.hw.template { (tx(" Bağlı klon diski kopyalamaz: saniyeler içinde hazır olur, yalnızca şablondan farkını tutar ve şablona bağlı kalır.")) }
                        }
                    }
                    form method="post" action={ (base) "/clone" } {
                        input id="clone" name="name" type="text" value={ (g.name) "-kopya" } autocomplete="off" aria-label=(tx("Yeni konuğun adı")) disabled[!stopped];
                        @if g.hw.template {
                            label class="check" { input type="checkbox" name="linked" value="1" checked disabled[!stopped]; (tx(" Bağlı klon")) }
                        }
                        button class="btn primary" type="submit" disabled[!stopped] { (icon("plus")) (tx("Klonla")) }
                    }
                }
            }
            div class="card" {
                header { h2 { (tx("Şablon")) } }
                div class="op" {
                    div {
                        @if g.hw.template {
                            h3 { (tx("Bu konuk bir şablon")) }
                            p { (tx("Bu konuk bir şablon: başlatılamaz, diski değişmez. Bağlı klonları ona dayandığı sürece silinemez ve şablonluktan çıkarılamaz.")) }
                        } @else {
                            h3 { (tx("Şablona çevir")) }
                            p { (tx("Şablona çevrilen konuk başlatılamaz; ondan hızlı (bağlı) klonlar üretilir.")) }
                            @if let Some(b) = g.hw.base { p { (tx("Bu konuk ")) a class="link" href={ "/guests/" (b) } { (b) } (tx(" şablonunun bağlı klonu: diski o şablona dayanır.")) } }
                        }
                    }
                    form method="post" action={ (base) "/template" } {
                        @if g.hw.template {
                            input type="hidden" name="on" value="0";
                            @if !clones.is_empty() { span class="pill warn" { (icon("alert")) (clones.len()) (tx(" bağlı klon")) } }
                            button class="btn" type="submit" disabled[!stopped] { (tx("Şablonluktan Çıkar")) }
                        } @else {
                            input type="hidden" name="on" value="1";
                            button class="btn" type="submit" disabled[!stopped || g.hw.base.is_some()] { (tx("Şablona Çevir")) }
                        }
                    }
                }
                @if !clones.is_empty() {
                    div class="tw" { table {
                        thead { tr { th { (tx("Bağlı klon")) } th { (tx("Durum")) } th class="num" { "Disk" } } }
                        tbody { @for c in &clones { tr {
                            td class="name" { (guest_name(c, "/guests")) }
                            td { (guest_state(c)) }
                            td class="num mono" { (c.disk_gb) " GB" }
                        } } }
                    } }
                }
            }
            @if let Some(backup) = &g.hw.instant {
                div class="card" {
                    header { h2 { (tx("Yedeğin üzerinde çalışıyor")) } span class="note" { (tx("anında kurtarma · ")) span class="mono" { (backup) } } }
                    div class="op" {
                        div {
                            h3 { (tx("Yedekten ayır")) }
                            p {
                                (tx("Bu konuğun diskleri ")) span class="mono" { (backup) } (tx(" yedeğine dayanan ince katmanlardır: konuk yalnızca değişiklikleri tutar, geri kalanını yedekten okur. "))
                                (tx("Ayırınca yedekteki veri disklere kopyalanır ve konuk bağımsız olur; konuk çalışırken de yapılır. O zamana kadar yedek silinemez ve budanmaz."))
                            }
                        }
                        form method="post" action={ (base) "/detach" } { button class="btn primary" type="submit" disabled[g.lock.is_some()] { (icon("check")) (tx("Yedekten Ayır")) } }
                    }
                }
            }
            (storage_card(p, g, stopped))
            div class="card" {
                header { h2 { (tx("Dışa aktar ve taşı")) } }
                @if p.real {
                    div class="op" {
                        div {
                            h3 { (tx("OVA olarak indir")) }
                            p { (tx("Konuğun ayarları ve diskleri (ek diskler dahil) tek bir OVA dosyasına konur; VMware ve VirtualBox gibi araçların içe aktardığı biçimdir. Disk önce dönüştürülür: indirme büyük disklerde geç başlar. Konuk kapalı olmalıdır.")) }
                        }
                        @if stopped { a class="btn" href={ (base) "/export" } download { (icon("disk")) (tx("OVA Olarak İndir")) } }
                        @else { span class="btn" aria-disabled="true" { (icon("disk")) (tx("OVA Olarak İndir")) } }
                    }
                }
                div class="op" {
                    div {
                        h3 { (tx("Başka bir RedoX sunucusuna kopyala")) }
                        p { (tx("Hedef sunucunun Küme sayfasından bu konuğu seçip “Buraya Al” deyin. Kaynak konuk yerinde kalır.")) }
                    }
                    a class="btn" href="/cluster" { (tx("Küme sayfası")) }
                }
            }
        }
    }
}

/// Geri alınamayan işlemler için onay sayfası.
pub struct Confirm<'a> {
    pub title: &'a str,
    pub message: Markup,
    pub action: String,
    pub button: &'a str,
    pub back: String,
    pub cli: String,
}

pub fn confirm_page(p: &Page, g: &Guest, c: &Confirm) -> String {
    let label = guest_label(g);
    let href = format!("/guests/{}", g.vmid);
    let body = html! {
        (page_head(html! { (c.title) }, html! {}, html! {}, &c.cli, &[]))
        div class="card" { div class="body" {
            p class="lead" { (c.message) }
            form class="btns" method="post" action=(c.action) {
                button class="btn danger solid" type="submit" { (c.button) }
                a class="btn" href=(c.back) { (tx("Vazgeç")) }
            }
        } }
    };
    layout(p, c.title, &[((tx("Konuklar")), "/guests"), (&label, &href), ((tx("Onay")), "")], true, body)
}
