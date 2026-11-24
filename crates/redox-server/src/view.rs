//! Sunucuda üretilen arayüz (maud). Tarayıcıda JavaScript çalışmaz: gezinme bağlantılarla,
//! işlemler HTML formlarıyla, canlı güncelleme sayfa yenilemesiyle yapılır.

use chrono::{Local, TimeZone};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use redox_core::fmt::{gb, mb_as_gb, pct, uptime};
use redox_core::{
    metrics, Action, Cluster, Guest, GuestKind, GuestStatus, Node, NodeStatus, Task, TaskStatus, VmId,
};

pub const STYLE_CSS: &str = include_str!("../assets/style.css");
pub const LOGO_SVG: &str = include_str!("../assets/logo.svg");
pub const WORDMARK_SVG: &str = include_str!("../assets/wordmark.svg");
pub const ICONS_SVG: &str = include_str!("../assets/icons.svg");
/// Uygulamadaki tek betik: canlı konsol istemcisi. Yalnızca konsol sayfasında yüklenir.
pub const CONSOLE_JS: &str = include_str!("../assets/console.js");

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
    /// Çalışan sanal makineye disk ve ağ kartı takılabiliyor mu?
    pub hotplug: bool,
    /// Yedeklerin sunucu dışı hedefinin kısa tanımı; yapılandırılmadıysa `None`.
    pub offsite: Option<&'a str>,
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

/// Sorgu dizgesi için yüzde kodlaması.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn hms(ts: u64) -> String {
    match Local.timestamp_opt(ts as i64, 0).single() {
        Some(t) => t.format("%H:%M:%S").to_string(),
        None => "--:--:--".to_string(),
    }
}

/// Tarih ve saat: `24.11.2026 14:02`.
pub(crate) fn datetime(ts: u64) -> String {
    match Local.timestamp_opt(ts as i64, 0).single() {
        Some(t) => t.format("%d.%m.%Y %H:%M").to_string(),
        None => "-".to_string(),
    }
}

pub fn icon(name: &str) -> Markup {
    icon_c(name, "")
}

/// İkon adları derleme zamanında sabittir; kullanıcı verisi buraya girmez.
pub fn icon_c(name: &str, class: &str) -> Markup {
    PreEscaped(format!(
        r#"<svg class="i {class}" aria-hidden="true"><use href="/assets/icons.svg?v={}#i-{name}"/></svg>"#,
        asset_version()
    ))
}

fn cur(on: bool) -> Option<&'static str> {
    on.then_some("page")
}

/// Konuğun durumu: şekil ve yazı birlikte (yalnızca renkle anlatılmaz).
pub(crate) fn guest_state(g: &Guest) -> Markup {
    html! {
        @if g.lock.is_some() {
            span class="pill warn" { (icon_c("restart", "spin")) "İşlem sürüyor" }
        } @else if g.hw.suspended && g.status == GuestStatus::Stopped {
            span class="st stop" { "Askıda" }
        } @else {
            span class={ "st " (status_class(g.status)) } { (g.status.label()) }
        }
    }
}

fn status_class(s: GuestStatus) -> &'static str {
    match s {
        GuestStatus::Running => "run",
        GuestStatus::Stopped => "stop",
        GuestStatus::Paused => "pause",
    }
}

fn node_state(n: &Node) -> Markup {
    let class = if n.status == NodeStatus::Online { "st run" } else { "st err" };
    html! { span class=(class) { (n.status.label()) } }
}

fn task_pill(t: &Task) -> Markup {
    html! {
        @match t.status {
            TaskStatus::Ok => span class="pill ok" { (icon("check")) "Tamam" },
            TaskStatus::Running => span class="pill warn" { (icon_c("restart", "spin")) "Sürüyor" },
            TaskStatus::Failed => span class="pill err" {
                (icon("alert")) "Hata"
                @if let Some(m) = &t.message { ": " (m) }
            },
        }
    }
}

fn tone(p: u8) -> &'static str {
    match p {
        90.. => "err",
        75..=89 => "warn",
        _ => "",
    }
}

pub(crate) fn bar(p: u8, label: &str, wide: bool) -> Markup {
    html! {
        span class={ "bar" @if wide { " wide" } } role="meter" aria-valuenow=(p) aria-valuemin="0" aria-valuemax="100" aria-label=(label) {
            i class=(tone(p)) style={ "width:" (p) "%" } {}
        }
    }
}

/// Özet şeridinin bir hücresi.
fn stat(label: &str, value: &str, unit: &str, detail: &str, p: Option<u8>) -> Markup {
    html! {
        div {
            div class="eyebrow" { (label) }
            div class="v" { (value) small { (unit) } }
            div class="d" { (detail) }
            @if let Some(p) = p { (bar(p, label, true)) }
        }
    }
}

/// Noktayla ayrılmış alt başlık satırı.
fn sub(items: Vec<Markup>) -> Markup {
    html! {
        @for (i, item) in items.iter().enumerate() {
            @if i > 0 { span class="sep" aria-hidden="true" { "·" } }
            (item)
        }
    }
}

/// Gerçek ölçüm geçmişini grafiğin `n` noktasına indirger. Seri sağa yaslanır:
/// henüz toplanmamış eski zaman dilimi taban çizgisi olarak görünür.
pub fn resample(series: &[f64], n: usize) -> Vec<f64> {
    let slots = crate::state::HISTORY_LEN;
    let tail = &series[series.len().saturating_sub(slots)..];
    let mut full = vec![1.0; slots - tail.len()];
    full.extend(tail.iter().map(|v| v.clamp(1.0, 100.0)));
    // Tam `n` dilim: dilim sınırları kesirli olabildiği için (120/48 = 2,5) dizinle bölünür.
    (0..n)
        .map(|i| {
            let start = i * slots / n;
            let end = ((i + 1) * slots / n).max(start + 1).min(slots);
            full[start..end].iter().sum::<f64>() / (end - start) as f64
        })
        .collect()
}

/// Grafik aralığı seçici (yalnızca gerçek sürücüde: simülasyonun geçmişi yoktur).
fn range_links(p: &Page) -> Markup {
    use crate::history::Range;
    // `back` adresinin sonundaki aralık atılır; her bağlantı kendi aralığını ekler.
    let base = match p.range.as_str() {
        "" => p.back.as_str(),
        range => p.back.strip_suffix(&format!("range={range}")).map(|b| b.trim_end_matches(['?', '&'])).unwrap_or(&p.back),
    };
    let href = |range: Range| match range.as_str() {
        "" => base.to_string(),
        name => format!("{base}{}range={name}", if base.contains('?') { '&' } else { '?' }),
    };
    html! {
        @if p.real {
            nav class="seg" aria-label="Grafik aralığı" {
                @for range in [Range::Recent, Range::Day, Range::Month] {
                    a href=(href(range)) aria-current=[cur(range == p.range)] { "Son " (range.label()) }
                }
            }
        }
    }
}

/// İşlemci ve bellek kullanımı tek grafikte: gerçek sürücüde ölçüm geçmişi, simülasyonda üretilmiş seri.
fn usage_card(p: &Page, prefix: &str, seed: u64, cpu_now: u8, mem_now: u8) -> Markup {
    let series = |metric: &str, seed: u64, base: f64| {
        if p.real {
            resample(p.history.get(&format!("{prefix}:{metric}")).map(Vec::as_slice).unwrap_or(&[]), 48)
        } else {
            metrics::series(seed, base, 48)
        }
    };
    let (span, (left, mid)) = if p.real { (p.range.label(), p.range.axis()) } else { ("60 dakika", ("-60 dk", "-30 dk")) };
    let line = |pts: &[f64]| {
        let last = (pts.len().max(2) - 1) as f64;
        pts.iter().enumerate().map(|(i, v)| format!("{:.1},{:.1}", i as f64 / last * 720.0, 165.0 - v * 1.6)).collect::<Vec<_>>().join(" ")
    };
    let cpu = line(&series("cpu", seed, cpu_now as f64));
    let mem = line(&series("mem", seed + 5, mem_now as f64));
    let grid: String = [45, 85, 125]
        .iter()
        .map(|y| format!(r#"<line x1="0" x2="720" y1="{y}" y2="{y}" stroke="var(--line)" vector-effect="non-scaling-stroke"/>"#))
        .collect();
    let svg = format!(
        r#"<svg viewBox="0 0 720 170" preserveAspectRatio="none" role="img" aria-label="İşlemci ve bellek kullanımı: son {span}">{grid}<polyline points="{mem}" fill="none" stroke="var(--muted)" stroke-width="1.5" stroke-dasharray="4 3" vector-effect="non-scaling-stroke"/><polyline points="{cpu}" fill="none" stroke="var(--ember-hi)" stroke-width="2" stroke-linejoin="round" vector-effect="non-scaling-stroke"/></svg>"#
    );
    html! {
        div class="card chart" {
            header {
                h2 { "Kullanım" }
                span class="legend" { span { i {} "işlemci " (cpu_now) "%" } span { i class="mem" {} "bellek " (mem_now) "%" } }
                div class="end" { (range_links(p)) }
            }
            div class="body" {
                (PreEscaped(svg))
                div class="axis" { span { (left) } span { (mid) } span { "şimdi" } }
            }
            footer {
                @if p.real { "Ölçümler 5 saniyede bir okunur. 24 saatlik grafik dakikalık, 30 günlük grafik saatlik ortalamaları gösterir; sunucunun kapalı olduğu süreler yer almaz." }
                @else { "Grafikler simülasyon sürücüsünden gelir; gerçek ölçüm değildir." }
            }
        }
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
        "yönetici"
    } else if p.operator {
        "operatör"
    } else {
        "salt okunur"
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
            a class="brand" href="/" aria-label="RedoX VE, genel bakış" {
                img class="mark" src=(asset("logo.svg")) alt="" width="33" height="34";
                img class="word" src=(asset("wordmark.svg")) alt="" width="113" height="40";
            }
            nav class="nav" aria-label="Bölümler" {
                div class="eyebrow" { "İşletim" }
                (item("/", "datacenter", "Genel Bakış", matches!(p.current, Current::Datacenter | Current::Node(_)), none()))
                (item("/guests", "vm", "Konuklar", matches!(p.current, Current::Guests | Current::Guest(_)) || at("/guests"),
                    html! { span class="count" { (p.cluster.guests.len()) } }))
                (item("/storage", "storage", "Depolama", p.current == Current::Storage, none()))
                (item("/backups", "backup", "Yedekler", p.current == Current::Backups,
                    html! { @if !p.cluster.backups.is_empty() { span class="count" { (p.cluster.backups.len()) } } }))
                (item("/cluster", "network", "Küme", p.current == Current::Peers, none()))
                div class="eyebrow" { "Yönetim" }
                (item("/tasks", "clock", "Görevler", p.current == Current::Tasks,
                    html! { @if running > 0 { span class="live" aria-label={ (running) " görev sürüyor" } { (running) } } }))
                @if p.admin {
                    (item("/users", "user", "Kullanıcılar", at("/users"), none()))
                    (item("/audit", "shield", "Denetim Günlüğü", at("/audit"), none()))
                }
                (item("/account", "settings", "Hesabım", at("/account"), none()))
                (item("/help", "terminal", "Komut Satırı", at("/help"), none()))
            }
            div class="host" aria-label="Sunucu durumu" {
                div class="name" {
                    span class={ "st " @if s.quorate { "run" } @else { "err" } } style="gap:0" {}
                    (p.cluster.name)
                    span class="dim" style="font-weight:400" { "· " (s.nodes_online) "/" (s.nodes_total) " düğüm" }
                }
                div class="row" { span { "CPU" } (bar(s.cpu_pct, "CPU", true)) span class="num" { (s.cpu_pct) "%" } }
                div class="row" { span { "Bellek" } (bar(s.mem_pct, "Bellek", true)) span class="num" { (s.mem_pct) "%" } }
                div class="row" { span { "Disk" } (bar(s.storage_pct, "Depolama", true)) span class="num" { (s.storage_pct) "%" } }
            }
            div class="me" {
                a class="who" href="/account" { (icon("user")) (p.user) " " small { (role_note(p)) } }
                a class="btn quiet sm sq" href={ "/theme?to=" (next) "&back=" (enc(&p.back)) }
                    aria-label={ @if next == "light" { "Açık temaya geç" } @else { "Koyu temaya geç" } } { (icon("theme")) }
                form method="post" action="/logout" {
                    button class="btn quiet sm sq" type="submit" aria-label="Çıkış" title="Çıkış" { (icon("off")) }
                }
            }
        }
    }
}

/// Üst çubuk: konum yolu, komut alanı ve sayfanın tek birincil eylemi.
fn top(p: &Page, crumbs: &[(&str, &str)]) -> Markup {
    html! {
        div class="top" {
            nav class="crumbs" aria-label="Konum" {
                @for (i, (text, href)) in crumbs.iter().enumerate() {
                    @if i + 1 == crumbs.len() { span aria-current="page" { (text) } }
                    @else { a href=(href) { (text) } b aria-hidden="true" { "/" } }
                }
            }
            form class="cmd" method="post" action="/cmd" role="search" {
                (icon("prompt"))
                input id="cmd" type="text" name="q" autocomplete="off" spellcheck="false" accesskey="k"
                    placeholder="ara ya da komut yaz: start 103, open db-postgres, help"
                    aria-label="Komut satırı: ara ya da komut yaz, Enter ile çalıştır";
                kbd { "Alt K" }
            }
            @if p.admin {
                a class="btn primary" href="/guests/new?kind=vm" { (icon("plus")) "Yeni Konuk" }
            }
        }
    }
}

fn flash(p: &Page) -> Markup {
    html! {
        @if let Some(n) = p.notice {
            div class="flash" role="status" { (icon("check")) span { (n) } }
        }
        @if let Some(e) = p.error {
            div class="flash bad" role="alert" { (icon("alert")) span { (e) } }
        }
    }
}

/// Uygulama kabuğu. `crumbs`: `(yazı, adres)`; sonuncusu bulunulan sayfadır.
pub fn layout(p: &Page, title: &str, crumbs: &[(&str, &str)], narrow: bool, body: Markup) -> String {
    html! {
        (DOCTYPE)
        html lang="tr" data-theme=(p.theme) {
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
                a class="skip" href="#main" { "İçeriğe geç" }
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
            nav class="tabs" aria-label="Bölümler" {
                @for (label, href, on) in tabs {
                    a href=(href) aria-current=[cur(*on)] { (label) }
                }
            }
        }
    }
}

/// Konuk adı hücresi: kimlik, ad, tür ve işaretler.
pub(crate) fn guest_name(g: &Guest, tag_base: &str) -> Markup {
    html! {
        a href={ "/guests/" (g.vmid) } { span class="mono dim" { (g.vmid) } " " (g.name) }
        span class="kind" { (g.kind.label()) }
        @for t in &g.tags { a class="tag" href={ (tag_base) "?q=" (t) } { (t) } }
        @if g.hw.template { span class="tag gold" { "şablon" } }
        @if g.hw.base.is_some() { span class="tag gold" { "bağlı klon" } }
    }
}

fn disk_total(g: &Guest) -> u64 {
    g.disk_gb + g.hw.disks.iter().map(|d| d.size_gb).sum::<u64>()
}

/// Konuk tablosu. `clear`: süzgeci temizleyen adres; `grouped`: düğüme göre gruplanır.
fn guest_table(p: &Page, guests: &[&Guest], clear: &str, filtered: bool, grouped: bool) -> Markup {
    let cols = if p.admin { 8 } else { 7 };
    let row = |g: &Guest| {
        let live = g.status != GuestStatus::Stopped;
        html! {
            tr {
                @if p.admin { td { input type="checkbox" form="bulk" name="id" value=(g.vmid) aria-label={ (g.vmid) " " (g.name) " seç" }; } }
                td class="name" { (guest_name(g, "/guests")) }
                td { (guest_state(g)) }
                td class="mono" { @if live { (bar(g.cpu_pct, "CPU", false)) " " (g.cpu_pct) "%" } @else { span class="dim" { "-" } } }
                td class="mono" {
                    @if live { (bar(g.mem_pct(), "Bellek", false)) " " (mb_as_gb(g.mem_used_mb)) " / " (mb_as_gb(g.mem_total_mb)) " GB" }
                    @else { span class="dim" { (mb_as_gb(g.mem_total_mb)) " GB" } }
                }
                td class="num mono" { (disk_total(g)) " GB" }
                td class="mono muted" { (g.ip.as_deref().unwrap_or("-")) }
                td class="act" {
                    @if p.may_operate(g) {
                        form method="post" action={ "/guests/" (g.vmid) "/action" } {
                            @if live { button class="btn sm" name="action" value="stop" disabled[g.lock.is_some()] { (icon("stop")) "Kapat" } }
                            @else { button class="btn sm go" name="action" value="start" disabled[g.lock.is_some() || g.hw.template] { (icon("play")) "Başlat" } }
                        }
                    }
                }
            }
        }
    };
    html! {
        div class="card" {
            @if guests.is_empty() {
                p class="empty" {
                    @if !filtered {
                        "Burada henüz konuk yok." @if p.admin { " " a href="/guests/new?kind=vm" { "İlk sanal makineyi oluşturun" } "." }
                    } @else {
                        @if p.q.is_empty() { "Bu süzgeçle eşleşen konuk yok. " } @else { "“" (p.q) "” ile eşleşen konuk yok. " }
                        a href=(clear) { "Filtreyi temizleyin" } "."
                    }
                }
            } @else {
                div class="tw" { table {
                    thead { tr {
                        @if p.admin { th { span class="sr" { "Seç" } } }
                        th { "Konuk" } th { "Durum" } th { "CPU" } th { "Bellek" } th class="num" { "Disk" } th { "Adres" } th { span class="sr" { "Eylem" } }
                    } }
                    tbody {
                        @if grouped {
                            @for n in &p.cluster.nodes {
                                @let mine: Vec<&&Guest> = guests.iter().filter(|g| g.node == n.id).collect();
                                @if !mine.is_empty() {
                                    tr class="grp" { td colspan=(cols) { a href={ "/nodes/" (n.id) } { (n.id) } " " (node_state(n)) } }
                                    @for g in mine { (row(g)) }
                                }
                            }
                        } @else {
                            @for g in guests { (row(g)) }
                        }
                    }
                } }
                footer { "Konuğun adına tıklayınca sayfası açılır; satırdaki düğme en olası eylemidir. Komut satırından: " span class="mono" { "redox vm list" } }
            }
        }
    }
}

/// Seçili konuklara toplu eylem: onay kutuları bu forma `form="bulk"` ile bağlanır.
fn bulk_tools(p: &Page, back: &str) -> Markup {
    html! {
        @if p.admin {
            form id="bulk" class="end" method="post" action="/guests/bulk" {
                input type="hidden" name="back" value=(back);
                span class="muted" { "Seçilenler:" }
                div class="group" {
                    button class="btn sm" name="action" value="start" { (icon("play")) "Başlat" }
                    button class="btn sm" name="action" value="stop" { (icon("stop")) "Kapat" }
                    button class="btn sm" name="action" value="reboot" { (icon("restart")) "Yeniden Başlat" }
                }
            }
        }
    }
}

fn feed(tasks: &[&Task], empty: &str) -> Markup {
    html! {
        @if tasks.is_empty() {
            p class="empty" { (empty) }
        } @else {
            ul class="feed" {
                @for t in tasks {
                    li {
                        span class={ "dot" @match t.status { TaskStatus::Running => " warn", TaskStatus::Failed => " err", TaskStatus::Ok => "" } } {}
                        div {
                            div class="what" { "redox " (t.command) }
                            div class="who" {
                                (t.user)
                                @match t.status { TaskStatus::Running => " · Sürüyor", TaskStatus::Failed => " · Hata", TaskStatus::Ok => " · Tamam" }
                                @if let Some(m) = &t.message { ": " (m) }
                            }
                        }
                        time { (hms(t.started)) }
                    }
                }
            }
        }
    }
}

fn guest_uptime(g: &Guest, now: u64) -> String {
    g.uptime_secs(now).map(uptime).unwrap_or_else(|| "-".to_string())
}

pub fn datacenter(p: &Page) -> String {
    let c = p.cluster;
    let s = c.summary();
    let templates = c.guests.iter().filter(|g| g.hw.template).count();
    let stopped = c.guests.iter().filter(|g| g.status == GuestStatus::Stopped && !g.hw.template).count();
    let mut busy: Vec<&Guest> = c.guests.iter().filter(|g| g.status == GuestStatus::Running).collect();
    busy.sort_by(|a, b| b.cpu_pct.cmp(&a.cpu_pct).then(a.vmid.cmp(&b.vmid)));
    busy.truncate(5);
    let recent: Vec<&Task> = c.tasks.iter().take(8).collect();
    let last = c.backups.iter().max_by_key(|b| b.created);
    let unprotected: Vec<&Guest> = c.guests.iter().filter(|g| !g.hw.template && c.backups_of(g.vmid).next().is_none()).take(6).collect();
    let scheduled = c.guests.iter().filter(|g| g.backup_time.is_some()).count();
    let offsite = c.backups.iter().filter(|b| b.offsite).count();
    let health = sub(vec![
        html! { @if s.quorate { span class="st run" { "Küme sağlıklı" } } @else { span class="st err" { "Çoğunluk yok" } } },
        html! { span { (c.name) } },
        html! { span {
            @if s.nodes_online == s.nodes_total { (s.nodes_total) " düğüm çevrimiçi" } @else { (s.nodes_total - s.nodes_online) " düğüm çevrimdışı" }
            ", " (s.guests_total) " konuk"
        } },
    ]);
    let body = html! {
        (page_head(html! { "Genel Bakış" }, health, html! {}, "cluster status", &[]))
        @for a in &crate::alerts::evaluate(c) {
            div class={ "notice" @if a.severity == crate::alerts::Severity::Critical { " err" } } role="status" { (icon("alert")) span { (a.message) } }
        }
        div class="stack" {
            div class="stats" {
                (stat("Konuklar", &s.guests_running.to_string(), &format!(" / {} çalışıyor", s.guests_total), &format!("{stopped} kapalı · {templates} şablon"), None))
                (stat("İşlemci", &s.cpu_pct.to_string(), "%", &format!("{} çekirdek", s.cores), Some(s.cpu_pct)))
                (stat("Bellek", &s.mem_pct.to_string(), "%", &format!("{} / {} GB", mb_as_gb(s.mem_used_mb), mb_as_gb(s.mem_total_mb)), Some(s.mem_pct)))
                (stat("Depolama", &s.storage_pct.to_string(), "%", &format!("{} / {}", gb(s.storage_used_gb), gb(s.storage_total_gb)), Some(s.storage_pct)))
            }
            div class="cols" {
                div class="stack" {
                    div class="card" {
                        header { h2 { "En çok kaynak kullananlar" } span class="note" { "çalışan konuklar" } div class="end" { a class="btn sm quiet" href="/guests" { "Tüm konuklar" } } }
                        @if busy.is_empty() {
                            p class="empty" { "Çalışan konuk yok." @if p.admin && c.guests.is_empty() { " " a href="/guests/new?kind=vm" { "İlk sanal makineyi oluşturun" } "." } }
                        } @else {
                            div class="tw" { table {
                                thead { tr { th { "Konuk" } th { "Durum" } th { "CPU" } th { "Bellek" } } }
                                tbody { @for g in &busy { tr {
                                    td class="name" { a href={ "/guests/" (g.vmid) } { span class="mono dim" { (g.vmid) } " " (g.name) } }
                                    td { (guest_state(g)) }
                                    td class="mono" { (bar(g.cpu_pct, "CPU", false)) " " (g.cpu_pct) "%" }
                                    td class="mono" { (bar(g.mem_pct(), "Bellek", false)) " " (mb_as_gb(g.mem_used_mb)) " / " (mb_as_gb(g.mem_total_mb)) " GB" }
                                } } }
                            } }
                        }
                    }
                    div class="card" {
                        header { h2 { "Düğümler" } span class="note" { (s.nodes_online) " / " (s.nodes_total) " çevrimiçi" } }
                        div class="tw" { table {
                            thead { tr { th { "Düğüm" } th { "Durum" } th { "CPU" } th { "Bellek" } th class="num" { "Konuk" } } }
                            tbody { @for n in &c.nodes { tr {
                                td class="name mono" { a href={ "/nodes/" (n.id) } { (n.id) } }
                                td { (node_state(n)) }
                                td class="mono" { (bar(n.cpu_pct, "CPU", false)) " " (n.cpu_pct) "%" }
                                td class="mono" { (bar(pct(n.mem_used_mb, n.mem_total_mb), "Bellek", false)) " " (pct(n.mem_used_mb, n.mem_total_mb)) "%" }
                                td class="num mono" { (c.guests_on(&n.id).count()) }
                            } } }
                        } }
                    }
                    div class="card" {
                        header { h2 { "Yedek durumu" } div class="end" { a class="btn sm quiet" href="/backups" { "Tüm yedekler" } } }
                        dl class="kv" {
                            dt { "Son yedek" } dd {
                                @match last {
                                    Some(b) => { a class="mono" href={ "/guests/" (b.vmid) "?tab=yedekler" } { (b.id) } span class="muted" { (datetime(b.created)) } },
                                    None => span class="muted" { "Henüz yedek alınmadı." },
                                }
                            }
                            dt { "Zamanlanmış" } dd { (scheduled) " / " (c.guests.len()) " konukta otomatik yedek açık" }
                            dt { "Dış kopya" } dd {
                                @match p.offsite {
                                    Some(target) => { span class="mono" { (target) } span class="muted" { (offsite) " / " (c.backups.len()) " yedek hedefte" } },
                                    None => span class="muted" { "Tanımlı değil; yedekler yalnızca bu sunucuda." },
                                }
                            }
                            dt { "Yedeksiz konuk" } dd {
                                @if unprotected.is_empty() { span class="muted" { "Yok" } }
                                @for g in &unprotected { a href={ "/guests/" (g.vmid) "?tab=yedekler" } { (g.vmid) " " (g.name) } }
                            }
                        }
                    }
                }
                div class="card" {
                    header { h2 { "Etkinlik" } div class="end" { a class="btn sm quiet" href="/tasks" { "Tümü" } } }
                    (feed(&recent, "Henüz görev yok. Bir konuğu başlattığınızda ya da oluşturduğunuzda burada görünür."))
                }
            }
        }
    };
    layout(p, "Genel Bakış", &[("Genel Bakış", "/")], false, body)
}

/// Konuk listesi. `only`: durum süzgeci (`calisan`, `kapali`, `sablon` ya da boş).
pub fn guests(p: &Page, only: &str) -> String {
    let c = p.cluster;
    let q = p.q.to_lowercase();
    let is = |g: &Guest, key: &str| match key {
        "calisan" => g.status != GuestStatus::Stopped,
        "kapali" => g.status == GuestStatus::Stopped && !g.hw.template,
        "sablon" => g.hw.template,
        _ => true,
    };
    let only = if ["calisan", "kapali", "sablon"].contains(&only) { only } else { "" };
    let shown: Vec<&Guest> = c.guests.iter().filter(|g| is(g, only) && matches_filter(g, &q)).collect();
    let running = c.guests.iter().filter(|g| g.status == GuestStatus::Running).count();
    let cores: u32 = c.guests.iter().map(|g| g.cores).sum();
    let mem: u64 = c.guests.iter().map(|g| g.mem_total_mb).sum();
    let href = |key: &str| {
        let mut parts = Vec::new();
        if !key.is_empty() { parts.push(format!("tab={key}")); }
        if !p.q.is_empty() { parts.push(format!("q={}", enc(p.q))); }
        if parts.is_empty() { "/guests".to_string() } else { format!("/guests?{}", parts.join("&")) }
    };
    let body = html! {
        (page_head(
            html! { "Konuklar" },
            sub(vec![
                html! { span { (c.guests.len()) " konuk" } },
                html! { span class="st run" { (running) " çalışıyor" } },
                html! { span { "toplam " (cores) " vCPU, " (mb_as_gb(mem)) " GB bellek ayrılmış" } },
            ]),
            html! {
                @if p.admin {
                    @if p.ct { a class="btn" href="/guests/new?kind=ct" { (icon("plus")) "Konteyner" } }
                    a class="btn primary" href="/guests/new?kind=vm" { (icon("plus")) "Sanal Makine" }
                }
            },
            "vm list",
            &[],
        ))
        div class="tools" {
            nav class="seg" aria-label="Duruma göre süz" {
                @for (key, label) in [("", "Tümü"), ("calisan", "Çalışan"), ("kapali", "Kapalı"), ("sablon", "Şablon")] {
                    a href=(href(key)) aria-current=[(key == only).then_some("true")] { (label) " " b { (c.guests.iter().filter(|g| is(g, key)).count()) } }
                }
            }
            form class="find" method="get" action="/guests" role="search" {
                (icon("prompt"))
                @if !only.is_empty() { input type="hidden" name="tab" value=(only); }
                input type="search" name="q" value=(p.q) placeholder="ad, kimlik ya da etiket" aria-label="Konukları filtrele";
            }
            (bulk_tools(p, "/guests"))
        }
        (guest_table(p, &shown, "/guests", !only.is_empty() || !p.q.is_empty(), true))
    };
    layout(p, "Konuklar", &[("Konuklar", "/guests")], false, body)
}

pub fn node(p: &Page, n: &Node) -> String {
    let c = p.cluster;
    let q = p.q.to_lowercase();
    let all: Vec<&Guest> = c.guests_on(&n.id).collect();
    let shown: Vec<&Guest> = all.iter().copied().filter(|g| matches_filter(g, &q)).collect();
    let running = all.iter().filter(|g| g.status == GuestStatus::Running).count();
    let mp = pct(n.mem_used_mb, n.mem_total_mb);
    let cli = format!("node status {}", n.id);
    let href = format!("/nodes/{}", n.id);
    let body = html! {
        (page_head(
            html! { (n.id) },
            sub(vec![node_state(n), html! { span class="mono" { (n.ip) } }, html! { span { "linux " (n.kernel) } }]),
            html! {},
            &cli,
            &[],
        ))
        div class="stack" {
            div class="stats" {
                (stat("İşlemci", &n.cpu_pct.to_string(), "%", &format!("{} çekirdek", n.cores), Some(n.cpu_pct)))
                (stat("Bellek", &mp.to_string(), "%", &format!("{} / {} GB", mb_as_gb(n.mem_used_mb), mb_as_gb(n.mem_total_mb)), Some(mp)))
                (stat("Konuklar", &all.len().to_string(), "", &format!("{running} çalışıyor"), None))
                (stat("Çalışma Süresi", &uptime(n.uptime_secs), "", &format!("linux {} · {}", n.kernel, n.ip), None))
            }
            (usage_card(p, "node", n.cores as u64, n.cpu_pct, mp))
            div {
                div class="tools" {
                    h2 { "Bu düğümdeki konuklar" }
                    form class="find" method="get" action=(href) role="search" {
                        (icon("prompt"))
                        input type="search" name="q" value=(p.q) placeholder="ad, kimlik ya da etiket" aria-label="Konukları filtrele";
                    }
                    (bulk_tools(p, &href))
                }
                (guest_table(p, &shown, &href, !p.q.is_empty(), false))
            }
        }
    };
    layout(p, &n.id, &[("Genel Bakış", "/"), (&n.id, &href)], false, body)
}

/// Konuk sayfasının başlığındaki eylemler: sık kullanılanlar düğme, kalanı "Diğer" menüsünde.
fn guest_actions(p: &Page, g: &Guest) -> Markup {
    if !p.may_operate(g) {
        return html! { span class="pill idle" { @if p.operator { "Bu konuk yetki kapsamınızda değil" } @else { "Salt okunur hesap" } } };
    }
    let base = format!("/guests/{}", g.vmid);
    let free = g.lock.is_none();
    let can = |a: Action| free && a.target(g.status).is_some();
    let toggle = if g.status == GuestStatus::Paused { Action::Resume } else { Action::Pause };
    let stopped = g.status == GuestStatus::Stopped;
    let live = p.live && g.kind == GuestKind::Vm;
    let can_backup = free && (stopped || live);
    html! {
        form class="acts" method="post" action={ (base) "/action" } {
            button class="btn go" name="action" value="start" disabled[!can(Action::Start) || g.hw.template] { (icon("play")) "Başlat" }
            div class="group" {
                button class="btn" name="action" value="stop" disabled[!can(Action::Stop)] { (icon("stop")) "Kapat" }
                button class="btn" name="action" value="reboot" disabled[!can(Action::Reboot)] { (icon("restart")) "Yeniden Başlat" }
            }
            div class="group" {
                button class="btn" name="action" value=(toggle.as_str()) disabled[!can(toggle)] {
                    (icon(if toggle == Action::Resume { "play" } else { "pause" })) (toggle.label())
                }
                @if p.admin && g.kind == GuestKind::Vm {
                    button class="btn" formaction={ (base) "/suspend" } disabled[stopped || !free] { (icon("half")) "Askıya Al" }
                }
            }
            details class="menu" {
                summary class="btn" { "Diğer" (icon("caret")) }
                div class="menu-list" {
                    button formaction={ (base) "/backups" } disabled[!can_backup] { (icon("backup")) "Şimdi yedekle" }
                    a href={ (base) "?tab=goruntuler" } { (icon("snapshot")) "Anlık görüntü al" }
                    @if p.admin {
                        hr;
                        @if disk_ops(p, g) {
                            a href={ (base) "?tab=cogalt" } { (icon("plus")) "Klonla…" small { "kapalıyken" } }
                            a href={ (base) "?tab=cogalt" } { (icon("vm")) @if g.hw.template { "Şablonluktan çıkar" } @else { "Şablona çevir" } small { "kapalıyken" } }
                            @if p.real { a href={ (base) "?tab=cogalt" } { (icon("disk")) "OVA olarak indir" small { "kapalıyken" } } }
                            hr;
                        }
                        @if free && stopped {
                            a class="danger" href={ (base) "/delete" } { (icon("trash")) "Konuğu sil…" }
                        } @else {
                            button type="button" disabled title="Silmek için önce kapatın" { (icon("trash")) "Konuğu sil…" small { "kapalıyken" } }
                        }
                    }
                }
            }
        }
    }
}

/// Disk işlemleri (büyütme, klon, şablon) bu konukta var mı? Gerçek sürücüde konteynerde yoktur.
pub(crate) fn disk_ops(p: &Page, g: &Guest) -> bool {
    !(p.real && g.kind == GuestKind::Ct)
}

/// Konuk sayfalarının yolu: Konuklar / kimlik ad.
pub(crate) fn guest_label(g: &Guest) -> String {
    format!("{} {}", g.vmid, g.name)
}

/// Konuk başlığı ve sekmeleri; konuğun bütün sekmeleri bunu kullanır.
fn guest_head(p: &Page, g: &Guest, tab: &str) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let link = |label: &'static str, key: &str| (label, format!("{base}?tab={key}"), tab == key);
    let mut tabs = vec![("Özet", base.clone(), tab == "ozet")];
    // Konteynerde konsol, komut çalıştırma olarak yalnızca gerçek sürücüde vardır.
    if p.may_operate(g) && (g.kind == GuestKind::Vm || p.real) {
        tabs.push(link("Konsol", "konsol"));
    }
    tabs.extend([link("Donanım", "donanim"), link("Seçenekler", "secenekler"), link("Anlık Görüntüler", "goruntuler"), link("Yedekler", "yedekler")]);
    if p.admin && disk_ops(p, g) {
        tabs.push(link("Çoğalt ve Taşı", "cogalt"));
    }
    let cli = format!("{} show {}", g.kind, g.vmid);
    let mut facts = vec![guest_state(g), html! { a href={ "/nodes/" (g.node) } { (g.node) } }];
    if let Some(ip) = &g.ip {
        facts.push(html! { span class="mono" { (ip) } });
    }
    facts.push(html! { span { (g.agent.as_deref().unwrap_or(&g.os)) } });
    if g.status != GuestStatus::Stopped {
        facts.push(html! { span { (guest_uptime(g, p.now)) " açık" } });
    }
    html! {
        (page_head(
            html! {
                span class="id" { (g.vmid) } (g.name)
                @if g.hw.template { span class="tag gold" { "şablon" } }
                @if g.hw.base.is_some() { span class="tag gold" { "bağlı klon" } }
            },
            sub(facts),
            guest_actions(p, g),
            &cli,
            &tabs,
        ))
        @if g.hw.suspended {
            div class="notice" role="status" {
                (icon("half"))
                span { "Bu konuk askıda: başlatırsanız kaldığı yerden devam eder. Askıdayken disk ve donanım değiştirilemez." }
                @if p.admin {
                    form method="post" action={ (base) "/discard-suspend" } {
                        button class="btn sm danger" type="submit" disabled[g.lock.is_some()] title="Konuk sıfırdan açılır; kaydedilmemiş bellek içeriği kaybolur" { "Askı Durumunu At" }
                    }
                }
            }
        }
    }
}

fn firewall_text(g: &Guest) -> String {
    if !g.hw.firewall {
        "kapalı".to_string()
    } else if g.hw.fw_rules.is_empty() {
        "açık · gelen trafik kapalı".to_string()
    } else {
        format!("açık · {}", g.hw.fw_rules.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
    }
}

fn overview(p: &Page, g: &Guest) -> Markup {
    let c = p.cluster;
    let base = format!("/guests/{}", g.vmid);
    let mp = g.mem_pct();
    let mine: Vec<&Task> = c.tasks.iter().filter(|t| t.vmid == Some(g.vmid)).take(6).collect();
    let backups: Vec<_> = c.backups_of(g.vmid).collect();
    let last = backups.iter().max_by_key(|b| b.created);
    let extra: String = g.hw.disks.iter().map(|d| format!(" + {}", d.size_gb)).collect();
    let disk_detail = if extra.is_empty() { g.storage.clone() } else { format!("{}{extra} GB · {}", g.disk_gb, g.storage) };
    html! {
        div class="stack" {
            div class="stats" {
                (stat("İşlemci", &g.cpu_pct.to_string(), "%", &format!("{} vCPU", g.cores), Some(g.cpu_pct)))
                (stat("Bellek", &mp.to_string(), "%", &format!("{} / {} GB", mb_as_gb(g.mem_used_mb), mb_as_gb(g.mem_total_mb)), Some(mp)))
                (stat("Disk", &disk_total(g).to_string(), " GB", &disk_detail, None))
                (stat("Çalışma Süresi", &guest_uptime(g, p.now), "", &format!("düğüm {}", g.node), None))
            }
            div class="cols" {
                div class="stack" {
                    (usage_card(p, &format!("guest:{}", g.vmid), g.vmid as u64, if g.status == GuestStatus::Running { g.cpu_pct } else { 0 }, mp))
                    div class="card" {
                        header { h2 { "Bu konuğun etkinliği" } div class="end" { a class="btn sm quiet" href={ "/tasks?q=" (g.vmid) } { "Tümü" } } }
                        (feed(&mine, "Bu konuk için kayıtlı görev yok."))
                    }
                }
                div class="stack" {
                    div class="card" {
                        header { h2 { "Konuk" } span class="note mono" { (g.vmid) ".conf" } }
                        dl class="kv" {
                            dt { "Tür" } dd { (g.kind.long_label()) " (" (g.kind.backend()) ")" }
                            dt { "İşletim sistemi" } dd class="mono" { (g.os) }
                            @if g.kind == GuestKind::Vm && p.real {
                                dt { "Konuk ajanı" } dd {
                                    @match (&g.agent, g.status) {
                                        (Some(os), _) => { span class="pill ok" { (icon("check")) "Çalışıyor" } span class="mono" { (os) } },
                                        (None, GuestStatus::Stopped) => { span class="dim" { "-" } },
                                        (None, _) => { span class="dim" { "Yanıt yok" } span class="muted" { "(konukta qemu-guest-agent kurulursa adres ve tutarlı yedek gelir)" } },
                                    }
                                }
                            }
                            dt { "IP adresi" } dd class="mono" { (g.ip.as_deref().unwrap_or("-")) }
                            dt { "Ağ" } dd class="mono" {
                                @if g.bridge == redox_core::NAT { "NAT" } @else { "köprü: " (g.bridge) @if g.hw.vlan > 0 { " · VLAN " (g.hw.vlan) } }
                                @for (i, n) in g.hw.nics.iter().enumerate() {
                                    @let (network, vlan) = redox_core::hw::parse_nic(n).unwrap_or((n.as_str(), 0));
                                    span class="dim" { "+ " @if network == redox_core::NAT { "NAT" } @else { (network) } @if vlan > 0 { " · VLAN " (vlan) } @if p.real { " (" (redox_core::mac_for_nic(g.vmid, i as u8 + 1)) ")" } }
                                }
                            }
                            @if p.real { dt { "MAC adresi" } dd class="mono" { (redox_core::mac_for(g.vmid)) } }
                            dt { "Donanım" } dd class="mono" {
                                (g.cores) " vCPU · " (g.mem_total_mb) " MB"
                                @if g.kind == GuestKind::Vm { " · " (g.hw.firmware_label()) @if g.hw.tpm { " · TPM 2.0" } }
                            }
                            dt { "Disk" } dd class="mono" { (g.disk_gb) " GB · " (g.storage) @for d in &g.hw.disks { " + " (d.size_gb) " GB" } }
                            @if g.hw.disk_mbps > 0 || g.hw.disk_iops > 0 || g.hw.net_mbps > 0 {
                                dt { "Hız sınırı" } dd class="mono" {
                                    @if g.hw.disk_mbps > 0 { span { "disk " (g.hw.disk_mbps) " MB/sn" } }
                                    @if g.hw.disk_iops > 0 { span { "disk " (g.hw.disk_iops) " işlem/sn" } }
                                    @if g.hw.net_mbps > 0 { span { "ağ " (g.hw.net_mbps) " Mbit/sn" } }
                                }
                            }
                            dt { "Kurulum kalıbı" } dd class="mono" { (g.iso.as_deref().unwrap_or("-")) }
                            @if let Some(image) = &g.hw.image { dt { "Bulut imajı" } dd class="mono" { (image) } }
                            @if let Some(key) = &g.hw.ci_ssh_key {
                                dt { "cloud-init" } dd class="mono" {
                                    (g.hw.ci_user.as_deref().unwrap_or(redox_core::hw::DEFAULT_CI_USER)) " · " (key.split(' ').next().unwrap_or("")) " " (key.split(' ').nth(2).unwrap_or(""))
                                }
                            }
                            @if !g.hw.usb.is_empty() || !g.hw.pci.is_empty() {
                                dt { "Aktarılan aygıtlar" } dd class="mono" {
                                    @for (i, d) in g.hw.usb.iter().map(|u| format!("USB {u}")).chain(g.hw.pci.iter().map(|p| format!("PCI {p}"))).enumerate() { @if i > 0 { ", " } (d) }
                                }
                            }
                            @if g.hw.template { dt { "Şablon" } dd { span class="tag gold" { "şablon" } "Başlatılamaz; klon kaynağıdır." } }
                            @if let Some(b) = g.hw.base { dt { "Şablon" } dd { "Bağlı klon: " a class="mono" href={ "/guests/" (b) } { (b) } } }
                            dt { "Açılışta başlat" } dd {
                                @if g.onboot {
                                    "Evet"
                                    @if g.hw.boot_order > 0 { span class="muted" { "sıra " (g.hw.boot_order) } }
                                    @if g.hw.boot_delay > 0 { span class="muted" { "sonra " (g.hw.boot_delay) " sn bekle" } }
                                } @else { "Hayır" }
                            }
                            @if !g.hw.shares.is_empty() {
                                dt { "Paylaşımlar" } dd class="mono" { @for s in &g.hw.shares { span class="tag" { (s.name) @if s.readonly { " · salt okunur" } } } }
                            }
                            @if g.hw.restart { dt { "Kapanırsa" } dd { "kendiliğinden yeniden başlatılır" } }
                            @if !g.hw.schedules.is_empty() {
                                dt { "Zamanlanmış" } dd {
                                    @for s in &g.hw.schedules { span class="tag" { (s.action.label()) " " (s.time) " · " (s.days_label()) } }
                                }
                            }
                            dt { "Etiketler" } dd { @if g.tags.is_empty() { "-" } @for t in &g.tags { a class="tag" href={ "/guests?q=" (t) } { (t) } } }
                            dt { "Güvenlik duvarı" } dd class="mono" {
                                (firewall_text(g))
                                @if g.hw.firewall && p.real && !crate::firewall::enforced(g) { span class="pill warn" { (icon("alert")) "NAT'ta etkisiz" } }
                            }
                            dt { "Port yönlendirme" } dd class="mono" {
                                @if g.forwards.is_empty() { "-" }
                                @for f in &g.forwards { (f.host_port) " → " (f.guest_port) " " }
                            }
                        }
                    }
                    div class="card" {
                        header { h2 { "Koruma" } }
                        dl class="kv" {
                            dt { "Son yedek" } dd {
                                @match last {
                                    Some(b) => {
                                        a href={ (base) "?tab=yedekler" } { (datetime(b.created)) }
                                        @if b.cbt { span class="tag" { "hızlı" } }
                                        @if b.quiesced == Some(true) { span class="tag" { "donduruldu" } }
                                    },
                                    None => span class="muted" { "Yedek yok" },
                                }
                            }
                            dt { "Zamanlama" } dd {
                                @match &g.backup_time {
                                    Some(t) => { "her gün " span class="mono" { (t) } ", son " (g.backup_keep) },
                                    None => span class="muted" { "kapalı" },
                                }
                            }
                            dt { "Dış kopya" } dd {
                                @if backups.is_empty() { span class="dim" { "-" } }
                                @else { (backups.iter().filter(|b| b.offsite).count()) " / " (backups.len()) }
                            }
                            dt { "Görüntüler" } dd { a href={ (base) "?tab=goruntuler" } { (g.snapshots.len()) " anlık görüntü" } }
                        }
                    }
                }
            }
        }
    }
}

pub fn guest(p: &Page, g: &Guest, tab: &str, opts: &FormOptions) -> String {
    let known = ["konsol", "donanim", "secenekler", "goruntuler", "yedekler", "cogalt"];
    let allowed = match tab {
        "konsol" => p.may_operate(g),
        "cogalt" => p.admin && disk_ops(p, g),
        _ => true,
    };
    let tab = if known.contains(&tab) && allowed { tab } else { "ozet" };
    let form = crate::view_ops::EditForm::from_guest(g);
    let content = match tab {
        "konsol" => crate::view_ops::console_tab(p, g, opts.vnc),
        "goruntuler" => crate::view_ops::snapshots_tab(p, g),
        "yedekler" => crate::view_ops::backups_tab(p, g),
        "donanim" => crate::view_ops::hardware_tab(p, g, &form, opts),
        "secenekler" => crate::view_ops::options_tab(p, g, &form),
        "cogalt" => crate::view_ops::clone_tab(p, g),
        _ => overview(p, g),
    };
    let body = html! {
        (guest_head(p, g, tab))
        (content)
    };
    let label = guest_label(g);
    layout(p, &label, &[("Konuklar", "/guests"), (&label, "")], false, body)
}

pub fn storage(p: &Page, media: &[crate::media::Media]) -> String {
    let c = p.cluster;
    let total_mb: u64 = media.iter().map(|m| m.bytes.div_ceil(1024 * 1024)).sum();
    let body = html! {
        (page_head(
            html! { "Depolama" },
            sub(vec![
                html! { span { (c.storages.len()) " depolama alanı" } },
                html! { @if p.real { span { (media.len()) " kalıp ve imaj, " (total_mb) " MB" } } @else { span { "simülasyon" } } },
            ]),
            html! {},
            "storage list",
            &[],
        ))
        div class="stack" {
            div class="card" {
                header { h2 { "Depolama alanları" } }
                div class="tw" { table {
                    thead { tr { th { "Alan" } th { "Tür" } th { "İçerik" } th style="width:34%" { "Kullanım" } th class="num" { "Dolu / Toplam" } } }
                    tbody { @for s in &c.storages { tr {
                        td class="mono" { (s.id) span class="sub" { (s.node.as_deref().unwrap_or("paylaşımlı")) } }
                        td { (s.kind.label()) }
                        td class="muted" { (s.content.join(", ")) }
                        td class="mono" {
                            (bar(s.used_pct(), &format!("{} kullanım", s.id), false)) " " (s.used_pct()) "%"
                            @if s.used_pct() >= 90 { " " span class="pill err" { (icon("alert")) "Dolmak üzere" } }
                        }
                        td class="num mono" { (gb(s.used_gb)) " / " (gb(s.total_gb)) }
                    } } }
                } }
            }
            @if p.real {
                div class="card" {
                    header { h2 { "Kalıplar ve disk imajları" } span class="note" { "konuk oluştururken seçilir" } }
                    @if media.is_empty() {
                        p class="empty" { "Henüz dosya yok. Kurulum kalıbı (.iso) ya da hazır disk imajı (.qcow2, .img, .vmdk, .ova …) yükleyin; konuk oluştururken seçilebilir olurlar." }
                    } @else {
                        div class="tw" { table {
                            thead { tr { th { "Dosya" } th { "Tür" } th class="num" { "Boyut" } th { "Kullanan" } th { span class="sr" { "İşlem" } } } }
                            tbody { @for m in media { tr {
                                td class="mono" { (m.name) }
                                td { @if m.iso { "Kurulum kalıbı" } @else if let Some(ova) = &m.ova {
                                    "OVA arşivi"
                                    @if ova.cores.is_some() || ova.memory_mb.is_some() {
                                        span class="dim" { " · öneri: " @if let Some(c) = ova.cores { (c) " çekirdek" } @if let (Some(_), Some(_)) = (ova.cores, ova.memory_mb) { ", " } @if let Some(mb) = ova.memory_mb { (mb) " MB" } }
                                    }
                                } @else { "Disk imajı" } }
                                td class="num mono" { (m.bytes.div_ceil(1024 * 1024)) " MB" }
                                td class="mono" { @if m.used_by.is_empty() { span class="dim" { "-" } } @for (i, id) in m.used_by.iter().enumerate() { @if i > 0 { ", " } a class="link" href={ "/guests/" (id) } { (id) } } }
                                td class="act" {
                                    @if p.admin { form method="post" action={ "/media/" (m.name) "/delete" } { button class="btn sm danger" type="submit" { (icon("trash")) "Sil" } } }
                                }
                            } } }
                        } }
                    }
                    @if p.admin {
                        div class="intake" {
                            form method="post" action="/media/upload" enctype="multipart/form-data" {
                                h3 { label for="media-file" { "Bilgisayardan yükle" } }
                                div class="line" {
                                    input id="media-file" name="file" type="file" required accept=".iso,.qcow2,.img,.raw,.vmdk,.vdi,.vhdx,.vhd,.ova";
                                    button class="btn" type="submit" { (icon("plus")) "Yükle" }
                                }
                                p { "Dosya adı türü belirler (.iso kalıptır). Yükleme bitene kadar sayfa bekler." }
                            }
                            form method="post" action="/media/fetch" {
                                h3 { label for="media-url" { "Adresten indir" } }
                                div class="line" { input id="media-url" name="url" type="url" required placeholder="https://…/debian-12.iso" autocomplete="off"; }
                                div class="line" {
                                    input name="sha256" type="text" placeholder="SHA-256 (isteğe bağlı)" autocomplete="off" aria-label="SHA-256 özeti";
                                    button class="btn" type="submit" { (icon("migrate")) "İndir" }
                                }
                                p { "Büyük dosyalar için adresten indirme arka planda çalışır ve sonucu görev günlüğüne yazar. Yalnızca https adresleri ve genel internet adresleri kabul edilir. SHA-256 verirseniz dosya onunla doğrulanır." }
                            }
                        }
                    }
                }
            }
        }
    };
    layout(p, "Depolama", &[("Depolama", "/storage")], false, body)
}

/// Birleşik görünüm: bu sunucu ve eş sunucuların konukları tek sayfada, salt okunur.
pub fn peers(p: &Page, peers: &[crate::peers::PeerView]) -> String {
    let c = p.cluster;
    let local = c.summary();
    let online = peers.iter().filter(|v| v.online()).count();
    let guests = local.guests_total + peers.iter().map(|v| v.guests.len()).sum::<usize>();
    let running = local.guests_running + peers.iter().map(|v| v.running()).sum::<usize>();
    let mut facts = vec![html! { span { (running) "/" (guests) " konuk çalışıyor" } }];
    if !peers.is_empty() {
        facts.insert(0, html! {
            @if online == peers.len() { span class="pill ok" { (icon("check")) (online) "/" (peers.len()) " eş erişilebilir" } }
            @else { span class="pill err" { (icon("alert")) (online) "/" (peers.len()) " eş erişilebilir" } }
        });
    }
    let body = html! {
        (page_head(html! { "Küme" }, sub(facts), html! {}, "peer list", &[]))
        div class="stack" {
            div class="card" {
                header { h2 { "Sunucular" } }
                div class="tw" { table {
                    thead { tr { th { "Sunucu" } th { "Küme adı" } th { "Durum" } th class="num" { "Düğüm" } th class="num" { "Konuk" } th { "Son okuma" } } }
                    tbody {
                        tr {
                            td class="name mono" { a href="/" { "bu sunucu" } }
                            td class="mono muted" { (c.name) }
                            td { span class="pill ok" { (icon("check")) "Yerel" } }
                            td class="num mono" { (local.nodes_online) "/" (local.nodes_total) }
                            td class="num mono" { (local.guests_running) "/" (local.guests_total) }
                            td class="mono muted" { "canlı" }
                        }
                        @for v in peers { tr {
                            td class="name mono" { a href=(v.url) rel="noreferrer" { (v.name) } }
                            td class="mono muted" { @if v.cluster.is_empty() { "-" } @else { (v.cluster) } }
                            td {
                                @if v.online() { span class="pill ok" { (icon("check")) "Erişilebilir" } }
                                @else if let Some(e) = &v.error { span class="pill err" { (icon("alert")) (e) } }
                                @else { span class="pill warn" { (icon("clock")) "Bekleniyor" } }
                            }
                            td class="num mono" { @if v.online() { (v.nodes.iter().filter(|n| n.status == NodeStatus::Online).count()) "/" (v.nodes.len()) } @else { "-" } }
                            td class="num mono" { @if v.online() { (v.running()) "/" (v.guests.len()) } @else { "-" } }
                            td class="mono muted" { @match v.checked { Some(t) => (hms(t)), None => "-" } }
                        } }
                    }
                } }
                @if peers.is_empty() {
                    p class="empty" {
                        "Eş sunucu tanımlı değil. Başka RedoX VE sunucularını burada görmek için sunucuyu "
                        span class="mono" { "--peers DOSYA" } " ile başlatın; dosyanın her satırı "
                        span class="mono" { "AD ADRES TOKEN" } " biçimindedir (eşte salt okunur bir kullanıcının token'ı yeterlidir)."
                    }
                }
                footer {
                    "Her sunucu kendi konuklarını yönetir: bir konuğu yönetmek için adına tıklayıp o sunucunun arayüzüne gidin. "
                    "“Buraya Al” kapalı bir sanal makineyi yedeği üzerinden bu sunucuya kopyalar; kaynak yerinde kalır, çalıştığını görünce orada siz silersiniz. "
                    "“Çalışırken Al” çalışan konuğu orada askıya alır, diskini ve belleğini kopyalar, burada kaldığı yerden sürdürür: konuk yeniden başlamaz ama kopyalama boyunca yanıt vermez. "
                    "Kaynaktaki kopya askıda kalır; burada çalıştığını görünce orada silin, başlatmayın. "
                    "Kesintisiz canlı taşıma, ortak küme durumu ve sunucular arası kendiliğinden devralma yoktur."
                }
            }
            @for v in peers.iter().filter(|v| v.online()) {
                div class="card" {
                    header { h2 { (v.name) } span class="note mono" { (v.url) } }
                    @if v.guests.is_empty() {
                        p class="empty" { "Bu sunucuda konuk yok." }
                    } @else {
                        div class="tw" { table {
                            thead { tr { th { "Konuk" } th { "Düğüm" } th { "Durum" } th { "CPU" } th { "Bellek" } @if p.admin && p.real { th { span class="sr" { "Taşı" } } } } }
                            tbody { @for g in &v.guests { tr {
                                td class="name" { a href={ (v.url) "/guests/" (g.vmid) } rel="noreferrer" { span class="mono dim" { (g.vmid) } " " (g.name) } span class="kind" { (g.kind.label()) } }
                                td class="mono muted" { (g.node) }
                                td { span class={ "st " (status_class(g.status)) } { (g.status.label()) } }
                                td class="mono" { (g.cpu_pct) "%" }
                                td class="mono" { (mb_as_gb(g.mem_used_mb)) " / " (mb_as_gb(g.mem_total_mb)) " GB" }
                                @if p.admin && p.real {
                                    td class="act" {
                                        @if g.kind == GuestKind::Vm && g.lock.is_none() {
                                            form method="post" action="/cluster/import" {
                                                input type="hidden" name="peer" value=(v.name);
                                                input type="hidden" name="vmid" value=(g.vmid);
                                                @if g.status == GuestStatus::Stopped && !g.hw.suspended {
                                                    button class="btn sm" type="submit" { (icon("migrate")) "Buraya Al" }
                                                } @else {
                                                    button class="btn sm" type="submit" title="Konuk orada askıya alınır, diski ve belleği kopyalanır, burada kaldığı yerden devam eder" { (icon("migrate")) "Çalışırken Al" }
                                                }
                                            }
                                        } @else { span class="dim" title="Yalnızca sanal makineler taşınır; üzerinde işlem süren konuk bekler" { "-" } }
                                    }
                                }
                            } } }
                        } }
                    }
                }
            }
        }
    };
    layout(p, "Küme", &[("Küme", "/cluster")], false, body)
}

/// Görev günlüğü. `q` konuk kimliğine ya da komut metnine göre süzer.
pub fn tasks(p: &Page) -> String {
    let q = p.q.to_lowercase();
    let shown: Vec<&Task> = p
        .cluster
        .tasks
        .iter()
        .filter(|t| q.is_empty() || t.command.to_lowercase().contains(&q) || t.user.to_lowercase().contains(&q) || t.vmid.is_some_and(|v| v.to_string() == q))
        .collect();
    let running = p.cluster.tasks.iter().filter(|t| t.status == TaskStatus::Running).count();
    let body = html! {
        (page_head(
            html! { "Görevler" },
            sub(vec![
                html! { @if running > 0 { span class="pill warn" { (icon_c("restart", "spin")) (running) " sürüyor" } } @else { span { "boşta" } } },
                html! { span { (p.cluster.tasks.len()) " kayıt" } },
            ]),
            html! {},
            "task list",
            &[],
        ))
        div class="tools" {
            form class="find" method="get" action="/tasks" role="search" {
                (icon("prompt"))
                input type="search" name="q" value=(p.q) placeholder="konuk kimliği, komut ya da kullanıcı" aria-label="Görevleri süz";
            }
        }
        div class="card" {
            @if shown.is_empty() {
                p class="empty" {
                    @if q.is_empty() { "Henüz görev yok. Bir konuğu başlattığınızda ya da oluşturduğunuzda burada görünür." }
                    @else { "“" (p.q) "” ile eşleşen görev yok. " a href="/tasks" { "Süzgeci temizleyin" } "." }
                }
            } @else {
                div class="tw" { table {
                    thead { tr { th { "Başlangıç" } th { "Bitiş" } th { "Düğüm" } th { "Kullanıcı" } th { "Komut" } th { "Durum" } } }
                    tbody { @for t in &shown { tr {
                        td class="mono dim" { (hms(t.started)) }
                        td class="mono dim" { @if let Some(e) = t.ended { (hms(e)) } @else { "--:--:--" } }
                        td class="mono muted" { (t.node) }
                        td class="muted" { (t.user) }
                        td class="mono" { span class="gold" { "$" } " redox " (t.command) }
                        td class="wrap" { (task_pill(t)) }
                    } } }
                } }
            }
        }
    };
    layout(p, "Görevler", &[("Görevler", "/tasks")], false, body)
}

/// Bütün konukların yedekleri tek listede.
pub fn backups(p: &Page) -> String {
    let c = p.cluster;
    let mut all: Vec<_> = c.backups.iter().collect();
    all.sort_by_key(|b| std::cmp::Reverse(b.created));
    let offsite = all.iter().filter(|b| b.offsite).count();
    let body = html! {
        (page_head(
            html! { "Yedekler" },
            sub(vec![
                html! { span { (all.len()) " yedek" } },
                html! { @match p.offsite {
                    Some(target) => span { (offsite) " / " (all.len()) " dış hedefte: " span class="mono" { (target) } },
                    None => span { "dış hedef tanımlı değil" },
                } },
            ]),
            html! {},
            "backup list",
            &[],
        ))
        div class="card" {
            @if all.is_empty() {
                p class="empty" { "Henüz yedek yok. Yedek, bir konuğun sayfasındaki Yedekler sekmesinden ya da zamanlamayla alınır." }
            } @else {
                div class="tw" { table {
                    thead { tr { th { "Yedek" } th { "Konuk" } th { "Alındığı an" } th class="num" { "Boyut" } th { "Dış kopya" } } }
                    tbody { @for b in &all { tr {
                        td class="mono" {
                            a class="link" href={ "/guests/" (b.vmid) "?tab=yedekler" } { (b.id) }
                            @if b.auto { " " span class="tag" { "otomatik" } }
                            @if b.new_mb.is_some() { " " span class="tag gold" { "artımlı" } }
                        }
                        td class="name" { a href={ "/guests/" (b.vmid) } { span class="mono dim" { (b.vmid) } " " (b.guest_name) } }
                        td class="mono muted" { (datetime(b.created)) }
                        td class="num mono" { (b.size_mb) " MB" }
                        td { @if b.offsite { span class="pill ok" { (icon("check")) "Var" } } @else { span class="pill idle" { "yalnızca yerel" } } }
                    } } }
                } }
                footer { "Geri yükleme, doğrulama ve silme konuğun Yedekler sekmesinden yapılır." }
            }
        }
    };
    layout(p, "Yedekler", &[("Yedekler", "/backups")], false, body)
}

/// Konuk oluşturma formunun (doğrulanmamış) alanları.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct NewGuestForm {
    pub kind: String,
    pub node: String,
    pub vmid: String,
    pub name: String,
    pub os: String,
    pub storage: String,
    pub disk_gb: String,
    pub cores: String,
    pub memory_mb: String,
    pub bridge: String,
    pub onboot: Option<String>,
    pub iso: String,
    pub image: String,
    pub ci_user: String,
    pub ci_ssh_key: String,
    pub firmware: String,
    pub tpm: Option<String>,
}

impl NewGuestForm {
    pub fn defaults(kind: GuestKind, cluster: &Cluster) -> Self {
        let node = cluster.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        let storage = cluster
            .storages
            .iter()
            .find(|s| s.available_on(&node))
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let (disk, mem) = match kind {
            GuestKind::Vm => ("32", "2048"),
            GuestKind::Ct => ("8", "512"),
        };
        NewGuestForm {
            kind: kind.as_str().to_string(),
            node,
            vmid: String::new(),
            name: String::new(),
            os: "debian-12".to_string(),
            storage,
            disk_gb: disk.to_string(),
            cores: "2".to_string(),
            memory_mb: mem.to_string(),
            bridge: "vmbr0".to_string(),
            onboot: None,
            iso: String::new(),
            ..Default::default()
        }
    }

    pub fn kind(&self) -> GuestKind {
        self.kind.parse().unwrap_or(GuestKind::Vm)
    }
}

/// Formlardaki seçim kutularının sürücüden gelen seçenekleri.
#[derive(Debug, Clone, Default)]
pub struct FormOptions {
    pub isos: Vec<String>,
    pub bridges: Vec<String>,
    pub templates: Vec<String>,
    /// Hazır bulut imajları.
    pub images: Vec<String>,
    /// Görüntülenen konuğun canlı konsolu (VNC soketi) hazır mı?
    pub vnc: bool,
}

/// `(alan, ileti)`; alan boşsa hata forma geneldir.
pub type FieldErrors = Vec<(&'static str, String)>;

pub fn new_guest(p: &Page, f: &NewGuestForm, errors: &FieldErrors, opts: &FormOptions) -> String {
    let isos = &opts.isos;
    let c = p.cluster;
    let kind = f.kind();
    let title = match kind {
        GuestKind::Vm => "Sanal Makine Oluştur",
        GuestKind::Ct => "Konteyner Oluştur",
    };
    let cli = format!("{kind} create");
    let err = |name: &str| errors.iter().find(|(n, _)| *n == name).map(|(_, m)| m.as_str());
    let field = |name: &'static str, label: &str, required: bool, hint: &str, control: Markup| {
        let e = err(name);
        html! {
            div class={ "field" @if e.is_some() { " bad" } } {
                label for=(name) { (label) @if required { " " span class="req" aria-hidden="true" { "*" } } }
                (control)
                @if let Some(m) = e { span class="err" id={ (name) "-err" } { (m) } }
                @else if !hint.is_empty() { span class="hint" { (hint) } }
            }
        }
    };
    let describe = |name: &str| err(name).map(|_| format!("{name}-err"));
    let mut storage_ids: Vec<&str> = c.storages.iter().map(|s| s.id.as_str()).collect();
    storage_ids.sort_unstable();
    storage_ids.dedup();

    let body = html! {
        (page_head(html! { (title) }, html! { span { "Konuk durdurulmuş halde eklenir; sonra sayfasından başlatabilirsiniz." } }, html! {}, &cli, &[]))
        @if !errors.is_empty() {
            div class="errsum" role="alert" {
                h2 { "Konuk oluşturulamadı" }
                ul { @for (name, m) in errors {
                    li { @if name.is_empty() { (m) } @else { a href={ "#" (name) } { (m) } } }
                } }
            }
        }
        form method="post" action="/guests" novalidate {
            input type="hidden" name="kind" value=(kind.as_str());
            fieldset {
                legend { "Genel" }
                div class="form" {
                    (field("node", "Düğüm", true, "", html! {
                        select id="node" name="node" aria-describedby=[describe("node")] {
                            @for n in &c.nodes { option value=(n.id) selected[n.id == f.node] { (n.id) @if n.status == NodeStatus::Offline { " (çevrimdışı)" } } }
                        }
                    }))
                    (field("vmid", "Kimlik", false, &format!("Boş bırakırsanız {} atanır.", c.next_vmid()), html! {
                        input id="vmid" name="vmid" type="text" inputmode="numeric" value=(f.vmid) placeholder=(c.next_vmid()) aria-describedby=[describe("vmid")];
                    }))
                    (field("name", "Ad", true, "Küçük harf, rakam ve tire; örn. web-prod-03.", html! {
                        input id="name" name="name" type="text" value=(f.name) required autocomplete="off" aria-describedby=[describe("name")];
                    }))
                    @if kind == GuestKind::Ct && !opts.templates.is_empty() {
                        (field("os", "Şablon", true, "Kök dosya sistemi bu dağıtımın hazır imajından indirilir.", html! {
                            select id="os" name="os" { @for t in &opts.templates { option value=(t) selected[*t == f.os] { (t) } } }
                        }))
                    } @else {
                        (field("os", "İşletim sistemi", false, "Bilgi amaçlı etiket.", html! {
                            input id="os" name="os" type="text" value=(f.os);
                        }))
                    }
                    @if kind == GuestKind::Vm && p.real {
                        (field("iso", "Kurulum kalıbı (ISO)", false,
                            if isos.is_empty() { "Kalıp yok. ISO dosyalarını Depolama sayfasından yükleyin ya da sunucuda veri dizininin iso/ klasörüne koyun." } else { "İlk açılışta bu kalıptan başlar." },
                            html! {
                                select id="iso" name="iso" {
                                    option value="" { "(yok)" }
                                    @for i in isos { option value=(i) selected[*i == f.iso] { (i) } }
                                }
                            }))
                    }
                }
            }
            @if kind == GuestKind::Vm && p.real {
                fieldset {
                    legend { "Hazır imaj ve ilk kurulum (cloud-init)" }
                    div class="form" {
                        (field("image", "Disk imajı", false,
                            if opts.images.is_empty() { "İmaj yok. Bulut imajını (qcow2) ya da başka sanallaştırıcıdan aldığınız diski (vmdk, vdi, vhdx) Depolama sayfasından yükleyin ya da sunucuda veri dizininin cloud/ klasörüne koyun." } else { "Disk bu imajdan kopyalanır (vmdk, vdi, vhdx qcow2'ye çevrilir) ve aşağıdaki boyuta büyütülür." },
                            html! {
                                select id="image" name="image" {
                                    option value="" { "(boş disk)" }
                                    @for i in &opts.images { option value=(i) selected[*i == f.image] { (i) } }
                                }
                            }))
                        (field("ci_user", "Kullanıcı", false, "Boş bırakırsanız “redox”. Parolasız sudo yetkisiyle açılır.", html! {
                            input id="ci_user" name="ci_user" type="text" value=(f.ci_user) autocomplete="off" spellcheck="false";
                        }))
                        div class="field full" {
                            label for="ci_ssh_key" { "SSH açık anahtarı" }
                            textarea id="ci_ssh_key" name="ci_ssh_key" rows="3" spellcheck="false" autocomplete="off" { (f.ci_ssh_key) }
                            span class="hint" { "Tek satır: ssh-ed25519 AAAA… Bulut imajlarında parola ile giriş kapalıdır; anahtar vermezseniz giriş yapamazsınız." }
                        }
                    }
                }
            }
            fieldset {
                legend { "Disk" }
                div class="form" {
                    (field("storage", "Depolama", true, "Seçtiğiniz düğümde kullanılabilir olmalı.", html! {
                        select id="storage" name="storage" aria-describedby=[describe("storage")] {
                            @for id in &storage_ids { option value=(id) selected[*id == f.storage] { (id) } }
                        }
                    }))
                    (field("disk_gb", "Boyut (GB)", true, "", html! {
                        input id="disk_gb" name="disk_gb" type="text" inputmode="numeric" value=(f.disk_gb) required aria-describedby=[describe("disk_gb")];
                    }))
                }
            }
            fieldset {
                legend { "CPU ve Bellek" }
                div class="form" {
                    (field("cores", "Çekirdek", true, "", html! {
                        input id="cores" name="cores" type="text" inputmode="numeric" value=(f.cores) required aria-describedby=[describe("cores")];
                    }))
                    (field("memory_mb", "Bellek (MB)", true, &format!("En az {} MB.", kind.min_memory_mb()), html! {
                        input id="memory_mb" name="memory_mb" type="text" inputmode="numeric" value=(f.memory_mb) required aria-describedby=[describe("memory_mb")];
                    }))
                    @if kind == GuestKind::Vm {
                        (field("firmware", "Donanım yazılımı", false, "Windows 11 için UEFI + güvenli önyükleme ve TPM gerekir.", html! {
                            select id="firmware" name="firmware" {
                                option value="bios" selected[f.firmware == "bios"] { "BIOS (SeaBIOS)" }
                                option value="uefi" selected[f.firmware == "uefi"] { "UEFI" }
                                option value="uefi-secure" selected[f.firmware == "uefi-secure"] { "UEFI + güvenli önyükleme" }
                            }
                        }))
                        div class="field" {
                            span class="label" { "Güvenlik yongası" }
                            label class="check" { input type="checkbox" name="tpm" value="1" checked[f.tpm.is_some()]; "Sanal TPM 2.0 ekle" }
                        }
                    }
                }
            }
            fieldset {
                legend { "Ağ ve Başlatma" }
                div class="form" {
                    @if p.real {
                        (field("bridge", "Ağ", true, "NAT: konuk internete çıkar, dışarıdan görünmez. Köprü: sunucunun ağına doğrudan bağlanır.", html! {
                            select id="bridge" name="bridge" {
                                option value="nat" selected[f.bridge == "nat"] { "NAT" }
                                @for b in &opts.bridges { option value=(b) selected[*b == f.bridge] { "Köprü: " (b) } }
                            }
                        }))
                    } @else {
                        (field("bridge", "Köprü", false, "", html! {
                            input id="bridge" name="bridge" type="text" value=(f.bridge);
                        }))
                    }
                    div class="field" {
                        span class="label" { "Açılış" }
                        label class="check" { input type="checkbox" name="onboot" value="1" checked[f.onboot.is_some()]; "Düğüm açıldığında başlat" }
                    }
                }
            }
            div class="savebar" {
                span class="note" { "Konuk durdurulmuş halde eklenir." }
                a class="btn" href="/guests" { "Vazgeç" }
                button class="btn primary" type="submit" { (icon("plus")) "Oluştur" }
            }
        }
    };
    layout(p, title, &[("Konuklar", "/guests"), (title, "")], true, body)
}

pub fn delete_confirm(p: &Page, g: &Guest) -> String {
    let cli = format!("{} destroy {}", g.kind, g.vmid);
    let label = guest_label(g);
    let href = format!("/guests/{}", g.vmid);
    let body = html! {
        (page_head(html! { span class="id" { (g.vmid) } (g.name) " silinsin mi?" }, html! {}, html! {}, &cli, &[]))
        div class="card" { div class="body" {
            p class="lead" { (g.kind.long_label()) " " b { (g.vmid) " (" (g.name) ")" } " kalıcı olarak silinecek. Bu işlem geri alınamaz." }
            p class="muted" { (g.disk_gb) " GB disk " span class="mono" { (g.storage) } " deposuna geri verilir." }
            form class="btns" method="post" action={ "/guests/" (g.vmid) "/delete" } {
                button class="btn danger solid" type="submit" { (icon("trash")) "Kalıcı Olarak Sil" }
                a class="btn" href=(href) { "Vazgeç" }
            }
        } }
    };
    layout(p, &format!("{} sil", g.vmid), &[("Konuklar", "/guests"), (&label, &href), ("Sil", "")], true, body)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HISTORY_LEN;

    #[test]
    fn resample_right_aligns_and_averages() {
        let empty = resample(&[], 48);
        assert_eq!(empty.len(), 48);
        assert!(empty.iter().all(|v| *v == 1.0), "veri yokken taban çizgisi");

        // Tek örnek en sağdaki dilime düşer.
        let one = resample(&[80.0], 48);
        assert_eq!(one.len(), 48);
        assert!(one[..47].iter().all(|v| *v == 1.0) && one[47] > 1.0);

        // Dolu geçmiş: sabit değer sabit kalır, sınır dışı değerler kırpılır.
        let full = resample(&vec![50.0; HISTORY_LEN], 48);
        assert!(full.iter().all(|v| (*v - 50.0).abs() < 1e-9));
        let clipped = resample(&vec![500.0; HISTORY_LEN], 48);
        assert!(clipped.iter().all(|v| *v == 100.0));

        // Sınırdan uzun seri: yalnızca en yeni örnekler kullanılır.
        let mut long = vec![10.0; HISTORY_LEN];
        long.extend(vec![90.0; HISTORY_LEN]);
        assert!(resample(&long, 48).iter().all(|v| (*v - 90.0).abs() < 1e-9));
    }

    #[test]
    fn query_encoding() {
        assert_eq!(enc("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(enc("güvenli-_.~"), "g%C3%BCvenli-_.~");
    }
}
