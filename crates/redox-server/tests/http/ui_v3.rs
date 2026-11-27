//! Arayüz v3: konuk listesi, görevler, yedekler, sekmeli ayarlar.

use super::*;

#[tokio::test]
pub(super) async fn guest_list_filters_by_state_and_groups_by_node() {
    let (app, state) = setup();
    let c = state.snapshot();
    let live = c.guests.iter().filter(|g| g.status != GuestStatus::Stopped).count();
    let all = page(&app, "/guests").await;
    assert!(all.contains(r#"<a href="/guests" aria-current="page">"#), "sol menüde Konuklar etkin");
    for g in &c.guests {
        assert!(all.contains(&format!(r#"href="/guests/{}""#, g.vmid)), "{}", g.vmid);
    }
    assert!(all.contains(r#"<tr class="grp"><td colspan="8"><a href="/nodes/pve-ist-01">pve-ist-01</a>"#), "konuklar düğüme göre gruplanır");
    assert!(all.contains(&format!("Tümü <b>{}</b>", c.guests.len())) && all.contains(&format!("Çalışan <b>{live}</b>")) && all.contains("Şablon <b>0</b>"));
    assert!(all.contains(r#"action="/guests/103/action""#) && all.contains(r#"name="action" value="start">"#) && all.contains(r#"name="action" value="stop">"#), "satırdaki düğme duruma göre başlatır ya da kapatır");

    let running = page(&app, "/guests?tab=calisan").await;
    assert!(running.contains(r#"href="/guests/102""#) && !running.contains(r#"href="/guests/103""#));
    assert!(running.contains(r#"<a href="/guests?tab=calisan" aria-current="true">"#));
    let stopped = page(&app, "/guests?tab=kapali").await;
    assert!(stopped.contains(r#"href="/guests/103""#) && !stopped.contains(r#"href="/guests/102""#));
    assert!(page(&app, "/guests?tab=sablon").await.contains("Bu süzgeçle eşleşen konuk yok."));

    // Şablon kendi süzgecine geçer, "kapalı" sayılmaz ve satırından başlatılamaz.
    assert!(post_ok(&app, "/guests/103/template", "on=1").await.contains("notice="));
    let templates = page(&app, "/guests?tab=sablon").await;
    assert!(templates.contains(r#"href="/guests/103""#) && templates.contains(r#"<span class="tag gold">şablon</span>"#) && templates.contains(r#"name="action" value="start" disabled>"#));
    assert!(!page(&app, "/guests?tab=kapali").await.contains(r#"href="/guests/103""#));

    // Durum ve metin süzgeci birlikte çalışır; arama kutusu durumu korur.
    let both = page(&app, "/guests?tab=calisan&q=web").await;
    assert!(both.contains("web-prod-01") && !both.contains(r#"href="/guests/102""#) && both.contains(r#"<input type="hidden" name="tab" value="calisan">"#));
    assert!(page(&app, "/guests?tab=uydurma").await.contains(r#"href="/guests/102""#), "bilinmeyen süzgeç yok sayılır");

    // İzleyiciye eylem ve seçim sunulmaz.
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests").body(Body::empty()).unwrap()).await;
    assert!(viewer.contains("web-prod-01") && !viewer.contains(r#"name="action""#) && !viewer.contains(r#"form="bulk""#) && !viewer.contains("/guests/bulk"));
}

#[tokio::test]
pub(super) async fn tasks_and_backups_have_their_own_pages() {
    let (app, state) = setup();
    assert!(page(&app, "/tasks").await.contains("Henüz görev yok"));
    assert!(page(&app, "/backups").await.contains("Henüz yedek yok"));
    assert!(page(&app, "/").await.contains("Henüz yedek alınmadı."));

    let id = state.backup_create(103, "admin").unwrap();
    send(&app, post_form("/guests/103/action", "action=start")).await;
    send(&app, post_form("/guests/202/action", "action=start")).await;

    let tasks = page(&app, "/tasks").await;
    assert!(tasks.contains("redox vm start 103") && tasks.contains("redox ct start 202") && tasks.contains(r#"<a href="/tasks" aria-current="page">"#));
    let mine = page(&app, "/tasks?q=103").await;
    assert!(mine.contains("redox vm start 103") && !mine.contains("redox ct start 202") && mine.contains(r#"value="103""#));
    assert!(page(&app, "/tasks?q=zzz").await.contains("ile eşleşen görev yok"));
    assert!(page(&app, "/guests/103").await.contains(r#"href="/tasks?q=103""#), "konuk özetinden görevlerine gidilir");

    let backups = page(&app, "/backups").await;
    assert!(backups.contains(&id) && backups.contains(r#"href="/guests/103?tab=yedekler""#) && backups.contains("yalnızca yerel"));
    assert!(backups.contains(r#"<a href="/backups" aria-current="page">"#) && backups.contains(r#"<span class="count">1</span>"#), "sol menü yedek sayısını gösterir");
    let home = page(&app, "/").await;
    assert!(home.contains(&id) && home.contains(r#"href="/guests/102?tab=yedekler""#), "genel bakış son yedeği ve yedeksiz konukları gösterir");
}

#[tokio::test]
pub(super) async fn settings_tabs_save_only_their_own_fields() {
    let (app, state) = setup();
    let guest = |id| state.snapshot().guest(id).unwrap().clone();
    let before = guest(103);

    let to = post_ok(&app, "/guests/103/edit", "part=secenekler&name=derleme&os=x&tags=ci&onboot=1&backup_time=03%3A30&backup_keep=3&firewall=1&fw_rules=tcp%2F22").await;
    assert!(to.starts_with("/guests/103?tab=secenekler&notice="), "{to}");
    let g = guest(103);
    assert_eq!((g.name.as_str(), g.onboot, g.backup_time.as_deref(), g.hw.firewall, g.hw.fw_rules.len()), ("derleme", true, Some("03:30"), true, 1));
    assert_eq!((g.cores, g.mem_total_mb), (before.cores, before.mem_total_mb), "seçenekler donanıma dokunmaz");

    // Donanım sekmesinde ad, açılış ve güvenlik duvarı alanları yoktur: işaretsiz kutu "kapat" sayılmamalı.
    let to = post_ok(&app, "/guests/103/edit", "part=donanim&cores=2&memory_mb=4096&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=").await;
    assert!(to.starts_with("/guests/103?tab=donanim&notice="), "{to}");
    let g = guest(103);
    assert_eq!((g.cores, g.mem_total_mb), (2, 4096));
    assert_eq!((g.name.as_str(), g.onboot, g.backup_time.as_deref(), g.backup_keep, g.hw.firewall, g.hw.fw_rules.len(), g.tags.clone()), ("derleme", true, Some("03:30"), 3, true, 1, vec!["ci".to_string()]));

    // Seçenekler formuna elle eklenen donanım alanları yok sayılır.
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=derleme2&os=x&tags=&backup_time=&backup_keep=3&fw_rules=&cores=16&memory_mb=1024").await.contains("notice="));
    let g = guest(103);
    assert_eq!((g.name.as_str(), g.onboot, g.backup_time.as_deref(), g.hw.firewall, g.cores, g.mem_total_mb), ("derleme2", false, None, false, 2, 4096));

    // Çalışan konuk: seçenekler değişir, donanım reddedilir ve hata kendi sekmesine döner.
    assert!(post_ok(&app, "/guests/100/edit", "part=secenekler&name=web-x&os=debian-12&tags=&backup_time=&backup_keep=7&fw_rules=").await.starts_with("/guests/100?tab=secenekler&notice="));
    assert_eq!(guest(100).name, "web-x");
    let cores = guest(100).cores;
    assert!(post_ok(&app, "/guests/100/edit", "part=donanim&cores=1&memory_mb=8192&forwards=").await.starts_with("/guests/100?tab=donanim&error="));
    assert_eq!(guest(100).cores, cores);
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=K%C3%B6t%C3%BC&os=x&tags=&backup_keep=3&fw_rules=").await.starts_with("/guests/103?tab=secenekler&error="));

    // Eski düzenleme adresi donanım sekmesine yönlenir.
    let (status, headers, _) = send(&app, get("/guests/103/edit")).await;
    assert_eq!((status, location(&headers)), (StatusCode::SEE_OTHER, "/guests/103?tab=donanim"));
}

#[tokio::test]
pub(super) async fn guest_header_menu_and_suspend_notice() {
    let (app, state) = setup();
    let stopped = page(&app, "/guests/103").await;
    assert!(stopped.contains(r#"<details class="menu">"#) && stopped.contains(r#"formaction="/guests/103/backups">"#) && stopped.contains(r#"href="/guests/103?tab=cogalt""#));
    assert!(!stopped.contains("/guests/103/suspend"), "kapalı konukta askıya alma sunulmaz");
    assert!(stopped.contains(r#"<a class="danger" href="/guests/103/delete">"#));

    let running = page(&app, "/guests/102").await;
    assert!(running.contains(r#"formaction="/guests/102/suspend">"#), "çalışan sanal makine askıya alınabilir");
    assert!(running.contains(r#"formaction="/guests/102/backups" disabled>"#), "simülasyonda çalışan konuk yedeklenmez");
    assert!(running.contains(r#"title="Silmek için önce kapatın""#));
    assert!(!page(&app, "/guests/200").await.contains("/suspend"), "konteyner askıya alınmaz");

    state.mutate(|c| c.guests.iter_mut().find(|g| g.vmid == 103).unwrap().hw.suspended = true);
    let suspended = page(&app, "/guests/103?tab=yedekler").await;
    assert!(suspended.contains("Bu konuk askıda") && suspended.contains(r#"action="/guests/103/discard-suspend""#) && suspended.contains(r#"<span class="st stop">Askıda</span>"#));
    assert!(page(&app, "/guests?tab=kapali").await.contains(r#"<span class="st stop">Askıda</span>"#));
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/103").body(Body::empty()).unwrap()).await;
    assert!(viewer.contains("Bu konuk askıda") && !viewer.contains("discard-suspend") && !viewer.contains(r#"<details class="menu">"#));
}

#[tokio::test]
pub(super) async fn real_container_clone_tab_offers_only_cloning() {
    let (app, state) = setup_real();
    // Gerçek konteynerde çoğaltma sekmesi yalnızca klonu sunar: şablon, bağlı klon ve OVA sanal makinelere özgüdür.
    let ct = page(&app, "/guests/202").await;
    assert!(ct.contains(r#"href="/guests/202?tab=cogalt""#) && ct.contains(">Çoğalt<") && !ct.contains("Çoğalt ve Taşı"));
    let tab = page(&app, "/guests/202?tab=cogalt").await;
    assert!(tab.contains("Bu konteynerin kopyası") && tab.contains(r#"action="/guests/202/clone""#), "{tab}");
    assert!(!tab.contains("Şablona Çevir") && !tab.contains("OVA") && !tab.contains(r#"name="linked""#));
    assert!(post_ok(&app, "/guests/202/clone", "name=ci-kopya").await.contains("notice="));
    let c = state.snapshot();
    let copy = c.guests.iter().find(|g| g.name == "ci-kopya").expect("klon kaydı");
    assert_eq!((copy.kind, copy.status), (GuestKind::Ct, GuestStatus::Stopped));
    // Çalışan konteynerde klon kapalıdır.
    let running = page(&app, "/guests/200?tab=cogalt").await;
    assert!(running.contains("önce konteyneri kapatın") && running.contains(r#"<button class="btn primary" type="submit" disabled>"#));
    let vm = page(&app, "/guests/103?tab=cogalt").await;
    assert!(vm.contains("Şablona Çevir") && vm.contains(r#"href="/guests/103/export""#) && vm.contains(r#"action="/guests/103/clone""#));
    // Çalışan konukta bağlantı ve düğmeler kapalıdır.
    let live = page(&app, "/guests/102?tab=cogalt").await;
    assert!(live.contains("önce konuğu kapatın") && !live.contains(r#"href="/guests/102/export""#) && live.contains(r#"<button class="btn primary" type="submit" disabled>"#));
}

#[tokio::test]
pub(super) async fn brand_lockup_is_proportional_and_login_reveals_nothing() {
    let (app, _) = setup();
    let home = page(&app, "/").await;
    // Yazı logo simgeden biraz yüksek: harfleri simgeyle aynı boyda görünür.
    assert!(home.contains(r#"<img class="mark" src="/assets/logo.svg?v="#) && home.contains(r#"alt="" width="33" height="34">"#));
    assert!(home.contains(r#"<img class="word" src="/assets/wordmark.svg?v="#) && home.contains(r#"alt="" width="113" height="40">"#));

    let (status, _, login) = send(&app, anon(Method::GET, "/login").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(login.contains(r#"height="96">"#) && login.contains(r#"height="110">"#));
    for secret in ["istanbul-01", "pve-ist-01", "db-postgres", "konuk çalışıyor"] {
        assert!(!login.contains(secret), "giriş sayfası oturumsuz ziyaretçiye sunucu bilgisi vermez: {secret}");
    }
    let css = page(&app, "/assets/style.css").await;
    assert!(css.contains(".brand .mark{height:34px") && css.contains(".brand .word{height:40px"));
}

// ---------- Açılışta başlatma ve zamanlanmış güç işlemleri ----------
