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
        html! { @if s.quorate { span class="st run" { (tx("Küme sağlıklı")) } } @else { span class="st err" { (tx("Çoğunluk yok")) } } },
        html! { span { (c.name) } },
        html! { span {
            @if s.nodes_online == s.nodes_total { (s.nodes_total) (tx(" düğüm çevrimiçi")) } @else { (s.nodes_total - s.nodes_online) (tx(" düğüm çevrimdışı")) }
            ", " (s.guests_total) " konuk"
        } },
    ]);
    let body = html! {
        (page_head(html! { (tx("Genel Bakış")) }, health, html! {}, "cluster status", &[]))
        @for a in &crate::alerts::evaluate(c) {
            div class={ "notice" @if a.severity == crate::alerts::Severity::Critical { " err" } } role="status" { (icon("alert")) span { (crate::i18n::message(&a.message)) } }
        }
        div class="stack" {
            div class="stats" {
                (stat((tx("Konuklar")), &s.guests_running.to_string(), &tfmt(format!(" / {} çalışıyor", s.guests_total)), &tfmt(format!("{stopped} kapalı · {templates} şablon")), None))
                (stat((tx("İşlemci")), &s.cpu_pct.to_string(), "%", &tfmt(format!("{} çekirdek", s.cores)), Some(s.cpu_pct)))
                (stat((tx("Bellek")), &s.mem_pct.to_string(), "%", &format!("{} / {} GB", mb_as_gb(s.mem_used_mb), mb_as_gb(s.mem_total_mb)), Some(s.mem_pct)))
                (stat((tx("Depolama")), &s.storage_pct.to_string(), "%", &format!("{} / {}", gb(s.storage_used_gb), gb(s.storage_total_gb)), Some(s.storage_pct)))
            }
            div class="cols" {
                div class="stack" {
                    div class="card" {
                        header { h2 { (tx("En çok kaynak kullananlar")) } (tip("İşlemciyi en çok kullanan çalışan konuklar.")) span class="note" { (tx("çalışan konuklar")) } div class="end" { a class="btn sm quiet" href="/guests" { (tx("Tüm konuklar")) } } }
                        @if busy.is_empty() {
                            p class="empty" { (tx("Çalışan konuk yok.")) @if p.admin && c.guests.is_empty() { " " a href="/guests/new?kind=vm" { (tx("İlk sanal makineyi oluşturun")) } "." } }
                        } @else {
                            div class="tw" { table {
                                thead { tr { th { (tx("Konuk")) } th { (tx("Durum")) } th { "CPU" } th { (tx("Bellek")) } } }
                                tbody { @for g in &busy { tr {
                                    td class="name" { a href={ "/guests/" (g.vmid) } { span class="mono dim" { (g.vmid) } " " (g.name) } }
                                    td { (guest_state(g)) }
                                    td class="mono" { (bar(g.cpu_pct, "CPU", false)) " " (g.cpu_pct) "%" }
                                    td class="mono" { (bar(g.mem_pct(), (tx("Bellek")), false)) " " (mb_as_gb(g.mem_used_mb)) " / " (mb_as_gb(g.mem_total_mb)) " GB" }
                                } } }
                            } }
                        }
                    }
                    div class="card" {
                        header { h2 { (tx("Düğümler")) } (tip("Kümedeki sunucular ve anlık yükleri.")) span class="note" { (s.nodes_online) " / " (s.nodes_total) (tx(" çevrimiçi")) } }
                        div class="tw" { table {
                            thead { tr { th { (tx("Düğüm")) } th { (tx("Durum")) } th { "CPU" } th { (tx("Bellek")) } th class="num" { (tx("Konuk")) } } }
                            tbody { @for n in &c.nodes { tr {
                                td class="name mono" { a href={ "/nodes/" (n.id) } { (n.id) } }
                                td { (node_state(n)) }
                                td class="mono" { (bar(n.cpu_pct, "CPU", false)) " " (n.cpu_pct) "%" }
                                td class="mono" { (bar(pct(n.mem_used_mb, n.mem_total_mb), (tx("Bellek")), false)) " " (pct(n.mem_used_mb, n.mem_total_mb)) "%" }
                                td class="num mono" { (c.guests_on(&n.id).count()) }
                            } } }
                        } }
                    }
                    div class="card" {
                        header { h2 { (tx("Yedek durumu")) } (tip("Otomatik yedeği açık konuk sayısı ve son alınan yedekler.")) div class="end" { a class="btn sm quiet" href="/backups" { (tx("Tüm yedekler")) } } }
                        dl class="kv" {
                            dt { (tx("Son yedek")) } dd {
                                @match last {
                                    Some(b) => { a class="mono" href={ "/guests/" (b.vmid) "?tab=yedekler" } { (b.id) } span class="muted" { (datetime(b.created)) } },
                                    None => span class="muted" { (tx("Henüz yedek alınmadı.")) },
                                }
                            }
                            dt { (tx("Zamanlanmış")) } dd { (scheduled) " / " (c.guests.len()) (tx(" konukta otomatik yedek açık")) }
                            dt { (tx("Dış kopya")) } dd {
                                @match p.offsite {
                                    Some(target) => { span class="mono" { (target) } span class="muted" { (offsite) " / " (c.backups.len()) (tx(" yedek hedefte")) } },
                                    None => span class="muted" { (tx("Tanımlı değil; yedekler yalnızca bu sunucuda.")) },
                                }
                            }
                            dt { (tx("Yedeksiz konuk")) } dd {
                                @if unprotected.is_empty() { span class="muted" { (tx("Yok")) } }
                                @for g in &unprotected { a href={ "/guests/" (g.vmid) "?tab=yedekler" } { (g.vmid) " " (g.name) } }
                            }
                        }
                    }
                }
                div class="card" {
                    header { h2 { (tx("Etkinlik")) } (tip("Son görevler: kim, ne zaman, ne yaptı ve sonucu.")) div class="end" { a class="btn sm quiet" href="/tasks" { (tx("Tümü")) } } }
                    (feed(&recent, (tx("Henüz görev yok. Bir konuğu başlattığınızda ya da oluşturduğunuzda burada görünür."))))
                }
            }
        }
    };
    layout(p, (tx("Genel Bakış")), &[((tx("Genel Bakış")), "/")], false, body)
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
                (stat((tx("İşlemci")), &n.cpu_pct.to_string(), "%", &tfmt(format!("{} çekirdek", n.cores)), Some(n.cpu_pct)))
                (stat((tx("Bellek")), &mp.to_string(), "%", &format!("{} / {} GB", mb_as_gb(n.mem_used_mb), mb_as_gb(n.mem_total_mb)), Some(mp)))
                (stat((tx("Konuklar")), &all.len().to_string(), "", &tfmt(format!("{running} çalışıyor")), None))
                (stat((tx("Çalışma Süresi")), &uptime(n.uptime_secs), "", &format!("linux {} · {}", n.kernel, n.ip), None))
            }
            (usage_card(p, "node", n.cores as u64, n.cpu_pct, mp))
            @if p.admin && !p.cluster.maintenance {
                div class="card" {
                    header { h2 { (tx("Bakım")) } (tip("Sunucuyu güncellemeden önce konukları düzenli kapatır; bakım bitince yeniden başlatır.")) span class="note" { (tx("sunucuyu güncellemeden ya da yeniden başlatmadan önce")) } }
                    div class="inline" {
                        form method="post" action="/maintenance/drain" { button class="btn sm" type="submit" { (icon("stop")) (tx("Konukları Kapat ve Bakıma Al")) } }
                        form method="post" action="/maintenance/on" { button class="btn sm quiet" type="submit" { (tx("Yalnızca Bakıma Al")) } }
                    }
                    footer { (tx("Bakım kipinde hiçbir konuk başlatılmaz; zamanlanmış yedekler, güç işlemleri ve kendiliğinden yeniden başlatma durur. Kapatılan konuklar bakım bitince açılış sırasıyla yeniden başlatılır. Sunucu bu sırada yeniden başlasa da kip korunur.")) }
                }
            }
            div {
                div class="tools" {
                    h2 { (tx("Bu düğümdeki konuklar")) }
                    form class="find" method="get" action=(href) role="search" {
                        (icon("prompt"))
                        input type="search" name="q" value=(p.q) placeholder=(tx("ad, kimlik ya da etiket")) aria-label=(tx("Konukları filtrele"));
                    }
                    (bulk_tools(p, &href))
                }
                (guest_table(p, &shown, &href, !p.q.is_empty(), false))
            }
        }
    };
    layout(p, &n.id, &[((tx("Genel Bakış")), "/"), (&n.id, &href)], false, body)
}

pub fn storage(p: &Page, media: &[crate::media::Media]) -> String {
    let c = p.cluster;
    let total_mb: u64 = media.iter().map(|m| m.bytes.div_ceil(1024 * 1024)).sum();
    let body = html! {
        (page_head(
            html! { (tx("Depolama")) },
            sub(vec![
                html! { span { (c.storages.len()) (tx(" depolama alanı")) } },
                html! { @if p.real { span { (media.len()) (tx(" kalıp ve imaj, ")) (total_mb) " MB" } } @else { span { (tx("simülasyon")) } } },
            ]),
            html! {},
            "storage list",
            &[],
        ))
        div class="stack" {
            div class="card" {
                header { h2 { (tx("Depolama alanları")) } (tip("Konuk disklerinin, kalıpların ve yedeklerin durduğu alanlar ve dolulukları.")) }
                div class="tw" { table {
                    thead { tr { th { (tx("Alan")) } th { (tx("Tür")) } th { (tx("İçerik")) } th style="width:34%" { (tx("Kullanım")) } th class="num" { (tx("Dolu / Toplam")) } } }
                    tbody { @for s in &c.storages { tr {
                        td class="mono" { (s.id) span class="sub" { (s.node.as_deref().unwrap_or((tx("paylaşımlı")))) } }
                        td { (tx(s.kind.label())) }
                        td class="muted" { (s.content.join(", ")) }
                        td class="mono" {
                            (bar(s.used_pct(), &tfmt(format!("{} kullanım", s.id)), false)) " " (s.used_pct()) "%"
                            @if s.used_pct() >= 90 { " " span class="pill err" { (icon("alert")) (tx("Dolmak üzere")) } }
                        }
                        td class="num mono" { (gb(s.used_gb)) " / " (gb(s.total_gb)) }
                    } } }
                } }
            }
            @if p.real {
                div class="card" {
                    header { h2 { (tx("Kalıplar ve disk imajları")) } (tip("Kurulum kalıpları (ISO) ve hazır disk imajları. Konuk oluştururken seçilirler.")) span class="note" { (tx("konuk oluştururken seçilir")) } }
                    @if media.is_empty() {
                        p class="empty" { (tx("Henüz dosya yok. Kurulum kalıbı (.iso) ya da hazır disk imajı (.qcow2, .img, .vmdk, .ova …) yükleyin; konuk oluştururken seçilebilir olurlar.")) }
                    } @else {
                        div class="tw" { table {
                            thead { tr { th { (tx("Dosya")) } th { (tx("Tür")) } th class="num" { (tx("Boyut")) } th { (tx("Kullanan")) } th { span class="sr" { (tx("İşlem")) } } } }
                            tbody { @for m in media { tr {
                                td class="mono" { (m.name) }
                                td { @if m.iso { (tx("Kurulum kalıbı")) } @else if let Some(ova) = &m.ova {
                                    (tx("OVA arşivi"))
                                    @if ova.cores.is_some() || ova.memory_mb.is_some() {
                                        span class="dim" { (tx(" · öneri: ")) @if let Some(c) = ova.cores { (c) (tx(" çekirdek")) } @if let (Some(_), Some(_)) = (ova.cores, ova.memory_mb) { ", " } @if let Some(mb) = ova.memory_mb { (mb) " MB" } }
                                    }
                                } @else { (tx("Disk imajı")) } }
                                td class="num mono" { (m.bytes.div_ceil(1024 * 1024)) " MB" }
                                td class="mono" { @if m.used_by.is_empty() { span class="dim" { "-" } } @for (i, id) in m.used_by.iter().enumerate() { @if i > 0 { ", " } a class="link" href={ "/guests/" (id) } { (id) } } }
                                td class="act" {
                                    @if p.admin { form method="post" action={ "/media/" (m.name) "/delete" } { button class="btn sm danger" type="submit" { (icon("trash")) (tx("Sil")) } } }
                                }
                            } } }
                        } }
                    }
                    @if p.admin {
                        div class="intake" {
                            div {
                                h3 { (tx("Hazır bulut imajları")) }
                                @for (key, label, _, _) in crate::media::CATALOG {
                                    form class="line" method="post" action={ "/media/catalog/" (key) } {
                                        span { (tx(label)) }
                                        button class="btn sm" type="submit" { (icon("migrate")) (tx("İndir")) }
                                    }
                                }
                                p { (tx("Dağıtımın kendi sunucusundan en güncel imaj iner. İmajlar cloud-init ile açılır: yeni konuk formunda imajı seçip kullanıcı adı ve SSH anahtarı verin.")) " " (tx("Windows sürücüleri bir kurulum kalıbıdır: Windows kurulurken diski ve ağ kartını tanıtmak, sonra balon sürücüsünü ve konuk ajanını kurmak için kullanılır.")) }
                            }
                            form method="post" action="/media/upload" enctype="multipart/form-data" {
                                h3 { label for="media-file" { (tx("Bilgisayardan yükle")) } }
                                div class="line" {
                                    input id="media-file" name="file" type="file" required accept=".iso,.qcow2,.img,.raw,.vmdk,.vdi,.vhdx,.vhd,.ova";
                                    button class="btn" type="submit" { (icon("plus")) (tx("Yükle")) }
                                }
                                p { (tx("Dosya adı türü belirler (.iso kalıptır). Yükleme bitene kadar sayfa bekler.")) }
                            }
                            form method="post" action="/media/fetch" {
                                h3 { label for="media-url" { (tx("Adresten indir")) } }
                                div class="line" { input id="media-url" name="url" type="url" required placeholder="https://…/debian-12.iso" autocomplete="off"; }
                                div class="line" {
                                    input name="sha256" type="text" placeholder=(tx("SHA-256 (isteğe bağlı)")) autocomplete="off" aria-label=(tx("SHA-256 özeti"));
                                    button class="btn" type="submit" { (icon("migrate")) (tx("İndir")) }
                                }
                                p { (tx("Büyük dosyalar için adresten indirme arka planda çalışır ve sonucu görev günlüğüne yazar. Yalnızca https adresleri ve genel internet adresleri kabul edilir. SHA-256 verirseniz dosya onunla doğrulanır.")) }
                            }
                        }
                    }
                }
            }
        }
    };
    layout(p, (tx("Depolama")), &[((tx("Depolama")), "/storage")], false, body)
}

/// Çoğaltmalar: eş sunuculardaki konukların buradaki yedek kopyaları. Eş sunucu tanımlı değilse çizilmez.
fn replicas_card(p: &Page, peers: &[crate::peers::PeerView]) -> Markup {
    let list = &p.cluster.replicas;
    html! {
        @if !peers.is_empty() || !list.is_empty() {
            div class="card" {
                header { h2 { (tx("Çoğaltma")) } (tip("Eş sunucudaki bir konuğun kopyasını burada düzenli tazeler. O sunucu ölürse kopyayı buradan başlatırsınız.")) span class="note" { (list.len()) " / " (redox_core::replicas::MAX_REPLICAS) } }
                @if list.is_empty() {
                    p class="empty" { (tx("Çoğaltma yok. Eş sunucudaki bir sanal makinenin kopyası burada düzenli tazelenir; o sunucu ölürse kopyayı buradan başlatırsınız.")) }
                } @else {
                    div class="tw" { table {
                        thead { tr { th { (tx("Kaynak")) } th { (tx("Aralık")) } th { (tx("Yedek kopya")) } th { (tx("Son başarılı tur")) } th { span class="sr" { (tx("İşlem")) } } } }
                        tbody { @for r in list { tr {
                            td class="mono" { (r.peer) " / " (r.source) }
                            td class="mono" { (r.every_min) " dk" }
                            td {
                                @match r.standby {
                                    Some(id) => a class="mono" href={ "/guests/" (id) } { (id) },
                                    None => span class="dim" { "-" },
                                }
                                @if r.running { " " span class="pill idle" { (tx("sürüyor")) } }
                                @if let Some(e) = &r.error { " " span class="pill err" title=(e) { (icon("alert")) "hata" } }
                            }
                            td class="mono muted" { @match r.last_ok { Some(ts) => (datetime(ts)), None => "-" } }
                            td class="act" { @if p.admin {
                                form class="inline" method="post" action="/cluster/replica" {
                                    input type="hidden" name="peer" value=(r.peer); input type="hidden" name="vmid" value=(r.source);
                                    button class="btn sm" type="submit" name="op" value="run" disabled[r.running] { (tx("Şimdi")) }
                                    button class="btn sm danger" type="submit" name="op" value="remove" { (tx("Kaldır")) }
                                }
                            } }
                        } } }
                    } }
                }
                @if p.admin && !peers.is_empty() {
                    form class="inline" method="post" action="/cluster/replica" {
                        label for="rep-peer" { (tx("Eş sunucu")) }
                        select id="rep-peer" name="peer" { @for v in peers { option value=(v.name) { (v.name) } } }
                        label for="rep-vmid" { (tx("Konuk kimliği")) }
                        input id="rep-vmid" name="vmid" type="text" inputmode="numeric" required;
                        label for="rep-every" { (tx("Aralık (dk)")) }
                        input id="rep-every" name="every_min" type="text" inputmode="numeric" value="60" required;
                        button class="btn sm" type="submit" name="op" value="add" { (icon("plus")) (tx("Çoğalt")) }
                    }
                }
                footer { (tx("Her tur konuğun diskinin tamamını kopyalar (kaynak çalışmaya devam eder); kayıp, son turdan beri yazılanlarla sınırlıdır. Yedek kopya kapalı durur; kaynak sunucu ölürse onu buradan başlatın. Çalışan bir kopyanın üzerine yazılmaz.")) }
            }
        }
    }
}

/// Birleşik görünüm: bu sunucu ve eş sunucuların konukları tek sayfada, salt okunur.
pub fn peers(p: &Page, peers: &[crate::peers::PeerView]) -> String {
    let c = p.cluster;
    let local = c.summary();
    let online = peers.iter().filter(|v| v.online()).count();
    let guests = local.guests_total + peers.iter().map(|v| v.guests.len()).sum::<usize>();
    let running = local.guests_running + peers.iter().map(|v| v.running()).sum::<usize>();
    let mut facts = vec![html! { span { (running) "/" (guests) (tx(" konuk çalışıyor")) } }];
    if !peers.is_empty() {
        facts.insert(0, html! {
            @if online == peers.len() { span class="pill ok" { (icon("check")) (online) "/" (peers.len()) (tx(" eş erişilebilir")) } }
            @else { span class="pill err" { (icon("alert")) (online) "/" (peers.len()) (tx(" eş erişilebilir")) } }
        });
    }
    let body = html! {
        (page_head(html! { (tx("Küme")) }, sub(facts), html! {}, "peer list", &[]))
        div class="stack" {
            (replicas_card(p, peers))
            div class="card" {
                header { h2 { (tx("Sunucular")) } (tip("Bu sunucu ve tanımlı eş sunucular. Her sunucu kendi konuklarını yönetir.")) }
                div class="tw" { table {
                    thead { tr { th { (tx("Sunucu")) } th { (tx("Küme adı")) } th { (tx("Durum")) } th class="num" { (tx("Düğüm")) } th class="num" { (tx("Konuk")) } th { (tx("Son okuma")) } } }
                    tbody {
                        tr {
                            td class="name mono" { a href="/" { (tx("bu sunucu")) } }
                            td class="mono muted" { (c.name) }
                            td { span class="pill ok" { (icon("check")) (tx("Yerel")) } }
                            td class="num mono" { (local.nodes_online) "/" (local.nodes_total) }
                            td class="num mono" { (local.guests_running) "/" (local.guests_total) }
                            td class="mono muted" { (tx("canlı")) }
                        }
                        @for v in peers { tr {
                            td class="name mono" { a href=(v.url) rel="noreferrer" { (v.name) } }
                            td class="mono muted" { @if v.cluster.is_empty() { "-" } @else { (v.cluster) } }
                            td {
                                @if v.online() { span class="pill ok" { (icon("check")) (tx("Erişilebilir")) } }
                                @else if let Some(e) = &v.error { span class="pill err" { (icon("alert")) (e) } }
                                @else { span class="pill warn" { (icon("clock")) (tx("Bekleniyor")) } }
                            }
                            td class="num mono" { @if v.online() { (v.nodes.iter().filter(|n| n.status == NodeStatus::Online).count()) "/" (v.nodes.len()) } @else { "-" } }
                            td class="num mono" { @if v.online() { (v.running()) "/" (v.guests.len()) } @else { "-" } }
                            td class="mono muted" { @match v.checked { Some(t) => (hms(t)), None => "-" } }
                        } }
                    }
                } }
                @if peers.is_empty() {
                    p class="empty" {
                        (tx("Eş sunucu tanımlı değil. Başka RedoX VE sunucularını burada görmek için sunucuyu "))
                        span class="mono" { (tx("--peers DOSYA")) } (tx(" ile başlatın; dosyanın her satırı "))
                        span class="mono" { (tx("AD ADRES TOKEN")) } (tx(" biçimindedir (eşte salt okunur bir kullanıcının token'ı yeterlidir)."))
                    }
                }
                footer {
                    (tx("Her sunucu kendi konuklarını yönetir: bir konuğu yönetmek için adına tıklayıp o sunucunun arayüzüne gidin. "))
                    (tx("“Buraya Al” kapalı bir sanal makineyi yedeği üzerinden bu sunucuya kopyalar; kaynak yerinde kalır, çalıştığını görünce orada siz silersiniz. "))
                    (tx("“Çalışırken Al” çalışan konuğu orada askıya alır, diskini ve belleğini kopyalar, burada kaldığı yerden sürdürür: konuk yeniden başlamaz ama kopyalama boyunca yanıt vermez. "))
                    (tx("Kaynaktaki kopya askıda kalır; burada çalıştığını görünce orada silin, başlatmayın. "))
                    (tx("Kesintisiz canlı taşıma, ortak küme durumu ve sunucular arası kendiliğinden devralma yoktur."))
                }
            }
            @for v in peers.iter().filter(|v| v.online()) {
                div class="card" {
                    header { h2 { (v.name) } span class="note mono" { (v.url) } }
                    @if v.guests.is_empty() {
                        p class="empty" { (tx("Bu sunucuda konuk yok.")) }
                    } @else {
                        div class="tw" { table {
                            thead { tr { th { (tx("Konuk")) } th { (tx("Düğüm")) } th { (tx("Durum")) } th { "CPU" } th { (tx("Bellek")) } @if p.admin && p.real { th { span class="sr" { (tx("Taşı")) } } } } }
                            tbody { @for g in &v.guests { tr {
                                td class="name" { a href={ (v.url) "/guests/" (g.vmid) } rel="noreferrer" { span class="mono dim" { (g.vmid) } " " (g.name) } span class="kind" { (tx(g.kind.label())) } }
                                td class="mono muted" { (g.node) }
                                td { span class={ "st " (status_class(g.status)) } { (tx(g.status.label())) } }
                                td class="mono" { (g.cpu_pct) "%" }
                                td class="mono" { (mb_as_gb(g.mem_used_mb)) " / " (mb_as_gb(g.mem_total_mb)) " GB" }
                                @if p.admin && p.real {
                                    td class="act" {
                                        @if g.lock.is_none() && (g.kind == GuestKind::Vm || p.ct) {
                                            form method="post" action="/cluster/import" {
                                                input type="hidden" name="peer" value=(v.name);
                                                input type="hidden" name="vmid" value=(g.vmid);
                                                @if g.kind == GuestKind::Ct {
                                                    button class="btn sm" type="submit" title=(tx("Konteyner yedeği üzerinden kopyalanır; kaynak çalışmaya devam eder, kopya kapalı gelir")) { (icon("migrate")) (tx("Buraya Al")) }
                                                } @else if g.status == GuestStatus::Stopped && !g.hw.suspended {
                                                    button class="btn sm" type="submit" { (icon("migrate")) (tx("Buraya Al")) }
                                                } @else {
                                                    button class="btn sm" type="submit" title=(tx("Konuk orada askıya alınır, diski ve belleği kopyalanır, burada kaldığı yerden devam eder")) { (icon("migrate")) (tx("Çalışırken Al")) }
                                                }
                                            }
                                        } @else { span class="dim" title=(tx("Üzerinde işlem süren konuk bekler; konteyner için bu sunucuda LXC gerekir")) { "-" } }
                                    }
                                }
                            } } }
                        } }
                    }
                }
            }
        }
    };
    layout(p, (tx("Küme")), &[((tx("Küme")), "/cluster")], false, body)
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
            html! { (tx("Görevler")) },
            sub(vec![
                html! { @if running > 0 { span class="pill warn" { (icon_c("restart", "spin")) (running) (tx(" sürüyor")) } } @else { span { (tx("boşta")) } } },
                html! { span { (p.cluster.tasks.len()) (tx(" kayıt")) } },
            ]),
            html! {},
            "task list",
            &[],
        ))
        div class="tools" {
            form class="find" method="get" action="/tasks" role="search" {
                (icon("prompt"))
                input type="search" name="q" value=(p.q) placeholder=(tx("konuk kimliği, komut ya da kullanıcı")) aria-label=(tx("Görevleri süz"));
            }
        }
        div class="card" {
            @if shown.is_empty() {
                p class="empty" {
                    @if q.is_empty() { (tx("Henüz görev yok. Bir konuğu başlattığınızda ya da oluşturduğunuzda burada görünür.")) }
                    @else { "“" (p.q) (tx("” ile eşleşen görev yok. ")) a href="/tasks" { (tx("Süzgeci temizleyin")) } "." }
                }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("Başlangıç")) } th { (tx("Bitiş")) } th { (tx("Düğüm")) } th { (tx("Kullanıcı")) } th { (tx("Komut")) } th { (tx("Durum")) } } }
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
    layout(p, (tx("Görevler")), &[((tx("Görevler")), "/tasks")], false, body)
}

/// Kaynak havuzları: konuk gruplarına toplam kota.
pub fn pools(p: &Page) -> String {
    let c = p.cluster;
    let full = c.pools.len() >= redox_core::MAX_POOLS;
    // "12 / 32" ya da sınırsızsa "12 / –"; doluluk çubuğu yalnızca sınır varken çizilir.
    let quota = |used: u64, limit: u64, unit: &str| {
        html! {
            span class="mono" { (used) " / " @if limit == 0 { (tx("sınırsız")) } @else { (limit) } @if !unit.is_empty() { " " (unit) } }
            @if limit > 0 {
                @let pct = (used * 100 / limit).min(100);
                span class="bar" { i class=(if pct >= 90 { "hot" } else { "" }) style={ "width:" (pct) "%" } {} }
            }
        }
    };
    let body = html! {
        (page_head(
            html! { (tx("Havuzlar")) },
            sub(vec![html! { span { (c.pools.len()) " havuz" } }, html! { span { (c.guests.iter().filter(|g| g.pool.is_some()).count()) (tx(" konuk havuzda")) } }]),
            html! {},
            "pool list",
            &[],
        ))
        div class="card" {
            header { h2 { (tx("Kaynak havuzları")) } (tip("Bir grup konuğun toplam çekirdek, bellek ve disk ayırmasına kota koyar.")) span class="note" { (tx("ayrılan kaynak / kota · ")) (c.pools.len()) " / " (redox_core::MAX_POOLS) } }
            @if c.pools.is_empty() {
                p class="empty" { (tx("Havuz yok. Havuz, bir grup konuğun toplam çekirdek, bellek ve disk ayırmasına sınır koyar; konuğu havuza Seçenekler sekmesinden alırsınız.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("Havuz")) } th { (tx("Konuk")) } th { (tx("Çekirdek")) } th { (tx("Bellek")) } th { "Disk" } th { span class="sr" { (tx("Sil")) } } } }
                    tbody { @for pool in &c.pools {
                        @let used = c.pool_usage(&pool.name, None);
                        tr {
                            td class="mono" { a class="link" href={ "/guests?q=" (pool.name) } { (pool.name) } }
                            td { (quota(used.guests as u64, pool.guests as u64, "")) }
                            td { (quota(used.cores as u64, pool.cores as u64, "")) }
                            td { (quota(used.memory_mb, pool.memory_mb, "MB")) }
                            td { (quota(used.disk_gb, pool.disk_gb, "GB")) }
                            td class="act" {
                                @if p.admin { form method="post" action={ "/pools/" (pool.name) "/delete" } { button class="btn sm danger" type="submit" disabled[used.guests > 0] aria-label={ (pool.name) (tx(" havuzunu sil")) } { (icon("trash")) (tx("Sil")) } } }
                            }
                        }
                    } }
                } }
            }
            @if p.admin {
                form class="inline" method="post" action="/pools" {
                    label for="pool-name" { (tx("Yeni havuz")) }
                    input id="pool-name" name="name" type="text" required placeholder=(tx("ad (örn. web)")) maxlength="24" disabled[full];
                    input name="cores" type="text" inputmode="numeric" placeholder=(tx("çekirdek")) aria-label=(tx("Çekirdek kotası")) style="max-width:7rem" disabled[full];
                    input name="memory_mb" type="text" inputmode="numeric" placeholder=(tx("bellek (MB)")) aria-label=(tx("Bellek kotası (MB)")) style="max-width:8rem" disabled[full];
                    input name="disk_gb" type="text" inputmode="numeric" placeholder="disk (GB)" aria-label=(tx("Disk kotası (GB)")) style="max-width:7rem" disabled[full];
                    input name="guests" type="text" inputmode="numeric" placeholder=(tx("konuk sayısı")) aria-label=(tx("En fazla konuk")) style="max-width:8rem" disabled[full];
                    button class="btn sm" type="submit" disabled[full] { (icon("plus")) (tx("Kaydet")) }
                }
            }
            footer {
                (tx("Kota, konukların ayrılmış kaynaklarına uygulanır (o anki kullanıma değil); boş bırakılan alan sınırsızdır. Kota dolduğunda havuza konuk alınamaz, "))
                (tx("havuzdaki konuğun çekirdeği, belleği ve diski artırılamaz, konuk klonlanamaz. Var olan bir havuzun adını yazarsanız kotaları güncellenir. İçinde konuk olan havuz silinmez. "))
                (tx("Komut satırından: ")) code { "redox pool add web cores=32 memory=65536 disk=500" }
            }
        }
    };
    layout(p, (tx("Havuzlar")), &[((tx("Havuzlar")), "/pools")], false, body)
}
