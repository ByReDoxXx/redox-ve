//! Kendiliğinden yeniden başlatma ve paylaşılan klasörler.

use super::*;

#[tokio::test]
pub(super) async fn guests_marked_for_restart_come_back_after_an_unexpected_stop() {
    let (app, state) = setup_with(Duration::ZERO, SimDriver::failing_on(102));
    let tab = page(&app, "/guests/100?tab=secenekler").await;
    assert!(tab.contains(r#"<input type="checkbox" name="restart" value="1">"#) && tab.contains("Kendiliğinden kapanırsa yeniden başlat"));
    for vmid in [100u32, 102] {
        let name = state.snapshot().guest(vmid).unwrap().name.clone();
        assert!(post_ok(&app, &format!("/guests/{vmid}/edit"), &format!("part=secenekler&name={name}&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&boot_order=0&boot_delay=0&restart=1")).await.contains("notice="));
    }
    assert!(state.snapshot().guest(100).unwrap().hw.restart);
    assert!(page(&app, "/guests/100").await.contains("kendiliğinden yeniden başlatılır"));
    assert!(page(&app, "/guests/100?tab=secenekler").await.contains(r#"name="restart" value="1" checked>"#));
    // Donanım formu işareti değiştirmez.
    assert!(post_ok(&app, "/guests/103/edit", "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=&restart=1").await.contains("notice="));
    assert!(!state.snapshot().guest(103).unwrap().hw.restart);

    assert_eq!(state.ha_tick(), 0, "ilk tur yalnızca durumu öğrenir");
    // Çökme: konuklar görev olmadan durur. 100 açılır; 102'nin sürücüsü hata verir ve günlüğe yazılır.
    let crash = |ids: &[u32]| state.mutate(|c| c.guests.iter_mut().filter(|g| ids.contains(&g.vmid)).for_each(|g| g.status = GuestStatus::Stopped));
    crash(&[100, 101, 102]);
    assert_eq!(state.ha_tick(), 2, "başlatma görevi iki konuk için açıldı");
    let c = state.snapshot();
    assert_eq!([100, 101, 102].map(|id| c.guest(id).unwrap().status), [GuestStatus::Running, GuestStatus::Stopped, GuestStatus::Stopped], "işaretsiz 101 ve açılamayan 102 kapalı kalır");
    let by_watch: Vec<(&str, TaskStatus)> = c.tasks.iter().rev().filter(|t| t.user == redox_server::state::HA).map(|t| (t.command.as_str(), t.status)).collect();
    assert_eq!(by_watch, [("vm start 100", TaskStatus::Ok), ("vm start 102", TaskStatus::Failed)]);
    assert_eq!(state.ha_tick(), 0, "aynı kapanış yeniden denenmez");

    // Panelden kapatılan konuk yeniden başlatılmaz.
    state.run_action(100, Action::Stop, "admin").unwrap();
    assert_eq!(state.ha_tick(), 0);
    assert_eq!(state.snapshot().guest(100).unwrap().status, GuestStatus::Stopped);

    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/100").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(json!({"restart": false}))).await;
    assert_eq!((status, body["restart"].as_bool()), (StatusCode::OK, Some(false)), "{body}");
}

// ---------- Paylaşılan klasörler ----------

#[tokio::test]
pub(super) async fn shared_folders_are_edited_from_the_hardware_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"form="hw" name="shares" type="text" value="""#) && tab.contains("mount -t 9p -o trans=virtio"));
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="shares""#), "konteynerde paylaşım alanı yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=&vlan=0";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&shares=ortak%2C+veri%3Aro")).await.starts_with("/guests/103?tab=donanim&notice="));
    let shares = state.snapshot().guest(103).unwrap().hw.shares.clone();
    assert_eq!(shares.iter().map(ToString::to_string).collect::<Vec<_>>(), ["ortak", "veri:ro"]);
    assert!(page(&app, "/guests/103?tab=donanim").await.contains(r#"name="shares" type="text" value="ortak, veri:ro""#));
    let home = page(&app, "/guests/103").await;
    assert!(home.contains("Paylaşımlar") && home.contains("veri · salt okunur"));

    for bad in ["shares=..%2Fetc", "shares=a%2Fb", "shares=Ortak", "shares=ortak%2C+ortak", "shares=a%2Cb%2Cc%2Cd%2Ce", "shares=veri%3Arx"] {
        let to = post_ok(&app, "/guests/103/edit", &format!("{base}&{bad}")).await;
        assert!(to.starts_with("/guests/103?tab=donanim&error="), "{bad}: {to}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.shares.len(), 2, "hatalı liste kaydı değiştirmez");
    // Alanı boşaltmak paylaşımları kaldırır; seçenekler formu paylaşımlara dokunmaz.
    assert!(post_ok(&app, "/guests/103/edit", "part=secenekler&name=win-build&os=x&tags=&backup_time=&backup_keep=7&fw_rules=&shares=").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.shares.len(), 2);
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&shares=")).await.contains("notice="));
    assert!(state.snapshot().guest(103).unwrap().hw.shares.is_empty());

    let patch = |vmid: u32, body: Value| request(Method::PATCH, &format!("/api/v1/guests/{vmid}")).header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (status, body) = api(&app, patch(103, json!({"shares": [{"name": "ortak"}, {"name": "veri", "readonly": true}]}))).await;
    assert_eq!((status, body["shares"][1]["readonly"].as_bool(), body["shares"][0]["readonly"].as_bool()), (StatusCode::OK, Some(true), Some(false)), "{body}");
    assert_eq!(api(&app, patch(103, json!({"shares": [{"name": "../etc"}]}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(api(&app, patch(100, json!({"shares": [{"name": "ortak"}]}))).await.0, StatusCode::CONFLICT, "çalışan konukta değişmez");
    assert_eq!(api(&app, patch(202, json!({"shares": [{"name": "ortak"}]}))).await.0, StatusCode::UNPROCESSABLE_ENTITY, "konteynerde yok");

    // Askıdaki konuğun donanımı değişmez.
    state.mutate(|c| c.after_suspend(103));
    let (status, body) = api(&app, patch(103, json!({"shares": []}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("askıda"));
}

// ---------- Yedek işleri ----------
