//! Köprü ağı ve konteynerler (gerçek sürücü kipi).

use super::*;

#[tokio::test]
pub(super) async fn real_driver_forms_offer_bridges_and_templates() {
    let (app, _) = setup_real();
    let vm = page(&app, "/guests/new?kind=vm").await;
    assert!(vm.contains(r#"<select id="bridge" name="bridge""#) && vm.contains(r#"<option value="nat" selected>NAT</option>"#), "varsayılan NAT");
    assert!(vm.contains(r#"<option value="vmbr0">Köprü: vmbr0</option>"#) && vm.contains(r#"<option value="br-lan">"#));
    assert!(vm.contains(r#"<input id="os" name="os" type="text""#), "sanal makinede işletim sistemi serbest metin");

    let ct = page(&app, "/guests/new?kind=ct").await;
    assert!(ct.contains(r#"<select id="os" name="os""#) && ct.contains(r#"<option value="alpine-3.20" selected>"#) && ct.contains("debian-12"));
    assert!(page(&app, "/guests").await.contains(r#"href="/guests/new?kind=ct""#));

    let edit = settings(&app, "/guests/103").await;
    assert!(edit.contains(r#"<select id="bridge" form="hw" name="bridge""#) && edit.contains("Klonla"));
    let ct_edit = settings(&app, "/guests/202").await;
    assert!(ct_edit.contains("Klonla") && !ct_edit.contains("Büyüt") && !ct_edit.contains("Şablona Çevir"), "konteynerde klon var; büyütme ve şablon yok");
}

#[tokio::test]
pub(super) async fn real_driver_container_create_and_network_edit() {
    let (app, state) = setup_real();
    let form = "kind=ct&node=pve-ist-01&vmid=&name=kutu&os=alpine-3.20&storage=local-zfs&disk_gb=8&cores=1&memory_mb=256&bridge=br-lan";
    let to = post_ok(&app, "/guests", form).await;
    assert!(to.starts_with("/guests/105?notice="), "{to}");
    let g = state.snapshot().guest(105).unwrap().clone();
    assert_eq!((g.kind, g.bridge.as_str(), g.os.as_str()), (GuestKind::Ct, "br-lan", "alpine-3.20"));

    let conf = page(&app, "/guests/105").await;
    assert!(conf.contains("köprü: br-lan") && conf.contains("52:54:00:00:00:69"), "ağ ve MAC gösterilir");
    // Konteynerde anlık görüntü ve yedek kapalıyken alınır.
    assert!(page(&app, "/guests/105?tab=goruntuler").await.contains("Henüz anlık görüntü yok"));
    assert!(post_ok(&app, "/guests/105/snapshots", "name=temiz").await.contains("notice="));
    assert!(post_ok(&app, "/guests/105/backups", "").await.contains("notice="));
    assert!(post_ok(&app, "/guests/200/snapshots", "name=canli").await.contains("error="), "çalışan konteynerde canlı görüntü yok");

    // Ağ değişikliği: var olmayan köprü ve köprüde port yönlendirme reddedilir.
    let base = "name=kutu&os=alpine-3.20&tags=&cores=1&memory_mb=256";
    assert!(post_ok(&app, "/guests/105/edit", &format!("{base}&bridge=br-yok&forwards=")).await.contains("&error="));
    assert!(post_ok(&app, "/guests/105/edit", &format!("{base}&bridge=nat&forwards=8080%3A80")).await.contains("&error="), "konteynerde port yönlendirme yok");
    assert!(post_ok(&app, "/guests/105/edit", &format!("{base}&bridge=nat&forwards=")).await.contains("notice="));
    assert_eq!(state.snapshot().guest(105).unwrap().bridge, "nat");
    assert!(post_ok(&app, "/guests/103/edit", "name=win-build&os=x&tags=&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=8080%3A80").await.contains("&error="));
}

#[tokio::test]
pub(super) async fn container_console_runs_commands() {
    let (app, state) = setup_real();
    let tab = page(&app, "/guests/200?tab=konsol").await;
    assert!(tab.contains(r#"action="/guests/200/exec""#) && tab.contains("konteynerde kök olarak") && !tab.contains("iframe"));
    assert!(page(&app, "/guests/202?tab=konsol").await.contains("Konteyner çalışmıyor"));

    let (status, headers, body) = send(&app, post_form("/guests/200/exec", "command=id+-u")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store");
    assert!(body.contains("dns-01 üzerinde: id -u") && body.contains("Çalıştı"));
    assert!(body.contains("&lt;b&gt;uid=0(root)&lt;/b&gt;") && !body.contains("<b>uid=0"), "komut çıktısı kaçışlı gösterilir");
    assert!(body.contains(r#"value="id -u""#), "komut yeniden düzenlenebilsin diye kutuda kalır");

    let (status, _, body) = send(&app, post_form("/guests/200/exec", "command=false")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Başarısız") && body.contains("çıkış kodu 1"));

    assert!(post_ok(&app, "/guests/200/exec", "command=++").await.contains("error="), "boş komut");
    assert!(post_ok(&app, "/guests/202/exec", "command=id").await.contains("error="), "duran konteyner");
    assert!(post_ok(&app, "/guests/999/exec", "command=id").await.starts_with("/?error="));

    // Yalnızca yöneticiler; komut metni görev günlüğünde ve denetim kaydında yer almaz.
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/200/exec"), "command=id")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    send(&app, post_form("/guests/200/exec", "command=echo+cok-gizli-deger")).await;
    let c = state.snapshot();
    assert!(c.tasks.iter().any(|t| t.command == "ct exec 200") && !format!("{:?}", c.tasks).contains("cok-gizli-deger"));
    assert!(!format!("{:?}", state.auth().audit.recent("", 50)).contains("cok-gizli-deger"));
}

// ---------- Zamanlanmış yedek, canlı işlemler, uyarılar ve yeni API uçları ----------

#[tokio::test]
pub(super) async fn security_groups_and_address_sets_are_managed_and_used_by_guests() {
    let (app, state) = setup();
    assert!(post_ok(&app, "/cluster/firewall", "op=save-set&name=ofis&entries=10.0.0.0%2F24%2C+192.168.1.5").await.contains("notice="));
    assert!(post_ok(&app, "/cluster/firewall", "op=save-group&name=web&rules=tcp%2F443%2C+tcp%2F22+from+%40ofis&out=udp%2F53").await.contains("notice="));
    assert!(post_ok(&app, "/cluster/firewall", "op=save-group&name=db&rules=tcp%2F5432+from+%40yok&out=").await.contains("error="));
    let cluster = page(&app, "/cluster").await;
    assert!(cluster.contains("@ofis") && cluster.contains("tcp/22 from @ofis") && cluster.contains("udp/53"), "{cluster}");

    // Konuğa verilir; Seçenekler sekmesi alanları gösterir ve kaydeder.
    let form = "part=secenekler&name=db-ana&os=debian-12&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&firewall=1&fw_rules=&fw_out_restrict=1&fw_out=tcp%2F443+to+%40ofis&fw_groups=web";
    assert!(post_ok(&app, "/guests/101/edit", form).await.contains("notice="));
    let hw = state.snapshot().guest(101).unwrap().hw.clone();
    assert_eq!((hw.fw_out_restrict, hw.fw_groups.clone(), hw.fw_out[0].outbound()), (true, vec!["web".to_string()], "tcp/443 to @ofis".to_string()));
    let tab = page(&app, "/guests/101?tab=secenekler").await;
    assert!(tab.contains(r#"name="fw_out_restrict" value="1" checked"#) && tab.contains("tcp/443 to @ofis") && tab.contains(r#"value="web""#), "{tab}");
    assert!(post_ok(&app, "/guests/101/edit", &form.replace("fw_groups=web", "fw_groups=yok")).await.contains("error="));

    // Kullanılan tanım silinmez; API aynı kuralları uygular.
    assert!(post_ok(&app, "/cluster/firewall", "op=remove-group&name=web").await.contains("error="));
    let delete = |uri: &str| request(Method::DELETE, uri).body(Body::empty()).unwrap();
    assert_eq!(api(&app, delete("/api/v1/firewall/sets/ofis")).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, sets) = api(&app, get("/api/v1/firewall/sets")).await;
    assert_eq!((status, sets[0]["name"].as_str()), (StatusCode::OK, Some("ofis")));
    let denied = send(&app, as_viewer(Method::POST, "/api/v1/firewall/sets").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "name": "x", "entries": ["10.0.0.1"] }).to_string())).unwrap()).await.0;
    assert_eq!(denied, StatusCode::FORBIDDEN);
    assert!(post_ok(&app, "/guests/101/edit", &form.replace("fw_groups=web", "fw_groups=").replace("%40ofis", "10.0.0.1")).await.contains("notice="));
    assert!(post_ok(&app, "/cluster/firewall", "op=remove-group&name=web").await.contains("notice="));
    assert!(api(&app, delete("/api/v1/firewall/sets/ofis")).await.0.is_success());
}
