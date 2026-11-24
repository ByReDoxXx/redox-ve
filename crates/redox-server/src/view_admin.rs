//! Yönetim sayfaları: kullanıcılar, hesabım, denetim günlüğü.

use maud::{html, Markup, PreEscaped};

use crate::audit::AuditEntry;
use crate::auth::{Role, User, MIN_PASSWORD_CHARS};
use crate::view::{datetime, icon, layout, page_head, Page};

fn role_label(r: Role) -> &'static str {
    match r {
        Role::Admin => "Yönetici",
        Role::Operator => "Operatör",
        Role::Viewer => "İzleyici",
    }
}

fn token_list(action_base: &str, user: &User) -> Markup {
    html! {
        @if user.tokens.is_empty() { span class="dim" { "-" } }
        @for t in &user.tokens {
            form method="post" action={ (action_base) "/tokens/" (t.name) "/revoke" } {
                span class="tag" { (t.name) }
                button class="btn sm danger" type="submit" aria-label={ (t.name) " token'ını iptal et" } { "İptal" }
            }
        }
    }
}

pub fn users_page(p: &Page, users: &[User]) -> String {
    let body = html! {
        (page_head(html! { "Kullanıcılar" }, html! {}, html! {}, "user list", &[]))
        div class="stack" {
            div class="card" {
                header { h2 { "Hesaplar" } span class="note mono" { (users.len()) " kullanıcı" } }
                div class="tw" { table {
                    thead { tr { th { "Ad" } th { "Rol" } th { "İki adımlı doğrulama" } th { "API token'ları" } th { span class="sr" { "İşlem" } } } }
                    tbody { @for u in users { tr {
                        td class="mono" { (u.name) @if u.name == p.user { " " span class="dim" { "(siz)" } } }
                        td {
                            (role_label(u.role))
                            @if u.role == Role::Operator { " " span class="mono dim" { @if u.scope.is_empty() { "(bütün konuklar)" } @else { "(" (u.scope.join(", ")) ")" } } }
                            @if let Some(sso) = &u.sso { br; span class="mono dim" { "SSO: " (sso) } }
                        }
                        td {
                            @if u.totp_secret.is_some() { span class="pill ok" { (icon("check")) "Açık" } }
                            @else { span class="pill idle" { "Kapalı" } }
                        }
                        td { (token_list(&format!("/users/{}", u.name), u)) }
                        td class="act" { div class="stackf" {
                            form method="post" action={ "/users/" (u.name) "/role" } {
                                @for (value, label) in [("viewer", "İzleyici Yap"), ("operator", "Operatör Yap"), ("admin", "Yönetici Yap")] {
                                    @if value.parse::<Role>().ok() != Some(u.role) { button class="btn sm" name="role" value=(value) { (label) } }
                                }
                            }
                            @if u.role == Role::Operator {
                                form method="post" action={ "/users/" (u.name) "/scope" } {
                                    input name="scope" type="text" value=(u.scope.join(", ")) placeholder="etiketler: web, test" aria-label={ (u.name) " için yetkili etiketler" } autocomplete="off";
                                    button class="btn sm" type="submit" { "Kapsamı Kaydet" }
                                }
                            }
                            @if u.totp_secret.is_some() {
                                form method="post" action={ "/users/" (u.name) "/totp-reset" } {
                                    button class="btn sm" type="submit" { "2 Adımı Sıfırla" }
                                }
                            }
                            @if u.name != p.user {
                                form method="post" action={ "/users/" (u.name) "/delete" } {
                                    label class="check" { input type="checkbox" name="confirm" value="1" required; "Eminim" }
                                    button class="btn sm danger" type="submit" { (icon("trash")) "Sil" }
                                }
                            }
                        } }
                    } } }
                } }
            }
            div class="cols even" {
                form class="card" method="post" action="/users" {
                    header { h2 { "Kullanıcı ekle" } }
                    div class="form" {
                        div class="field" { label for="new-name" { "Ad" } input id="new-name" name="name" type="text" required autocomplete="off" autocapitalize="none";
                            span class="hint" { "Küçük harf, rakam, nokta, tire, alt çizgi." } }
                        div class="field" { label for="new-role" { "Rol" } select id="new-role" name="role" { option value="viewer" { "İzleyici (salt okunur)" } option value="operator" { "Operatör (güç, konsol, yedek)" } option value="admin" { "Yönetici" } } }
                        div class="field full" { label for="new-pw" { "Parola" } input id="new-pw" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS);
                            span class="hint" { "En az " (MIN_PASSWORD_CHARS) " karakter. Kullanıcı ilk girişte Hesabım sayfasından değiştirebilir." } }
                        div class="btns field full" { button class="btn primary" type="submit" { (icon("plus")) "Ekle" } }
                    }
                }
                div class="stack" {
                    form class="card" method="post" action="/users/-/password" {
                        header { h2 { "Parola sıfırla" } }
                        div class="form" {
                            div class="field" { label for="pw-user" { "Kullanıcı" } select id="pw-user" name="name" { @for u in users { option value=(u.name) { (u.name) } } } }
                            div class="field" { label for="pw-new" { "Yeni parola" } input id="pw-new" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS); }
                            div class="btns field full" { button class="btn" type="submit" { "Sıfırla" } }
                        }
                        footer { "Kullanıcının açık oturumları hemen kapanır." }
                    }
                    form class="card" method="post" action="/users/-/tokens" {
                        header { h2 { "API token'ı oluştur" } }
                        div class="form" {
                            div class="field" { label for="tk-user" { "Kullanıcı" } select id="tk-user" name="name" { @for u in users { option value=(u.name) { (u.name) } } } }
                            div class="field" { label for="tk-label" { "Etiket" } input id="tk-label" name="label" type="text" required autocomplete="off" placeholder="örn. dizustu"; }
                            div class="btns field full" { button class="btn" type="submit" { "Oluştur" } }
                        }
                    }
                }
            }
        }
    };
    layout(p, "Kullanıcılar", &[("Kullanıcılar", "/users")], false, body)
}

/// Yeni token yalnızca bu sayfada, bir kez gösterilir.
pub fn token_created(p: &Page, user: &str, label: &str, token: &str, back: &str) -> String {
    let body = html! {
        (page_head(html! { "Token oluşturuldu" }, html! {}, html! {}, "", &[]))
        div class="stack" {
            div class="card" { div class="body" {
                p class="lead" { b { (user) } " için " b { (label) } " etiketli token:" }
                pre class="conf secret" { (token) }
                p class="muted" { "Bu değer bir daha gösterilmez; şimdi güvenli bir yere kaydedin. Kullanımı:" }
                pre class="conf" { span class="gold" { "$" } " REDOX_TOKEN=" (token.chars().take(12).collect::<String>()) "… redox cluster status" }
                div class="btns" { a class="btn" href=(back) { "Tamam" } }
            } }
        }
    };
    layout(p, "Token oluşturuldu", &[("Token oluşturuldu", "")], true, body)
}

pub struct TotpSetup {
    pub secret: String,
    /// QR kodun SVG'si (kitaplık üretir; kullanıcı verisi içermez).
    pub qr_svg: String,
}

pub fn account_page(p: &Page, user: &User, setup: Option<&TotpSetup>) -> String {
    let body = html! {
        (page_head(html! { "Hesabım" }, html! { span class="pill idle" { (role_label(user.role)) } }, html! {}, "", &[]))
        div class="stack" {
            form class="card" method="post" action="/account/password" {
                header { h2 { "Parola değiştir" } }
                div class="form" {
                    div class="field" { label for="current" { "Şimdiki parola" } input id="current" name="current" type="password" required autocomplete="current-password"; }
                    div class="field" { label for="password" { "Yeni parola" } input id="password" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS);
                        span class="hint" { "En az " (MIN_PASSWORD_CHARS) " karakter." } }
                    div class="btns field full" { button class="btn primary" type="submit" { "Değiştir" } }
                }
                footer { "Değişiklikten sonra bütün oturumlarınız kapanır ve yeniden giriş yaparsınız." }
            }
            div class="card" {
                header {
                    h2 { "İki adımlı doğrulama" }
                    @if user.totp_secret.is_some() { span class="pill ok" { (icon("check")) "Açık" } } @else { span class="pill idle" { "Kapalı" } }
                }
                div class="body" {
                    @if user.totp_secret.is_some() {
                        p class="lead" { "Girişte parolanıza ek olarak kimlik doğrulama uygulamanızdaki 6 haneli kod istenir." }
                        form class="inline flat" method="post" action="/account/totp/disable" {
                            label for="off-pw" { "Kapatmak için parolanız" }
                            input id="off-pw" name="current" type="password" required autocomplete="current-password";
                            button class="btn danger" type="submit" { "Kapat" }
                        }
                    } @else if let Some(s) = setup {
                        p class="lead" { "1. Kimlik doğrulama uygulamanızla (Aegis, Google Authenticator, 1Password …) bu kodu okutun:" }
                        div class="qr" { (PreEscaped(&s.qr_svg)) }
                        p class="muted" { "Okutamıyorsanız anahtarı elle girin:" }
                        pre class="conf secret" { (s.secret) }
                        form class="inline flat" method="post" action="/account/totp/confirm" {
                            label for="code" { "2. Uygulamadaki kod" }
                            input id="code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9]{6}" maxlength="6" required autofocus;
                            button class="btn primary" type="submit" { "Etkinleştir" }
                        }
                    } @else {
                        p class="lead" { "Parolanız ele geçirilse bile hesabınıza girilemesin: girişte telefonunuzdaki uygulamanın ürettiği kod da istenir." }
                        form method="post" action="/account/totp/start" { button class="btn primary" type="submit" { (icon("shield")) "Kurulumu Başlat" } }
                    }
                    p class="hint" { "Telefonunuzu kaybederseniz bir yönetici Kullanıcılar sayfasından ya da sunucuda " span class="mono" { "redox user totp-reset" } " ile sıfırlayabilir." }
                }
            }
            div class="card" {
                header { h2 { "API token'larım" } }
                div class="body" {
                    div class="tokens" { (token_list("/account", user)) }
                    form class="inline flat" method="post" action="/account/tokens" {
                        label for="label" { "Yeni token etiketi" }
                        input id="label" name="label" type="text" required autocomplete="off" placeholder="örn. dizustu";
                        button class="btn" type="submit" { "Oluştur" }
                    }
                    p class="hint" { "Token, " span class="mono" { "redox" } " komut satırının ve betiklerin sizin adınıza bağlanmasını sağlar; rolünüzün yetkilerini taşır." }
                }
            }
        }
    };
    layout(p, "Hesabım", &[("Hesabım", "/account")], true, body)
}

pub fn audit_page(p: &Page, entries: &[AuditEntry], query: &str) -> String {
    let body = html! {
        (page_head(html! { "Denetim Günlüğü" }, html! {}, html! {}, "", &[]))
        div class="stack" {
            div class="card" {
                header {
                    h2 { "Girişler ve değişiklikler" } span class="note" { (entries.len()) " kayıt" }
                    form class="find end" method="get" action="/audit" role="search" {
                        input type="search" name="q" value=(query) placeholder="kullanıcı, adres ya da eylem" aria-label="Kayıtları süz";
                    }
                }
                @if entries.is_empty() {
                    p class="empty" {
                        @if query.is_empty() { "Henüz kayıt yok. Girişler ve durum değiştiren her istek burada görünür." }
                        @else { "“" (query) "” ile eşleşen kayıt yok. " a href="/audit" { "Süzgeci temizleyin" } "." }
                    }
                } @else {
                    div class="tw" { table {
                        thead { tr { th { "Zaman" } th { "Kullanıcı" } th { "Adres" } th { "Eylem" } th { "Sonuç" } } }
                        tbody { @for e in entries { tr {
                            td class="mono muted" { (datetime(e.ts)) }
                            td class="mono" { (e.user) }
                            td class="mono muted" { (e.ip) }
                            td class="mono" { (e.action) }
                            td { @if e.ok { span class="pill ok" { (icon("check")) "Tamam" } } @else { span class="pill err" { (icon("alert")) "Reddedildi" } } }
                        } } }
                    } }
                }
                footer { "Son " (crate::audit::KEEP) " kayıt aranabilir; tamamı sunucuda kullanıcı dosyasının yanındaki audit.log dosyasındadır." }
            }
        }
    };
    layout(p, "Denetim Günlüğü", &[("Denetim Günlüğü", "/audit")], false, body)
}
