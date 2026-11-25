//! Yedekten yeni konuk, dosya düzeyinde geri yükleme, GFS, kilit, sınama, anında kurtarma, rapor, pencere.

use super::*;

#[tokio::test]
pub(super) async fn backups_can_be_opened_as_new_guests_from_the_pages_and_the_api() {
    let (app, state) = setup();
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&format!(r#"action="/backups/{id}/restore-new""#)) && tab.contains("Yeni konuk olarak aç"), "{tab}");
    let all = page(&app, "/backups").await;
    assert!(all.contains(&format!(r#"aria-label="{id} yedeğinden yeni konuk aç""#)));

    let before = state.snapshot().guests.len();
    let to = post_ok(&app, &format!("/backups/{id}/restore-new"), "").await;
    let c = state.snapshot();
    let copy = c.guests.iter().find(|g| g.name == "win-build-yedekten").expect("yeni konuk");
    assert!(to.starts_with(&format!("/guests/{}?notice=", copy.vmid)), "{to}");
    assert_eq!((c.guests.len(), copy.kind, copy.status, copy.cores), (before + 1, GuestKind::Vm, GuestStatus::Stopped, 8));
    assert_eq!(c.tasks[0].command, format!("vm restore-new {} {id}", copy.vmid));
    // Aynı ad ikinci kez reddedilmez (adlar tekil olmak zorunda değil) ama izleyici açamaz; konteyner yedeği açılmaz.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, &format!("/backups/{id}/restore-new")), "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let ct = state.backup_create(202, "admin").unwrap();
    assert!(post_ok(&app, &format!("/backups/{ct}/restore-new"), "").await.contains("error="));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains("Yeni konuk olarak aç"));
    assert!(post_ok(&app, "/backups/vm-999-1/restore-new", "").await.contains("error="));

    // API: ad verilir, yeni kimlik döner.
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{id}/restore-new"), &json!({ "name": "api-kopyasi" }))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{v}");
    assert_eq!(state.snapshot().guest(v["vmid"].as_u64().unwrap() as u32).unwrap().name, "api-kopyasi");
    let (status, _) = api(&app, post_json(&format!("/api/v1/backups/{id}/restore-new"), &json!({ "name": "Geçersiz Ad" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(redox_core::restored_name("web"), "web-yedekten");
    assert!(redox_core::restored_name(&"a".repeat(80)).len() <= 63);
}

// ---------- Dosya düzeyinde geri yükleme ----------

#[tokio::test]
pub(super) async fn files_inside_a_backup_need_guestfish_and_an_admin() {
    let (app, state) = setup_real();
    let id = state.backup_create(103, "admin").unwrap();
    // guestfish yokken: bağlantı sunulmaz, istek açık bir hatayla döner.
    assert!(!page(&app, "/guests/103?tab=yedekler").await.contains("Dosyalara göz at"));
    let listing = page(&app, &format!("/backups/{id}/files?path=/")).await;
    assert!(listing.contains("guestfish gerekir") && listing.contains("libguestfs-tools"), "{listing}");
    let (status, v) = api(&app, get(&format!("/api/v1/backups/{id}/files?path=/"))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let (status, _, body) = send(&app, get(&format!("/backups/{id}/file?path=/etc/hostname"))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && body.contains("guestfish"), "{status} {body}");

    // Yol denetimi ve yetki, araçtan önce gelir.
    for bad in ["etc", "/etc/../shadow", "-a"] {
        let (status, _) = api(&app, get(&format!("/api/v1/backups/{id}/files?path={bad}"))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    assert_eq!(send(&app, get(&format!("/backups/{id}/file?path=/"))).await.0, StatusCode::UNPROCESSABLE_ENTITY, "kök tek dosya olarak indirilemez");
    let viewer = |uri: String| as_viewer(Method::GET, &uri).body(Body::empty()).unwrap();
    assert_eq!(send(&app, viewer(format!("/backups/{id}/file?path=/etc/hostname"))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, viewer(format!("/backups/{id}/files?path=/"))).await.0, StatusCode::NOT_FOUND, "izleyiciye sayfa gösterilmez");
    let (status, v) = api(&app, viewer(format!("/api/v1/backups/{id}/files?path=/"))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("yalnızca yönetici"), "{v}");
    assert_eq!(send(&app, get("/backups/vm-999-1/files?path=/")).await.0, StatusCode::NOT_FOUND);

    // guestfish tanımlıyken bağlantı görünür; her erişim denetim günlüğüne yazılır.
    state.set_guestfish(Some("/yok/guestfish".into()));
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains(&format!(r#"href="/backups/{id}/files?path=/""#)));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains("Dosyalara göz at"), "konteyner yedeği tar arşividir");
    let _ = page(&app, &format!("/backups/{id}/files?path=/etc")).await;
    let audit = format!("{:?}", state.auth().audit.recent("", 20));
    assert!(audit.contains(&format!("backup ls {id} /etc")) && audit.contains(&format!("backup get {id} /etc/hostname")), "{audit}");
}

// ---------- GFS saklama ve değiştirilemez yedek ----------

#[tokio::test]
pub(super) async fn backups_can_be_locked_and_keep_weekly_monthly_yearly_points() {
    let (app, state) = setup();
    // Konuğun kendi zamanlamasında haftalık/aylık/yıllık sayıları.
    let options = page(&app, "/guests/103?tab=secenekler").await;
    assert!(options.contains(r#"name="backup_weekly" type="text" inputmode="numeric" value="0""#) && options.contains(r#"name="backup_yearly""#));
    let base = "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=03%3A30&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=&pool=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_weekly=4&backup_monthly=12&backup_yearly=2")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().backup_gfs, redox_core::Gfs { weekly: 4, monthly: 12, yearly: 2 });
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_weekly=99999")).await.contains("error="));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&backup_weekly=x")).await.contains("error="));

    // İş formu: dönem yedekleri ve kilit süresi.
    let days = "day=1&day=2&day=3&day=4&day=5&day=6&day=7";
    assert!(post_ok(&app, "/backups/jobs", &format!("name=gece&time=03%3A00&{days}&tags=&keep=7&weekly=4&monthly=6&yearly=&lock=14")).await.contains("notice="));
    let job = state.snapshot().job("gece").unwrap().clone();
    assert_eq!((job.gfs, job.lock_days), (redox_core::Gfs { weekly: 4, monthly: 6, yearly: 0 }, 14));
    assert!(page(&app, "/backups").await.contains("son 7 · 4H 6A · kilit 14 gün"));
    assert!(post_ok(&app, "/backups/jobs", &format!("name=x&time=03%3A00&{days}&tags=&keep=7&lock=cok")).await.contains("error="));

    // İşin aldığı yedek kilitli doğar: silinemez, silme bağlantısı sunulmaz, budanmaz.
    state.update_guest(103, &redox_core::GuestPatch { backup_time: Some(String::new()), ..Default::default() }, "admin").unwrap();
    state.job_put(redox_core::BackupJob::from_text("tek", "01:00", "", "kilitli-is", Some(1)).unwrap().with_retention(redox_core::Gfs::default(), 14).unwrap()).unwrap();
    state.update_guest(103, &redox_core::GuestPatch { tags: Some(vec!["kilitli-is".into()]), ..Default::default() }, "admin").unwrap();
    assert_eq!(state.jobs_tick("2026-11-25", 2, 100), 1);
    let locked = state.snapshot().backups[0].clone();
    assert_eq!(locked.locked_until, Some(locked.created + 14 * 86_400));
    assert!(post_ok(&app, &format!("/backups/{}/delete", locked.id), "").await.contains("error="));
    assert!(state.snapshot().backup(&locked.id).is_ok(), "kilitli yedek durur");
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains("kilitli: ") && tab.contains("Kilitli yedek silinemez") && !tab.contains(&format!("op=delbackup&amp;name={}", locked.id)), "{tab}");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(state.jobs_tick("2026-11-26", 3, 100), 1);
    state.scheduler_tick("2026-11-26", 200);
    assert_eq!(state.snapshot().backups.iter().filter(|b| b.job.as_deref() == Some("tek")).count(), 2, "saklama sayısı 1 olsa da kilitli yedek budanmaz");

    // Elle kilit: panelden 30 gün, API'den istenen gün; kilit kısaltılamaz.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let manual = state.backup_create(103, "admin").unwrap();
    assert!(post_ok(&app, &format!("/backups/{manual}/lock"), "").await.contains("notice="));
    let until = state.snapshot().backup(&manual).unwrap().locked_until.unwrap();
    assert!(until > now() + 29 * 86_400 && until <= now() + 30 * 86_400);
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{manual}/lock"), &json!({ "days": 60 }))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["locked_until"].as_u64().unwrap() > until);
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{manual}/lock"), &json!({ "days": 5 }))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("kısaltılamaz"), "{v}");
    let delete = request(Method::DELETE, &format!("/api/v1/backups/{manual}")).body(Body::empty()).unwrap();
    let (status, v) = api(&app, delete).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("kilitli"), "{v}");
    let viewer = as_viewer(Method::POST, &format!("/api/v1/backups/{manual}/lock")).header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "days": 1 }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    assert!(state.snapshot().tasks.iter().any(|t| t.command == format!("backup lock {manual} 60")));
}

// ---------- Yedeği açarak sınama ----------

#[tokio::test]
pub(super) async fn backups_are_boot_tested_from_the_menu_and_the_api() {
    let (app, state) = setup();
    state.set_boot_test_wait(Duration::from_millis(50));
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&format!(r#"action="/backups/{id}/test""#)) && tab.contains("Açarak sına") && !tab.contains("sınandı:"));
    let before = state.snapshot().guests.len();
    assert!(post_ok(&app, &format!("/backups/{id}/test"), "").await.contains("notice="));
    let c = state.snapshot();
    let test = c.backup(&id).unwrap().tested.clone().expect("sonuç");
    assert!(test.ok, "{}", test.note);
    assert_eq!(c.guests.len(), before, "geçici konuk silindi");
    assert!(c.tasks.iter().any(|t| t.command == format!("backup test {id}") && t.status == TaskStatus::Ok));
    assert!(page(&app, "/guests/103?tab=yedekler").await.contains("sınandı: "));
    assert!(page(&app, "/backups").await.contains("sınandı: "));
    // Konteyner yedeği açılarak sınanmaz; izleyici başlatamaz.
    let ct = state.backup_create(202, "admin").unwrap();
    assert!(post_ok(&app, &format!("/backups/{ct}/test"), "").await.contains("error="));
    assert!(!page(&app, "/guests/202?tab=yedekler").await.contains("Açarak sına"));
    let viewer = as_viewer(Method::POST, &format!("/api/v1/backups/{id}/test")).body(Body::empty()).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
    let (status, _) = api(&app, request(Method::POST, &format!("/api/v1/backups/{id}/test")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

// ---------- Anında kurtarma ----------

#[tokio::test]
pub(super) async fn instant_recovery_is_offered_for_full_backups_and_pins_them() {
    let (app, state) = setup();
    let id = state.backup_create(103, "admin").unwrap();
    let tab = page(&app, "/guests/103?tab=yedekler").await;
    assert!(tab.contains(&format!(r#"action="/backups/{id}/instant""#)) && tab.contains("Anında aç"));
    let to = post_ok(&app, &format!("/backups/{id}/instant"), "").await;
    let c = state.snapshot();
    let new = c.guests.iter().find(|g| g.hw.instant.as_deref() == Some(id.as_str())).expect("anında açılan konuk").clone();
    assert!(to.starts_with(&format!("/guests/{}?notice=", new.vmid)), "{to}");
    assert!(c.tasks.iter().any(|t| t.command == format!("vm instant {} {id}", new.vmid) && t.status == TaskStatus::Ok));
    // Yedek sabitlenir: silme reddedilir; konuğun sayfası ayırma kartını gösterir.
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.contains("error="));
    let clone_tab = page(&app, &format!("/guests/{}?tab=cogalt", new.vmid)).await;
    assert!(clone_tab.contains("Yedeğin üzerinde çalışıyor") && clone_tab.contains(&format!(r#"action="/guests/{}/detach""#, new.vmid)), "{clone_tab}");
    assert!(!page(&app, "/guests/103?tab=cogalt").await.contains("Yedeğin üzerinde çalışıyor"));
    assert!(post_ok(&app, "/guests/103/detach", "").await.contains("error="), "yedeğe dayanmayan konuk ayrılamaz");
    assert!(post_ok(&app, &format!("/guests/{}/detach", new.vmid), "").await.contains("notice="));
    assert_eq!(state.snapshot().guest(new.vmid).unwrap().hw.instant, None);
    assert!(post_ok(&app, &format!("/backups/{id}/delete"), "").await.contains("notice="), "ayrıldıktan sonra yedek silinebilir");

    // Artımlı yedek anında açılamaz; menüde de sunulmaz. API ve yetki.
    let (real, real_state) = setup_real();
    real_state.mutate(|c| c.add_backup(redox_core::Backup { id: "vm-103-9".into(), vmid: 103, guest_name: "win-build".into(), created: now(), size_mb: 5, auto: false, offsite: false, new_mb: Some(3), disks: Vec::new(), disk_gb: 200, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 }));
    assert!(!page(&real, "/guests/103?tab=yedekler").await.contains("Anında aç"));
    let (status, v) = api(&real, post_json("/api/v1/backups/vm-103-9/instant", &json!({ "name": "x" }))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("artımlı"), "{v}");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = state.backup_create(103, "admin").unwrap();
    let (status, v) = api(&app, post_json(&format!("/api/v1/backups/{second}/instant"), &json!({ "name": "api-acil" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    let vmid = v["vmid"].as_u64().unwrap();
    let (status, task) = api(&app, request(Method::POST, &format!("/api/v1/guests/{vmid}/detach")).body(Body::empty()).unwrap()).await;
    assert_eq!((status, task["command"].as_str()), (StatusCode::ACCEPTED, Some(format!("vm detach {vmid}").as_str())));
    let viewer = as_viewer(Method::POST, &format!("/api/v1/backups/{second}/instant")).header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "name": "x" }).to_string())).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::FORBIDDEN);
}

// ---------- Yedek raporu ve korumasız konuklar ----------

#[tokio::test]
pub(super) async fn unprotected_guests_and_the_backup_report_are_shown() {
    let (app, state) = setup();
    let all = state.snapshot().guests.len();
    let listed = page(&app, "/backups").await;
    assert!(listed.contains("Korumasız konuklar") && listed.contains(&format!("{all} konuk")) && listed.contains("hiç yedeği yok") && listed.contains("zamanlanmamış"), "{listed}");
    // Yedeği alınan konuk listeden düşer; zamanlaması olan konuk öyle gösterilir.
    state.backup_create(103, "admin").unwrap();
    state.update_guest(202, &redox_core::GuestPatch { backup_time: Some("03:30".into()), ..Default::default() }, "admin").unwrap();
    let listed = page(&app, "/backups").await;
    assert!(listed.contains(&format!("{} konuk", all - 1)) && listed.contains("kendi zamanlaması var"), "{listed}");
    assert!(!listed.contains(r#"href="/guests/103?tab=yedekler"><span class="mono dim">103</span>"#) || listed.matches("win-build").count() >= 1);
    // Süre 0: kart çizilmez.
    state.set_backup_age_days(0);
    assert!(!page(&app, "/backups").await.contains("Korumasız konuklar"));
    state.set_backup_age_days(7);

    // API ve izleyici: rapor salt okunurdur.
    let (status, v) = api(&app, get("/api/v1/backups/report")).await;
    assert_eq!((status, v["taken"].as_u64(), v["unprotected"].as_u64()), (StatusCode::OK, Some(1), Some(all as u64 - 1)), "{v}");
    assert!(v["body"].as_str().unwrap().contains("Son 24 saat: 1 yedek alındı") && v["subject"].as_str().unwrap().contains("yedek raporu"));
    let viewer = as_viewer(Method::GET, "/api/v1/backups/report").body(Body::empty()).unwrap();
    assert_eq!(send(&app, viewer).await.0, StatusCode::OK);

    // Zamanlama: saat ayarlı değilken hiçbir şey olmaz; ayarlıyken günde bir kez dener. E-posta ayarı yoksa neden görevde yazar.
    assert!(state.report_tick("2026-11-25", 600).is_none());
    state.set_backup_report(7 * 60 + 30);
    assert!(state.report_tick("2026-11-25", 7 * 60).is_none(), "saati gelmedi");
    let outcome = state.report_tick("2026-11-25", 7 * 60 + 31).expect("saati geldi");
    assert!(outcome.unwrap_err().contains("e-posta ayarı yok"));
    assert!(state.report_tick("2026-11-25", 900).is_none(), "aynı gün ikinci kez denenmez");
    assert!(state.report_tick("2026-11-26", 7 * 60 + 30).is_some(), "ertesi gün yeniden");
    let task = state.snapshot().tasks.iter().find(|t| t.command == "backup report").cloned().expect("görev kaydı");
    assert_eq!((task.status, task.user.as_str()), (TaskStatus::Failed, redox_server::state::SCHEDULER));
}

// ---------- Yedek penceresi ve iş sonrası doğrulama ----------

#[tokio::test]
pub(super) async fn job_windows_and_verification_are_set_from_the_form() {
    let (app, state) = setup();
    let form = page(&app, "/backups").await;
    assert!(form.contains(r#"name="until" type="time""#) && form.contains(r#"name="verify" value="1""#));
    let days = "day=1&day=2&day=3&day=4&day=5&day=6&day=7";
    assert!(post_ok(&app, "/backups/jobs", &format!("name=gece&time=22%3A00&{days}&tags=pencereli&keep=7&until=06%3A00&verify=1")).await.contains("notice="));
    let job = state.snapshot().job("gece").unwrap().clone();
    assert_eq!((job.until.as_deref(), job.verify), (Some("06:00"), true));
    let listed = page(&app, "/backups").await;
    assert!(listed.contains("22:00–06:00") && listed.contains("doğrulamalı"), "{listed}");
    assert!(post_ok(&app, "/backups/jobs", &format!("name=x&time=22%3A00&{days}&tags=&keep=7&until=22%3A00")).await.contains("error="), "pencere sonu başlangıçla aynı olamaz");

    // Üç kapalı konuk kapsamda; tur 22:00'de başlar, her turda bir konuk yedeklenir.
    for vmid in [103, 202] {
        state.update_guest(vmid, &redox_core::GuestPatch { tags: Some(vec!["pencereli".into()]), ..Default::default() }, "admin").unwrap();
    }
    assert_eq!(state.jobs_tick("2026-11-25", 2, 22 * 60), 1, "103");
    // Yedeği alınan yedek, sonraki turda doğrulanır (iş doğrulama istiyor).
    assert_eq!(state.jobs_tick("2026-11-25", 2, 22 * 60 + 1), 1, "202");
    let c = state.snapshot();
    assert!(c.tasks.iter().any(|t| t.command.starts_with("backup verify vm-103-") && t.user == redox_server::state::SCHEDULER), "{:?}", c.tasks.iter().map(|t| &t.command).collect::<Vec<_>>());
    state.jobs_tick("2026-11-25", 2, 22 * 60 + 2);
    assert!(state.snapshot().tasks.iter().any(|t| t.command.starts_with("backup verify ct-202-")));

    // Pencere kapanınca sıradaki konuklar yedeklenmez ve bu günlüğe yazılır.
    state.update_guest(101, &redox_core::GuestPatch { tags: Some(vec!["pencereli".into()]), ..Default::default() }, "admin").unwrap();
    state.mutate(|c| c.jobs[0].last_run = None);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(state.jobs_tick("2026-11-26", 3, 23 * 60), 0, "ilk konuk (101) çalışıyor: simülasyonda yedeği alınamaz");
    assert_eq!(state.snapshot().job("gece").unwrap().queue.len(), 2);
    assert_eq!(state.jobs_tick("2026-11-27", 4, 6 * 60), 0, "pencere kapandı");
    let c = state.snapshot();
    assert!(c.job("gece").unwrap().queue.is_empty());
    let closed = c.tasks.iter().find(|t| t.command == "job gece").expect("pencere kaydı");
    assert!(closed.status == TaskStatus::Failed && closed.message.as_deref().unwrap().contains("2 konuk bu turda yedeklenmedi"), "{:?}", closed.message);

    // API: alanlar kabul edilir ve doğrulanır.
    let (status, _) = api(&app, post_json("/api/v1/jobs", &json!({ "name": "api", "time": "01:00", "until": "05:00", "verify": true }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state.snapshot().job("api").unwrap().until.as_deref(), Some("05:00"));
    let (status, _) = api(&app, post_json("/api/v1/jobs", &json!({ "name": "api", "time": "01:00", "until": "99:00" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn archived_backups_are_recovered_from_the_backups_page() {
    let (app, state) = setup();
    assert!(!page(&app, "/backups").await.contains("Geri Getir"), "arşiv boşken kart çizilmez");
    assert!(post_ok(&app, "/guests/103/backups", "").await.contains("notice="));
    state.mutate(|c| {
        let mut b = c.backups.remove(0);
        b.id = "vm-103-arsivde".into();
        c.archive_backup(b);
    });
    let list = page(&app, "/backups").await;
    assert!(list.contains("Arşiv") && list.contains(r#"action="/backups/vm-103-arsivde/recover""#));
    // Kopya hedefi yokken geri getirilemez; nedeni söylenir.
    assert!(post_ok(&app, "/backups/vm-103-arsivde/recover", "").await.contains("error="));
    let dir = std::env::temp_dir().join(format!("redox-arsiv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    state.add_offsite_copy(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\nkeep_days = 30\n", dir.display())).unwrap());
    assert!(post_ok(&app, "/backups/vm-103-arsivde/recover", "").await.contains("notice="));
    let c = state.snapshot();
    assert!(c.archive.is_empty() && c.backup("vm-103-arsivde").is_ok());
    let _ = std::fs::remove_dir_all(dir);
}
