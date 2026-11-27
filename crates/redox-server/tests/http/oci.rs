//! OCI imajından konteyner: form alanları, API kuralları ve sürücüye giden tanım.

use super::*;

use std::sync::Mutex;

#[derive(Clone, Default)]
struct Images {
    made: std::sync::Arc<Mutex<Vec<redox_core::Guest>>>,
}

impl Hypervisor for Images {
    fn name(&self) -> &'static str {
        "imaj-sinama"
    }

    fn is_real(&self) -> bool {
        true
    }

    fn supports(&self, _: GuestKind) -> bool {
        true
    }

    fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
        Ok(())
    }

    fn bridges(&self) -> Vec<String> {
        vec!["vmbr0".into()]
    }

    fn templates(&self) -> Vec<String> {
        vec!["debian-12".into()]
    }

    fn oci_images(&self) -> bool {
        true
    }

    fn create(&self, g: &Guest) -> Result<(), String> {
        self.made.lock().unwrap().push(g.clone());
        Ok(())
    }
}

fn images() -> (Router, AppState, Images) {
    let driver = Images::default();
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(driver.clone()), Duration::ZERO, None, auth);
    (app(state.clone()), state, driver)
}

const CT_FORM: &str = "kind=ct&node=pve-ist-01&vmid=&name=web-oci&os=debian-12&storage=local-zfs&disk_gb=4&cores=1&memory_mb=256&bridge=vmbr0";

#[tokio::test]
async fn the_form_offers_an_image_and_a_static_address_and_passes_them_on() {
    let (app, _, driver) = images();
    let form = page(&app, "/guests/new?kind=ct").await;
    assert!(form.contains(r#"name="oci""#) && form.contains(r#"name="ct_address""#) && form.contains(r#"name="ct_gateway""#), "{form}");
    assert!(!page(&app, "/guests/new?kind=vm").await.contains(r#"name="oci""#));
    // Sürücü desteklemiyorsa alanlar görünmez.
    let (plain, _) = setup_real();
    assert!(!page(&plain, "/guests/new?kind=ct").await.contains(r#"name="oci""#));

    let (status, headers, body) = send(&app, post_form("/guests", &format!("{CT_FORM}&oci=nginx%3A1.29&ct_address=10.0.3.50%2F24&ct_gateway=10.0.3.1"))).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(location(&headers).contains("notice="));
    let made = driver.made.lock().unwrap().pop().unwrap();
    assert_eq!((made.os.as_str(), made.hw.ct_address.as_deref(), made.hw.ct_gateway.as_deref()), ("oci:nginx:1.29", Some("10.0.3.50/24"), Some("10.0.3.1")));
    // Alan boşsa seçilen şablon kullanılır.
    send(&app, post_form("/guests", &CT_FORM.replace("web-oci", "web-iki"))).await;
    assert_eq!(driver.made.lock().unwrap().pop().unwrap().os, "debian-12");
}

#[tokio::test]
async fn bad_images_and_addresses_are_refused_before_the_driver_is_called() {
    let (app, _, driver) = images();
    // Donanım alanları API tanımında üst düzeydedir.
    let base = |os: &str, kind: &str, hw: Value| {
        let mut spec = json!({ "kind": kind, "node": "pve-ist-01", "name": "x-oci", "os": os, "storage": "local-zfs", "disk_gb": 4, "cores": 1, "memory_mb": 256, "bridge": "vmbr0", "onboot": false });
        spec.as_object_mut().unwrap().extend(hw.as_object().unwrap().clone());
        spec
    };
    for (spec, expected) in [
        (base("oci:Nginx", "ct", json!({})), "geçersiz imaj adı"),
        (base("oci:x.io/a?b=1", "ct", json!({})), "geçersiz imaj adı"),
        (base("oci:nginx", "vm", json!({})), "yalnızca konteynerlerde"),
        (base("oci:nginx", "ct", json!({ "ct_address": "10.0.0.5" })), "geçersiz konteyner adresi"),
        (base("oci:nginx", "ct", json!({ "ct_address": "10.0.0.5/24\nlxc.hook.pre-start = /x" })), "geçersiz konteyner adresi"),
        (base("oci:nginx", "ct", json!({ "ct_gateway": "10.0.0.1" })), "geçersiz ağ geçidi"),
        (base("debian-12", "vm", json!({ "ct_address": "10.0.0.5/24" })), "yalnızca konteynerlerde"),
        (base("debian-12", "vm", json!({ "tun": true })), "yalnızca konteynerlerde"),
        (base("yok-sablon", "ct", json!({})), "bilinmeyen konteyner şablonu"),
    ] {
        let (status, body) = api(&app, post_json("/api/v1/guests", &spec)).await;
        assert!(status.is_client_error() && body["error"].as_str().is_some_and(|e| e.contains(expected)), "{spec}: {status} {body}");
    }
    assert!(driver.made.lock().unwrap().is_empty());
    let (plain, _) = setup_real();
    let (status, body) = api(&plain, post_json("/api/v1/guests", &base("oci:nginx", "ct", json!({})))).await;
    assert!(status.is_client_error() && body["error"].as_str().is_some_and(|e| e.contains("OCI imajından konteyner oluşturamıyor")), "{status} {body}");
}
