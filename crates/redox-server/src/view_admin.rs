//! Yönetim sayfaları: kullanıcılar, hesabım, denetim günlüğü.

use maud::{html, Markup, PreEscaped};

use crate::i18n::tx;

use crate::audit::AuditEntry;
use crate::auth::{Role, User, MIN_PASSWORD_CHARS};
use crate::view::{tip, datetime, icon, layout, page_head, security_keys_card, sub, Page};

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

fn second_step(u: &User) -> bool {
    u.totp_secret.is_some() || !u.keys.is_empty()
}

/// Rol ve kapsam özeti: tabloda ve kullanıcının kendi sayfasında aynıdır.
fn role_summary(u: &User) -> Markup {
    html! {
        (role_label(u.role))
        @if u.role == Role::Operator { " " span class="mono dim" { @if u.scope.is_empty() { (tx("(bütün konuklar)")) } @else { "(" (u.scope.join(", ")) ")" } } }
        @if let (Role::Operator, Some(permits)) = (u.role, &u.permits) { br; span class="dim" { (tx("Yalnızca: ")) (permits.iter().map(|p| tx(p.label())).collect::<Vec<_>>().join(", ")) } }
        @if let Some(sso) = &u.sso { br; span class="mono dim" { "SSO: " (sso) } }
        @if u.ldap { br; span class="tag" { "LDAP" } }
    }
}

/// Kullanıcı listesi. Her satırın işlemleri tek "Yönet" menüsündedir; formlar kullanıcının kendi sayfasındadır.
pub fn users_page(p: &Page, users: &[User]) -> String {
    let admins = users.iter().filter(|u| u.role == Role::Admin).count();
    let body = html! {
        (page_head(html! { (tx("Kullanıcılar")) }, sub(vec![
            html! { span { (users.len()) (tx(" kullanıcı")) } },
            html! { span { (admins) (tx(" yönetici")) } },
        ]), html! { a class="btn primary" href="/users?tab=ekle" { (icon("plus")) (tx("Kullanıcı Ekle")) } }, "user list", &[]))
        div class="card" {
            div class="tw free" { table {
                thead { tr { th { (tx("Ad")) } th { (tx("Rol ve kapsam")) } th { (tx("İki adımlı doğrulama")) } th { (tx("API token'ları")) } th { span class="sr" { (tx("İşlem")) } } } }
                tbody { @for u in users { tr {
                    td class="mono" { a href={ "/users/" (u.name) } { (u.name) } @if u.name == p.user { " " span class="dim" { "(siz)" } } }
                    td { (role_summary(u)) }
                    td {
                        @if second_step(u) { span class="pill ok" { (icon("check")) (tx("Açık")) } }
                        @else { span class="pill idle" { (tx("Kapalı")) } }
                        @if !u.keys.is_empty() { " " span class="tag" { (u.keys.len()) (tx(" güvenlik anahtarı")) } }
                    }
                    td {
                        @if u.tokens.is_empty() { span class="dim" { "-" } }
                        @for t in &u.tokens { span class="tag" { (t.name) @if t.read_only { (tx(" · salt okunur")) }
                            @if let Some(end) = t.expires {
                                @if t.expired(crate::state::now()) { (tx(" · süresi doldu")) } @else { (tx(" · son gün ")) (datetime(end)) }
                            }
                        } " " }
                    }
                    td class="act" {
                        form method="post" action={ "/users/" (u.name) "/sessions/close" } {
                            details class="menu" {
                                summary class="btn sm" aria-label={ (u.name) ": " (tx("Yönet")) } { (tx("Yönet")) (icon("caret")) }
                                div class="menu-list" {
                                    a href={ "/users/" (u.name) } { (icon("user")) (tx("Rol ve yetkiler…")) }
                                    a href={ "/users/" (u.name) "#parola" } { (icon("settings")) (tx("Parolayı sıfırla…")) }
                                    a href={ "/users/" (u.name) "#token" } { (icon("plus")) (tx("API token'ı oluştur…")) }
                                    hr;
                                    button formaction={ "/users/" (u.name) "/sessions/close" } { (icon("off")) (tx("Oturumları Kapat")) }
                                    @if second_step(u) {
                                        button formaction={ "/users/" (u.name) "/totp-reset" } { (icon("restart")) (tx("2 Adımı Sıfırla")) }
                                    }
                                    @if u.name != p.user {
                                        hr;
                                        a class="danger" href={ "/users/" (u.name) "#sil" } { (icon("trash")) (tx("Kullanıcıyı sil…")) }
                                    }
                                }
                            }
                        }
                    }
                } } }
            } }
        }
    };
    layout(p, (tx("Kullanıcılar")), &[((tx("Kullanıcılar")), "/users")], false, body)
}

/// Yeni kullanıcı sayfası.
pub fn user_new_page(p: &Page) -> String {
    let body = html! {
        (page_head(html! { (tx("Kullanıcı ekle")) }, html! {}, html! {}, "user add", &[]))
        form class="card" method="post" action="/users" {
            div class="form" {
                div class="field" { label for="new-name" { (tx("Ad")) } input id="new-name" name="name" type="text" required autocomplete="off" autocapitalize="none";
                    span class="help" { (tx("Küçük harf, rakam, nokta, tire, alt çizgi.")) } }
                div class="field" { label for="new-role" { (tx("Rol")) } select id="new-role" name="role" { option value="viewer" { (tx("İzleyici (salt okunur)")) } option value="operator" { (tx("Operatör (güç, konsol, yedek)")) } option value="admin" { (tx("Yönetici")) } }
                    span class="help" { (tx("Yönetici her şeyi, operatör kapsamındaki konuklarda güç ve yedek işlemlerini yapar; izleyici yalnızca okur.")) } }
                div class="field full" { label for="new-pw" { (tx("Parola")) } input id="new-pw" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS);
                    span class="help" { (tx("En az ")) (MIN_PASSWORD_CHARS) (tx(" karakter. Kullanıcı ilk girişte Hesabım sayfasından değiştirebilir.")) } }
                (operator_start("new"))
                div class="btns field full" { button class="btn primary" type="submit" { (icon("plus")) (tx("Ekle")) } a class="btn" href="/users" { (tx("Vazgeç")) } }
            }
        }
    };
    layout(p, (tx("Kullanıcı ekle")), &[((tx("Kullanıcılar")), "/users"), ((tx("Kullanıcı ekle")), "")], true, body)
}

/// Operatör olacak hesabın başlangıç yetkileri ve kapsamı: rolle aynı formda seçilir, kutular boş gelir.
fn operator_start(prefix: &str) -> Markup {
    html! {
        div class="field full" {
            label { (tx("Operatör olacaksa yetkileri")) }
            div class="opts" {
                @for permit in crate::auth::Permit::ALL {
                    label { input type="checkbox" name=(permit.name()) value="1"; b { (tx(permit.label())) } }
                }
            }
            span class="help" { (tx("Operatör rolü için en az birini işaretleyin; öbür rollerde dikkate alınmaz. Konsol, konteynerlerde kök kabuğu da açar.")) }
        }
        div class="field full" { label for={ (prefix) "-scope" } { (tx("Operatörün yetkili etiketleri")) }
            input id={ (prefix) "-scope" } name="scope" type="text" placeholder=(tx("etiketler: web, test")) autocomplete="off";
            span class="help" { (tx("Operatör yalnızca bu etiketleri taşıyan konukları yönetir. Boş: bütün konuklar.")) } }
    }
}

/// Tek kullanıcının yönetim sayfası: rol, yetkiler, parola, token'lar, oturumlar ve silme.
pub fn user_page(p: &Page, u: &User) -> String {
    let base = format!("/users/{}", u.name);
    let body = html! {
        (page_head(html! { span class="mono" { (u.name) } }, html! { span { (role_summary(u)) } }, html! { a class="btn" href="/users" { (tx("Bütün Kullanıcılar")) } }, "user list", &[]))
        div class="cols even" {
            div class="stack" {
                form class="card" method="post" action={ (base) "/role" } {
                    header { h2 { (tx("Rol")) } }
                    div class="form" {
                        div class="field full" { label for="role" { (tx("Rol")) }
                            select id="role" name="role" {
                                option value="viewer" selected[u.role == Role::Viewer] { (tx("İzleyici (salt okunur)")) }
                                option value="operator" selected[u.role == Role::Operator] { (tx("Operatör (güç, konsol, yedek)")) }
                                option value="admin" selected[u.role == Role::Admin] { (tx("Yönetici")) }
                            }
                            span class="help" { (tx("Yönetici her şeyi, operatör kapsamındaki konuklarda güç ve yedek işlemlerini yapar; izleyici yalnızca okur.")) }
                        }
                        @if u.role != Role::Operator { (operator_start("role")) }
                        div class="btns field full" { button class="btn primary" type="submit" { (tx("Rolü Kaydet")) } }
                    }
                }
                @if u.role == Role::Operator {
                    form class="card" method="post" action={ (base) "/permits" } {
                        header { h2 { (tx("Operatör yetkileri")) } }
                        div class="opts" {
                            @for permit in crate::auth::Permit::ALL {
                                label { input type="checkbox" name=(permit.name()) value="1" checked[u.permits.as_ref().is_none_or(|p| p.contains(&permit))]; b { (tx(permit.label())) } }
                            }
                        }
                        div class="form" { div class="btns field full" { button class="btn" type="submit" { (tx("Yetkileri Kaydet")) } } }
                    }
                    form class="card" method="post" action={ (base) "/scope" } {
                        header { h2 { (tx("Kapsam")) } }
                        div class="form" {
                            div class="field full" { label for="scope" { (tx("Yetkili etiketler")) }
                                input id="scope" name="scope" type="text" value=(u.scope.join(", ")) placeholder=(tx("etiketler: web, test")) autocomplete="off";
                                span class="help" { (tx("Operatör yalnızca bu etiketleri taşıyan konukları yönetir. Boş: bütün konuklar.")) } }
                            div class="btns field full" { button class="btn" type="submit" { (tx("Kapsamı Kaydet")) } }
                        }
                    }
                }
                form class="card" id="parola" method="post" action={ (base) "/password" } {
                    header { h2 { (tx("Parola sıfırla")) } }
                    div class="form" {
                        div class="field full" { label for="pw-new" { (tx("Yeni parola")) } input id="pw-new" name="password" type="password" required autocomplete="new-password" minlength=(MIN_PASSWORD_CHARS);
                            span class="help" { (tx("Kullanıcının açık oturumları hemen kapanır.")) } }
                        div class="btns field full" { button class="btn" type="submit" { (tx("Sıfırla")) } }
                    }
                }
            }
            div class="stack" {
                div class="card" id="token" {
                    header { h2 { (tx("API token'ları")) } (tip("Komut satırının ve betiklerin kullanıcı adına bağlanmasını sağlayan anahtar. Değeri yalnızca bir kez gösterilir.")) }
                    @if !u.tokens.is_empty() { div class="body" { div class="stackf" { (token_list(&base, u)) } } }
                    form method="post" action={ (base) "/tokens" } {
                        div class="form" {
                            div class="field" { label for="tk-label" { (tx("Etiket")) } input id="tk-label" name="label" type="text" required autocomplete="off" placeholder=(tx("örn. dizustu"));
                                span class="help" { (tx("Token'ı tanımanız için bir ad (ör. betik, izleme).")) } }
                            div class="field" { label for="tk-days" { (tx("Geçerlilik")) } (token_days("tk-days")) }
                            div class="btns field full" { button class="btn" type="submit" { (icon("plus")) (tx("Oluştur")) } }
                        }
                    }
                }
                div class="card" {
                    header { h2 { (tx("Oturumlar ve ikinci adım")) } }
                    div class="body" { div class="btns" {
                        form method="post" action={ (base) "/sessions/close" } { button class="btn" type="submit" { (icon("off")) (tx("Oturumları Kapat")) } }
                        @if second_step(u) {
                            form method="post" action={ (base) "/totp-reset" } { button class="btn" type="submit" { (icon("restart")) (tx("2 Adımı Sıfırla")) } }
                        }
                    } }
                }
                @if u.name != p.user {
                    form class="card" id="sil" method="post" action={ (base) "/delete" } {
                        header { h2 { (tx("Kullanıcıyı sil")) } }
                        div class="body" {
                            p class="muted" { (tx("Hesap, token'ları ve açık oturumlarıyla birlikte silinir. Geri alınamaz.")) }
                            div class="btns" {
                                label class="check" { input type="checkbox" name="confirm" value="1" required; (tx("Eminim")) }
                                button class="btn danger" type="submit" { (icon("trash")) (tx("Sil")) }
                            }
                        }
                    }
                }
            }
        }
    };
    layout(p, &u.name, &[((tx("Kullanıcılar")), "/users"), (&u.name, "")], false, body)
}

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
                        form class="inline flat" method="post" action="/account/totp/start" {
                            label for="totp-current" { (tx("Parolanız")) }
                            input id="totp-current" name="current" type="password" required autocomplete="current-password";
                            button class="btn primary" type="submit" { (icon("shield")) (tx("Kurulumu Başlat")) }
                        }
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
                        label for="token-current" { (tx("Parolanız")) }
                        input id="token-current" name="current" type="password" required autocomplete="current-password";
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
