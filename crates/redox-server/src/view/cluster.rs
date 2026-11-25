//! Küme sayfaları: veri merkezi özeti, düğüm, depolama, eş sunucular, görevler ve havuzlar.

use super::*;

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
            @if p.admin && !p.cluster.maintenance {
                div class="card" {
                    header { h2 { "Bakım" } span class="note" { "sunucuyu güncellemeden ya da yeniden başlatmadan önce" } }
                    div class="inline" {
                        form method="post" action="/maintenance/drain" { button class="btn sm" type="submit" { (icon("stop")) "Konukları Kapat ve Bakıma Al" } }
                        form method="post" action="/maintenance/on" { button class="btn sm quiet" type="submit" { "Yalnızca Bakıma Al" } }
                    }
                    footer { "Bakım kipinde hiçbir konuk başlatılmaz; zamanlanmış yedekler, güç işlemleri ve kendiliğinden yeniden başlatma durur. Kapatılan konuklar bakım bitince açılış sırasıyla yeniden başlatılır. Sunucu bu sırada yeniden başlasa da kip korunur." }
                }
            }
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
                            div {
                                h3 { "Hazır bulut imajları" }
                                @for (key, label, _, _) in crate::media::CATALOG {
                                    form class="line" method="post" action={ "/media/catalog/" (key) } {
                                        span { (label) }
                                        button class="btn sm" type="submit" { (icon("migrate")) "İndir" }
                                    }
                                }
                                p { "Dağıtımın kendi sunucusundan en güncel imaj iner. İmajlar cloud-init ile açılır: yeni konuk formunda imajı seçip kullanıcı adı ve SSH anahtarı verin." }
                            }
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

/// Kaynak havuzları: konuk gruplarına toplam kota.
pub fn pools(p: &Page) -> String {
    let c = p.cluster;
    let full = c.pools.len() >= redox_core::MAX_POOLS;
    // "12 / 32" ya da sınırsızsa "12 / –"; doluluk çubuğu yalnızca sınır varken çizilir.
    let quota = |used: u64, limit: u64, unit: &str| {
        html! {
            span class="mono" { (used) " / " @if limit == 0 { "sınırsız" } @else { (limit) } @if !unit.is_empty() { " " (unit) } }
            @if limit > 0 {
                @let pct = (used * 100 / limit).min(100);
                span class="bar" { i class=(if pct >= 90 { "hot" } else { "" }) style={ "width:" (pct) "%" } {} }
            }
        }
    };
    let body = html! {
        (page_head(
            html! { "Havuzlar" },
            sub(vec![html! { span { (c.pools.len()) " havuz" } }, html! { span { (c.guests.iter().filter(|g| g.pool.is_some()).count()) " konuk havuzda" } }]),
            html! {},
            "pool list",
            &[],
        ))
        div class="card" {
            header { h2 { "Kaynak havuzları" } span class="note" { "ayrılan kaynak / kota · " (c.pools.len()) " / " (redox_core::MAX_POOLS) } }
            @if c.pools.is_empty() {
                p class="empty" { "Havuz yok. Havuz, bir grup konuğun toplam çekirdek, bellek ve disk ayırmasına sınır koyar; konuğu havuza Seçenekler sekmesinden alırsınız." }
            } @else {
                div class="tw" { table {
                    thead { tr { th { "Havuz" } th { "Konuk" } th { "Çekirdek" } th { "Bellek" } th { "Disk" } th { span class="sr" { "Sil" } } } }
                    tbody { @for pool in &c.pools {
                        @let used = c.pool_usage(&pool.name, None);
                        tr {
                            td class="mono" { a class="link" href={ "/guests?q=" (pool.name) } { (pool.name) } }
                            td { (quota(used.guests as u64, pool.guests as u64, "")) }
                            td { (quota(used.cores as u64, pool.cores as u64, "")) }
                            td { (quota(used.memory_mb, pool.memory_mb, "MB")) }
                            td { (quota(used.disk_gb, pool.disk_gb, "GB")) }
                            td class="act" {
                                @if p.admin { form method="post" action={ "/pools/" (pool.name) "/delete" } { button class="btn sm danger" type="submit" disabled[used.guests > 0] aria-label={ (pool.name) " havuzunu sil" } { (icon("trash")) "Sil" } } }
                            }
                        }
                    } }
                } }
            }
            @if p.admin {
                form class="inline" method="post" action="/pools" {
                    label for="pool-name" { "Yeni havuz" }
                    input id="pool-name" name="name" type="text" required placeholder="ad (örn. web)" maxlength="24" disabled[full];
                    input name="cores" type="text" inputmode="numeric" placeholder="çekirdek" aria-label="Çekirdek kotası" style="max-width:7rem" disabled[full];
                    input name="memory_mb" type="text" inputmode="numeric" placeholder="bellek (MB)" aria-label="Bellek kotası (MB)" style="max-width:8rem" disabled[full];
                    input name="disk_gb" type="text" inputmode="numeric" placeholder="disk (GB)" aria-label="Disk kotası (GB)" style="max-width:7rem" disabled[full];
                    input name="guests" type="text" inputmode="numeric" placeholder="konuk sayısı" aria-label="En fazla konuk" style="max-width:8rem" disabled[full];
                    button class="btn sm" type="submit" disabled[full] { (icon("plus")) "Kaydet" }
                }
            }
            footer {
                "Kota, konukların ayrılmış kaynaklarına uygulanır (o anki kullanıma değil); boş bırakılan alan sınırsızdır. Kota dolduğunda havuza konuk alınamaz, "
                "havuzdaki konuğun çekirdeği, belleği ve diski artırılamaz, konuk klonlanamaz. Var olan bir havuzun adını yazarsanız kotaları güncellenir. İçinde konuk olan havuz silinmez. "
                "Komut satırından: " code { "redox pool add web cores=32 memory=65536 disk=500" }
            }
        }
    };
    layout(p, "Havuzlar", &[("Havuzlar", "/pools")], false, body)
}
