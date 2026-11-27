//! Güvenlik anahtarı görünümleri: Hesabım sayfasındaki kart ve tarayıcıdan anahtar isteyen ara sayfa.

use super::*;
use crate::auth::{KeyChallenge, User, MAX_KEYS};
use crate::oidc::base64url;

/// Ara sayfanın yaptığı iş.
pub enum KeyStep<'a> {
    Register,
    /// `next`: girişten sonra gidilecek yol.
    Login { next: &'a str },
}

/// Uygulamada `navigator.credentials` çağırabilen tek sayfa: kabuksuzdur ve yalnızca bu sunucunun betiğini yükler.
pub fn key_page(theme: &str, step: KeyStep, challenge: &KeyChallenge) -> String {
    let (mode, action, title, lead, button, back) = match &step {
        KeyStep::Register => ("register", "/account/keys/finish", tx("Güvenlik anahtarı ekle"), tx("Düğmeye basın, tarayıcı isteyince anahtarınızı takıp dokunun."), tx("Anahtarı Kaydet"), "/account"),
        KeyStep::Login { .. } => ("login", "/login/key", tx("Güvenlik anahtarı"), tx("Parolanız doğrulandı. Girişi tamamlamak için güvenlik anahtarınızı kullanın."), tx("Anahtarı Kullan"), "/login"),
    };
    html! {
        (DOCTYPE)
        html lang=(crate::i18n::current().code()) data-theme=(theme) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · RedoX VE" }
                link rel="icon" type="image/svg+xml" href=(asset("logo.svg"));
                link rel="stylesheet" href=(asset("fonts/fonts.css"));
                link rel="stylesheet" href=(asset("style.css"));
            }
            body {
                main class="signed" {
                    div class="card keycard" {
                        header { h1 { (title) } }
                        div class="body" {
                            p class="lead" { (lead) }
                            noscript { p class="errsum" role="alert" { (tx("Güvenlik anahtarı JavaScript gerektirir.")) } }
                            p id="key-status" class="errsum" role="alert" hidden {}
                            form id="key-form" method="post" action=(action)
                                data-mode=(mode) data-challenge=(challenge.challenge) data-rp=(challenge.rp_id)
                                data-user=(base64url(challenge.user.as_bytes())) data-name=(challenge.user) data-keys=(challenge.key_ids.join(","))
                                data-unsupported=(tx("Bu tarayıcı bu adreste güvenlik anahtarını desteklemiyor."))
                                data-failed=(tx("Anahtar kullanılamadı. Yeniden deneyebilirsiniz.")) {
                                input type="hidden" name="ticket" value=(challenge.ticket);
                                input type="hidden" name="id";
                                input type="hidden" name="client_data";
                                input type="hidden" name="auth_data";
                                @match &step {
                                    KeyStep::Register => { input type="hidden" name="public_key"; input type="hidden" name="alg"; }
                                    KeyStep::Login { next } => { input type="hidden" name="signature"; input type="hidden" name="next" value=(next); }
                                }
                                div class="btns" {
                                    button id="key-go" class="btn primary" type="button" autofocus { (icon("shield")) (button) }
                                    a class="btn" href=(back) { (tx("Vazgeç")) }
                                }
                            }
                        }
                    }
                }
                script src=(asset("webauthn.js")) defer {}
            }
        }
    }
    .into_string()
}

/// `ready`: sunucu `--webauthn-origin` ile çalışıyor mu?
pub fn security_keys_card(user: &User, ready: bool) -> Markup {
    html! {
        div class="card" id="keys" {
            header {
                h2 { (tx("Güvenlik anahtarları")) } (tip("Donanım anahtarı ya da cihazınızın parmak izi / PIN doğrulaması (WebAuthn). Girişte parolaya ek ikinci adımdır."))
                @if user.keys.is_empty() { span class="pill idle" { (tx("Kapalı")) } } @else { span class="pill ok" { (icon("check")) (tx("Açık")) } }
            }
            div class="body" {
                @if !ready {
                    p class="empty" { (tx("Güvenlik anahtarı için sunucu ")) span class="mono" { "--webauthn-origin" } (tx(" ile, panelin tarayıcıdaki adresi verilerek çalıştırılmalı.")) }
                    @if !user.keys.is_empty() {
                        p class="errsum" role="alert" { (tx("Hesabınızda kayıtlı anahtar var ama sunucu şu anda denetleyemiyor: anahtarla giriş yapılamaz.")) }
                    }
                }
                @for key in &user.keys {
                    form class="inline flat" method="post" action="/account/keys/remove" {
                        span class="grow" { strong { (key.name) } " " span class="dim mono" { (datetime(key.added)) } }
                        input type="hidden" name="id" value=(key.id);
                        input name="current" type="password" required autocomplete="current-password" placeholder=(tx("parolanız")) aria-label={ (key.name) (tx(" anahtarını silmek için parolanız")) };
                        button class="btn sm danger" type="submit" { (tx("Kaldır")) }
                    }
                }
                @if ready && user.keys.len() < MAX_KEYS {
                    form class="inline flat" method="post" action="/account/keys/start" {
                        label for="key-name" { (tx("Yeni anahtarın adı")) }
                        input id="key-name" name="label" type="text" required maxlength="40" autocomplete="off" placeholder=(tx("örn. YubiKey"));
                        label for="key-pw" { (tx("Parolanız")) }
                        input id="key-pw" name="current" type="password" required autocomplete="current-password";
                        button class="btn" type="submit" { (icon("plus")) (tx("Anahtar Ekle")) }
                    }
                }
                p class="hint" { (tx("Anahtar ekliyken giriş, parolaya ek olarak anahtar (ya da açıksa uygulama kodu) ister. Anahtarınızı kaybederseniz bir yönetici Kullanıcılar sayfasından ya da sunucuda ")) span class="mono" { "redox user keys-reset" } (tx(" ile kaldırabilir. API token'ları bundan etkilenmez.")) }
            }
        }
    }
}
