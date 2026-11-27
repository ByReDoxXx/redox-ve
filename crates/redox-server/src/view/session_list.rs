//! Hesabım sayfasındaki açık oturumlar kartı.

use super::*;
use crate::auth::SessionInfo;

pub fn sessions_card(sessions: &[SessionInfo]) -> Markup {
    html! {
        div class="card" id="sessions" {
            header {
                h2 { (tx("Açık oturumlar")) } (tip("Hesabınızla açık tarayıcı oturumları. Tanımadığınız bir oturum görürseniz kapatın ve parolanızı değiştirin."))
                span class="note mono" { (sessions.len()) }
            }
            div class="tw" { table {
                thead { tr { th { (tx("Adres")) } th { (tx("Tarayıcı")) } th { (tx("Açılış")) } th { (tx("Son kullanım")) } th { span class="sr" { (tx("İşlem")) } } } }
                tbody { @for s in sessions { tr {
                    td class="mono" { @if s.ip.is_empty() { "-" } @else { (s.ip) } }
                    td class="muted" { @if s.agent.is_empty() { "-" } @else { (s.agent) } }
                    td class="mono muted" { (datetime(s.created)) }
                    td class="mono muted" { (datetime(s.last_seen)) }
                    td class="act" {
                        @if s.current { span class="pill ok" { (tx("Bu oturum")) } }
                        @else { form method="post" action={ "/account/sessions/" (s.id) "/close" } { button class="btn sm" type="submit" { (tx("Kapat")) } } }
                    }
                } } }
            } }
            @if sessions.iter().any(|s| !s.current) {
                div class="body" { form method="post" action="/account/sessions/close-others" { button class="btn danger" type="submit" { (tx("Diğer Oturumları Kapat")) } } }
            }
        }
    }
}
