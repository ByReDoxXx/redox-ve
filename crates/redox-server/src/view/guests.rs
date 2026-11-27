//! Konuk listesi ve konuk sayfası: başlık, işlemler, özet ve silme onayı.

use super::*;

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
            html! { (tx("Konuklar")) },
            sub(vec![
                html! { span { (c.guests.len()) " konuk" } },
                html! { span class="st run" { (running) (tx(" çalışıyor")) } },
                html! { span { "toplam " (cores) " vCPU, " (mb_as_gb(mem)) (tx(" GB bellek ayrılmış")) } },
            ]),
            html! {
                @if p.admin {
                    @if p.ct { a class="btn" href="/guests/new?kind=ct" { (icon("plus")) (tx("Konteyner")) } }
                    a class="btn primary" href="/guests/new?kind=vm" { (icon("plus")) (tx("Sanal Makine")) }
                }
            },
            "vm list",
            &[],
        ))
        div class="tools" {
            nav class="seg" aria-label=(tx("Duruma göre süz")) {
                @for (key, label) in [("", (tx("Tümü"))), ("calisan", (tx("Çalışan"))), ("kapali", (tx("Kapalı"))), ("sablon", (tx("Şablon")))] {
                    a href=(href(key)) aria-current=[(key == only).then_some("true")] { (label) " " b { (c.guests.iter().filter(|g| is(g, key)).count()) } }
                }
            }
            form class="find" method="get" action="/guests" role="search" {
                (icon("prompt"))
                @if !only.is_empty() { input type="hidden" name="tab" value=(only); }
                input type="search" name="q" value=(p.q) placeholder=(tx("ad, kimlik ya da etiket")) aria-label=(tx("Konukları filtrele"));
            }
            (bulk_tools(p, "/guests"))
        }
        (guest_table(p, &shown, "/guests", !only.is_empty() || !p.q.is_empty(), true))
    };
    layout(p, (tx("Konuklar")), &[((tx("Konuklar")), "/guests")], false, body)
}

/// Konuk sayfasının başlığındaki eylemler: sık kullanılanlar düğme, kalanı "Diğer" menüsünde.
pub(super) fn guest_actions(p: &Page, g: &Guest) -> Markup {
    if !p.may_operate(g) {
        return html! { span class="pill idle" { @if p.operator { (tx("Bu konuk yetki kapsamınızda değil")) } @else { (tx("Salt okunur hesap")) } } };
    }
    let base = format!("/guests/{}", g.vmid);
    let free = g.lock.is_none();
    let can = |a: Action| free && a.target(g.status).is_some();
    let toggle = if g.status == GuestStatus::Paused { Action::Resume } else { Action::Pause };
    let stopped = g.status == GuestStatus::Stopped;
    let live = if g.kind == GuestKind::Vm { p.live } else { p.live_ct };
    let can_backup = free && (stopped || live);
    html! {
        form class="acts" method="post" action={ (base) "/action" } {
            button class="btn go" name="action" value="start" disabled[!can(Action::Start) || g.hw.template] { (icon("play")) (tx("Başlat")) }
            div class="group" {
                button class="btn" name="action" value="stop" disabled[!can(Action::Stop)] { (icon("stop")) (tx("Kapat")) }
                button class="btn" name="action" value="reboot" disabled[!can(Action::Reboot)] { (icon("restart")) (tx("Yeniden Başlat")) }
            }
            div class="group" {
                button class="btn" name="action" value=(toggle.as_str()) disabled[!can(toggle)] {
                    (icon(if toggle == Action::Resume { "play" } else { "pause" })) (tx(toggle.label()))
                }
                @if p.admin && g.kind == GuestKind::Vm {
                    button class="btn" formaction={ (base) "/suspend" } disabled[stopped || !free] { (icon("half")) (tx("Askıya Al")) }
                }
            }
            details class="menu" {
                summary class="btn" { (tx("Diğer")) (icon("caret")) }
                div class="menu-list" {
                    button formaction={ (base) "/backups" } disabled[!can_backup] { (icon("backup")) (tx("Şimdi yedekle")) }
                    a href={ (base) "?tab=goruntuler" } { (icon("snapshot")) (tx("Anlık görüntü al")) }
                    @if p.admin {
                        hr;
                        @if disk_ops(p, g) {
                            a href={ (base) "?tab=cogalt" } { (icon("plus")) (tx("Klonla…")) small { (tx("kapalıyken")) } }
                            a href={ (base) "?tab=cogalt" } { (icon("vm")) @if g.hw.template { (tx("Şablonluktan çıkar")) } @else { (tx("Şablona çevir")) } small { (tx("kapalıyken")) } }
                            @if p.real { a href={ (base) "?tab=cogalt" } { (icon("disk")) (tx("OVA olarak indir")) small { (tx("kapalıyken")) } } }
                            hr;
                        }
                        @if free && stopped {
                            a class="danger" href={ (base) "/delete" } { (icon("trash")) (tx("Konuğu sil…")) }
                        } @else {
                            button type="button" disabled title=(tx("Silmek için önce kapatın")) { (icon("trash")) (tx("Konuğu sil…")) small { (tx("kapalıyken")) } }
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
pub(super) fn guest_head(p: &Page, g: &Guest, tab: &str) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let link = |label: &'static str, key: &str| (label, format!("{base}?tab={key}"), tab == key);
    let mut tabs = vec![((tx("Özet")), base.clone(), tab == "ozet")];
    // Konteynerde konsol, komut çalıştırma olarak yalnızca gerçek sürücüde vardır.
    if p.may_operate(g) && (g.kind == GuestKind::Vm || p.real) {
        tabs.push(link((tx("Konsol")), "konsol"));
    }
    tabs.extend([link((tx("Donanım")), "donanim"), link((tx("Seçenekler")), "secenekler"), link((tx("Anlık Görüntüler")), "goruntuler"), link((tx("Yedekler")), "yedekler")]);
    if p.admin {
        tabs.push(link(if disk_ops(p, g) { (tx("Çoğalt ve Taşı")) } else { (tx("Çoğalt")) }, "cogalt"));
    }
    let cli = format!("{} show {}", g.kind, g.vmid);
    let mut facts = vec![guest_state(g), html! { a href={ "/nodes/" (g.node) } { (g.node) } }];
    if let Some(ip) = &g.ip {
        facts.push(html! { span class="mono" { (ip) } });
    }
    facts.push(html! { span { (g.agent.as_deref().unwrap_or(&g.os)) } });
    if g.status != GuestStatus::Stopped {
        facts.push(html! { span { (guest_uptime(g, p.now)) (tx(" açık")) } });
    }
    html! {
        (page_head(
            html! {
                span class="id" { (g.vmid) } (g.name)
                @if g.hw.template { span class=(tx("tag gold")) { (tx("şablon")) } }
                @if g.hw.base.is_some() { span class=(tx("tag gold")) { (tx("bağlı klon")) } }
                @if g.hw.encrypted { span class="tag" { (tx("şifreli disk")) } }
            },
            sub(facts),
            guest_actions(p, g),
            &cli,
            &tabs,
        ))
        @if let Some(to) = &g.hw.handed_to {
            div class="notice" role="status" {
                (icon("alert"))
                span { (tfmt(format!("Bu konuğun diskleri {to} sunucusuna devredildi: burada yalnızca kaydı duruyor. Kaydı silmek disklere dokunmaz."))) }
            }
        }
        @if g.hw.suspended {
            div class="notice" role="status" {
                (icon("half"))
                span { (tx("Bu konuk askıda: başlatırsanız kaldığı yerden devam eder. Askıdayken disk ve donanım değiştirilemez.")) }
                @if p.admin {
                    form method="post" action={ (base) "/discard-suspend" } {
                        button class="btn sm danger" type="submit" disabled[g.lock.is_some()] title=(tx("Konuk sıfırdan açılır; kaydedilmemiş bellek içeriği kaybolur")) { (tx("Askı Durumunu At")) }
                    }
                }
            }
        }
    }
}

pub(super) fn firewall_text(g: &Guest) -> String {
    if !g.hw.firewall {
        return (tx("kapalı")).to_string();
    }
    let mut text = if g.hw.fw_rules.is_empty() && g.hw.fw_groups.is_empty() {
        (tx("açık · gelen trafik kapalı")).to_string()
    } else {
        let groups = g.hw.fw_groups.iter().map(|n| format!("[{n}]"));
        tfmt(format!("açık · {}", g.hw.fw_rules.iter().map(ToString::to_string).chain(groups).collect::<Vec<_>>().join(", ")))
    };
    if g.hw.fw_out_restrict {
        text.push_str(tx(" · giden kısıtlı"));
    }
    text
}

pub(super) fn overview(p: &Page, g: &Guest) -> Markup {
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
                (stat((tx("İşlemci")), &g.cpu_pct.to_string(), "%", &format!("{} vCPU", g.cores), Some(g.cpu_pct)))
                (stat((tx("Bellek")), &mp.to_string(), "%", &format!("{} / {} GB", mb_as_gb(g.mem_used_mb), mb_as_gb(g.mem_total_mb)), Some(mp)))
                (stat("Disk", &disk_total(g).to_string(), " GB", &disk_detail, None))
                (stat((tx("Çalışma Süresi")), &guest_uptime(g, p.now), "", &tfmt(format!("düğüm {}", g.node)), None))
            }
            div class="cols" {
                div class="stack" {
                    (usage_card(p, &format!("guest:{}", g.vmid), g.vmid as u64, if g.status == GuestStatus::Running { g.cpu_pct } else { 0 }, mp))
                    @if p.real {
                        @let u = c.usage_of(g.vmid);
                        div class="card" {
                            header { h2 { (tx("Kaynak ölçümü")) } (tip("Konuğun birikimli tüketimi: açık kalma süresi, işlemci, bellek, disk ve ağ. Faturalama için dışa aktarılır.")) span class="note" { @if u.since > 0 { (datetime(u.since)) (tx(" tarihinden beri")) } @else { (tx("henüz ölçüm yok")) } } }
                            dl class="kv" {
                                dt { (tx("Açık kaldığı süre")) } dd class="mono" { (uptime(u.uptime_secs)) }
                                dt { (tx("İşlemci")) } dd class="mono" { (format!("{:.2}", u.cpu_core_hours())) (tx(" çekirdek-saat")) }
                                dt { (tx("Bellek")) } dd class="mono" { (format!("{:.2}", u.mem_gb_hours())) " GB-saat" }
                                dt { (tx("Disk okuma / yazma")) } dd class="mono" { (u.disk_read_bytes >> 20) " MB / " (u.disk_write_bytes >> 20) " MB" }
                                dt { (tx("Ağ alınan / gönderilen")) } dd class="mono" { (u.net_rx_bytes >> 20) " MB / " (u.net_tx_bytes >> 20) " MB" @if g.bridge == redox_core::NAT { " " span class="muted" { (tx("(NAT'taki kart sayılmaz)")) } } }
                            }
                            footer { (tx("Birikimli sayaçlardır: konuk kapanıp açılsa da sürer. Dışa aktarım: ")) span class="mono" { "redox usage" } (tx(" ya da ")) span class="mono" { "GET /api/v1/usage" } ". " a href="/usage.csv" { (tx("CSV indir")) } }
                        }
                    }
                    div class="card" {
                        header { h2 { (tx("Bu konuğun etkinliği")) } (tip("Bu konukta yapılan son işlemler.")) div class="end" { a class="btn sm quiet" href={ "/tasks?q=" (g.vmid) } { (tx("Tümü")) } } }
                        (feed(&mine, (tx("Bu konuk için kayıtlı görev yok."))))
                    }
                }
                div class="stack" {
                    div class="card" {
                        header { h2 { (tx("Konuk")) } (tip("Konuğun türü, işletim sistemi, ağ adresi ve ayarlarının özeti.")) span class="note mono" { (g.vmid) ".conf" } }
                        dl class="kv" {
                            dt { (tx("Tür")) } dd { (tx(g.kind.long_label())) " (" (g.kind.backend()) ")" }
                            dt { (tx("İşletim sistemi")) } dd class="mono" { (g.os) }
                            @if g.kind == GuestKind::Vm && p.real {
                                dt { (tx("Konuk ajanı")) } dd {
                                    @match (&g.agent, g.status) {
                                        (Some(os), _) => { span class="pill ok" { (icon("check")) (tx("Çalışıyor")) } span class="mono" { (os) } },
                                        (None, GuestStatus::Stopped) => { span class="dim" { "-" } },
                                        (None, _) => { span class="dim" { (tx("Yanıt yok")) } span class="muted" { (tx("(konukta qemu-guest-agent kurulursa adres ve tutarlı yedek gelir)")) } },
                                    }
                                }
                            }
                            dt { (tx("IP adresi")) } dd class="mono" { (g.ip.as_deref().unwrap_or("-")) }
                            @let fs = c.filesystems_of(g.vmid);
                            @if !fs.is_empty() {
                                dt { (tx("Konuk içi diskler")) } dd class="mono" {
                                    @for f in fs {
                                        @let tone = if f.pct() >= 90 { "pill err" } else if f.pct() >= 80 { "pill warn" } else { "tag" };
                                        span class=(tone) title={ (f.used_mb) " / " (f.total_mb) " MB" } { (f.mount) " %" (f.pct()) } " "
                                    }
                                }
                            }
                            dt { (tx("Ağ")) } dd class="mono" {
                                @if g.bridge == redox_core::NAT { "NAT" } @else { (tx("köprü: ")) (g.bridge) @if g.hw.vlan > 0 { " · VLAN " (g.hw.vlan) } }
                                @for (i, n) in g.hw.nics.iter().enumerate() {
                                    @let (network, vlan) = redox_core::hw::parse_nic(n).unwrap_or((n.as_str(), 0));
                                    span class="dim" { "+ " @if network == redox_core::NAT { "NAT" } @else { (network) } @if vlan > 0 { " · VLAN " (vlan) } @if p.real { " (" (redox_core::mac_for_nic(g.vmid, i as u8 + 1)) ")" } }
                                }
                            }
                            @if p.real { dt { (tx("MAC adresi")) } dd class="mono" { (redox_core::mac_for(g.vmid)) } }
                            dt { (tx("Donanım")) } dd class="mono" {
                                (g.cores) " vCPU · " (g.mem_total_mb) " MB"
                                @if g.kind == GuestKind::Vm { " · " (g.hw.firmware_label()) @if g.hw.tpm { " · TPM 2.0" } }
                            }
                            dt { "Disk" } dd class="mono" { (g.disk_gb) " GB · " (g.storage) @for d in &g.hw.disks { " + " (d.size_gb) " GB" } }
                            @if g.hw.disk_mbps > 0 || g.hw.disk_iops > 0 || g.hw.net_mbps > 0 {
                                dt { (tx("Hız sınırı")) } dd class="mono" {
                                    @if g.hw.disk_mbps > 0 { span { "disk " (g.hw.disk_mbps) " MB/sn" } }
                                    @if g.hw.disk_iops > 0 { span { "disk " (g.hw.disk_iops) (tx(" işlem/sn")) } }
                                    @if g.hw.net_mbps > 0 { span { (tx("ağ ")) (g.hw.net_mbps) " Mbit/sn" } }
                                }
                            }
                            dt { (tx("Kurulum kalıbı")) } dd class="mono" { (g.iso.as_deref().unwrap_or("-")) }
                            @if let Some(image) = &g.hw.image { dt { (tx("Bulut imajı")) } dd class="mono" { (image) } }
                            @if let Some(key) = &g.hw.ci_ssh_key {
                                dt { "cloud-init" } dd class="mono" {
                                    (g.hw.ci_user.as_deref().unwrap_or(redox_core::hw::DEFAULT_CI_USER)) " · " (key.split(' ').next().unwrap_or("")) " " (key.split(' ').nth(2).unwrap_or(""))
                                }
                            }
                            @if !g.hw.usb.is_empty() || !g.hw.pci.is_empty() {
                                dt { (tx("Aktarılan aygıtlar")) } dd class="mono" {
                                    @for (i, d) in g.hw.usb.iter().map(|u| format!("USB {u}")).chain(g.hw.pci.iter().map(|p| format!("PCI {p}"))).enumerate() { @if i > 0 { ", " } (d) }
                                }
                            }
                            @if g.hw.template { dt { (tx("Şablon")) } dd { span class=(tx("tag gold")) { (tx("şablon")) } (tx("Başlatılamaz; klon kaynağıdır.")) } }
                            @if let Some(b) = g.hw.base { dt { (tx("Şablon")) } dd { (tx("Bağlı klon: ")) a class="mono" href={ "/guests/" (b) } { (b) } } }
                            dt { (tx("Açılışta başlat")) } dd {
                                @if g.onboot {
                                    (tx("Evet"))
                                    @if g.hw.boot_order > 0 { span class="muted" { (tx("sıra ")) (g.hw.boot_order) } }
                                    @if g.hw.boot_delay > 0 { span class="muted" { "sonra " (g.hw.boot_delay) (tx(" sn bekle")) } }
                                } @else { (tx("Hayır")) }
                            }
                            @if !g.hw.shares.is_empty() {
                                dt { (tx("Paylaşımlar")) } dd class="mono" { @for s in &g.hw.shares { span class="tag" { (s.name) @if s.readonly { (tx(" · salt okunur")) } } } }
                            }
                            @if g.hw.restart { dt { (tx("Kapanırsa")) } dd { (tx("kendiliğinden yeniden başlatılır")) } }
                            @if !g.hw.schedules.is_empty() {
                                dt { (tx("Zamanlanmış")) } dd {
                                    @for s in &g.hw.schedules { span class="tag" { (tx(s.action.label())) " " (s.time) " · " (tfmt(s.days_label())) } }
                                }
                            }
                            dt { (tx("Etiketler")) } dd { @if g.tags.is_empty() { "-" } @for t in &g.tags { a class="tag" href={ "/guests?q=" (t) } { (t) } } }
                            dt { (tx("Güvenlik duvarı")) } dd class="mono" {
                                (firewall_text(g))
                                @if g.hw.firewall && p.real && !crate::firewall::enforced(g) { span class="pill warn" { (icon("alert")) (tx("NAT'ta etkisiz")) } }
                            }
                            @if g.hw.nic_guard {
                                dt { (tx("Ağ kartı koruması")) } dd { (tx("açık · sahte MAC, DHCP sunucusu ve RA engellenir")) }
                            }
                            dt { (tx("Port yönlendirme")) } dd class="mono" {
                                @if g.forwards.is_empty() { "-" }
                                @for f in &g.forwards { (f.host_port) " → " (f.guest_port) " " }
                            }
                        }
                    }
                    div class="card" {
                        header { h2 { (tx("Koruma")) } (tip("Konuğun yedek ve anlık görüntü durumu.")) }
                        dl class="kv" {
                            dt { (tx("Son yedek")) } dd {
                                @match last {
                                    Some(b) => {
                                        a href={ (base) "?tab=yedekler" } { (datetime(b.created)) }
                                        @if b.cbt { span class="tag" { (tx("hızlı")) } }
                                        @if b.quiesced == Some(true) { span class="tag" { "donduruldu" } }
                                    },
                                    None => span class="muted" { (tx("Yedek yok")) },
                                }
                            }
                            dt { (tx("Zamanlama")) } dd {
                                @match &g.backup_time {
                                    Some(t) => { (tx("her gün ")) span class="mono" { (t) } (tx(", son ")) (g.backup_keep) },
                                    None => span class="muted" { (tx("kapalı")) },
                                }
                            }
                            dt { (tx("Dış kopya")) } dd {
                                @if backups.is_empty() { span class="dim" { "-" } }
                                @else { (backups.iter().filter(|b| b.offsite).count()) " / " (backups.len()) }
                            }
                            dt { (tx("Görüntüler")) } dd { a href={ (base) "?tab=goruntuler" } { (g.snapshots.len()) (tx(" anlık görüntü")) } }
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
        "cogalt" => p.admin,
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
    layout(p, &label, &[((tx("Konuklar")), "/guests"), (&label, "")], false, body)
}

pub fn delete_confirm(p: &Page, g: &Guest) -> String {
    let cli = format!("{} destroy {}", g.kind, g.vmid);
    let label = guest_label(g);
    let href = format!("/guests/{}", g.vmid);
    let body = html! {
        (page_head(html! { span class="id" { (g.vmid) } (g.name) (tx(" silinsin mi?")) }, html! {}, html! {}, &cli, &[]))
        div class="card" { div class="body" {
            p class="lead" { (tx(g.kind.long_label())) " " b { (g.vmid) " (" (g.name) ")" } (tx(" kalıcı olarak silinecek. Bu işlem geri alınamaz.")) }
            p class="muted" { (g.disk_gb) " GB disk " span class="mono" { (g.storage) } (tx(" deposuna geri verilir.")) }
            form class="btns" method="post" action={ "/guests/" (g.vmid) "/delete" } {
                button class="btn danger solid" type="submit" { (icon("trash")) (tx("Kalıcı Olarak Sil")) }
                a class="btn" href=(href) { (tx("Vazgeç")) }
            }
        } }
    };
    layout(p, &tfmt(format!("{} sil", g.vmid)), &[((tx("Konuklar")), "/guests"), (&label, &href), ((tx("Sil")), "")], true, body)
}
