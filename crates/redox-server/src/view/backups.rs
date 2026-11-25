//! Yedekler sayfası: liste, dosya tarayıcısı, arşiv, korumasız konuklar ve yedek işleri.

use super::*;

/// Bütün konukların yedekleri tek listede.
pub fn backups(p: &Page) -> String {
    let c = p.cluster;
    let mut all: Vec<_> = c.backups.iter().collect();
    all.sort_by_key(|b| std::cmp::Reverse(b.created));
    let offsite = all.iter().filter(|b| b.offsite).count();
    let body = html! {
        (page_head(
            html! { (tx("Yedekler")) },
            sub(vec![
                html! { span { (all.len()) " yedek" } },
                html! { @match p.offsite {
                    Some(target) => span { (offsite) " / " (all.len()) (tx(" dış hedefte: ")) span class="mono" { (target) } },
                    None => span { (tx("dış hedef tanımlı değil")) },
                } },
            ]),
            html! {},
            "backup list",
            &[],
        ))
        @if let Some((storage, f)) = p.cluster.backup_forecast(p.now) {
            div class="card" style="margin-bottom:var(--s4)" {
                header {
                    h2 { (tx("Doluluk tahmini")) }
                    @match f.days_left {
                        Some(days) if days <= crate::alerts::FORECAST_CRIT_DAYS => span class="pill err" { (icon("alert")) (days) (tx(" gün kaldı")) },
                        Some(days) if days <= crate::alerts::FORECAST_WARN_DAYS => span class="pill warn" { (icon("alert")) (days) (tx(" gün kaldı")) },
                        Some(days) => span class="pill ok" { (days) (tx(" gün kaldı")) },
                        None => span class="pill ok" { (tx("büyüme yok")) },
                    }
                    span class="note mono" { (storage) " · " (f.mb_per_day) (tx(" MB/gün")) }
                }
                footer { (tx("Son iki haftada alınan yedeklerin eklediği veriden hesaplanır; silinen yedeklerin geri verdiği yer sayılmaz, yani tahmin temkinlidir. 14 gün kala uyarı, 3 gün kala kritik uyarı verilir.")) }
            }
        }
        (unprotected_card(p))
        (archive_card(p))
        (jobs_card(p))
        div class="card" style="margin-top:var(--s4)" {
            header { h2 { (tx("Yedekler")) } (tip("Bütün konukların yedekleri. Yedek, konuğun disklerinin sunucudaki yedek dizinine alınmış kopyasıdır.")) }
            @if all.is_empty() {
                p class="empty" { (tx("Henüz yedek yok. Yedek, bir konuğun sayfasındaki Yedekler sekmesinden ya da zamanlamayla alınır.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("Yedek")) } th { (tx("Konuk")) } th { (tx("Alındığı an")) } th class="num" { (tx("Boyut")) } th { (tx("Dış kopya")) } th { span class="sr" { (tx("İşlemler")) } } } }
                    tbody { @for b in &all { tr {
                        td class="mono" {
                            a class="link" href={ "/guests/" (b.vmid) "?tab=yedekler" } { (b.id) }
                            @if b.auto { " " span class="tag" { "otomatik" } }
                            @if let Some(job) = &b.job { " " span class="tag" { (tx("iş: ")) (job) } }
                            @if let Some(test) = &b.tested { " " span class="tag" title=(test.note) { @if test.ok { (tx("sınandı: ")) (redox_core::retention::date_label(test.at)) } @else { (tx("sınama başarısız")) } } }
                            @if b.is_locked(crate::state::now()) { " " span class=(tx("tag gold")) title=(tx("Bu tarihe kadar silinemez")) { "kilitli: " (redox_core::retention::date_label(b.locked_until.unwrap_or(0))) } }
                            @if b.machine { " " span class="tag" { "UEFI/TPM" } }
                            @if b.new_mb.is_some() { " " span class=(tx("tag gold")) { (tx("artımlı")) } }
                        }
                        td class="name" { a href={ "/guests/" (b.vmid) } { span class="mono dim" { (b.vmid) } " " (b.guest_name) } }
                        td class="mono muted" { (datetime(b.created)) }
                        td class="num mono" { (b.size_mb) " MB" }
                        td {
                            @if b.offsite { span class="pill ok" { (icon("check")) (tx("Var")) } } @else { span class="pill idle" { (tx("yalnızca yerel")) } }
                            @if b.copies > 0 { " " span class="tag" title=(tx("Ek dış hedeflerdeki kopya sayısı")) { "+" (b.copies) " kopya" } }
                            @if b.offsite && !b.offsite_bound { " " span class="pill warn" title=(tx("Bu kopya eski biçimde gönderilmiş: hedefte başka bir yedeğin dosyasıyla değiştirilirse fark edilmez. Konuğun Yedekler sekmesinden yeniden gönderin.")) { (tx("ada bağlı değil")) } }
                        }
                        td class="act" {
                            @if p.admin && b.id.starts_with("vm-") {
                                form method="post" action={ "/backups/" (b.id) "/restore-new" } { button class="btn sm" type="submit" aria-label={ (b.id) (tx(" yedeğinden yeni konuk aç")) } { (icon("plus")) (tx("Yeni Konuk")) } }
                            }
                        }
                    } } }
                } }
                footer { (tx("Geri yükleme, doğrulama ve silme konuğun Yedekler sekmesinden yapılır. Yeni Konuk, yedeği özgün konuğa dokunmadan ayrı bir sanal makine olarak açar; konuğu silinmiş yedekler de böyle kurtarılır.")) }
            }
        }
    };
    layout(p, (tx("Yedekler")), &[((tx("Yedekler")), "/backups")], false, body)
}

/// Yedeğin içindeki bir dizin: alt dizinlere inilir, dosyalar tek tek ya da dizin tar olarak indirilir.
pub fn backup_files(p: &Page, b: &redox_core::Backup, disk: u8, path: &str, listed: &Result<Vec<crate::guestfs::Entry>, String>) -> String {
    let link = |target: &str| format!("/backups/{}/files?disk={disk}&path={}", b.id, enc(target));
    let download = |target: &str, tar: bool| format!("/backups/{}/file?disk={disk}&path={}{}", b.id, enc(target), if tar { "&tar=1" } else { "" });
    let body = html! {
        (page_head(
            html! { (tx("Yedeğin içindeki dosyalar")) },
            sub(vec![
                html! { span class="mono" { (b.id) } },
                html! { a href={ "/guests/" (b.vmid) "?tab=yedekler" } { (b.vmid) " " (b.guest_name) } },
                html! { span { (datetime(b.created)) } },
            ]),
            html! {},
            &format!("backup ls {} {path}", b.id),
            &[],
        ))
        @if !b.disks.is_empty() {
            nav class="tabs" aria-label=(tx("Diskler")) {
                a href={ "/backups/" (b.id) "/files?disk=0&path=/" } aria-current=[(disk == 0).then_some("page")] { (tx("Asıl disk")) }
                @for d in &b.disks { a href={ "/backups/" (b.id) "/files?disk=" (d.id) "&path=/" } aria-current=[(disk == d.id).then_some("page")] { "Disk " (d.id) } }
            }
        }
        div class="card" {
            header {
                h2 class="mono" { (path) }
                @if path != "/" { a class="btn sm end" href=(download(path, true)) download { (icon("disk")) (tx("Bu klasörü indir (tar)")) } }
            }
            @match listed {
                Err(message) => div class="notice err" role="alert" { (icon("alert")) span { (message) } },
                Ok(entries) => {
                    div class="tw" { table {
                        thead { tr { th { (tx("Ad")) } th { (tx("Tür")) } th { span class="sr" { (tx("İndir")) } } } }
                        tbody {
                            @if path != "/" { tr { td class="mono" { a class="link" href=(link(&crate::guestfs::parent(path))) { ".." } } td class="muted" { (tx("üst klasör")) } td {} } }
                            @for e in entries {
                                @let full = crate::guestfs::join(path, &e.name);
                                tr {
                                    td class="mono" { @if e.is_dir() { a class="link" href=(link(&full)) { (e.name) "/" } } @else { (e.name) } }
                                    td class="muted" { @match e.kind { 'd' => (tx("klasör")), 'r' => "dosya", 'l' => (tx("bağ")), _ => (tx("özel")) } }
                                    td class="act" {
                                        @if e.kind == 'r' { a class="btn sm" href=(download(&full, false)) download { (icon("disk")) (tx("İndir")) } }
                                        @if e.is_dir() { a class="btn sm" href=(download(&full, true)) download { (icon("disk")) "tar" } }
                                    }
                                }
                            }
                        }
                    } }
                    @if entries.is_empty() { p class="empty" { (tx("Bu klasör boş.")) } }
                }
            }
            footer {
                (tx("Yedek salt okunur açılır; konuğa ve yedeğe dokunulmaz. Her listeleme birkaç saniye sürer (artımlı yedekte önce disk geçici bir dosyaya dökülür). "))
                (tx("Yedeğin içeriği konuğun bütün dosyalarıdır: bu sayfayı yalnızca yöneticiler açabilir ve her erişim denetim günlüğüne yazılır."))
            }
        }
    };
    layout(p, (tx("Yedekteki dosyalar")), &[((tx("Yedekler")), "/backups"), (b.id.as_str(), "")], false, body)
}

/// Arşiv: yerelde silinmiş ama kopya hedefinde saklanan yedekler. Boşsa kart çizilmez.
pub(super) fn archive_card(p: &Page) -> Markup {
    let list = &p.cluster.archive;
    html! {
        @if !list.is_empty() {
            div class="card" style="margin-bottom:var(--s4)" {
                header { h2 { (tx("Arşiv")) } (tip("Yerelde silinmiş ama saklama süresi olan kopya hedefinde duran yedekler. Geri getirilip geri yüklenebilirler.")) span class="note" { (list.len()) (tx(" yedek yalnızca kopya hedefinde duruyor")) } }
                div class="tw" { table {
                    thead { tr { th { (tx("Yedek")) } th { (tx("Konuk")) } th { (tx("Alındığı an")) } th class="num" { (tx("Boyut")) } th { span class="sr" { (tx("İşlem")) } } } }
                    tbody { @for b in list { tr {
                        td class="mono" { (b.id) }
                        td { (b.guest_name) " " span class="mono muted" { (b.vmid) } }
                        td class="mono muted" { (datetime(b.created)) }
                        td class="num mono" { (b.size_mb) " MB" }
                        td class="act" { @if p.admin { form method="post" action={ "/backups/" (b.id) "/recover" } { button class="btn sm" type="submit" { (icon("restart")) (tx("Geri Getir")) } } } }
                    } } }
                } }
                footer { (tx("Bu yedekler yerelde silindi; kopya hedefinin saklama süresi (keep_days) dolana kadar orada durur. Geri getirilen yedek listeye döner ve olağan bir yedek gibi geri yüklenir (dosyalar o sırada kopya hedefinden iner).")) }
            }
        }
    }
}

/// Korumasız konuklar: belirlenen süredir yedeği alınmamış olanlar. Hepsi korunuyorsa kart çizilmez.
pub(super) fn unprotected_card(p: &Page) -> Markup {
    let now = crate::state::now();
    let list = p.cluster.unprotected(now, p.backup_age);
    html! {
        @if !list.is_empty() {
            div class="card" style="margin-bottom:var(--s4)" {
                header { h2 { (tx("Korumasız konuklar")) } (tip("Son yedeği belirlenen süreden eski olan ya da hiç yedeği olmayan konuklar.")) span class="pill warn" { (icon("alert")) (list.len()) " konuk" } span class="note" { @if p.backup_age > 0 { (p.backup_age) (tx(" gündür ya da kendi hedefinden uzun süredir yedeği yok")) } @else { (tx("yedek yaşı hedefini kaçıranlar")) } } }
                div class="tw" { table {
                    thead { tr { th { (tx("Konuk")) } th { (tx("Son yedek")) } th { (tx("Zamanlama")) } } }
                    tbody { @for (vmid, name, last) in &list {
                        @let guest = p.cluster.guest(*vmid).ok();
                        tr {
                            td class="name" { a href={ "/guests/" (vmid) "?tab=yedekler" } { span class="mono dim" { (vmid) } " " (name) } }
                            td class="mono muted" {
                                @match last {
                                    Some(ts) => { (redox_core::retention::date_label(*ts)) " · " (now.saturating_sub(*ts) / redox_core::retention::DAY_SECS) (tx(" gün önce")) },
                                    None => (tx("hiç yedeği yok")),
                                }
                            }
                            td class="muted" {
                                @let in_job = guest.is_some_and(|g| p.cluster.jobs.iter().any(|j| p.cluster.job_guests(j).contains(&g.vmid)));
                                @if guest.is_some_and(|g| g.backup_time.is_some()) { (tx("kendi zamanlaması var")) } @else if in_job { (tx("bir yedek işinin kapsamında")) } @else { (tx("zamanlanmamış")) }
                            }
                        }
                    } }
                } }
                footer { (tx("Zamanlaması olduğu halde burada görünen konuğun yedekleri alınamıyor olabilir: Görevler sayfasına bakın. Süre ")) code { "--backup-age-days" } (tx(" ile değişir (0 kapatır).")) }
            }
        }
    }
}

/// Yedek işleri: birden çok konuğu tek zamanlamayla, sırayla yedekler.
pub(super) fn jobs_card(p: &Page) -> Markup {
    let c = p.cluster;
    let full = c.jobs.len() >= redox_core::MAX_JOBS;
    html! {
        div class="card" {
            header { h2 { (tx("Yedek işleri")) } (tip("Bir etiketteki (ya da bütün) konukları belirlenen gün ve saatte sırayla yedekleyen zamanlamalar.")) span class="note" { "sunucu saatiyle · " (c.jobs.len()) " / " (redox_core::MAX_JOBS) } }
            @if c.jobs.is_empty() {
                p class="empty" { (tx("Yedek işi yok. Bir iş, seçtiğiniz etiketteki (ya da bütün) konukları belirlediğiniz gün ve saatte sırayla yedekler.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("İş")) } th { (tx("Saat")) } th { (tx("Günler")) } th { (tx("Kapsam")) } th class="num" { (tx("Saklama")) } th { (tx("Durum")) } th { span class="sr" { (tx("İşlemler")) } } } }
                    tbody { @for j in &c.jobs { tr {
                        td class="mono" { (j.name) }
                        td class="mono" { (j.time) @if let Some(until) = &j.until { "–" (until) } }
                        td { (tfmt(j.days_label())) @if j.verify { " " span class="tag" title=(tx("Her yedekten sonra bozulma denetimi yapılır")) { (tx("doğrulamalı")) } } }
                        td { (j.selection_label()) " " span class="muted" { "(" (c.job_guests(j).len()) " konuk)" } }
                        td class="num mono" { (j.retention_label()) }
                        td {
                            @if j.active() {
                                span class="pill warn" { (tx("Sürüyor")) } " " span class="muted" { (j.queue.len() + usize::from(j.running.is_some())) (tx(" konuk kaldı")) }
                            } @else {
                                span class="mono muted" { "son: " (j.last_run.as_deref().unwrap_or("-")) }
                            }
                        }
                        td class="act" {
                            @if p.admin {
                                form method="post" action={ "/backups/jobs/" (j.name) "/run" } { button class="btn sm" type="submit" disabled[j.active()] aria-label={ (j.name) (tx(" işini şimdi çalıştır")) } { (icon("play")) (tx("Şimdi Çalıştır")) } }
                                form method="post" action={ "/backups/jobs/" (j.name) "/delete" } { button class="btn sm danger" type="submit" aria-label={ (j.name) (tx(" işini sil")) } { (icon("trash")) (tx("Sil")) } }
                            }
                        }
                    } } }
                } }
            }
            @if p.admin {
                form class="inline" method="post" action="/backups/jobs" {
                    label for="job-name" { (tx("Yeni iş")) }
                    input id="job-name" name="name" type="text" required placeholder=(tx("ad (örn. gece)")) maxlength="24" disabled[full];
                    input name="time" type="time" required aria-label=(tx("Saat")) disabled[full];
                    @for (n, day) in [(tx("Pzt")), (tx("Sal")), (tx("Çar")), (tx("Per")), (tx("Cum")), (tx("Cmt")), (tx("Paz"))].iter().enumerate() {
                        label class="check" { input type="checkbox" name="day" value=(n + 1) checked disabled[full]; (day) }
                    }
                    input name="tags" type="text" placeholder=(tx("etiketler (boş: hepsi)")) aria-label=(tx("Etiketler")) disabled[full];
                    input name="keep" type="text" inputmode="numeric" value=(redox_core::DEFAULT_KEEP) aria-label=(tx("Saklanacak yedek sayısı")) style="max-width:5rem" disabled[full];
                    input name="weekly" type="text" inputmode="numeric" placeholder=(tx("haftalık")) aria-label=(tx("Saklanacak haftalık yedek")) style="max-width:6rem" disabled[full];
                    input name="monthly" type="text" inputmode="numeric" placeholder=(tx("aylık")) aria-label=(tx("Saklanacak aylık yedek")) style="max-width:6rem" disabled[full];
                    input name="yearly" type="text" inputmode="numeric" placeholder=(tx("yıllık")) aria-label=(tx("Saklanacak yıllık yedek")) style="max-width:6rem" disabled[full];
                    input name="lock" type="text" inputmode="numeric" placeholder=(tx("kilit (gün)")) aria-label=(tx("Yedekler kaç gün silinemez")) style="max-width:7rem" disabled[full];
                    label for="job-until" { (tx("pencere sonu")) }
                    input id="job-until" name="until" type="time" aria-label=(tx("Yedek penceresinin sonu")) disabled[full];
                    label class="check" { input type="checkbox" name="verify" value="1" disabled[full]; (tx("Doğrula")) }
                    button class="btn sm" type="submit" disabled[full] { (icon("plus")) (tx("Kaydet")) }
                }
            }
            footer {
                (tx("Konuklar tek tek yedeklenir: biri bitmeden sıradaki başlamaz. Saklama sayısı konuk başınadır ve yalnızca bu işin aldığı yedeklere uygulanır. "))
                (tx("Haftalık, aylık ve yıllık alanlar son N yedeğe ek olarak saklanacak dönem yedekleridir; kilit, işin aldığı yedekleri o kadar gün silinemez yapar. "))
                (tx("Pencere sonu verilirse o saatten sonra sıradaki konuklara başlanmaz (süren yedek kesilmez; elle başlatılan tur pencereye bakmaz). Doğrula, her yedekten sonra bozulma denetimi yapar. "))
                (tx("Var olan bir işin adını yazarsanız iş güncellenir. Komut satırından: ")) code { "redox job add gece time=03:00 days=1-5 tags=web" }
            }
        }
    }
}
