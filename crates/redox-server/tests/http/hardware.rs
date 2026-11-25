//! cloud-init düzenleme, işlemci türü ve bellek balonu.

use super::*;

#[tokio::test]
pub(super) async fn cloud_init_is_edited_from_the_hardware_tab_and_the_api() {
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGtMF0qv2yBAZ8lfmTtk1mKkZHjPl7Yj8d0Lr0oQ1X2w ali@dizustu";
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    for field in ["ci_user", "ci_ssh_key", "ci_ip", "ci_gw", "ci_dns"] {
        assert!(tab.contains(&format!(r#"form="hw" name="{field}""#)), "{field}");
    }
    assert!(tab.contains("cloud-init") && tab.contains("kapalı"));
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="ci_ssh_key""#), "konteynerde yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    let key = KEY.replace(' ', "+");
    let to = post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&ci_ip=10.0.0.5%2F24&ci_gw=10.0.0.1&ci_dns=1.1.1.1%2C+9.9.9.9")).await;
    assert!(to.contains("notice="), "{to}");
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.ci_user.as_deref(), hw.ci_ssh_key.as_deref(), hw.ci_ip.as_deref(), hw.ci_gw.as_deref(), hw.ci_dns.len(), hw.ci_gen), (Some("ali"), Some(KEY), Some("10.0.0.5/24"), Some("10.0.0.1"), 2, 1));
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains("açık · sürüm 1") && tab.contains(r#"name="ci_ip" type="text" value="10.0.0.5/24""#) && tab.contains(r#"value="1.1.1.1, 9.9.9.9""#));
    // Aynı formu yeniden kaydetmek sürümü artırmaz; hatalı adres reddedilir ve kayıt değişmez.
    post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&ci_ip=10.0.0.5%2F24&ci_gw=10.0.0.1&ci_dns=1.1.1.1%2C+9.9.9.9")).await;
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_gen, 1);
    for bad in ["ci_ip=10.0.0.5&ci_gw=&ci_dns=", "ci_ip=10.0.0.5%2F24&ci_gw=gecit&ci_dns=", "ci_ip=&ci_gw=10.0.0.1&ci_dns="] {
        assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&ci_user=ali&ci_ssh_key={key}&{bad}")).await.contains("error="), "{bad}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_ip.as_deref(), Some("10.0.0.5/24"));
    // Seçenekler sekmesinden kaydetmek cloud-init'e dokunmaz.
    post_ok(&app, "/guests/103/edit", "part=secenekler&name=win-build&os=windows-2022&tags=&backup_time=&backup_keep=7&boot_order=0&boot_delay=0&fw_rules=").await;
    assert_eq!(state.snapshot().guest(103).unwrap().hw.ci_gen, 1);
    // Çalışan konukta değişmez.
    assert!(post_ok(&app, "/guests/102/edit", &format!("part=donanim&cores=8&memory_mb=32768&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=&ci_ssh_key={key}")).await.contains("error="));

    // API: adresi boşaltmak DHCP'ye döndürür, anahtarı boşaltmak cloud-init'i kapatır.
    let patch = |body: Value| request(Method::PATCH, "/api/v1/guests/103").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert!(api(&app, patch(json!({ "ci_ip": "", "ci_gw": "", "ci_dns": "" }))).await.0.is_success());
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.ci_ip.clone(), hw.cloud_init(), hw.ci_gen), (None, true, 2));
    let (status, v) = api(&app, patch(json!({ "ci_ip": "10.0.0.9/99" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(api(&app, patch(json!({ "ci_ssh_key": "" }))).await.0.is_success());
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cloud_init(), hw.ci_user.clone(), hw.ci_gen), (false, None, 3));
}

// ---------- İşlemci türü ve bellek balonu ----------

#[tokio::test]
pub(super) async fn cpu_type_and_memory_balloon_are_set_from_the_hardware_tab() {
    let (app, state) = setup();
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<select id="cpu_type" form="hw" name="cpu_type""#) && tab.contains(r#"<option value="" selected>Sunucunun işlemcisi (host)</option>"#), "{tab}");
    assert!(tab.contains(r#"<option value="Haswell">Haswell</option>"#) && tab.contains(r#"name="balloon" value="1""#));
    assert!(!tab.contains("Bellek hedefi"), "balon takılı değilken hedef formu yok");
    assert!(!page(&app, "/guests/202?tab=donanim").await.contains(r#"name="cpu_type""#), "konteynerde yok");

    let base = "part=donanim&cores=8&memory_mb=16384&iso=&bridge=vmbr0&forwards=&firmware=bios&nics=";
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=Haswell&balloon=1")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("Haswell", true, 0));
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=Pentium9&balloon=1")).await.contains("error="));
    let tab = page(&app, "/guests/103?tab=donanim").await;
    assert!(tab.contains(r#"<option value="Haswell" selected>Haswell</option>"#) && tab.contains(r#"action="/guests/103/balloon""#) && tab.contains("tamamı (16384 MB)"));

    // Bellek hedefi: sınırlar içinde kabul edilir, görev günlüğüne yazılır; 0 balonu boşaltır.
    assert!(post_ok(&app, "/guests/103/balloon", "mb=4096").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 4096);
    assert_eq!(state.snapshot().tasks[0].command, "vm balloon 103 4096");
    assert!(page(&app, "/guests/103?tab=donanim").await.contains("4096 / 16384 MB"));
    for bad in ["mb=100", "mb=99999", "mb=cok", "mb="] {
        assert!(post_ok(&app, "/guests/103/balloon", bad).await.contains("error="), "{bad}");
    }
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 4096, "reddedilen hedef kaydı değiştirmez");
    assert!(post_ok(&app, "/guests/103/balloon", "mb=0").await.contains("notice="));
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 0);
    // Balonu olmayan konukta ve izleyici için reddedilir; operatöre açık değildir.
    assert!(post_ok(&app, "/guests/102/balloon", "mb=4096").await.contains("error="));
    let (status, _, _) = send(&app, form_body(as_viewer(Method::POST, "/guests/103/balloon"), "mb=4096")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(redox_server::operator_target("/guests/103/balloon"), None);

    // API.
    let (status, _) = api(&app, post_json("/api/v1/guests/103/balloon", &json!({ "mb": 8192 }))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(state.snapshot().guest(103).unwrap().hw.balloon_mb, 8192);
    let (status, v) = api(&app, post_json("/api/v1/guests/103/balloon", &json!({ "mb": 1 }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Başka üreticinin modeli, üreticisi bilinen sunucuda reddedilir.
    let (real, real_state) = setup_real();
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=EPYC-Milan")).await.contains("error="));
    assert_eq!(real_state.snapshot().guest(103).unwrap().hw.cpu_type, "");
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=Skylake-Server")).await.contains("notice="));
    assert!(post_ok(&real, "/guests/103/edit", &format!("{base}&cpu_type=kvm64")).await.contains("notice="));
    // Balon kaldırılınca hedef sıfırlanır; işlemci türü "host"a döner.
    assert!(post_ok(&app, "/guests/103/edit", &format!("{base}&cpu_type=")).await.contains("notice="));
    let hw = state.snapshot().guest(103).unwrap().hw.clone();
    assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("", false, 0));
}

// ---------- Kaynak havuzları ve kotalar ----------
