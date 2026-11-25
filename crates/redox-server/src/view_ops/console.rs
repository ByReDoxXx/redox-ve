//! Konsol sekmesi, canlı konsol sayfası ve komut çalıştırma.

use super::*;

pub(super) fn stopped_note(g: &Guest, what: &str) -> Markup {
    html! {
        @if g.status != GuestStatus::Stopped {
            p { (what) (tx(" için önce konuğu kapatın.")) }
        }
    }
}

/// Konteynerde çalıştırılan komutun çıktısı (ve yeni komut için form).
pub fn exec_page(p: &Page, g: &Guest, command: &str, result: &Result<String, String>) -> String {
    let base = format!("/guests/{}", g.vmid);
    let cli = format!("{} exec {}", g.kind, g.vmid);
    let label = guest_label(g);
    let body = html! {
        (page_head(html! { span class="id" { (g.vmid) } (g.name) }, html! {}, html! {}, &cli, &[]))
        div class="card" {
            header {
                h2 { (tx("Komut çıktısı")) }
                @match result {
                    Ok(_) => span class="pill ok" { (icon("check")) (tx("Çalıştı")) },
                    Err(_) => span class="pill err" { (icon("alert")) (tx("Başarısız")) },
                }
            }
            div class="body" {
                pre class="conf" { span class="gold" { "#" } " " (command) }
                pre class="conf output" { @match result { Ok(out) => (out), Err(e) => (e) } }
                (exec_form(&base, command))
                div class="btns" { a class="btn" href={ (base) "?tab=konsol" } { (tx("Konsola Dön")) } }
            }
        }
    };
    layout(p, &tfmt(format!("{} komut", g.vmid)), &[((tx("Konuklar")), "/guests"), (&label, &base), ((tx("Komut")), "")], false, body)
}

pub(super) fn exec_form(base: &str, command: &str) -> Markup {
    html! {
        form class="keys" method="post" action={ (base) "/exec" } {
            label class="sr" for="command" { (tx("Çalıştırılacak komut")) }
            input id="command" name="command" type="text" value=(command) autocomplete="off" autocapitalize="none" spellcheck="false"
                placeholder=(tx("örn. ip addr, apk update, systemctl status ssh")) required autofocus;
            button class="btn primary" type="submit" { (tx("Çalıştır")) }
        }
    }
}

pub(super) fn container_console(p: &Page, g: &Guest) -> Markup {
    html! {
        div class="card" {
            header { h2 { (tx("Komut çalıştır")) } (tip("Konteynerin içinde bir komut çalıştırır ve çıktısını gösterir.")) span class="note" { (tx("konteynerde kök olarak")) } }
            @if !p.real {
                p class="empty" { (tx("Simülasyon sürücüsünde konteynere komut gönderilemez.")) }
            } @else if g.status != GuestStatus::Running {
                p class="empty" { (tx("Konteyner çalışmıyor. Komut çalıştırmak için başlatın.")) }
            } @else {
                div class="console" {
                    (exec_form(&format!("/guests/{}", g.vmid), ""))
                    p class="hint" { (tx("Komut ")) span class="mono" { "/bin/sh -c" } (tx(" ile, en çok 20 saniye çalışır ve çıktısı gösterilir. Etkileşimli programlar (vi, top) çalışmaz. Komut metni günlüğe yazılmaz.")) }
                }
            }
        }
    }
}

/// `live`: konuğun canlı konsolu (VNC) hazır mı?
pub fn console_tab(p: &Page, g: &Guest, live: bool) -> Markup {
    if g.kind == GuestKind::Ct {
        return container_console(p, g);
    }
    let base = format!("/guests/{}", g.vmid);
    html! {
        div class="card" {
            header {
                h2 { (tx("Konsol")) }
                span class="note" { (tx("ekran 2 saniyede bir yenilenir")) }
                @if live { div class="end" { a class="btn primary sm" href={ (base) "/console" } { (icon("terminal")) (tx("Canlı Konsolu Aç")) } } }
            }
            @if !p.real {
                p class="empty" { (tx("Simülasyon sürücüsünde konsol yok. Gerçek ekran için sunucuyu ")) span class="mono" { "--driver qemu" } (tx(" ile çalıştırın.")) }
            } @else if g.status != GuestStatus::Running {
                p class="empty" { (tx("Konuk çalışmıyor. Ekranı görmek için başlatın.")) }
            } @else {
                div class="console" {
                    iframe class="screen" src={ (base) "/screen" } title={ (g.name) (tx(" ekranı")) } {}
                    form class="keys" method="post" action={ (base) "/keys" } {
                        label class="sr" for="text" { (tx("Gönderilecek metin")) }
                        input id="text" name="text" type="text" autocomplete="off" autocapitalize="none" spellcheck="false"
                            placeholder=(tx("yazılacak metin, örn. root")) autofocus;
                        label class="check" { input type="checkbox" name="enter" value="1" checked; (tx("Enter ile bitir")) }
                        button class="btn primary" type="submit" { (tx("Gönder")) }
                    }
                    form class="keys" method="post" action={ (base) "/keys" } aria-label=(tx("Özel tuşlar")) {
                        @for (value, label) in SPECIAL_KEYS {
                            button class="btn sm" name="key" value=(value) { (tx(label)) }
                        }
                    }
                    p class="hint" {
                        (tx("Metin ABD klavye düzeniyle gönderilir; Türkçe harfler gönderilemez. Fare desteklenmez. "))
                        @if live { (tx("Gerçek zamanlı ekran, fare ve kendi klavye düzeniniz için ")) a class="link" href={ (base) "/console" } { (tx("canlı konsolu")) } (tx(" açın (o sayfa JavaScript kullanır).")) }
                        @else { (tx("Canlı konsol bu konukta hazır değil: konuk o özellikten önce açılmışsa bir kez kapatıp açın.")) }
                    }
                }
            }
        }
    }
}

/// Canlı konsol sayfası: uygulamada betik yükleyen tek sayfa. Kabuk (sol menü, otomatik yenileme) yoktur;
/// sayfa yenilenirse oturum kopacağı için kendi kendini yenilemez.
pub fn live_console_page(p: &Page, g: &Guest, available: bool) -> String {
    let base = format!("/guests/{}", g.vmid);
    html! {
        (DOCTYPE)
        html lang="tr" data-theme=(p.theme) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (g.vmid) " " (g.name) (tx(" · Konsol · RedoX VE")) }
                link rel="icon" type="image/svg+xml" href=(crate::view::asset("logo.svg"));
                link rel="stylesheet" href=(crate::view::asset("fonts/fonts.css"));
                link rel="stylesheet" href=(crate::view::asset("style.css"));
            }
            body class="console-page" {
                header class="top" {
                    a class="btn sm" href={ (base) "?tab=konsol" } { (tx("← Konuğa dön")) }
                    strong { span class="mono gold" { (g.vmid) } " " (g.name) }
                    @if available {
                        span id="console-status" class="pill idle" role="status" { (tx("Betik bekleniyor…")) }
                        div class="end" {
                            label class="sr" for="console-text" { (tx("Konuğa yazdırılacak metin")) }
                            span class="joined" {
                                input id="console-text" type="text" autocomplete="off" spellcheck="false" placeholder=(tx("Konuğa yazdırılacak metin"));
                                button id="console-type" class="btn sm" type="button" data-skipped=(tx("karakter yazılamadı")) { (crate::view::icon("prompt")) (tx("Yazdır")) }
                            }
                            (crate::view::tip("Metni konuğa tuş vuruşları olarak gönderir. Pano yerine geçer; konukta ajan gerekmez. Yalnızca ABD klavyesindeki karakterler yazılır; konuğun klavye düzeni ABD İngilizcesi olmalıdır."))
                            span class="vsep" aria-hidden="true" {}
                            span class="group" {
                                button id="console-cad" class="btn sm" type="button" { "Ctrl+Alt+Del" }
                                button id="console-full" class="btn sm" type="button" { (tx("Tam ekran")) }
                                button id="console-blur" class="btn sm" type="button" title=(tx("Klavye odağını konuktan geri alır")) { (tx("Odağı bırak")) }
                            }
                        }
                    }
                }
                main id="main" {
                    @if available {
                        canvas id="screen" tabindex="0" width="640" height="400" data-ws={ (base) "/vnc" }
                            aria-label={ (g.name) (tx(" konuğunun ekranı; tıklayınca klavye ve fare konuğa gider")) } {}
                        noscript { p class="empty" { (tx("Canlı konsol JavaScript gerektirir. JavaScript'siz ekran için ")) a class="link" href={ (base) "?tab=konsol" } { (tx("konsol sekmesini")) } (tx(" kullanın.")) } }
                        p class="hint" { (tx("Ekrana tıklayınca klavye ve fare konuğa gider; tuşlar konuğun kendi klavye düzeniyle yorumlanır. Bu sayfa dışında uygulamada betik çalışmaz.")) }
                    } @else {
                        p class="empty" {
                            @if g.status != GuestStatus::Running { (tx("Konuk çalışmıyor. Ekranı görmek için başlatın.")) }
                            @else { (tx("Bu konukta canlı konsol hazır değil: konuk o özellikten önce açılmışsa bir kez kapatıp açın.")) }
                            " " a class="link" href={ (base) "?tab=konsol" } { (tx("Konuğa dön")) } "."
                        }
                    }
                }
                @if available { script src=(crate::view::asset("console.js")) defer {} }
            }
        }
    }
    .into_string()
}

/// Konsol sekmesindeki çerçevenin içeriği: yalnızca ekran görüntüsü, kendini yeniler.
/// Ana sayfa yenilenmediği için yazı kutusundaki metin kaybolmaz.
pub fn console_frame(vmid: u32, now: u64) -> String {
    html! {
        (DOCTYPE)
        html lang="tr" {
            head {
                meta charset="utf-8";
                meta http-equiv="refresh" content="2";
                title { (tx("Ekran")) }
                (PreEscaped("<style>html,body{margin:0;background:#000;height:100%}img{display:block;max-width:100%;max-height:100vh;margin:0 auto;color:#bbb;font:14px sans-serif}</style>"))
            }
            body {
                img src={ "/guests/" (vmid) "/screen.png?t=" (now) } alt=(tx("Ekran görüntüsü alınamadı. Konuk kapalı olabilir."));
            }
        }
    }
    .into_string()
}
