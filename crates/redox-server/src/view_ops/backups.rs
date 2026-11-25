//! Anlık görüntüler ve yedekler sekmeleri.

use super::*;

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
                        td class="mono" {
                            (s.name)
                            @if s.disk_only { " " span class="tag" title="Üretim denetim noktası: bellek durumu yok. Geri dönmek için konuk kapalı olmalıdır; konuk o andaki diskle yeniden açılır." { "yalnızca disk" } }
                            @if s.quiesced { " " span class="tag" title="Görüntü alınırken dosya sistemleri konuk ajanıyla donduruldu." { "tutarlı" } }
                        }
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
                    @if live && g.kind == GuestKind::Vm && g.status != GuestStatus::Stopped {
                        label class="check" title="Bellek kaydedilmez; konuk ajanı varsa dosya sistemleri dondurulur. Küçük ve hızlıdır; geri dönüşte konuk o andaki diskle yeniden açılır." { input type="checkbox" name="production" value="1" disabled[!can_create]; "Üretim (belleksiz)" }
                    }
                    button class="btn primary sm" type="submit" disabled[!can_create] { (icon("snapshot")) "Al" }
                }
                @if live {
                    @if g.status != GuestStatus::Stopped {
                        footer { "Konuk çalışırken alınan görüntü bellek durumunu da içerir; kayıt sırasında konuk kısa süre duraklar. \"Üretim\" seçilirse bellek kaydedilmez: yalnızca diskler, dosya sistemleri dondurularak görüntülenir." }
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
    let live = if g.kind == GuestKind::Vm { p.live } else { p.live_ct };
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
                                @if let Some(note) = &b.suspicious { span class="pill warn" title=(note) { (icon("alert")) "olağan dışı değişim" } }
                                @if b.offsite && !b.offsite_bound { span class="tag" title="Dış kopya eski biçimde: hedefte başka bir yedeğin dosyasıyla değiştirilirse fark edilmez. Menüden yeniden gönderin." { "dış kopya ada bağlı değil" } }
                                @if b.is_locked(crate::state::now()) { span class="tag gold" title="Bu tarihe kadar silinemez ve budanmaz" { "kilitli: " (redox_core::retention::date_label(b.locked_until.unwrap_or(0))) } }
                                @if b.copies > 0 { span class="tag" title="Ek dış hedeflerdeki kopya sayısı" { "+" (b.copies) " kopya" } }
                                @if let Some(test) = &b.tested {
                                    @if test.ok { span class="tag" title=(test.note) { "sınandı: " (redox_core::retention::date_label(test.at)) } }
                                    @else { span class="tag" title=(test.note) { "sınama başarısız" } }
                                }
                                @if b.machine { span class="tag" title="UEFI değişkenleri ve sanal TPM durumu yedeğe dahil" { "UEFI/TPM" } }
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
                                        @if g.kind == GuestKind::Vm && b.new_mb.is_none() {
                                            form method="post" action={ "/backups/" (b.id) "/instant" } { button type="submit" { (icon("play")) "Anında aç" small { "kopyalama beklemeden" } } }
                                        }
                                        @if g.kind == GuestKind::Vm {
                                            form method="post" action={ "/backups/" (b.id) "/test" } { button type="submit" { (icon("play")) "Açarak sına" small { "geçici, yalıtılmış konuk" } } }
                                        }
                                        @if p.files && g.kind == GuestKind::Vm {
                                            a href={ "/backups/" (b.id) "/files?path=/" } { (icon("disk")) "Dosyalara göz at" small { "tek dosya geri al" } }
                                        }
                                        @if g.kind == GuestKind::Vm {
                                            form method="post" action={ "/backups/" (b.id) "/restore-new" } { button type="submit" { (icon("plus")) "Yeni konuk olarak aç" small { "bu konuğa dokunmaz" } } }
                                        }
                                        @if !b.offsite && p.offsite.is_some() {
                                            form method="post" action={ "/backups/" (b.id) "/push" } { button type="submit" { (icon("migrate")) "Dış hedefe gönder" } }
                                        }
                                        form method="post" action={ "/backups/" (b.id) "/lock" } { button type="submit" { (icon("shield")) "Kilitle" small { (redox_core::retention::DEFAULT_LOCK_DAYS) " gün silinemez" } } }
                                        hr;
                                        @if b.is_locked(crate::state::now()) {
                                            span class="muted" { (icon("shield")) "Kilitli yedek silinemez" }
                                        } @else {
                                            a class="danger" href={ (base) "/confirm?op=delbackup&name=" (b.id) } { (icon("trash")) "Yedeği sil…" }
                                        }
                                    }
                                }
                            }
                        }
                    } } }
                }
                footer {
                    @if p.admin { (stopped_note(g, if live { "Geri yüklemek" } else { "Yedek almak ve geri yüklemek" })) }
                    @if p.real && g.kind == GuestKind::Vm { p { "Konuk çalışırken alınan yedekte, konukta ajan (qemu-guest-agent) varsa dosya sistemleri kopyadan hemen önce dondurulur (" span class="mono" { "donduruldu" } "). Ajan yoksa yedek yine alınır; elektriği kesilmiş bir diskin tutarlılığındadır." } }
                    @if p.live_ct && g.kind == GuestKind::Ct { p { "Çalışan konteyner yedek süresince dondurulur (içindeki programlar durur, ağ bağlantıları bekler) ve yedek bitince kaldığı yerden sürer. Kesinti istemiyorsanız yedeği sakin bir saate zamanlayın." } }
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
