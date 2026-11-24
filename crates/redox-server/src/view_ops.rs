//! Konuk işlemlerinin sayfaları: konsol, anlık görüntüler, yedekler, donanım, seçenekler, çoğaltma ve onay.

use maud::{html, Markup, PreEscaped, DOCTYPE};
use redox_core::{Guest, GuestKind, GuestStatus};

use crate::console::SPECIAL_KEYS;
use crate::view::{datetime, guest_label, guest_name, guest_state, icon, layout, page_head, FormOptions, Page};

fn stopped_note(g: &Guest, what: &str) -> Markup {
    html! {
        @if g.status != GuestStatus::Stopped {
            p { (what) " için önce konuğu kapatın." }
        }
    }
}

/// Konteynerde çalıştırılan komutun çıktısı (ve yeni komut için form).
pub fn exec_page(p: &Page, g: &Guest, command: &str, result: &Result<String, String>) -> String {
    let base = format!("/guests/{}", g.vmid);
    let cli = format!("{} exec {}", g.kind, g.vmid);
    let label = guest_label(g);
    let body = html! {
        (page_head(html! { span class="id" { (g.vmid) } (g.name) }, html! {}, html! {}, &cli, &[]))
        div class="card" {
            header {
                h2 { "Komut çıktısı" }
                @match result {
                    Ok(_) => span class="pill ok" { (icon("check")) "Çalıştı" },
                    Err(_) => span class="pill err" { (icon("alert")) "Başarısız" },
                }
            }
            div class="body" {
                pre class="conf" { span class="gold" { "#" } " " (command) }
                pre class="conf output" { @match result { Ok(out) => (out), Err(e) => (e) } }
                (exec_form(&base, command))
                div class="btns" { a class="btn" href={ (base) "?tab=konsol" } { "Konsola Dön" } }
            }
        }
    };
    layout(p, &format!("{} komut", g.vmid), &[("Konuklar", "/guests"), (&label, &base), ("Komut", "")], false, body)
}

fn exec_form(base: &str, command: &str) -> Markup {
    html! {
        form class="keys" method="post" action={ (base) "/exec" } {
            label class="sr" for="command" { "Çalıştırılacak komut" }
            input id="command" name="command" type="text" value=(command) autocomplete="off" autocapitalize="none" spellcheck="false"
                placeholder="örn. ip addr, apk update, systemctl status ssh" required autofocus;
            button class="btn primary" type="submit" { "Çalıştır" }
        }
    }
}

fn container_console(p: &Page, g: &Guest) -> Markup {
    html! {
        div class="card" {
            header { h2 { "Komut çalıştır" } span class="note" { "konteynerde kök olarak" } }
            @if !p.real {
                p class="empty" { "Simülasyon sürücüsünde konteynere komut gönderilemez." }
            } @else if g.status != GuestStatus::Running {
                p class="empty" { "Konteyner çalışmıyor. Komut çalıştırmak için başlatın." }
            } @else {
                div class="console" {
                    (exec_form(&format!("/guests/{}", g.vmid), ""))
                    p class="hint" { "Komut " span class="mono" { "/bin/sh -c" } " ile, en çok 20 saniye çalışır ve çıktısı gösterilir. Etkileşimli programlar (vi, top) çalışmaz. Komut metni günlüğe yazılmaz." }
                }
            }
        }
    }
}

/// `live`: konuğun canlı konsolu (VNC) hazır mı?
pub fn console_tab(p: &Page, g: &Guest, live: bool) -> Markup {
    if g.kind == GuestKind::Ct {
        return container_console(p, g);
    }
    let base = format!("/guests/{}", g.vmid);
    html! {
        div class="card" {
            header {
                h2 { "Konsol" }
                span class="note" { "ekran 2 saniyede bir yenilenir" }
                @if live { div class="end" { a class="btn primary sm" href={ (base) "/console" } { (icon("terminal")) "Canlı Konsolu Aç" } } }
            }
            @if !p.real {
                p class="empty" { "Simülasyon sürücüsünde konsol yok. Gerçek ekran için sunucuyu " span class="mono" { "--driver qemu" } " ile çalıştırın." }
            } @else if g.status != GuestStatus::Running {
                p class="empty" { "Konuk çalışmıyor. Ekranı görmek için başlatın." }
            } @else {
                div class="console" {
                    iframe class="screen" src={ (base) "/screen" } title={ (g.name) " ekranı" } {}
                    form class="keys" method="post" action={ (base) "/keys" } {
                        label class="sr" for="text" { "Gönderilecek metin" }
                        input id="text" name="text" type="text" autocomplete="off" autocapitalize="none" spellcheck="false"
                            placeholder="yazılacak metin, örn. root" autofocus;
                        label class="check" { input type="checkbox" name="enter" value="1" checked; "Enter ile bitir" }
                        button class="btn primary" type="submit" { "Gönder" }
                    }
                    form class="keys" method="post" action={ (base) "/keys" } aria-label="Özel tuşlar" {
                        @for (value, label) in SPECIAL_KEYS {
                            button class="btn sm" name="key" value=(value) { (label) }
                        }
                    }
                    p class="hint" {
                        "Metin ABD klavye düzeniyle gönderilir; Türkçe harfler gönderilemez. Fare desteklenmez. "
                        @if live { "Gerçek zamanlı ekran, fare ve kendi klavye düzeniniz için " a class="link" href={ (base) "/console" } { "canlı konsolu" } " açın (o sayfa JavaScript kullanır)." }
                        @else { "Canlı konsol bu konukta hazır değil: konuk o özellikten önce açılmışsa bir kez kapatıp açın." }
                    }
                }
            }
        }
    }
}

/// Canlı konsol sayfası: uygulamada betik yükleyen tek sayfa. Kabuk (sol menü, otomatik yenileme) yoktur;
/// sayfa yenilenirse oturum kopacağı için kendi kendini yenilemez.
pub fn live_console_page(p: &Page, g: &Guest, available: bool) -> String {
    let base = format!("/guests/{}", g.vmid);
    html! {
        (DOCTYPE)
        html lang="tr" data-theme=(p.theme) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (g.vmid) " " (g.name) " · Konsol · RedoX VE" }
                link rel="icon" type="image/svg+xml" href=(crate::view::asset("logo.svg"));
                link rel="stylesheet" href=(crate::view::asset("fonts/fonts.css"));
                link rel="stylesheet" href=(crate::view::asset("style.css"));
            }
            body class="console-page" {
                header class="top" {
                    a class="btn sm" href={ (base) "?tab=konsol" } { "← Konuğa dön" }
                    strong { span class="mono gold" { (g.vmid) } " " (g.name) }
                    @if available {
                        span id="console-status" class="pill idle" role="status" { "Betik bekleniyor…" }
                        div class="btns" style="margin-left:auto" {
                            button id="console-cad" class="btn sm" type="button" { "Ctrl+Alt+Del" }
                            button id="console-full" class="btn sm" type="button" { "Tam Ekran" }
                            button id="console-blur" class="btn sm" type="button" title="Klavye odağını konuktan geri alır" { "Odağı Bırak" }
                        }
                    }
                }
                main id="main" {
                    @if available {
                        canvas id="screen" tabindex="0" width="640" height="400" data-ws={ (base) "/vnc" }
                            aria-label={ (g.name) " konuğunun ekranı; tıklayınca klavye ve fare konuğa gider" } {}
                        noscript { p class="empty" { "Canlı konsol JavaScript gerektirir. JavaScript'siz ekran için " a class="link" href={ (base) "?tab=konsol" } { "konsol sekmesini" } " kullanın." } }
                        p class="hint" { "Ekrana tıklayınca klavye ve fare konuğa gider; tuşlar konuğun kendi klavye düzeniyle yorumlanır. Bu sayfa dışında uygulamada betik çalışmaz." }
                    } @else {
                        p class="empty" {
                            @if g.status != GuestStatus::Running { "Konuk çalışmıyor. Ekranı görmek için başlatın." }
                            @else { "Bu konukta canlı konsol hazır değil: konuk o özellikten önce açılmışsa bir kez kapatıp açın." }
                            " " a class="link" href={ (base) "?tab=konsol" } { "Konuğa dön" } "."
                        }
                    }
                }
                @if available { script src=(crate::view::asset("console.js")) defer {} }
            }
        }
    }
    .into_string()
}

/// Konsol sekmesindeki çerçevenin içeriği: yalnızca ekran görüntüsü, kendini yeniler.
/// Ana sayfa yenilenmediği için yazı kutusundaki metin kaybolmaz.
pub fn console_frame(vmid: u32, now: u64) -> String {
    html! {
        (DOCTYPE)
        html lang="tr" {
            head {
                meta charset="utf-8";
                meta http-equiv="refresh" content="2";
                title { "Ekran" }
                (PreEscaped("<style>html,body{margin:0;background:#000;height:100%}img{display:block;max-width:100%;max-height:100vh;margin:0 auto;color:#bbb;font:14px sans-serif}</style>"))
            }
            body {
                img src={ "/guests/" (vmid) "/screen.png?t=" (now) } alt="Ekran görüntüsü alınamadı. Konuk kapalı olabilir.";
            }
        }
    }
    .into_string()
}

pub fn snapshots_tab(p: &Page, g: &Guest) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let live = p.live && g.kind == GuestKind::Vm;
    let can = p.admin && g.lock.is_none() && (g.status == GuestStatus::Stopped || live);
    // Operatör görüntü alabilir; geri dönmek ve silmek yöneticiye kalır.
    let can_create = p.may_operate(g) && g.lock.is_none() && (g.status == GuestStatus::Stopped || live);
    html! {
        div class="card" {
            header { h2 { "Anlık görüntüler" } span class="note mono" { (g.snapshots.len()) " / " (redox_core::MAX_SNAPSHOTS) } }
            @if g.snapshots.is_empty() {
                p class="empty" { "Henüz anlık görüntü yok. Riskli bir değişiklikten önce alın; sorun çıkarsa diske o ana dönersiniz." }
            } @else {
                div class="tw" { table {
                    thead { tr { th { "Ad" } th { "Alındığı an" } th { span class="sr" { "İşlem" } } } }
                    tbody { @for s in g.snapshots.iter().rev() { tr {
                        td class="mono" { (s.name) }
                        td class="mono muted" { (datetime(s.created)) }
                        td class="act" {
                            @if can {
                                a class="btn sm" href={ (base) "/confirm?op=rollback&name=" (s.name) } { (icon("restart")) "Geri Dön" }
                                a class="btn sm danger" href={ (base) "/confirm?op=delsnapshot&name=" (s.name) } { (icon("trash")) "Sil" }
                            }
                        }
                    } } }
                } }
            }
            @if p.may_operate(g) {
                form class="inline" method="post" action={ (base) "/snapshots" } {
                    label for="snap" { "Yeni anlık görüntü" }
                    input id="snap" name="name" type="text" placeholder="örn. guncelleme-oncesi" autocomplete="off" required disabled[!can_create];
                    button class="btn primary sm" type="submit" disabled[!can_create] { (icon("snapshot")) "Al" }
                }
                @if live {
                    @if g.status != GuestStatus::Stopped {
                        footer { "Konuk çalışırken alınan görüntü bellek durumunu da içerir; kayıt sırasında konuk kısa süre duraklar." }
                    }
                } @else if g.status != GuestStatus::Stopped {
                    footer { (stopped_note(g, "Anlık görüntü işlemleri")) }
                }
            }
        }
    }
}

pub fn backups_tab(p: &Page, g: &Guest) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let mine: Vec<_> = p.cluster.backups_of(g.vmid).collect();
    let live = p.live && g.kind == GuestKind::Vm;
    let stopped = p.admin && g.lock.is_none() && g.status == GuestStatus::Stopped;
    let can = p.may_operate(g) && g.lock.is_none() && (g.status == GuestStatus::Stopped || live);
    let stored: u64 = mine.iter().map(|b| b.new_mb.unwrap_or(b.size_mb)).sum();
    let apart: u64 = mine.iter().map(|b| b.size_mb).sum();
    let incremental = mine.iter().any(|b| b.new_mb.is_some());
    html! {
        div class="cols" {
            div class="card" {
                header {
                    h2 { "Yedekler" }
                    span class="note" { (mine.len()) " yedek" }
                    @if p.may_operate(g) {
                        form class="end" method="post" action={ (base) "/backups" } {
                            button class="btn primary sm" type="submit" disabled[!can] { (icon("backup")) "Şimdi Yedekle" }
                        }
                    }
                }
                @if mine.is_empty() {
                    p class="empty" { "Bu konuğun yedeği yok. Yedek, diskin sıkıştırılmış tam kopyasıdır ve sunucudaki yedek dizininde durur." }
                } @else {
                    ul class="rows" { @for b in &mine { li {
                        div {
                            div class="l1" {
                                span { (datetime(b.created)) }
                                @if b.auto { span class="tag" { "otomatik" } }
                                @if b.new_mb.is_some() { span class="tag gold" { "artımlı" } } @else { span class="tag" { "tam" } }
                                @if b.cbt { span class="tag" { "hızlı" } }
                                @if b.quiesced == Some(true) { span class="tag" { "donduruldu" } }
                                @if b.offsite { span class="pill ok" { (icon("check")) "dış kopya" } } @else { span class="pill idle" { "yalnızca yerel" } }
                            }
                            div class="l2" {
                                span { (b.id) }
                                span { b { (b.size_mb) " MB" } @if let Some(added) = b.new_mb { " · +" (added) " MB yeni" } }
                                span { @if b.disks.is_empty() { "yalnızca asıl disk" } @else { "asıl disk + " (b.disks.len()) " ek disk" } }
                            }
                        }
                        div class="acts" {
                            @if p.admin {
                                @if stopped { a class="btn sm" href={ (base) "/confirm?op=restore&name=" (b.id) } { (icon("restart")) "Geri Yükle…" } }
                                details class="menu" {
                                    summary class="btn sm" { "Diğer" (icon("caret")) }
                                    div class="menu-list" {
                                        @if p.real {
                                            form method="post" action={ "/backups/" (b.id) "/verify" } { button type="submit" { (icon("check")) "Doğrula" small { "bozulma denetimi" } } }
                                            a href={ "/backups/" (b.id) "/download" } download { (icon("disk")) "İndir: asıl disk" }
                                            @for d in &b.disks { a href={ "/backups/" (b.id) "/download?disk=" (d.id) } download { (icon("disk")) "İndir: disk " (d.id) } }
                                        }
                                        @if !b.offsite && p.offsite.is_some() {
                                            form method="post" action={ "/backups/" (b.id) "/push" } { button type="submit" { (icon("migrate")) "Dış hedefe gönder" } }
                                        }
                                        @if p.real || (!b.offsite && p.offsite.is_some()) { hr; }
                                        a class="danger" href={ (base) "/confirm?op=delbackup&name=" (b.id) } { (icon("trash")) "Yedeği sil…" }
                                    }
                                }
                            }
                        }
                    } } }
                }
                footer {
                    @if p.admin { (stopped_note(g, if live { "Geri yüklemek" } else { "Yedek almak ve geri yüklemek" })) }
                    @if p.real && g.kind == GuestKind::Vm { p { "Konuk çalışırken alınan yedekte, konukta ajan (qemu-guest-agent) varsa dosya sistemleri kopyadan hemen önce dondurulur (" span class="mono" { "donduruldu" } "). Ajan yoksa yedek yine alınır; elektriği kesilmiş bir diskin tutarlılığındadır." } }
                    @if incremental { p { "Artımlı yedekler ortak bir parça deposunu paylaşır: her yedek yalnızca öncekilerde bulunmayan veriyi ekler, yine de tek başına geri yüklenir. Birini silmek diğerlerini etkilemez." } }
                    @if !g.hw.disks.is_empty() || mine.iter().any(|b| !b.disks.is_empty()) { p { "Yedek, alındığı andaki ek diskleri de kapsar. Geri yükleme, yedekteki ek disklerden konuğa hâlâ takılı olanları geri getirir; sonradan eklenen disklere dokunmaz, kaldırılmış diskleri geri eklemez." } }
                }
            }
            div class="stack" {
                div class="card" {
                    header { h2 { "Zamanlama" } @if p.admin { div class="end" { a class="btn sm" href={ (base) "?tab=secenekler" } { "Zamanlamayı değiştir" } } } }
                    @match &g.backup_time {
                        Some(t) => dl class="kv" {
                            dt { "Sıklık" } dd { "Otomatik yedek: her gün " b class="mono" { (t) } }
                            dt { "Saklama" } dd { "son " (g.backup_keep) " yedek saklanır" }
                        },
                        None => p class="empty" { "Otomatik yedek kapalı." },
                    }
                }
                div class="card" {
                    header { h2 { "Dış kopya" } }
                    @match p.offsite {
                        Some(target) => {
                            dl class="kv" {
                                dt { "Hedef" } dd class="mono" { (target) }
                                dt { "Durum" } dd { (mine.iter().filter(|b| b.offsite).count()) " / " (mine.len()) " hedefte" }
                            }
                            footer { "Her yeni yedek sunucu dışına da kopyalanır: " span class="mono" { (target) } ". Yerel dosya kaybolursa geri yükleme dış kopyayı indirir." }
                        },
                        None => p class="empty" { "Yedekler aynı sunucuda durur; disk arızasına karşı sunucuyu " span class="mono" { "--offsite DOSYA" } " ile başlatıp bir dış hedef (dizin ya da S3) tanımlayın." },
                    }
                }
                @if incremental {
                    div class="card" {
                        header { h2 { "Depo kullanımı" } }
                        dl class="kv" {
                            dt { "Eklenen veri" } dd class="mono" { (stored) " MB" }
                            dt { "Tek tek dursaydı" } dd class="mono" { (apart) " MB" }
                        }
                    }
                }
            }
        }
    }
}

/// Düzenleme formunun (doğrulanmamış) alanları.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct EditForm {
    /// Formun hangi sekmeden geldiği: `donanim`, `secenekler` ya da boş (bütün alanlar).
    pub part: String,
    pub name: String,
    pub os: String,
    pub tags: String,
    pub cores: String,
    pub memory_mb: String,
    pub iso: String,
    pub forwards: String,
    pub bridge: String,
    pub onboot: Option<String>,
    pub boot_order: String,
    pub boot_delay: String,
    pub restart: Option<String>,
    pub disk_mbps: String,
    pub disk_iops: String,
    pub net_mbps: String,
    pub vlan: String,
    pub shares: String,
    pub backup_time: String,
    pub backup_keep: String,
    pub firewall: Option<String>,
    pub fw_rules: String,
    pub firmware: String,
    pub tpm: Option<String>,
    pub nics: String,
    pub usb: String,
    pub pci: String,
}

impl EditForm {
    pub fn from_guest(g: &Guest) -> Self {
        EditForm {
            part: String::new(),
            name: g.name.clone(),
            os: g.os.clone(),
            tags: g.tags.join(", "),
            cores: g.cores.to_string(),
            memory_mb: g.mem_total_mb.to_string(),
            iso: g.iso.clone().unwrap_or_default(),
            forwards: g.forwards.iter().map(|f| format!("{}:{}", f.host_port, f.guest_port)).collect::<Vec<_>>().join(", "),
            bridge: g.bridge.clone(),
            backup_time: g.backup_time.clone().unwrap_or_default(),
            backup_keep: g.backup_keep.to_string(),
            onboot: g.onboot.then(|| "1".to_string()),
            restart: g.hw.restart.then(|| "1".to_string()),
            boot_order: g.hw.boot_order.to_string(),
            boot_delay: g.hw.boot_delay.to_string(),
            disk_mbps: g.hw.disk_mbps.to_string(),
            disk_iops: g.hw.disk_iops.to_string(),
            net_mbps: g.hw.net_mbps.to_string(),
            vlan: g.hw.vlan.to_string(),
            shares: g.hw.shares.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
            firewall: g.hw.firewall.then(|| "1".to_string()),
            firmware: g.hw.firmware_choice().to_string(),
            tpm: g.hw.tpm.then(|| "1".to_string()),
            nics: g.hw.nics.join(", "),
            usb: g.hw.usb.join(", "),
            pci: g.hw.pci.join(", "),
            fw_rules: g.hw.fw_rules.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
        }
    }
}

/// Donanım sekmesi. Alanlar `form="hw"` ile tek forma bağlanır; diskler kendi formlarıyla araya girebilir.
pub fn hardware_tab(p: &Page, g: &Guest, f: &EditForm, opts: &FormOptions) -> Markup {
    let isos = &opts.isos;
    let disk_ops = !(p.real && g.kind == GuestKind::Ct);
    let base = format!("/guests/{}", g.vmid);
    let stopped = g.status == GuestStatus::Stopped && g.lock.is_none();
    let off = !(p.admin && stopped);
    let vm = g.kind == GuestKind::Vm;
    let total = g.disk_gb + g.hw.disks.iter().map(|d| d.size_gb).sum::<u64>();
    // Çalışırken takma: disk ve ağ kartı eklemek, konuk çalışırken de açıktır (duraklatılmışken değil).
    let hot = p.admin && vm && p.hotplug && g.status == GuestStatus::Running && g.lock.is_none();
    let add_off = off && !hot;
    html! {
        @if p.admin {
            form id="hw" method="post" action={ (base) "/edit" } novalidate { input type="hidden" name="part" value="donanim"; }
        }
        @if !stopped {
            div class="notice" role="status" { (icon("alert")) span { "Donanım ayarları yalnızca konuk kapalıyken değişir." @if hot { " Disk ve ağ kartı çalışırken de eklenebilir." } } }
        }
        div class="stack" {
            div class="card" {
                header { h2 { @if vm { "İşlemci, bellek ve donanım yazılımı" } @else { "İşlemci ve bellek" } } }
                div class="form" {
                    div class="field" { label for="cores" { "Çekirdek" } input id="cores" form="hw" name="cores" type="text" inputmode="numeric" value=(f.cores) disabled[off]; }
                    div class="field" { label for="memory_mb" { "Bellek (MB)" } input id="memory_mb" form="hw" name="memory_mb" type="text" inputmode="numeric" value=(f.memory_mb) disabled[off]; }
                    @if vm {
                        div class="field" {
                            label for="firmware" { "Donanım yazılımı" }
                            select id="firmware" form="hw" name="firmware" disabled[off] {
                                option value="bios" selected[f.firmware == "bios"] { "BIOS (SeaBIOS)" }
                                option value="uefi" selected[f.firmware == "uefi"] { "UEFI" }
                                option value="uefi-secure" selected[f.firmware == "uefi-secure"] { "UEFI + güvenli önyükleme" }
                            }
                            span class="hint" { "Kurulu bir işletim sistemi, donanım yazılımı değişince açılmayabilir." }
                        }
                        div class="field" {
                            span class="label" { "Güvenlik yongası" }
                            label class="check" { input type="checkbox" form="hw" name="tpm" value="1" checked[f.tpm.is_some()] disabled[off]; "Sanal TPM 2.0" }
                        }
                    }
                }
            }
            @if disk_ops {
                div class="card" {
                    header { h2 { "Diskler" } span class="note" { "toplam " (total) " GB · " (g.storage) } }
                    div class="tw" { table {
                        thead { tr { th { "Disk" } th class="num" { "Boyut" } th { span class="sr" { "İşlem" } } } }
                        tbody {
                            tr { td class="mono" { "disk 0 " span class="tag" { "asıl" } } td class="num mono" { (g.disk_gb) " GB" } td class="act" {} }
                            @for d in &g.hw.disks { tr {
                                td class="mono" { "disk " (d.id) span class="sub" { "vm-" (g.vmid) "-disk-" (d.id) } }
                                td class="num mono" { (d.size_gb) " GB" }
                                td class="act" { @if !off { a class="btn sm danger" href={ (base) "/confirm?op=deldisk&name=" (d.id) } { (icon("trash")) "Sil" } } }
                            } }
                        }
                    } }
                    @if p.admin {
                        form class="inline" method="post" action={ (base) "/resize" } {
                            label for="disk_gb" { "Yeni boyut (GB), şimdi " (g.disk_gb) }
                            input id="disk_gb" name="disk_gb" type="text" inputmode="numeric" size="6" value=(g.disk_gb + 10) disabled[off];
                            button class="btn sm" type="submit" disabled[off] { (icon("disk")) "Büyüt" }
                            span class="hint" { "Disk yalnızca büyütülür. Konuğun içindeki dosya sistemini ayrıca genişletmeniz gerekir." }
                        }
                        @if vm {
                            form class="inline" method="post" action={ (base) "/disks" } {
                                label for="new_disk_gb" { "Yeni disk (GB)" }
                                input id="new_disk_gb" name="disk_gb" type="text" inputmode="numeric" size="6" value="10" disabled[add_off];
                                button class="btn sm" type="submit" disabled[add_off] { (icon("plus")) "Disk Ekle" }
                                span class="hint" { "Ek diskler konuğa boş takılır. Anlık görüntülere, klonlara ve yedeklere dahildir." }
                            }
                        }
                    }
                }
            }
            div class="card" {
                header { h2 { "Ağ" } }
                div class="form" {
                    div class="field" {
                        label for="bridge" { "Ağ" }
                        @if p.real {
                            select id="bridge" form="hw" name="bridge" disabled[off] {
                                option value="nat" selected[f.bridge == "nat"] { "NAT" }
                                @for b in &opts.bridges { option value=(b) selected[*b == f.bridge] { "Köprü: " (b) } }
                            }
                        } @else {
                            input id="bridge" form="hw" name="bridge" type="text" value=(f.bridge) disabled[off];
                        }
                        span class="hint" { "Köprüde konuk sunucunun ağından adres alır; port yönlendirme gerekmez." }
                    }
                    @if vm {
                        div class="field" {
                            label for="vlan" { "VLAN etiketi" }
                            input id="vlan" form="hw" name="vlan" type="text" inputmode="numeric" value=(f.vlan) disabled[off];
                            span class="hint" { "Birinci kartın etiketi (1-" (redox_core::hw::MAX_VLAN) "); 0 etiketsiz. Yalnızca köprüde. Köprüde VLAN süzme açık, hizmette CAP_NET_ADMIN yetkisi olmalı; kurulamazsa konuk başlatılmaz." }
                        }
                    }
                    div class="field" {
                        label for="nics" { "Ek ağ kartları" }
                        input id="nics" form="hw" name="nics" type="text" value=(f.nics) placeholder="vmbr1, nat" autocomplete="off" disabled[off];
                        span class="hint" { "Virgülle ayırın: köprü adı ya da nat. En fazla " (redox_core::hw::MAX_EXTRA_NICS) ". VLAN etiketi için köprü:etiket yazın, örn. vmbr1:20." }
                    }
                    div class="field" {
                        label for="forwards" { "Port yönlendirme" }
                        input id="forwards" form="hw" name="forwards" type="text" value=(f.forwards) placeholder="8080:80, 2222:22" autocomplete="off" disabled[off];
                        span class="hint" { "sunucu-portu:konuk-portu (TCP). Sunucu portu 1024 ve üzeri olmalı; güvenlik duvarında ayrıca açın." }
                    }
                }
                @if p.admin && vm {
                    @let full = g.hw.nics.len() >= redox_core::hw::MAX_EXTRA_NICS;
                    form class="inline" method="post" action={ (base) "/nics" } {
                        label for="new_nic" { "Ağ kartı ekle" }
                        @if p.real {
                            select id="new_nic" name="network" disabled[add_off || full] {
                                option value="nat" { "NAT" }
                                @for b in &opts.bridges { option value=(b) { "Köprü: " (b) } }
                            }
                        } @else {
                            input id="new_nic" name="network" type="text" value="vmbr1" autocomplete="off" disabled[add_off || full];
                        }
                        input name="vlan" type="text" inputmode="numeric" size="5" placeholder="VLAN" aria-label="VLAN etiketi (isteğe bağlı)" disabled[add_off || full];
                        button class="btn sm" type="submit" disabled[add_off || full] { (icon("plus")) "Kart Ekle" }
                        span class="hint" { @if p.hotplug { "Kart, konuk çalışırken de takılır; çıkarmak için konuğu kapatıp üstteki listeden silin." } @else { "Kart, konuk kapalıyken eklenir." } }
                    }
                }
            }
            @if vm {
                div class="card" {
                    header { h2 { "Hız sınırları" } span class="note" { "0 sınırsız" } }
                    div class="form" {
                        div class="field" {
                            label for="disk_mbps" { "Disk (MB/sn)" }
                            input id="disk_mbps" form="hw" name="disk_mbps" type="text" inputmode="numeric" value=(f.disk_mbps) disabled[off];
                            span class="hint" { "Okuma ve yazmanın toplamı; her diske ayrı uygulanır. En çok " (redox_core::hw::MAX_DISK_MBPS) "." }
                        }
                        div class="field" {
                            label for="disk_iops" { "Disk (işlem/sn)" }
                            input id="disk_iops" form="hw" name="disk_iops" type="text" inputmode="numeric" value=(f.disk_iops) disabled[off];
                            span class="hint" { "Saniyedeki okuma ve yazma işlemi; her diske ayrı. En çok " (redox_core::hw::MAX_DISK_IOPS) "." }
                        }
                        div class="field" {
                            label for="net_mbps" { "Ağ (Mbit/sn)" }
                            input id="net_mbps" form="hw" name="net_mbps" type="text" inputmode="numeric" value=(f.net_mbps) disabled[off];
                            span class="hint" { "Her yönde, köprüye bağlı her karta ayrı. NAT kartına uygulanamaz. Sunucuda iproute2 ve hizmette CAP_NET_ADMIN yetkisi gerekir; uygulanamazsa konuk başlatılmaz." }
                        }
                    }
                }
                div class="card" {
                    header { h2 { "Paylaşılan klasörler" } span class="note" { (g.hw.shares.len()) " / " (redox_core::hw::MAX_SHARES) } }
                    div class="form" {
                        div class="field full" {
                            label for="shares" { "Paylaşımlar" }
                            input id="shares" form="hw" name="shares" type="text" value=(f.shares) placeholder="ortak, veri:ro" autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" {
                                "Virgülle ayırın; salt okunur için ad:ro. Her ad, sunucuda veri dizinindeki " span class="mono" { "shares/<ad>" }
                                " klasörüdür; aynı adı kullanan konuklar aynı klasörü görür. Konukta bağlamak için: "
                                span class="mono" { "mount -t 9p -o trans=virtio,version=9p2000.L ortak /mnt/ortak" }
                                ". Paylaşım konukta bağlıyken konuk askıya alınamaz, çalışırken anlık görüntüsü alınamaz ve çalışırken taşınamaz."
                            }
                        }
                    }
                }
                div class="card" {
                    header { h2 { "Kurulum kalıbı ve aygıt aktarımı" } }
                    div class="form" {
                        div class="field" {
                            label for="iso" { "Kurulum kalıbı (ISO)" }
                            @if p.real {
                                select id="iso" form="hw" name="iso" disabled[off] {
                                    option value="" { "(yok)" }
                                    @for i in isos { option value=(i) selected[*i == f.iso] { (i) } }
                                }
                            } @else {
                                input id="iso" form="hw" name="iso" type="text" value=(f.iso) placeholder="debian-12.iso" disabled[off];
                            }
                            span class="hint" { "Takılıysa ilk açılış bu kalıptan yapılır. Kalıpları " a class="link" href="/storage" { "Depolama" } " sayfasından yüklersiniz." }
                        }
                        div class="field" {
                            label for="usb" { "USB aygıtları" }
                            input id="usb" form="hw" name="usb" type="text" value=(f.usb) placeholder="046d:c52b" autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" { "Sunucuya takılı aygıtın lsusb çıktısındaki kimliği. Hizmet kullanıcısının aygıta erişim izni olmalıdır." }
                        }
                        div class="field" {
                            label for="pci" { "PCI aygıtları" }
                            input id="pci" form="hw" name="pci" type="text" value=(f.pci) placeholder="0000:01:00.0" autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" { "lspci -D çıktısındaki adres. Sunucuda IOMMU açık ve aygıt vfio-pci sürücüsüne bağlı olmalıdır." }
                        }
                    }
                }
            }
        }
        @if p.admin {
            div class="savebar" {
                span class="note" { @if stopped { "Değişiklikler kaydedince uygulanır; konuk bir sonraki açılışta yeni donanımla başlar." } @else { "Kaydetmek için önce konuğu kapatın." } }
                a class="btn" href={ (base) "?tab=donanim" } { "Vazgeç" }
                button class="btn primary" type="submit" form="hw" disabled[off] { (icon("check")) "Donanımı Kaydet" }
            }
        }
    }
}

/// Seçenekler sekmesi: ad, etiketler, zamanlanmış yedek ve güvenlik duvarı. Konuk çalışırken de değişir.
pub fn options_tab(p: &Page, g: &Guest, f: &EditForm) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let off = !p.admin;
    let fields = html! {
            fieldset disabled[off] {
                legend { "Genel" }
                div class="form" {
                    div class="field" { label for="name" { "Ad" } input id="name" name="name" type="text" value=(f.name) required autocomplete="off"; }
                    div class="field" { label for="os" { "İşletim sistemi" } input id="os" name="os" type="text" value=(f.os); }
                    div class="field" {
                        label for="tags" { "Etiketler" }
                        input id="tags" name="tags" type="text" value=(f.tags) autocomplete="off";
                        span class="hint" { "Virgülle ayırın; küçük harf, rakam ve tire. En fazla " (redox_core::MAX_TAGS) "." }
                    }
                }
            }
            fieldset disabled[off] {
                legend { "Açılış ve yeniden başlatma" }
                div class="form" {
                    div class="field" {
                        span class="label" { "Açılış" }
                        label class="check" { input type="checkbox" name="onboot" value="1" checked[f.onboot.is_some()]; "Düğüm açıldığında başlat" }
                        span class="hint" { "Sunucu yeniden açıldığında, kapalıysa başlatılır. Hizmetin yeniden başlaması konuklara dokunmaz." }
                    }
                    div class="field" {
                        span class="label" { "Beklenmedik kapanma" }
                        label class="check" { input type="checkbox" name="restart" value="1" checked[f.restart.is_some()]; "Kendiliğinden kapanırsa yeniden başlat" }
                        span class="hint" { "Konuk çökerse ya da içinden kapatılırsa yaklaşık 15 saniye içinde yeniden açılır (10 dakikada en çok " (crate::ha::MAX_RESTARTS) " kez). Kalıcı kapatmak için panelden kapatın." }
                    }
                    div class="field" {
                        label for="boot_order" { "Başlatma sırası" }
                        input id="boot_order" name="boot_order" type="text" inputmode="numeric" value=(f.boot_order);
                        span class="hint" { "1 ilk başlar. 0 sırasızdır: sıralılardan sonra, kimlik sırasıyla başlar. En çok " (redox_core::hw::MAX_BOOT_ORDER) "." }
                    }
                    div class="field" {
                        label for="boot_delay" { "Sonraki konuktan önce bekle (saniye)" }
                        input id="boot_delay" name="boot_delay" type="text" inputmode="numeric" value=(f.boot_delay);
                        span class="hint" { "Örneğin veritabanı açıldıktan 30 saniye sonra uygulama sunucusu başlasın. En çok " (redox_core::hw::MAX_BOOT_DELAY) "." }
                    }
                }
            }
            fieldset disabled[off] {
                legend { "Zamanlanmış yedek" }
                div class="form" {
                    div class="field" {
                        label for="backup_time" { "Her gün saat" }
                        input id="backup_time" name="backup_time" type="time" value=(f.backup_time);
                        span class="hint" { "Sunucu saatiyle. Boş bırakırsanız otomatik yedek alınmaz." }
                    }
                    div class="field" {
                        label for="backup_keep" { "Saklanacak otomatik yedek" }
                        input id="backup_keep" name="backup_keep" type="text" inputmode="numeric" value=(f.backup_keep);
                        span class="hint" { "Fazlası en eskiden başlayarak silinir. Elle aldıklarınıza dokunulmaz." }
                    }
                }
            }
            fieldset disabled[off] {
                legend { "Güvenlik duvarı" }
                div class="form" {
                    div class="field" {
                        span class="label" { "Durum" }
                        label class="check" { input type="checkbox" name="firewall" value="1" checked[f.firewall.is_some()]; "Gelen trafiği kısıtla" }
                        span class="hint" {
                            "Açıkken yalnızca aşağıdaki kurallara uyan gelen bağlantılar geçer; konuğun başlattığı bağlantılar serbesttir. "
                            @if g.kind == GuestKind::Vm { "Yalnızca köprüye bağlı sanal makinede etkilidir; NAT'ta dışarıdan zaten yalnızca yönlendirilen kapılara erişilir." }
                        }
                    }
                    div class="field" {
                        label for="fw_rules" { "İzin verilen gelen trafik" }
                        textarea id="fw_rules" name="fw_rules" rows="4" spellcheck="false" autocomplete="off" placeholder="tcp/22 from 10.0.0.0/8" { (f.fw_rules) }
                        span class="hint" { "Satır başına bir kural: tcp/22, tcp/80-443, udp/53 from 192.168.1.0/24, icmp. En fazla " (redox_core::hw::MAX_FW_RULES) "." }
                    }
                }
            }
    };
    html! {
        @if p.admin {
            form method="post" action={ (base) "/edit" } novalidate {
                input type="hidden" name="part" value="secenekler";
                (fields)
                div class="savebar" {
                    span class="note" { "Bu ayarlar konuk çalışırken de değişir." }
                    a class="btn" href={ (base) "?tab=secenekler" } { "Vazgeç" }
                    button class="btn primary" type="submit" { (icon("check")) "Kaydet" }
                }
            }
        } @else {
            (fields)
        }
        (schedules_card(p, g))
    }
}

/// Zamanlanmış güç işlemleri: liste, ekleme formu ve silme düğmeleri.
fn schedules_card(p: &Page, g: &Guest) -> Markup {
    use redox_core::Action;
    let base = format!("/guests/{}", g.vmid);
    let full = g.hw.schedules.len() >= redox_core::hw::MAX_SCHEDULES;
    html! {
        div class="card" style="margin-top:var(--s4)" {
            header { h2 { "Zamanlanmış güç işlemleri" } span class="note" { "sunucu saatiyle · " (g.hw.schedules.len()) " / " (redox_core::hw::MAX_SCHEDULES) } }
            @if g.hw.schedules.is_empty() {
                p class="empty" { "Zamanlanmış işlem yok. Konuğu belirli gün ve saatlerde başlatmak, kapatmak ya da yeniden başlatmak için ekleyin." }
            } @else {
                div class="tw" { table {
                    thead { tr { th { "İşlem" } th { "Saat" } th { "Günler" } th { "Son çalışma" } th { span class="sr" { "Sil" } } } }
                    tbody { @for (i, s) in g.hw.schedules.iter().enumerate() { tr {
                        td { (s.action.label()) }
                        td class="mono" { (s.time) }
                        td { (s.days_label()) }
                        td class="mono muted" { (s.last_run.as_deref().unwrap_or("-")) }
                        td class="act" {
                            @if p.admin { form method="post" action={ (base) "/schedules/" (i) "/delete" } { button class="btn sm danger" type="submit" aria-label={ (s.action.label()) " " (s.time) " işlemini sil" } { (icon("trash")) "Sil" } } }
                        }
                    } } }
                } }
            }
            @if p.admin {
                form class="inline" method="post" action={ (base) "/schedules" } {
                    label for="sch-action" { "Yeni işlem" }
                    select id="sch-action" name="action" disabled[full] {
                        @for a in [Action::Start, Action::Stop, Action::Reboot] { option value=(a.as_str()) { (a.label()) } }
                    }
                    input name="time" type="time" required aria-label="Saat" disabled[full];
                    @for (n, day) in ["Pzt", "Sal", "Çar", "Per", "Cum", "Cmt", "Paz"].iter().enumerate() {
                        label class="check" { input type="checkbox" name="day" value=(n + 1) checked disabled[full]; (day) }
                    }
                    button class="btn sm" type="submit" disabled[full] { (icon("plus")) "Ekle" }
                }
            }
            footer {
                "Saati geldiğinde konuk zaten istenen durumdaysa işlem atlanır. Sunucu o saatte kapalıysa işlem, "
                (redox_core::hw::SCHEDULE_GRACE_MIN) " dakika içinde açılırsa yine çalışır; sonrası o gün atlanır."
            }
        }
    }
}

/// Çoğalt ve Taşı sekmesi: klon, şablon ve dışa aktarma (yalnızca yönetici).
pub fn clone_tab(p: &Page, g: &Guest) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let stopped = g.status == GuestStatus::Stopped && g.lock.is_none();
    let clones: Vec<&Guest> = p.cluster.guests.iter().filter(|c| c.hw.base == Some(g.vmid)).collect();
    html! {
        @if !stopped {
            div class="notice" role="status" { (icon("alert")) span { "Klonlamak, şablona çevirmek ve dışa aktarmak için önce konuğu kapatın." } }
        }
        div class="stack" {
            div class="card" {
                header { h2 { "Klonla" } }
                div class="op" {
                    div {
                        h3 { @if g.hw.template { "Bu şablondan yeni konuk" } @else { "Bu konuğun kopyası" } }
                        p {
                            "Disk ve ayarlar kopyalanır; anlık görüntüler ve port yönlendirmeleri kopyalanmaz."
                            @if g.hw.template { " Bağlı klon diski kopyalamaz: saniyeler içinde hazır olur, yalnızca şablondan farkını tutar ve şablona bağlı kalır." }
                        }
                    }
                    form method="post" action={ (base) "/clone" } {
                        input id="clone" name="name" type="text" value={ (g.name) "-kopya" } autocomplete="off" aria-label="Yeni konuğun adı" disabled[!stopped];
                        @if g.hw.template {
                            label class="check" { input type="checkbox" name="linked" value="1" checked disabled[!stopped]; " Bağlı klon" }
                        }
                        button class="btn primary" type="submit" disabled[!stopped] { (icon("plus")) "Klonla" }
                    }
                }
            }
            div class="card" {
                header { h2 { "Şablon" } }
                div class="op" {
                    div {
                        @if g.hw.template {
                            h3 { "Bu konuk bir şablon" }
                            p { "Bu konuk bir şablon: başlatılamaz, diski değişmez. Bağlı klonları ona dayandığı sürece silinemez ve şablonluktan çıkarılamaz." }
                        } @else {
                            h3 { "Şablona çevir" }
                            p { "Şablona çevrilen konuk başlatılamaz; ondan hızlı (bağlı) klonlar üretilir." }
                            @if let Some(b) = g.hw.base { p { "Bu konuk " a class="link" href={ "/guests/" (b) } { (b) } " şablonunun bağlı klonu: diski o şablona dayanır." } }
                        }
                    }
                    form method="post" action={ (base) "/template" } {
                        @if g.hw.template {
                            input type="hidden" name="on" value="0";
                            @if !clones.is_empty() { span class="pill warn" { (icon("alert")) (clones.len()) " bağlı klon" } }
                            button class="btn" type="submit" disabled[!stopped] { "Şablonluktan Çıkar" }
                        } @else {
                            input type="hidden" name="on" value="1";
                            button class="btn" type="submit" disabled[!stopped || g.hw.base.is_some()] { "Şablona Çevir" }
                        }
                    }
                }
                @if !clones.is_empty() {
                    div class="tw" { table {
                        thead { tr { th { "Bağlı klon" } th { "Durum" } th class="num" { "Disk" } } }
                        tbody { @for c in &clones { tr {
                            td class="name" { (guest_name(c, "/guests")) }
                            td { (guest_state(c)) }
                            td class="num mono" { (c.disk_gb) " GB" }
                        } } }
                    } }
                }
            }
            div class="card" {
                header { h2 { "Dışa aktar ve taşı" } }
                @if p.real {
                    div class="op" {
                        div {
                            h3 { "OVA olarak indir" }
                            p { "Konuğun ayarları ve diskleri (ek diskler dahil) tek bir OVA dosyasına konur; VMware ve VirtualBox gibi araçların içe aktardığı biçimdir. Disk önce dönüştürülür: indirme büyük disklerde geç başlar. Konuk kapalı olmalıdır." }
                        }
                        @if stopped { a class="btn" href={ (base) "/export" } download { (icon("disk")) "OVA Olarak İndir" } }
                        @else { span class="btn" aria-disabled="true" { (icon("disk")) "OVA Olarak İndir" } }
                    }
                }
                div class="op" {
                    div {
                        h3 { "Başka bir RedoX sunucusuna kopyala" }
                        p { "Hedef sunucunun Küme sayfasından bu konuğu seçip “Buraya Al” deyin. Kaynak konuk yerinde kalır." }
                    }
                    a class="btn" href="/cluster" { "Küme sayfası" }
                }
            }
        }
    }
}

/// Geri alınamayan işlemler için onay sayfası.
pub struct Confirm<'a> {
    pub title: &'a str,
    pub message: Markup,
    pub action: String,
    pub button: &'a str,
    pub back: String,
    pub cli: String,
}

pub fn confirm_page(p: &Page, g: &Guest, c: &Confirm) -> String {
    let label = guest_label(g);
    let href = format!("/guests/{}", g.vmid);
    let body = html! {
        (page_head(html! { (c.title) }, html! {}, html! {}, &c.cli, &[]))
        div class="card" { div class="body" {
            p class="lead" { (c.message) }
            form class="btns" method="post" action=(c.action) {
                button class="btn danger solid" type="submit" { (c.button) }
                a class="btn" href=(c.back) { "Vazgeç" }
            }
        } }
    };
    layout(p, c.title, &[("Konuklar", "/guests"), (&label, &href), ("Onay", "")], true, body)
}
