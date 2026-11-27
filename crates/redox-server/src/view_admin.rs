//! Yönetim sayfaları: kullanıcılar, hesabım, denetim günlüğü.

use maud::{html, Markup, PreEscaped};

use crate::i18n::tx;

use crate::audit::AuditEntry;
use crate::auth::{Role, User, MIN_PASSWORD_CHARS};
use crate::view::{tip, datetime, icon, layout, page_head, security_keys_card, Page};

fn role_label(r: Role) -> &'static str {
    match r {
        Role::Admin => (tx("Yönetici")),
        Role::Operator => (tx("Operatör")),
        Role::Viewer => (tx("İzleyici")),
    }
}

fn token_days(id: &str) -> Markup {
    html! {
        select id=(id) name="days" {
            option value="0" { (tx("Süresiz")) }
            option value="30" { (tx("30 gün")) }
            option value="90" { (tx("90 gün")) }
            option value="365" { (tx("1 yıl")) }
        }
    }
}

fn token_list(action_base: &str, user: &User) -> Markup {
    html! {
        @if user.tokens.is_empty() { span class="dim" { "-" } }
        @for t in &user.tokens {
            form method="post" action={ (action_base) "/tokens/" (t.name) "/revoke" } {
                span class="tag" { (t.name) @if t.read_only { (tx(" · salt okunur")) }
                    @if let Some(end) = t.expires {
                        @if t.expired(crate::state::now()) { (tx(" · süresi doldu")) } @else { (tx(" · son gün ")) (datetime(end)) }
                    }
                }
                button class="btn sm danger" type="submit" aria-label={ (t.name) (tx(" token'ını iptal et")) } { (tx("İptal")) }
            }
        }
    }
}

pub fn users_page(p: &Page, users: &[User]) -> String {
    let body = html! {
        (page_head(html! { (tx("Kullanıcılar")) }, html! {}, html! {}, "user list", &[]))
        div class="stack" {
            div class="card" {
                header { h2 { (tx("Hesaplar")) } (tip("Panele girebilen kullanıcılar, rolleri ve yetki kapsamları.")) span class="note mono" { (users.len()) (tx(" kullanıcı")) } }
                div class="tw" { table {
                    thead { tr { th { (tx("Ad")) } th { (tx("Rol")) } th { (tx("İki adımlı doğrulama")) } th { (tx("API token'ları")) } th { span class="sr" { (tx("İşlem")) } } } }
                    tbody { @for u in users { tr {
                        td class="mono" { (u.name) @if u.name == p.user { " " span class="dim" { "(siz)" } } }
                        td {
                            (role_label(u.role))
                            @if u.role == Role::Operator { " " span class="mono dim" { @if u.scope.is_empty() { (tx("(bütün konuklar)")) } @else { "(" (u.scope.join(", ")) ")" } } }
                            @if let (Role::Operator, Some(permits)) = (u.role, &u.permits) { br; span class="dim" { (tx("Yalnızca: ")) (permits.iter().map(|p| tx(p.label())).collect::<Vec<_>>().join(", ")) } }
                            @if let Some(sso) = &u.sso { br; span class="mono dim" { "SSO: " (sso) } }
                            @if u.ldap { br; span class="tag" { "LDAP" } }
                        }
                        td {
                            @if u.totp_secret.is_some() || !u.keys.is_empty() { span class="pill ok" { (icon("check")) (tx("Açık")) } }
                            @else { span class="pill idle" { (tx("Kapalı")) } }
                            @if !u.keys.is_empty() { " " span class="tag" { (u.keys.len()) (tx(" güvenlik anahtarı")) } }
                        }
                        td { (token_list(&format!("/users/{}", u.name), u)) }
                        td class="act" { div class="stackf" {
                            form method="post" action={ "/users/" (u.name) "/role" } {
                                @for (value, label) in [("viewer", (tx("İzleyici Yap"))), ("operator", (tx("Operatör Yap"))), ("admin", (tx("Yönetici Yap")))] {
                                    @if value.parse::<Role>().ok() != Some(u.role) { button class="btn sm" name="role" value=(value) { (label) } }
                                }
                            }
                            @if u.role == Role::Operator {
                                form method="post" action={ "/users/" (u.name) "/permits" } {
                                    @for permit in crate::auth::Permit::ALL {
                                        label class="check" { input type="checkbox" name=(permit.name()) value="1" checked[u.permits.as_ref().is_none_or(|p| p.contains(&permit))]; (tx(permit.label())) }
                                    }
                                    button class="btn sm" type="submit" { (tx("Yetkileri Kaydet")) }
                                }
                                form method="post" action={ "/users/" (u.name) "/scope" } {
                                    input name="scope" type="text" value=(u.scope.join(", ")) placeholder=(tx("etiketler: web, test")) aria-label={ (u.name) (tx(" için yetkili etiketler")) } autocomplete="off";
                                    button class="btn sm" type="submit" { (tx("Kapsamı Kaydet")) }
                                }
                            }
                            @if u.totp_secret.is_some() || !u.keys.is_empty() {
                                form method="post" action={ "/users/" (u.name) "/totp-reset" } {
                                    button class="btn sm" type="submit" { (tx("2 Adımı Sıfırla")) }
                                }
                            }
                            form method="post" action={ "/users/" (u.name) "/sessions/close" } {
                                button class="btn sm" type="submit" { (tx("Oturumları Kapat")) }
                            }
                            @if u.name != p.user {
                                form method="post" action={ "/users/" (u.name) "/delete" } {
                                    label class="check" { input type="checkbox" name="confirm" value="1" required; (tx("Eminim")) }
                                    button class="btn sm danger" type="submit" { (icon("trash")) (tx("Sil")) }
                                }
                            }
                        } }
                    } } }
                } }
            }
            div class="cols even" {
                form class="card" method="post" action="/users" {
                    header { h2 { (tx("Kullanıcı ekle")) } (tip("Yeni bir hesap açar. Yönetici her şeyi, operatör kapsamındaki konuklarda güç ve yedek işlemlerini yapar; izleyici yalnızca okur.")) }
                    div class="form" {
                        div class="field" { label for="new-name" { (tx("Ad")) } input id="new-name" name="name" type="text" required autocomplete="off" autocapitalize="none";
                            span class="hint" tabindex="0" role="note" { (tx("Küçük harf, rakam, nokta, tire, alt çizgi.")) } }
                        div class="field" { label for="new-role" { (tx("Rol")) } select id="new-role" name="role" { option value="viewer" { (tx("İzleyici (salt okunur)")) } option value="operator" { (tx("Operatör (güç, konsol, yedek)")) } option value="admin" { (tx("Yönetici")) } } }
                        div class="field full" { label for="new-pw" { (tx("Parola")) } input id="new-pw" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS);
                            span class="hint" tabindex="0" role="note" { (tx("En az ")) (MIN_PASSWORD_CHARS) (tx(" karakter. Kullanıcı ilk girişte Hesabım sayfasından değiştirebilir.")) } }
                        div class="btns field full" { button class="btn primary" type="submit" { (icon("plus")) (tx("Ekle")) } }
                    }
                }
                div class="stack" {
                    form class="card" method="post" action="/users/-/password" {
                        header { h2 { (tx("Parola sıfırla")) } (tip("Bir kullanıcının parolasını değiştirir; açık oturumları kapanır.")) }
                        div class="form" {
                            div class="field" { label for="pw-user" { (tx("Kullanıcı")) } select id="pw-user" name="name" { @for u in users { option value=(u.name) { (u.name) } } } }
                            div class="field" { label for="pw-new" { (tx("Yeni parola")) } input id="pw-new" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS); }
                            div class="btns field full" { button class="btn" type="submit" { (tx("Sıfırla")) } }
                        }
                        footer { (tx("Kullanıcının açık oturumları hemen kapanır.")) }
                    }
                    form class="card" method="post" action="/users/-/tokens" {
                        header { h2 { (tx("API token'ı oluştur")) } (tip("Komut satırının ve betiklerin kullanıcı adına bağlanmasını sağlayan anahtar. Değeri yalnızca bir kez gösterilir.")) }
                        div class="form" {
                            div class="field" { label for="tk-user" { (tx("Kullanıcı")) } select id="tk-user" name="name" { @for u in users { option value=(u.name) { (u.name) } } } }
                            div class="field" { label for="tk-label" { (tx("Etiket")) } input id="tk-label" name="label" type="text" required autocomplete="off" placeholder=(tx("örn. dizustu")); span class="hint" tabindex="0" role="note" { (tx("Token'ı tanımanız için bir ad (ör. betik, izleme).")) } }
                            div class="field" { label for="tk-days" { (tx("Geçerlilik")) } (token_days("tk-days")) }
                            div class="btns field full" { button class="btn" type="submit" { (tx("Oluştur")) } }
                        }
                    }
                }
            }
        }
    };
    layout(p, (tx("Kullanıcılar")), &[((tx("Kullanıcılar")), "/users")], false, body)
}

/// Yeni token yalnızca bu sayfada, bir kez gösterilir.
pub fn token_created(p: &Page, user: &str, label: &str, token: &str, back: &str) -> String {
    let body = html! {
        (page_head(html! { (tx("Token oluşturuldu")) }, html! {}, html! {}, "", &[]))
        div class="stack" {
            div class="card" { div class="body" {
                p class="lead" { b { (user) } (tx(" için ")) b { (label) } (tx(" etiketli token:")) }
                pre class="conf secret" { (token) }
                p class="muted" { (tx("Bu değer bir daha gösterilmez; şimdi güvenli bir yere kaydedin. Kullanımı:")) }
                pre class="conf" { span class="gold" { "$" } " REDOX_TOKEN=" (token.chars().take(12).collect::<String>()) "… redox cluster status" }
                div class="btns" { a class="btn" href=(back) { (tx("Tamam")) } }
            } }
        }
    };
    layout(p, (tx("Token oluşturuldu")), &[((tx("Token oluşturuldu")), "")], true, body)
}

pub struct TotpSetup {
    pub secret: String,
    /// QR kodun SVG'si (kitaplık üretir; kullanıcı verisi içermez).
    pub qr_svg: String,
}

pub fn account_page(p: &Page, user: &User, setup: Option<&TotpSetup>, keys_ready: bool, sessions: &[crate::auth::SessionInfo]) -> String {
    let body = html! {
        (page_head(html! { (tx("Hesabım")) }, html! { span class="pill idle" { (role_label(user.role)) } }, html! {}, "", &[]))
        div class="stack" {
            form class="card" method="post" action="/account/password" {
                header { h2 { (tx("Parola değiştir")) } (tip("Kendi parolanızı değiştirir; bütün oturumlarınız kapanır.")) }
                div class="form" {
                    div class="field" { label for="current" { (tx("Şimdiki parola")) } input id="current" name="current" type="password" required autocomplete="current-password"; }
                    div class="field" { label for="password" { (tx("Yeni parola")) } input id="password" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS);
                        span class="hint" tabindex="0" role="note" { (tx("En az ")) (MIN_PASSWORD_CHARS) " karakter." } }
                    div class="btns field full" { button class="btn primary" type="submit" { (tx("Değiştir")) } }
                }
                footer { (tx("Değişiklikten sonra bütün oturumlarınız kapanır ve yeniden giriş yaparsınız.")) }
            }
            div class="card" {
                header {
                    h2 { (tx("İki adımlı doğrulama")) }
                    @if user.totp_secret.is_some() { span class="pill ok" { (icon("check")) (tx("Açık")) } } @else { span class="pill idle" { (tx("Kapalı")) } }
                }
                div class="body" {
                    @if user.totp_secret.is_some() {
                        p class="lead" { (tx("Girişte parolanıza ek olarak kimlik doğrulama uygulamanızdaki 6 haneli kod istenir.")) }
                        form class="inline flat" method="post" action="/account/totp/disable" {
                            label for="off-pw" { (tx("Kapatmak için parolanız")) }
                            input id="off-pw" name="current" type="password" required autocomplete="current-password";
                            button class="btn danger" type="submit" { (tx("Kapat")) }
                        }
                    } @else if let Some(s) = setup {
                        p class="lead" { (tx("1. Kimlik doğrulama uygulamanızla (Aegis, Google Authenticator, 1Password …) bu kodu okutun:")) }
                        div class="qr" { (PreEscaped(&s.qr_svg)) }
                        p class="muted" { (tx("Okutamıyorsanız anahtarı elle girin:")) }
                        pre class="conf secret" { (s.secret) }
                        form class="inline flat" method="post" action="/account/totp/confirm" {
                            label for="code" { (tx("2. Uygulamadaki kod")) }
                            input id="code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9]{6}" maxlength="6" required autofocus;
                            button class="btn primary" type="submit" { (tx("Etkinleştir")) }
                        }
                    } @else {
                        p class="lead" { (tx("Parolanız ele geçirilse bile hesabınıza girilemesin: girişte telefonunuzdaki uygulamanın ürettiği kod da istenir.")) }
                        form method="post" action="/account/totp/start" { button class="btn primary" type="submit" { (icon("shield")) (tx("Kurulumu Başlat")) } }
                    }
                    p class="hint" { (tx("Telefonunuzu kaybederseniz bir yönetici Kullanıcılar sayfasından ya da sunucuda ")) span class="mono" { "redox user totp-reset" } (tx(" ile sıfırlayabilir.")) }
                }
            }
            (security_keys_card(user, keys_ready))
            (crate::view::sessions_card(sessions))
            div class="card" {
                header { h2 { (tx("API token'larım")) } (tip("Adınıza bağlanan komut satırı ve betikler için anahtarlar.")) }
                div class="body" {
                    div class="tokens" { (token_list("/account", user)) }
                    form class="inline flat" method="post" action="/account/tokens" {
                        label for="label" { (tx("Yeni token etiketi")) }
                        input id="label" name="label" type="text" required autocomplete="off" placeholder=(tx("örn. dizustu"));
                        label for="days" { (tx("Geçerlilik")) }
                        (token_days("days"))
                        button class="btn" type="submit" { (tx("Oluştur")) }
                    }
                    p class="hint" { (tx("Token, ")) span class="mono" { "redox" } (tx(" komut satırının ve betiklerin sizin adınıza bağlanmasını sağlar; rolünüzün yetkilerini taşır.")) }
                }
            }
        }
    };
    layout(p, (tx("Hesabım")), &[((tx("Hesabım")), "/account")], true, body)
}

pub fn audit_page(p: &Page, entries: &[AuditEntry], query: &str) -> String {
    let body = html! {
        (page_head(html! { (tx("Denetim Günlüğü")) }, html! {}, html! {}, "", &[]))
        div class="stack" {
            div class="card" {
                header {
                    h2 { (tx("Girişler ve değişiklikler")) } span class="note" { (entries.len()) (tx(" kayıt")) }
                    form class="find end" method="get" action="/audit" role="search" {
                        input type="search" name="q" value=(query) placeholder=(tx("kullanıcı, adres ya da eylem")) aria-label=(tx("Kayıtları süz"));
                    }
                    a class="btn sm" href={ "/audit.csv" @if !query.is_empty() { "?q=" (crate::view::enc(query)) } } { (tx("CSV indir")) }
                }
                @if entries.is_empty() {
                    p class="empty" {
                        @if query.is_empty() { (tx("Henüz kayıt yok. Girişler ve durum değiştiren her istek burada görünür.")) }
                        @else { "“" (query) (tx("” ile eşleşen kayıt yok. ")) a href="/audit" { (tx("Süzgeci temizleyin")) } "." }
                    }
                } @else {
                    div class="tw" { table {
                        thead { tr { th { (tx("Zaman")) } th { (tx("Kullanıcı")) } th { (tx("Adres")) } th { (tx("Eylem")) } th { (tx("Sonuç")) } } }
                        tbody { @for e in entries { tr {
                            td class="mono muted" { (datetime(e.ts)) }
                            td class="mono" { (e.user) }
                            td class="mono muted" { (e.ip) }
                            td class="mono" { (e.action) }
                            td { @if e.ok { span class="pill ok" { (icon("check")) (tx("Tamam")) } } @else { span class="pill err" { (icon("alert")) (tx("Reddedildi")) } } }
                        } } }
                    } }
                }
                footer { (tx("Son ")) (crate::audit::KEEP) (tx(" kayıt aranabilir; tamamı sunucuda kullanıcı dosyasının yanındaki audit.log dosyasındadır.")) }
            }
        }
    };
    layout(p, (tx("Denetim Günlüğü")), &[((tx("Denetim Günlüğü")), "/audit")], false, body)
}
