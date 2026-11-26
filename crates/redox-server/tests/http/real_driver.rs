//! Gerçek sürücü kipi.

use super::*;

/// Bellekte çalışan, gerçek sürücü gibi davranan sahte: sayfaların gerçek kipteki halini sınar.
pub(super) struct RealLike;

impl Hypervisor for RealLike {
    fn name(&self) -> &'static str {
        "qemu-kvm"
    }
    fn is_real(&self) -> bool {
        true
    }
    fn supports(&self, _: GuestKind) -> bool {
        true
    }
    fn isos(&self) -> Vec<String> {
        vec!["debian-12.iso".into(), "ubuntu-24.04.iso".into()]
    }
    fn bridges(&self) -> Vec<String> {
        vec!["vmbr0".into(), "br-lan".into()]
    }
    fn live_disk_ops(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm
    }
    fn hotplug(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm
    }
    fn host_cpu_vendor(&self) -> Option<&'static str> {
        Some("intel")
    }
    // Çalışan sanal makinenin canlı konsolu var (soketin kendisi bu testlerde açılmaz).
    fn vnc_socket(&self, g: &Guest) -> Option<std::path::PathBuf> {
        (g.kind == GuestKind::Vm && g.status == GuestStatus::Running).then(|| std::path::PathBuf::from(format!("/olmayan/{}.vnc", g.vmid)))
    }
    // Üçüncü disk ve üçüncü kart "takılamaz": geri alma yolu sınansın.
    fn hot_add_disk(&self, _: &Guest, id: u8) -> Result<(), String> {
        if id == 3 { Err("bağlantı noktası yanıt vermedi".into()) } else { Ok(()) }
    }
    fn hot_add_nic(&self, _: &Guest, index: u8, _: &str) -> Result<(), String> {
        if index == 3 { Err("bağlantı noktası yanıt vermedi".into()) } else { Ok(()) }
    }
    // Beşinci çekirdekten sonrası "eklenemez": yarıda kalan eklemenin kaydı sınansın.
    fn hot_add_core(&self, g: &Guest) -> Result<(), String> {
        if g.cores >= 5 { Err("çekirdek yuvası yanıt vermedi".into()) } else { Ok(()) }
    }
    fn online_cores(&self, _: &Guest) -> bool {
        true
    }
    // Üçüncü bellek parçası "eklenemez": başarısız eklemenin kaydı değiştirmediği sınansın.
    fn hot_add_memory(&self, g: &Guest, _: u64) -> Result<(), String> {
        if g.hw.dimms.len() >= 2 { Err("bellek yuvası yanıt vermedi".into()) } else { Ok(()) }
    }
    fn online_memory(&self, _: &Guest) -> bool {
        true
    }
    fn templates(&self) -> Vec<String> {
        vec!["alpine-3.20".into(), "debian-12".into()]
    }
    fn exec(&self, g: &Guest, command: &str) -> Result<String, String> {
        match command {
            "false" => Err("çıkış kodu 1".into()),
            _ => Ok(format!("{} üzerinde: {command}\n<b>uid=0(root)</b>", g.name)),
        }
    }
    fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
        Ok(())
    }
    fn probe(&self, g: &Guest) -> Option<Probe> {
        Some(Probe { status: g.status, cpu_pct: Some(37), mem_used_mb: Some(g.mem_total_mb / 2) })
    }
    // Konuk 1 GB kullanıyor görünür (balon sürücüsü ölçümü).
    fn balloon_stats(&self, g: &Guest) -> Option<redox_core::BalloonStats> {
        Some(redox_core::BalloonStats { total_mb: g.mem_total_mb, available_mb: g.mem_total_mb - 1024 })
    }
}

pub(super) fn setup_real() -> (Router, AppState) {
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(RealLike), Duration::ZERO, None, auth);
    (app(state.clone()), state)
}

#[tokio::test]
pub(super) async fn real_driver_pages_show_measurements_not_simulation() {
    let (app, state) = setup_real();
    state.refresh(None, None);
    state.refresh(None, None);

    let node = page(&app, "/nodes/pve-ist-01").await;
    assert!(node.contains("-10 dk") && !node.contains("-60 dk"), "gerçek grafik son 10 dakikayı gösterir");
    assert!(page(&app, "/").await.contains("37%"), "ölçülen CPU tabloya yansır");

    let guest = page(&app, "/guests/102").await;
    assert!(guest.contains("Ölçümler 5 saniyede bir okunur") && !guest.contains("simülasyon"));
    assert!(guest.contains("köprü: vmbr0") && guest.contains("52:54:00:00:00:66") && !guest.contains("Mbit/s"), "gerçek ağ bilgisi; uydurma ağ grafiği yok");
    assert!(guest.contains("son 10 dakika"));

    let form = page(&app, "/guests/new?kind=vm").await;
    assert!(form.contains(r#"<select id="iso" name="iso""#) && form.contains(r#"<option value="ubuntu-24.04.iso">"#));
    assert!(!page(&app, "/guests/new?kind=ct").await.contains(r#"name="iso""#), "konteynere kalıp takılmaz");
}

#[tokio::test]
pub(super) async fn real_driver_create_validates_iso_and_kind() {
    let (app, state) = setup_real();
    let with_iso = format!("{VALID_FORM}&iso=debian-12.iso");
    let (status, headers, _) = send(&app, post_form("/guests", &with_iso)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location(&headers).starts_with("/guests/105"));
    assert_eq!(state.snapshot().guest(105).unwrap().iso.as_deref(), Some("debian-12.iso"));
    assert!(page(&app, "/guests/105").await.contains("debian-12.iso"));

    for (form, needle) in [
        (format!("{}&iso=yok.iso", VALID_FORM.replace("name=yeni-vm", "name=baska")), "ISO bulunamadı: yok.iso"),
        (format!("{}&iso=..%2F..%2Fetc%2Fpasswd", VALID_FORM.replace("name=yeni-vm", "name=baska")), "ISO bulunamadı"),
        (VALID_FORM.replace("kind=vm", "kind=ct").replace("name=yeni-vm", "name=kutu").replace("os=debian-12", "os=windows-11"), "konteyner şablonu: windows-11"),
        (VALID_FORM.replace("bridge=vmbr0", "bridge=br-yok").replace("name=yeni-vm", "name=agsiz"), "adında bir köprü yok"),
    ] {
        let (status, _, body) = send(&app, post_form("/guests", &form)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{form}");
        assert!(body.contains(needle), "{form}: beklenen ileti yok");
    }
    assert_eq!(state.snapshot().guests.len(), 9);

    // Konteyner: şablon listeden olmalı, ağ var olan bir köprü ya da NAT olmalı.
    let spec = |os: &str, bridge: &str| json!({"name": "api-ct", "kind": "ct", "node": "pve-ank-01", "cores": 1, "memory_mb": 256, "disk_gb": 8, "storage": "local-lvm", "os": os, "bridge": bridge});
    for bad in [spec("windows-11", "nat"), spec("alpine-3.20", "br-yok"), spec("alpine-3.20", "br0;reboot")] {
        let (status, v) = api(&app, post_json("/api/v1/guests", &bad)).await;
        assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_spec")), "{bad}");
    }
    let (status, g) = api(&app, post_json("/api/v1/guests", &spec("alpine-3.20", "br-lan"))).await;
    assert_eq!((status, g["bridge"].as_str(), g["os"].as_str()), (StatusCode::CREATED, Some("br-lan"), Some("alpine-3.20")));
}

// ---------- Konuk işlemleri: düzenleme, anlık görüntü, yedek, klon, toplu işlem, konsol ----------

#[tokio::test]
async fn cloud_image_catalog_is_listed_and_fetched_by_key() {
    let (app, state) = setup_real();
    let storage = page(&app, "/storage").await;
    assert!(storage.contains("Hazır bulut imajları") && storage.contains(r#"action="/media/catalog/debian-12""#) && storage.contains("Ubuntu 24.04 LTS"));
    let (status, list) = api(&app, request(Method::GET, "/api/v1/media/catalog").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list.as_array().unwrap().iter().any(|e| e["key"] == "rocky-9" && e["url"].as_str().unwrap().starts_with("https://")));
    // Bilinmeyen anahtar reddedilir ve geçerli olanlar söylenir.
    let err = state.catalog_fetch("windows-11", "admin").unwrap_err().to_string();
    assert!(err.contains("katalogda") && err.contains("debian-12"), "{err}");
}
