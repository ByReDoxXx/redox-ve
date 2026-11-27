//! Yardım, giriş ve hata sayfaları.

use super::*;

pub fn help(p: &Page) -> String {
    let rows: [(&str, &str); 6] = [
        ((tx("start | stop | reboot | pause | resume <id ya da ad>")), (tx("Konuğa eylem uygular. Ad tam yazılmalıdır."))),
        ((tx("open <id | ad | düğüm>")), (tx("Konuğun ya da düğümün sayfasını açar; adın bir parçası tek konukla eşleşiyorsa yeter."))),
        ((tx("<kimlik ya da ad>")), (tx("Doğrudan yazarsanız eşleşen kaynağı açar; eşleşme yoksa konukları filtreler."))),
        ("storage", (tx("Depolama listesini açar."))),
        ("home", (tx("Genel bakışa döner."))),
        ("help", (tx("Bu sayfayı açar."))),
    ];
    let body = html! {
        (page_head(html! { (tx("Komut Satırı")) }, html! { span { (tx("Üstteki alana ")) kbd { "Alt K" } (tx(" ile odaklanın.")) } }, html! {}, "", &[]))
        div class="stack" {
            div class="card" {
                header { h2 { (tx("Üstteki komut satırı")) } }
                div class="tw" { table { tbody {
                    @for (cmd, what) in rows { tr { td class="mono gold" { (cmd) } td class="wrap" { (what) } } }
                } } }
            }
            div class="card" {
                header { h2 { (tx("Terminalden: redox")) } }
                div class="body" { pre class="conf" {
                    span class="gold" { "$" } " redox cluster status\n"
                    span class="gold" { "$" } " redox vm list\n"
                    span class="gold" { "$" } " redox vm start 103\n"
                    span class="gold" { "$" } " redox storage list\n"
                } }
            }
        }
    };
    layout(p, (tx("Komut Satırı")), &[((tx("Komut Satırı")), "/help")], true, body)
}

/// Giriş sayfası: uygulama kabuğu olmadan, tek başına. Oturum açılmadan sunucu bilgisi gösterilmez.
pub fn login(theme: &str, next: &str, username: &str, error: Option<&str>, sso: bool) -> String {
    html! {
        (DOCTYPE)
        html lang=(crate::i18n::current().code()) data-theme=(theme) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (tx("Giriş · RedoX VE")) }
                link rel="icon" type="image/svg+xml" href=(asset("logo.svg"));
                link rel="stylesheet" href=(asset("fonts/fonts.css"));
                link rel="stylesheet" href=(asset("style.css"));
            }
            body {
                main class="login" {
                    section class="hero" {
                        div class="lock" role="img" aria-label="RedoX VE" {
                            img class="mark" src=(asset("logo.svg")) alt="" width="93" height="96";
                            img class="word" src=(asset("wordmark.svg")) alt="" width="311" height="110";
                        }
                        div {
                            h2 { (tx("Sunucunuz, sizin kurallarınızla.")) }
                            p { (tx("Sanal makineler ve konteynerler için tek dosyalık yönetim paneli.")) }
                        }
                        p class="hint" { (tx("Hesaplar sunucuda ")) span class="mono" { "redox user add" } (tx(" ile oluşturulur.")) }
                    }
                    section class="side" {
                        form method="post" action="/login" {
                            h1 { (tx("Giriş")) }
                            @if let Some(e) = error {
                                p class="errsum" role="alert" { (crate::i18n::message(e)) }
                            }
                            input type="hidden" name="next" value=(next);
                            div class="field" {
                                label for="username" { (tx("Kullanıcı adı")) }
                                input id="username" name="username" type="text" value=(username) autocomplete="username"
                                    autocapitalize="none" spellcheck="false" required autofocus[username.is_empty()];
                            }
                            div class="field" {
                                label for="password" { (tx("Parola")) }
                                input id="password" name="password" type="password" autocomplete="current-password" required autofocus[!username.is_empty()];
                            }
                            div class="field" {
                                label for="code" { (tx("Doğrulama kodu")) }
                                input id="code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code" maxlength="6" pattern="[0-9]{6}";
                                span class="hint" tabindex="0" role="note" { (tx("Yalnızca iki adımlı doğrulamayı açtıysanız.")) }
                            }
                            button class="btn primary" type="submit" { (tx("Giriş Yap")) }
                            @let other = crate::i18n::current().other();
                            p class="hint" { a href={ "/lang?to=" (other.code()) "&back=/login" } lang=(other.code()) { @if other == crate::i18n::Lang::En { "English" } @else { "Türkçe" } } }
                            @if sso {
                                a class="btn" href="/login/oidc" { (icon("shield")) (tx("Kurumsal hesapla giriş yap")) }
                            }
                        }
                    }
                }
            }
        }
    }
    .into_string()
}

/// Tek oturum açma tamamlanınca gösterilen ara sayfa: hemen ana sayfaya geçer.
pub fn signed_in(theme: &str) -> String {
    html! {
        (DOCTYPE)
        html lang=(crate::i18n::current().code()) data-theme=(theme) {
            head {
                meta charset="utf-8";
                meta http-equiv="refresh" content="0;url=/";
                title { (tx("Giriş yapıldı · RedoX VE")) }
                link rel="stylesheet" href=(asset("fonts/fonts.css"));
                link rel="stylesheet" href=(asset("style.css"));
            }
            body { main class="signed" { p { (tx("Giriş yapıldı. ")) a class="link" href="/" { (tx("Panele geçin")) } "." } } }
        }
    }
    .into_string()
}

pub fn error_page(p: &Page, title: &str, message: &str) -> String {
    let body = html! {
        (page_head(html! { (crate::i18n::message(title)) }, html! {}, html! {}, "", &[]))
        div class="card" { div class="body" {
            p class="lead" { (crate::i18n::message(message)) }
            div class="btns" { a class="btn" href="/" { (icon("datacenter")) (tx("Genel Bakışa Dön")) } }
        } }
    };
    layout(p, title, &[(title, "")], true, body)
}
