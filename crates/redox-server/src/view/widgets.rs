//! Sayfalarda paylaşılan küçük parçalar: simge, durum etiketi, çubuk, grafik kartı, konuk tablosu, akış.

use super::*;

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

pub(super) fn hms(ts: u64) -> String {
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

pub(super) fn cur(on: bool) -> Option<&'static str> {
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

pub(super) fn status_class(s: GuestStatus) -> &'static str {
    match s {
        GuestStatus::Running => "run",
        GuestStatus::Stopped => "stop",
        GuestStatus::Paused => "pause",
    }
}

pub(super) fn node_state(n: &Node) -> Markup {
    let class = if n.status == NodeStatus::Online { "st run" } else { "st err" };
    html! { span class=(class) { (n.status.label()) } }
}

pub(super) fn task_pill(t: &Task) -> Markup {
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

pub(super) fn tone(p: u8) -> &'static str {
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
pub(super) fn stat(label: &str, value: &str, unit: &str, detail: &str, p: Option<u8>) -> Markup {
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
pub(super) fn sub(items: Vec<Markup>) -> Markup {
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
pub(super) fn range_links(p: &Page) -> Markup {
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
pub(super) fn usage_card(p: &Page, prefix: &str, seed: u64, cpu_now: u8, mem_now: u8) -> Markup {
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

pub(super) fn disk_total(g: &Guest) -> u64 {
    g.disk_gb + g.hw.disks.iter().map(|d| d.size_gb).sum::<u64>()
}

/// Konuk tablosu. `clear`: süzgeci temizleyen adres; `grouped`: düğüme göre gruplanır.
pub(super) fn guest_table(p: &Page, guests: &[&Guest], clear: &str, filtered: bool, grouped: bool) -> Markup {
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
pub(super) fn bulk_tools(p: &Page, back: &str) -> Markup {
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

pub(super) fn feed(tasks: &[&Task], empty: &str) -> Markup {
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

pub(super) fn guest_uptime(g: &Guest, now: u64) -> String {
    g.uptime_secs(now).map(uptime).unwrap_or_else(|| "-".to_string())
}
