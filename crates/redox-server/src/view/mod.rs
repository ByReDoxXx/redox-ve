//! Sunucuda üretilen arayüz (maud). Tarayıcıda JavaScript çalışmaz: gezinme bağlantılarla,
//! işlemler HTML formlarıyla, canlı güncelleme sayfa yenilemesiyle yapılır.

use chrono::{Local, TimeZone};
use maud::{html, Markup, PreEscaped, DOCTYPE};

pub(crate) use crate::i18n::{tfmt, tx};
use redox_core::fmt::{gb, mb_as_gb, pct, uptime};

mod account;
mod backups;
mod cluster;
mod forms;
mod ft;
mod health;
mod hostnet;
mod fwsets;
mod guests;
mod mirrors;
mod perf;
mod shared;
mod switches;
mod widgets;

pub use account::*;
pub use backups::*;
pub use cluster::*;
pub use forms::*;
pub use guests::*;
pub use widgets::*;
use redox_core::{
    metrics, Action, Cluster, Guest, GuestKind, GuestStatus, Node, NodeStatus, Task, TaskStatus, VmId,
};

pub const STYLE_CSS: &str = include_str!("../../assets/style.css");
pub const LOGO_SVG: &str = include_str!("../../assets/logo.svg");
pub const WORDMARK_SVG: &str = include_str!("../../assets/wordmark.svg");
pub const ICONS_SVG: &str = include_str!("../../assets/icons.svg");
/// Uygulamadaki tek betik: canlı konsol istemcisi. Yalnızca konsol sayfasında yüklenir.
pub const CONSOLE_JS: &str = include_str!("../../assets/console.js");

/// Gömülü varlıkların içerik damgası. Adreslere `?v=` olarak eklenir; böylece
/// güncellemeden sonra tarayıcı eski stil dosyasını önbellekten kullanmaz.
pub fn asset_version() -> &'static str {
    static VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let all = [STYLE_CSS, LOGO_SVG, WORDMARK_SVG, ICONS_SVG, CONSOLE_JS, crate::fonts::FONTS_CSS].concat();
        crate::auth::sha256_hex(&all)[..12].to_string()
    });
    &VERSION
}

/// Sürüm damgalı varlık adresi.
pub fn asset(file: &str) -> String {
    format!("/assets/{file}?v={}", asset_version())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Current {
    Datacenter,
    Node(String),
    Guest(VmId),
    Guests,
    Storage,
    Backups,
    Peers,
    Tasks,
    Other,
}

/// Her sayfanın ortak bağlamı.
pub struct Page<'a> {
    pub cluster: &'a Cluster,
    pub theme: &'a str,
    pub current: Current,
    /// Ağaç ve konuk tablosu filtresi.
    pub q: &'a str,
    pub notice: Option<&'a str>,
    pub error: Option<&'a str>,
    pub now: u64,
    /// Bu sayfanın bildirimsiz adresi (tema değişimi ve otomatik yenileme buraya döner).
    pub back: String,
    /// Oturum sahibi.
    pub user: &'a str,
    /// Durum değiştirebilir mi (yönetici)?
    pub admin: bool,
    /// Operatör mü, ve yetkili olduğu etiketler (boşsa bütün konuklar).
    pub operator: bool,
    pub scope: &'a [String],
    /// Gerçek sürücü mü (ölçümler gerçek)?
    pub real: bool,
    /// Sürücü konteyner destekliyor mu?
    pub ct: bool,
    /// Sanal makinelerde anlık görüntü ve yedek, konuk çalışırken de alınabiliyor mu?
    pub live: bool,
    /// Konteyner yedeği, konteyner çalışırken de (dondurularak) alınabiliyor mu?
    pub live_ct: bool,
    /// Çalışan sanal makineye disk ve ağ kartı takılabiliyor mu?
    pub hotplug: bool,
    /// Yedeklerin içindeki dosyalar okunabiliyor mu (sunucuda guestfish var mı)?
    pub files: bool,
    /// Bu kadar gündür yedeği olmayan konuk "korumasız" sayılır; 0 kapalı.
    pub backup_age: u32,
    /// Yedeklerin sunucu dışı hedefinin kısa tanımı; yapılandırılmadıysa `None`.
    pub offsite: Option<&'a str>,
    /// Yerel yedek deposu şifreli mi?
    pub sealed: bool,
    /// Grafiklerin zaman aralığı.
    pub range: crate::history::Range,
    pub history: &'a crate::state::History,
}

impl Page<'_> {
    /// Bu konukta güç, konsol, anlık görüntü ve yedek işlemleri yapılabilir mi?
    pub fn may_operate(&self, g: &Guest) -> bool {
        self.admin || (self.operator && (self.scope.is_empty() || g.tags.iter().any(|t| self.scope.contains(t))))
    }
}

fn matches_filter(g: &Guest, q: &str) -> bool {
    q.is_empty()
        || g.name.contains(q)
        || g.vmid.to_string().contains(q)
        || g.node.contains(q)
        || g.kind.as_str() == q
        || g.tags.iter().any(|t| t == q)
}

fn role_note(p: &Page) -> &'static str {
    if p.admin {
        (tx("yönetici"))
    } else if p.operator {
        (tx("operatör"))
    } else {
        (tx("salt okunur"))
    }
}

/// Sol menü: bölümler, sunucu sağlığı ve oturum.
fn rail(p: &Page) -> Markup {
    let s = p.cluster.summary();
    let next = if p.theme == "dark" { "light" } else { "dark" };
    let running = p.cluster.tasks.iter().filter(|t| t.status == TaskStatus::Running).count();
    let at = |prefix: &str| p.back.starts_with(prefix);
    let item = |href: &str, ic: &str, label: &str, on: bool, extra: Markup| {
        html! { a href=(href) aria-current=[cur(on)] { (icon(ic)) (label) (extra) } }
    };
    let none = || html! {};
    html! {
        aside class="rail" {
            a class="brand" href="/" aria-label=(tx("RedoX VE, genel bakış")) {
                img class="mark" src=(asset("logo.svg")) alt="" width="33" height="34";
                img class="word" src=(asset("wordmark.svg")) alt="" width="113" height="40";
            }
            nav class="nav" aria-label=(tx("Bölümler")) {
                div class="eyebrow" { (tx("İşletim")) }
                (item("/", "datacenter", (tx("Genel Bakış")), matches!(p.current, Current::Datacenter | Current::Node(_)), none()))
                (item("/guests", "vm", (tx("Konuklar")), matches!(p.current, Current::Guests | Current::Guest(_)) || at("/guests"),
                    html! { span class="count" { (p.cluster.guests.len()) } }))
                (item("/storage", "storage", (tx("Depolama")), p.current == Current::Storage, none()))
                (item("/backups", "backup", (tx("Yedekler")), p.current == Current::Backups,
                    html! { @if !p.cluster.backups.is_empty() { span class="count" { (p.cluster.backups.len()) } } }))
                (item("/pools", "cpu", (tx("Havuzlar")), at("/pools"),
                    html! { @if !p.cluster.pools.is_empty() { span class="count" { (p.cluster.pools.len()) } } }))
                (item("/cluster", "network", (tx("Küme")), p.current == Current::Peers, none()))
                div class="eyebrow" { (tx("Yönetim")) }
                (item("/tasks", "clock", (tx("Görevler")), p.current == Current::Tasks,
                    html! { @if running > 0 { span class="live" aria-label={ (running) (tx(" görev sürüyor")) } { (running) } } }))
                @if p.admin {
                    (item("/users", "user", (tx("Kullanıcılar")), at("/users"), none()))
                    (item("/audit", "shield", (tx("Denetim Günlüğü")), at("/audit"), none()))
                }
                (item("/account", "settings", (tx("Hesabım")), at("/account"), none()))
                (item("/help", "terminal", (tx("Komut Satırı")), at("/help"), none()))
            }
            div class="host" aria-label=(tx("Sunucu durumu")) {
                div class="name" {
                    span class={ "st " @if s.quorate { "run" } @else { "err" } } style="gap:0" {}
                    (p.cluster.name)
                    span class="dim" style="font-weight:400" { "· " (s.nodes_online) "/" (s.nodes_total) (tx(" düğüm")) }
                }
                div class="row" { span { "CPU" } (bar(s.cpu_pct, "CPU", true)) span class="num" { (s.cpu_pct) "%" } }
                div class="row" { span { (tx("Bellek")) } (bar(s.mem_pct, (tx("Bellek")), true)) span class="num" { (s.mem_pct) "%" } }
                div class="row" { span { "Disk" } (bar(s.storage_pct, (tx("Depolama")), true)) span class="num" { (s.storage_pct) "%" } }
            }
            div class="me" {
                a class="who" href="/account" { (icon("user")) (p.user) " " small { (role_note(p)) } }
                @let other = crate::i18n::current().other();
                a class="btn quiet sm" href={ "/lang?to=" (other.code()) "&back=" (enc(&p.back)) } lang=(other.code())
                    aria-label={ @if other == crate::i18n::Lang::En { "Switch to English" } @else { "Türkçeye geç" } } { (other.code().to_uppercase()) }
                a class="btn quiet sm sq" href={ "/theme?to=" (next) "&back=" (enc(&p.back)) }
                    aria-label={ @if next == "light" { (tx("Açık temaya geç")) } @else { (tx("Koyu temaya geç")) } } { (icon("theme")) }
                form method="post" action="/logout" {
                    button class="btn quiet sm sq" type="submit" aria-label=(tx("Çıkış")) title=(tx("Çıkış")) { (icon("off")) }
                }
            }
        }
    }
}

/// Üst çubuk: konum yolu, komut alanı ve sayfanın tek birincil eylemi.
fn top(p: &Page, crumbs: &[(&str, &str)]) -> Markup {
    html! {
        div class="top" {
            nav class="crumbs" aria-label=(tx("Konum")) {
                @for (i, (text, href)) in crumbs.iter().enumerate() {
                    @if i + 1 == crumbs.len() { span aria-current="page" { (text) } }
                    @else { a href=(href) { (text) } b aria-hidden="true" { "/" } }
                }
            }
            form class="cmd" method="post" action="/cmd" role="search" {
                (icon("prompt"))
                input id="cmd" type="text" name="q" autocomplete="off" spellcheck="false" accesskey="k"
                    placeholder=(tx("ara ya da komut yaz: start 103, open db-postgres, help"))
                    aria-label=(tx("Komut satırı: ara ya da komut yaz, Enter ile çalıştır"));
                kbd { "Alt K" }
            }
            @if p.admin {
                a class="btn primary" href="/guests/new?kind=vm" { (icon("plus")) (tx("Yeni Konuk")) }
            }
        }
    }
}

fn flash(p: &Page) -> Markup {
    let pending = if p.admin { p.cluster.pending_approvals(p.now) } else { Vec::new() };
    html! {
        @if p.cluster.lockdown {
            div class="flash bad" role="status" {
                (icon("shield"))
                span { (tx("Kilit kipi açık: arayüzden ve API'den hiçbir değişiklik yapılamaz. Konuklar ve zamanlanmış işler çalışmayı sürdürür.")) " " }
                @if p.admin { form class="inline" method="post" action="/lockdown/off" { button class="btn sm" type="submit" { (tx("Kilidi Kaldır")) } } }
            }
        }
        @if p.cluster.maintenance {
            div class="flash bad" role="status" {
                (icon("alert"))
                span { (tx("Bakım kipi açık: konuk başlatılmaz, zamanlanmış yedekler ve güç işlemleri durdu.")) @if !p.cluster.maintenance_resume.is_empty() { (tx(" Bakım bitince ")) (p.cluster.maintenance_resume.len()) (tx(" konuk yeniden başlatılacak.")) } " " }
                @if p.admin { form class="inline" method="post" action="/maintenance/off" { button class="btn sm" type="submit" { (tx("Bakımı Bitir")) } } }
            }
        }
        // Dört göz ilkesi: onay bekleyen silme istekleri her sayfada yöneticilere gösterilir.
        @for a in &pending {
            div class="flash" role="status" {
                (icon("alert"))
                span { (tx("Onay bekliyor (#")) (a.id) "): " strong { (a.by) } ", " (a.describe()) " istiyor. " }
                @if a.by == p.user {
                    span class="muted" { (tx("Başka bir yönetici onaylamalı. ")) }
                } @else {
                    form class="inline" method="post" action={ "/approvals/" (a.id) "/approve" } { button class="btn sm danger" type="submit" { (tx("Onayla")) } }
                }
                form class="inline" method="post" action={ "/approvals/" (a.id) "/reject" } { button class="btn sm" type="submit" { @if a.by == p.user { (tx("Geri Çek")) } @else { (tx("Reddet")) } } }
            }
        }
        @if let Some(n) = p.notice {
            div class="flash" role="status" { (icon("check")) span { (crate::i18n::message(n)) } }
        }
        @if let Some(e) = p.error {
            div class="flash bad" role="alert" { (icon("alert")) span { (crate::i18n::message(e)) } }
        }
    }
}

/// Uygulama kabuğu. `crumbs`: `(yazı, adres)`; sonuncusu bulunulan sayfadır.
pub fn layout(p: &Page, title: &str, crumbs: &[(&str, &str)], narrow: bool, body: Markup) -> String {
    html! {
        (DOCTYPE)
        html lang=(crate::i18n::current().code()) data-theme=(p.theme) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                // Süren görev varken sayfa kendini yeniler (JavaScript yok).
                @if p.cluster.has_running_tasks() {
                    meta http-equiv="refresh" content={ "2;url=" (p.back) };
                }
                title { (title) " · RedoX VE" }
                link rel="icon" type="image/svg+xml" href=(asset("logo.svg"));
                link rel="stylesheet" href=(asset("fonts/fonts.css"));
                link rel="stylesheet" href=(asset("style.css"));
            }
            body {
                a class="skip" href="#main" { (tx("İçeriğe geç")) }
                div class="app" {
                    (rail(p))
                    div class="main" {
                        (top(p, crumbs))
                        div class="scroll" {
                            main id="main" tabindex="-1" class={ "page" @if narrow { " narrow" } } {
                                (flash(p))
                                (body)
                            }
                        }
                    }
                }
            }
        }
    }
    .into_string()
}

/// Sayfa başlığı: başlık, alt satır, eylemler, komut satırı karşılığı ve sekmeler.
pub(crate) fn page_head(title: Markup, subline: Markup, actions: Markup, cli: &str, tabs: &[(&str, String, bool)]) -> Markup {
    html! {
        div class="ph" {
            div {
                h1 { (title) }
                div class="sub" { (subline) }
                @if !cli.is_empty() { p class="cli" { b aria-hidden="true" { "$" } " redox " (cli) } }
            }
            div class="acts" { (actions) }
        }
        @if tabs.len() > 1 {
            nav class="tabs" aria-label=(tx("Bölümler")) {
                @for (label, href, on) in tabs {
                    a href=(href) aria-current=[cur(*on)] { (label) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
