//! Yardım, giriş ve hata sayfaları.

use super::*;

pub fn help(p: &Page) -> String {
    let rows: [(&str, &str); 6] = [
        ("start | stop | reboot | pause | resume <id ya da ad>", "Konuğa eylem uygular. Ad tam yazılmalıdır."),
        ("open <id | ad | düğüm>", "Konuğun ya da düğümün sayfasını açar; adın bir parçası tek konukla eşleşiyorsa yeter."),
        ("<kimlik ya da ad>", "Doğrudan yazarsanız eşleşen kaynağı açar; eşleşme yoksa konukları filtreler."),
        ("storage", "Depolama listesini açar."),
        ("home", "Genel bakışa döner."),
        ("help", "Bu sayfayı açar."),
    ];
    let body = html! {
        (page_head(html! { "Komut Satırı" }, html! { span { "Üstteki alana " kbd { "Alt K" } " ile odaklanın." } }, html! {}, "", &[]))
        div class="stack" {
            div class="card" {
                header { h2 { "Üstteki komut satırı" } }
                div class="tw" { table { tbody {
                    @for (cmd, what) in rows { tr { td class="mono gold" { (cmd) } td class="wrap" { (what) } } }
                } } }
            }
            div class="card" {
                header { h2 { "Terminalden: redox" } }
                div class="body" { pre class="conf" {
                    span class="gold" { "$" } " redox cluster status\n"
                    span class="gold" { "$" } " redox vm list\n"
                    span class="gold" { "$" } " redox vm start 103\n"
                    span class="gold" { "$" } " redox storage list\n"
                } }
            }
        }
    };
    layout(p, "Komut Satırı", &[("Komut Satırı", "/help")], true, body)
}

/// Giriş sayfası: uygulama kabuğu olmadan, tek başına. Oturum açılmadan sunucu bilgisi gösterilmez.
pub fn login(theme: &str, next: &str, username: &str, error: Option<&str>, sso: bool) -> String {
    html! {
        (DOCTYPE)
        html lang="tr" data-theme=(theme) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Giriş · RedoX VE" }
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
                            h2 { "Sunucunuz, sizin kurallarınızla." }
                            p { "Sanal makineler ve konteynerler için tek dosyalık, JavaScript'siz yönetim paneli." }
                        }
                        p class="hint" { "Hesaplar sunucuda " span class="mono" { "redox user add" } " ile oluşturulur." }
                    }
                    section class="side" {
                        form method="post" action="/login" {
                            h1 { "Giriş" }
                            @if let Some(e) = error {
                                p class="errsum" role="alert" { (e) }
                            }
                            input type="hidden" name="next" value=(next);
                            div class="field" {
                                label for="username" { "Kullanıcı adı" }
                                input id="username" name="username" type="text" value=(username) autocomplete="username"
                                    autocapitalize="none" spellcheck="false" required autofocus[username.is_empty()];
                            }
                            div class="field" {
                                label for="password" { "Parola" }
                                input id="password" name="password" type="password" autocomplete="current-password" required autofocus[!username.is_empty()];
                            }
                            div class="field" {
                                label for="code" { "Doğrulama kodu" }
                                input id="code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code" maxlength="6" pattern="[0-9]{6}";
                                span class="hint" { "Yalnızca iki adımlı doğrulamayı açtıysanız." }
                            }
                            button class="btn primary" type="submit" { "Giriş Yap" }
                            @if sso {
                                a class="btn" href="/login/oidc" { (icon("shield")) "Kurumsal hesapla giriş yap" }
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
        html lang="tr" data-theme=(theme) {
            head {
                meta charset="utf-8";
                meta http-equiv="refresh" content="0;url=/";
                title { "Giriş yapıldı · RedoX VE" }
                link rel="stylesheet" href=(asset("fonts/fonts.css"));
                link rel="stylesheet" href=(asset("style.css"));
            }
            body { main class="signed" { p { "Giriş yapıldı. " a class="link" href="/" { "Panele geçin" } "." } } }
        }
    }
    .into_string()
}

pub fn error_page(p: &Page, title: &str, message: &str) -> String {
    let body = html! {
        (page_head(html! { (title) }, html! {}, html! {}, "", &[]))
        div class="card" { div class="body" {
            p class="lead" { (message) }
            div class="btns" { a class="btn" href="/" { (icon("datacenter")) "Genel Bakışa Dön" } }
        } }
    };
    layout(p, title, &[(title, "")], true, body)
}
