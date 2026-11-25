//! Açılışta başlatma ve zamanlanmış güç işlemleri.

use super::*;

#[tokio::test]
pub(super) async fn autostart_follows_order_and_delays() {
    let (app, state) = setup_with(Duration::ZERO, SimDriver::failing_on(202));
    let tab = page(&app, "/guests/103?tab=secenekler").await;
    assert!(tab.contains(r#"name="boot_order" type="text" inputmode="numeric" value="0""#) && tab.contains(r#"name="boot_delay" type="text" inputmode="numeric" value="0""#));

    state.mutate(|c| c.guests.iter_mut().for_each(|g| g.onboot = false));
    assert_eq!(state.autostart(&|_| panic!("başlatılacak konuk yokken beklenmez")), 0);
    state.run_action(100, Action::Stop, "admin").unwrap();
    let set = |vmid: u32, extra: &'static str| {
        let name = state.snapshot().guest(vmid).unwrap().name.clone();
        let app = app.clone();
        async move { post_ok(&app, &format!("/guests/{vmid}/edit"), &format!("part=secenekler&name={name}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&{extra}")).await }
    };
    assert!(set(103, "onboot=1&boot_order=2&boot_delay=30").await.contains("notice="));
    assert!(set(202, "onboot=1&boot_order=1&boot_delay=5").await.contains("notice="));
    assert!(set(100, "onboot=1&boot_order=0&boot_delay=10").await.contains("notice="));
    assert!(set(103, "onboot=1&boot_order=1000&boot_delay=30").await.contains("error="));
    assert!(set(103, "onboot=1&boot_order=2&boot_delay=601").await.contains("error="));
    assert!(set(103, "onboot=1&boot_order=iki&boot_delay=30").await.contains("error="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.boot_order, hw.boot_delay), (2, 30), "hatalı form kaydı değiştirmez");
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("sıra 2") && home.contains("sonra 30 sn bekle"));

    // Sıra: 202 (1, açılamaz) → 103 (2) → 100 (sırasız, en son). Yalnızca açılan 103'ün ardından beklenir.
    let pauses = std::sync::Mutex::new(Vec::new());
    assert_eq!(state.autostart(&|d| pauses.lock().unwrap().push(d)), 2);
    assert_eq!(*pauses.lock().unwrap(), [Duration::from_secs(30)], "açılamayan konuktan sonra ve son konuktan sonra beklenmez");
    let c = state.snapshot();
    assert_eq!([103, 100, 202].map(|id| c.guest(id).unwrap().status), [GuestStatus::Running, GuestStatus::Running, GuestStatus::Stopped]);
    let boot: Vec<(&str, TaskStatus)> = c.tasks.iter().rev().filter(|t| t.user == redox_server::state::BOOT).map(|t| (t.command.as_str(), t.status)).collect();
    assert_eq!(boot, [("ct start 202", TaskStatus::Failed), ("vm start 103", TaskStatus::Ok), ("vm start 100", TaskStatus::Ok)]);
    // Çalışanlar yeniden başlatılmaz; açılamayan yeniden denenir.
    assert_eq!(state.autostart(&|_| ()), 0);
    assert_eq!(state.snapshot().tasks.iter().filter(|t| t.user == redox_server::state::BOOT).count(), 4);
}

#[tokio::test]
pub(super) async fn power_schedules_are_managed_from_the_options_tab() {
    let (app, state) = setup();
    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert!(page(&app, "/guests/103?tab=secenekler").await.contains("Zamanlanmış işlem yok."));

    let to = post_ok(&app, "/guests/103/schedules", "action=start&time=08%3A00&day=1&day=2&day=3&day=4&day=5").await;
    assert!(to.starts_with("/guests/103?tab=secenekler&notice="), "{to}");
    assert!(post_ok(&app, "/guests/103/schedules", "action=stop&time=20%3A00&day=1&day=2&day=3&day=4&day=5&day=6&day=7").await.contains("notice="));
    for bad in ["action=start&time=08%3A00", "action=pause&time=08%3A00&day=1", "action=start&time=25%3A00&day=1", "action=start&time=&day=1", "time=08%3A00&day=1", "action=start&time=08%3A00&day=9"] {
        assert!(post_ok(&app, "/guests/103/schedules", bad).await.contains("error="), "{bad}");
    }
    assert!(post_ok(&app, "/guests/999/schedules", "action=start&time=08%3A00&day=1").await.contains("error="));
    let list = state.snapshot().guest(103).unwrap().hw.schedules.clone();
    assert_eq!(list.iter().map(ToString::to_string).collect::<Vec<_>>(), ["start@08:00@12345", "stop@20:00"]);

    let tab = page(&app, "/guests/103?tab=secenekler").await;
    assert!(tab.contains("hafta içi") && tab.contains("her gün") && tab.contains(r#"action="/guests/103/schedules/0/delete""#) && tab.contains(r#"<input type="checkbox" name="day" value="7" checked>"#));
    assert!(page(&app, "/guests/103").await.contains("Başlat 08:00 · hafta içi"), "özet zamanlamayı gösterir");
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/103?tab=secenekler").body(Body::empty()).unwrap()).await;
    assert!(viewer.contains("hafta içi") && !viewer.contains("/schedules"), "izleyici listeyi görür, değiştiremez");
    let (status, _, _) = send(&app, as_viewer(Method::POST, "/guests/103/schedules").header(CONTENT_TYPE, "application/x-www-form-urlencoded").body(Body::from("action=stop&time=01%3A00&day=1")).unwrap()).await;
    assert_ne!(status, StatusCode::SEE_OTHER, "izleyici zamanlama ekleyemez");
    assert_eq!(state.snapshot().guest(103).unwrap().hw.schedules.len(), 2);

    // 2026-11-24 Salı: 08:00'de başlar, 20:00'de kapanır; her biri günde bir kez.
    assert_eq!(state.power_tick("2026-11-24", 1, 479), 0);
    assert_eq!(state.power_tick("2026-11-24", 1, 480), 1);
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Running);
    assert_eq!(state.power_tick("2026-11-24", 1, 481), 0);
    assert_eq!(state.power_tick("2026-11-24", 1, 1200), 1);
    let c = state.snapshot();
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].user.as_str(), c.tasks[0].status), ("vm stop 103", "zamanlayıcı", TaskStatus::Ok));
    assert!(page(&app, "/guests/103?tab=secenekler").await.contains("2026-11-24"), "son çalışma günü görünür");

    // Konuk zaten istenen durumdaysa işlem atlanır: hatalı görev yazılmaz ve o gün yeniden denenmez.
    state.run_action(103, Action::Start, "admin").unwrap();
    let before = state.snapshot().tasks.len();
    assert_eq!(state.power_tick("2026-11-25", 2, 480), 0);
    assert_eq!(state.snapshot().tasks.len(), before);
    state.run_action(103, Action::Stop, "admin").unwrap();
    assert_eq!(state.power_tick("2026-11-25", 2, 485), 0, "o günün işlemi atlandı sayılır");
    // Hafta sonu "başlat" çalışmaz; saati kaçan işlem telafi edilmez.
    assert_eq!(state.power_tick("2026-11-28", 5, 480), 0);
    assert_eq!(state.power_tick("2026-11-26", 3, 600), 0);
    assert_eq!(state.snapshot().guest(103).unwrap().status, GuestStatus::Stopped);

    assert!(post_ok(&app, "/guests/103/schedules/7/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/schedules/x/delete", "").await.contains("error="));
    assert!(post_ok(&app, "/guests/103/schedules/0/delete", "").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.schedules.iter().map(ToString::to_string).collect::<Vec<_>>(), ["stop@20:00"]);

    // API: liste bütün olarak yazılır; günler verilmezse her gün.
    let (status, body) = api(&app, patch(json!({"schedules": [{"action": "reboot", "time": "03:00"}], "boot_order": 3}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["schedules"][0]["days"].as_u64(), body["schedules"][0]["action"].as_str(), body["boot_order"].as_u64()), (Some(127), Some("reboot"), Some(3)));
    for bad in [json!({"schedules": [{"action": "pause", "time": "03:00"}]}), json!({"schedules": [{"action": "stop", "time": "3"}]}), json!({"schedules": vec![json!({"action": "stop", "time": "03:00"}); 9]}), json!({"boot_delay": 601})] {
        let (status, body) = api(&app, patch(bad.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.schedules.len(), 1);
}

#[tokio::test]
pub(super) async fn rate_limits_are_edited_from_the_hardware_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    for field in ["disk_mbps", "disk_iops", "net_mbps"] {
        assert!(tab.contains(&format!(r#"form="hw" name="{field}" type="text" inputmode="numeric" value="0">"#)), "{field}");
    }
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains("Hız sınırları"), "konteynerde hız sınırı yok");
    assert!(page(&app, "/guests/100?tab=donanim").await.contains(r#"name="net_mbps" type="text" inputmode="numeric" value="0" disabled>"#), "çalışan konukta alan kapalı");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    let to = post_ok(&app, "/guests/103/edit", &format!("{base}&disk_mbps=50&disk_iops=2000&net_mbps=100")).await;
    assert!(to.starts_with("/guests/103?tab=donanim&notice="), "{to}");
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.disk_mbps, hw.disk_iops, hw.net_mbps), (50, 2000, 100));
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("disk 50 MB/sn") && home.contains("disk 2000 işlem/sn") && home.contains("ağ 100 Mbit/sn"));
    assert!(page(&app, "/guests/103?tab=donanim").await.contains(r#"name="disk_mbps" type="text" inputmode="numeric" value="50">"#));

    for bad in ["disk_mbps=10001&disk_iops=0&net_mbps=0", "disk_mbps=hızlı&disk_iops=0&net_mbps=0", "disk_mbps=0&disk_iops=1000001&net_mbps=0"] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await.starts_with("/guests/103?tab=donanim&error="), "{bad}");
    }
    // Bütün kartlar NAT'tayken ağ sınırı reddedilir.
    let nat = base.replace("bridge=vmbr0", "bridge=nat");
    let to = post_ok(&app, "/guests/103/edit", &format!("{nat}&disk_mbps=50&disk_iops=2000&net_mbps=100")).await;
    assert!(to.contains("error=") && to.contains("k%C3%B6pr%C3%BCye"), "{to}");
    assert_eq!(state.snapshot().guest(103).unwrap().bridge, "vmbr0");
    // Seçenekler formu sınırlara dokunmaz.
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=win-build&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&disk_mbps=1").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.disk_mbps, 50);

    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(json!({"disk_mbps": 0, "net_mbps": 20}))).await;
    assert_eq!((status, body["disk_mbps"].as_u64(), body["disk_iops"].as_u64(), body["net_mbps"].as_u64()), (StatusCode::OK, Some(0), Some(2000), Some(20)), "{body}");
    assert_eq!(api(&app, patch(json!({"net_mbps": 100001}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = api(&app, request(Method::PATCH, "/api/v1/guests/100").header(CONTENT_TYPE, "application/json").body(Body::from(json!({"disk_mbps": 5}).to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::CONFLICT, "çalışan konukta değişmez: {body}");
}

#[tokio::test]
pub(super) async fn disks_and_nics_are_added_to_running_guests_when_the_driver_can() {
    // Simülasyon çalışırken takamaz: çalışan konukta form kapalı, istek reddedilir.
    let (sim, sim_state) = setup();
    let tab = page(&sim, "/guests/102?tab=donanim").await;
    assert!(tab.contains(r#"name="disk_gb" type="text" inputmode="numeric" size="6" value="10" disabled>"#) && tab.contains("Kart, konuk kapalıyken eklenir."));
    assert!(!tab.contains("çalışırken de eklenebilir"));
    assert!(post_ok(&sim, "/guests/102/disks", "disk_gb=5").await.contains("error="));
    assert!(post_ok(&sim, "/guests/102/nics", "network=vmbr1").await.contains("error="));
    assert!(sim_state.snapshot().guest(102).unwrap().hw.nics.is_empty());
    // Kapalı konuğa kart her sürücüde eklenir.
    let to = post_ok(&sim, "/guests/103/nics", "network=+vmbrT+").await;
    assert!(to.starts_with("/guests/103?tab=donanim&notice="), "{to}");
    assert!(post_ok(&sim, "/guests/103/nics", "network=NAT").await.contains("notice="));
    assert_eq!(sim_state.snapshot().guest(103).unwrap().hw.nics, ["vmbrT", "nat"], "köprü adının harfleri korunur; NAT her yazımıyla kabul edilir");
    // Düzenleme formu da köprü adını olduğu gibi kaydeder.
    assert!(post_ok(&sim, "/guests/103/edit", "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=Br-LAN%2C+NAT").await.contains("notice="));
    assert_eq!(sim_state.snapshot().guest(103).unwrap().hw.nics, ["Br-LAN", "nat"]);
    assert!(post_ok(&sim, "/guests/103/nics", "network=").await.contains("error="));
    assert!(post_ok(&sim, "/guests/202/nics", "network=nat").await.contains("error="), "konteynere ek kart eklenmez");

    let (app, state) = setup_real();
    let tab = page(&app, "/guests/102?tab=donanim").await;
    assert!(tab.contains("Disk ve ağ kartı çalışırken de eklenebilir.") && tab.contains(r#"name="disk_gb" type="text" inputmode="numeric" size="6" value="10">"#));
    assert!(tab.contains(r#"<select id="new_nic" name="network">"#) && tab.contains(r#"<option value="br-lan">Köprü: br-lan</option>"#));
    assert!(tab.contains(r#"name="cores" type="text" inputmode="numeric" value="8" disabled"#), "öbür donanım alanları kapalı kalır");
    let (_, _, viewer) = send(&app, as_viewer(Method::GET, "/guests/102?tab=donanim").body(Body::empty()).unwrap()).await;
    assert!(!viewer.contains("/nics") && !viewer.contains("/disks"));
    let used = |s: &AppState| s.snapshot().storages.iter().map(|x| x.used_gb).sum::<u64>();
    let before = used(&state);

    assert!(post_ok(&app, "/guests/102/disks", "disk_gb=5").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/disks", "disk_gb=7").await.contains("notice="));
    let to = post_ok(&app, "/guests/102/disks", "disk_gb=9").await;
    assert!(to.contains("error=") && to.contains("yan%C4%B1t%20vermedi"), "{to}");
    let g = state.snapshot().guest(102).unwrap().clone();
    assert_eq!((g.status, g.hw.disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>()), (GuestStatus::Running, vec![(1, 5), (2, 7)]));
    assert_eq!(used(&state), before + 12, "takılamayan diskin yeri geri verilir");

    assert!(post_ok(&app, "/guests/102/nics", "network=br-lan").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/nics", "network=nat").await.contains("notice="));
    assert!(post_ok(&app, "/guests/102/nics", "network=br-lan").await.contains("error="), "üçüncü kart takılamadı");
    assert!(post_ok(&app, "/guests/102/nics", "network=br-yok").await.contains("error="), "olmayan köprü");
    assert_eq!(state.snapshot().guest(102).unwrap().hw.nics, ["br-lan", "nat"]);
    let c = state.snapshot();
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].status), ("vm nic add 102 br-lan", TaskStatus::Failed));
    assert!(c.tasks.iter().any(|t| t.command == "vm disk add 102 9G" && t.status == TaskStatus::Failed) && c.tasks.iter().any(|t| t.command == "vm nic add 102 nat" && t.status == TaskStatus::Ok));

    // Duraklatılmış konuğa takılmaz; çalışan konuktan disk çıkarılamaz.
    assert!(post_ok(&app, "/guests/104/disks", "disk_gb=5").await.contains("error="));
    assert!(post_ok(&app, "/guests/102/disks/1/delete", "").await.contains("error="));

    // API
    let post_json = |uri: &str, body: Value| request(Method::POST, uri).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, post_json("/api/v1/guests/100/nics", json!({"network": "NAT"}))).await;
    assert_eq!((status, body["index"].as_u64(), body["network"].as_str()), (StatusCode::CREATED, Some(1), Some("nat")), "{body}");
    assert_eq!(api(&app, post_json("/api/v1/guests/100/nics", json!({"network": "br-yok"}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(api(&app, post_json("/api/v1/guests/999/nics", json!({"network": "nat"}))).await.0, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, as_viewer(Method::POST, "/api/v1/guests/100/nics").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"network":"nat"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = api(&app, post_json("/api/v1/guests/100/disks", json!({"size_gb": 4}))).await;
    assert_eq!((status, body["id"].as_u64()), (StatusCode::CREATED, Some(1)), "çalışan konuğa API'den disk: {body}");
}

#[tokio::test]
pub(super) async fn vlan_tags_are_set_per_card() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"form="hw" name="vlan" type="text" inputmode="numeric" value="0">"#) && tab.contains(r#"<input name="vlan" type="text" inputmode="numeric" size="5""#));
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="vlan""#), "konteynerde VLAN alanı yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&vlan=20&nics=vmbr1%3A30%2C+nat")).await.starts_with("/guests/103?tab=donanim&notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.vlan, hw.nics.clone()), (20, vec!["vmbr1:30".to_string(), "nat".to_string()]));
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("köprü: vmbr0 · VLAN 20") && home.contains("+ vmbr1 · VLAN 30"), "özet etiketleri gösterir");
    assert!(page(&app, "/guests/103?tab=donanim").await.contains(r#"name="nics" type="text" value="vmbr1:30, nat""#));

    for bad in ["vlan=4095&nics=", "vlan=yirmi&nics=", "vlan=0&nics=nat%3A5", "vlan=0&nics=vmbr1%3A0"] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await.contains("error="), "{bad}");
    }
    // NAT'a alınan birinci kartta etiket kalamaz.
    let to = post_ok(&app, "/guests/103/edit", &format!("{}&vlan=20&nics=", base.replace("bridge=vmbr0", "bridge=nat"))).await;
    assert!(to.contains("error=") && to.contains("VLAN"), "{to}");
    assert_eq!(state.snapshot().guest(103).unwrap().hw.vlan, 20);

    // Kart ekleme formu etiketi ağ tanımına işler.
    assert!(post_ok(&app, "/guests/103/nics", "network=vmbr2&vlan=40").await.contains("notice="));
    assert!(post_ok(&app, "/guests/103/nics", "network=nat&vlan=40").await.contains("error="), "NAT kartına etiket verilmez");
    assert_eq!(state.snapshot().guest(103).unwrap().hw.nics, ["vmbr1:30", "nat", "vmbr2:40"]);

    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(json!({"vlan": 0, "nics": ["vmbr1:4094"]}))).await;
    assert_eq!((status, body["vlan"].as_u64(), body["nics"][0].as_str()), (StatusCode::OK, Some(0), Some("vmbr1:4094")), "{body}");
    assert_eq!(api(&app, patch(json!({"vlan": 4095}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    // Gerçek sürücü: etiketli ek kartın köprüsü de sunucuda bulunmalı.
    let (real, _) = setup_real();
    let real_base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&vlan=0";
    assert!(post_ok(&real, "/guests/103/edit", &format!("{real_base}&nics=br-yok%3A5")).await.contains("error="));
    assert!(post_ok(&real, "/guests/103/edit", &format!("{real_base}&nics=br-lan%3A5")).await.contains("notice="));
    assert!(post_ok(&real, "/guests/103/nics", "network=br-yok&vlan=7").await.contains("error="));
}

// ---------- Canlı konsol: uygulamada betik yükleyen tek sayfa ----------
