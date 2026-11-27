//! Ortak depo kartı, uçları ve devir. Gerçek disklerle davranış iki gerçek sunuculu uçtan uca sınamada
//! denetlenir; burada gerçek sahiplik dosyaları, silmeyi sayan sahte bir sürücüyle kullanılır.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use redox_server::shared::{self, Owner};
use redox_server::state::shared::QUIET_SECS;

use super::*;

const MINE: &str = "11111111111111111111111111111111";
const THEIRS: &str = "22222222222222222222222222222222";

#[derive(Clone)]
struct Sharing {
    dir: PathBuf,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Hypervisor for Sharing {
    fn name(&self) -> &'static str {
        "ortak-sinama"
    }

    fn apply(&self, guest: &Guest, action: Action) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("{action:?} {}", guest.vmid));
        Ok(())
    }

    fn destroy(&self, guest: &Guest) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("destroy {}", guest.vmid));
        Ok(())
    }

    fn shared_storages(&self) -> Vec<(String, PathBuf)> {
        vec![("local-zfs".to_string(), self.dir.clone())]
    }

    fn server_id(&self) -> String {
        MINE.to_string()
    }
}

fn world(name: &str) -> (Router, AppState, Sharing) {
    let dir = std::env::temp_dir().join(format!("redox-http-ortak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    shared::init(&dir).unwrap();
    let driver = Sharing { dir, calls: Default::default() };
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(driver.clone()), Duration::ZERO, None, auth);
    (app(state.clone()), state, driver)
}

fn calls(d: &Sharing) -> Vec<String> {
    std::mem::take(&mut d.calls.lock().unwrap())
}

#[tokio::test]
async fn without_shared_storage_there_is_no_card_and_nothing_to_adopt() {
    let (app, _) = setup();
    assert!(!page(&app, "/cluster").await.contains("/cluster/shared"));
    let (status, list) = api(&app, get("/api/v1/shared")).await;
    assert_eq!((status, list.as_array().map(Vec::len)), (StatusCode::OK, Some(0)));
    assert_eq!(send(&app, get("/api/v1/shared/offer/103")).await.0, StatusCode::NOT_FOUND, "ortak depoda olmayan konuk için eş 404 görür");
    let (status, v) = api(&app, post_json("/api/v1/shared/adopt/ortak/103", &json!({}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(post_ok(&app, "/cluster/shared", "op=adopt&storage=ortak&vmid=103").await.contains("error="));
    assert!(post_ok(&app, "/cluster/shared", "op=take&peer=yok&vmid=abc").await.contains("error="));
    for uri in ["/api/v1/shared/release/103", "/api/v1/shared/take/ankara/103", "/api/v1/shared/adopt/ortak/103"] {
        let denied = send(&app, as_viewer(Method::POST, uri).header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "server": MINE, "node": "x" }).to_string())).unwrap()).await.0;
        assert_eq!(denied, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn releasing_a_guest_moves_ownership_and_leaves_a_record_that_never_touches_disks() {
    let (app, state, driver) = world("devir");
    assert!(state.shared_claim_existing().is_empty());
    assert_eq!(shared::read_owner(&driver.dir, 103).unwrap().unwrap().server, MINE, "var olan konuklar üstlenilir");
    state.shared_tick(1000);
    assert!(shared::manifest(&driver.dir, 103).unwrap().is_some(), "sahibi kaydı paylaşıma yazar");
    let release = |id: u32, server: &str| post_json(&format!("/api/v1/shared/release/{id}"), &json!({ "server": server, "node": "ankara" }));
    for (id, server) in [(103, "kisa"), (103, MINE), (100, THEIRS)] {
        assert!(!api(&app, release(id, server)).await.0.is_success(), "{id} {server}");
    }
    assert_eq!(send(&app, release(103, THEIRS)).await.0, StatusCode::NO_CONTENT);
    assert_eq!(shared::read_owner(&driver.dir, 103).unwrap().unwrap(), Owner { server: THEIRS.into(), node: "ankara".into() });
    let shown = page(&app, "/guests/103").await;
    assert!(shown.contains("ankara sunucusuna devredildi"), "{shown}");
    let (status, v) = api(&app, post_json("/api/v1/guests/103/start", &json!({}))).await;
    assert!(!status.is_success() && v.to_string().contains("devredildi"), "{status} {v}");
    calls(&driver);
    assert!(api(&app, request(Method::DELETE, "/api/v1/guests/103").body(Body::empty()).unwrap()).await.0.is_success());
    assert!(calls(&driver).is_empty(), "devredilmiş kaydı silmek sürücüye disk sildirmez");
}

#[tokio::test]
async fn a_guest_whose_ownership_was_taken_is_stopped_and_a_quiet_owner_can_be_replaced() {
    let (app, state, driver) = world("yitik");
    state.shared_claim_existing();
    calls(&driver);
    // 100 çalışıyor; başka sunucu sahipliği almış: ilk turda kapatılır.
    shared::transfer(&driver.dir, 100, MINE, &Owner { server: THEIRS.into(), node: "izmir".into() }).unwrap();
    state.shared_tick(1000);
    assert_eq!(calls(&driver), ["Stop 100"]);
    let g = state.snapshot().guest(100).unwrap().clone();
    assert_eq!((g.status, g.hw.handed_to.as_deref()), (GuestStatus::Stopped, Some("izmir")));

    // Devralma: öbür sunucu sayacını artırdıkça reddedilir, yeterince susunca kabul edilir.
    shared::publish(&driver.dir, &state.snapshot().guest(103).unwrap().clone(), MINE).unwrap();
    shared::transfer(&driver.dir, 103, MINE, &Owner { server: THEIRS.into(), node: "izmir".into() }).unwrap();
    shared::beat(&driver.dir, THEIRS, 1).unwrap();
    state.shared_tick(2000);
    assert!(state.shared_adopt("local-zfs", 103, "admin", 2000).unwrap_err().to_string().contains("yaşıyor olabilir"));
    shared::beat(&driver.dir, THEIRS, 2).unwrap();
    state.shared_tick(2000 + QUIET_SECS);
    assert!(state.shared_adopt("local-zfs", 103, "admin", 2000 + QUIET_SECS).is_err(), "sayaç değiştiyse süre baştan sayılır");
    state.shared_tick(2000 + 2 * QUIET_SECS);
    let item = state.shared_list(2000 + 2 * QUIET_SECS).into_iter().find(|i| i.vmid == 103).unwrap();
    assert_eq!((item.mine, item.quiet, item.owner.as_str()), (false, true, "izmir"));
    state.shared_adopt("local-zfs", 103, "admin", 2000 + 2 * QUIET_SECS).unwrap();
    assert_eq!(shared::read_owner(&driver.dir, 103).unwrap().unwrap().server, MINE);
    let g = state.snapshot().guest(103).unwrap().clone();
    assert_eq!((g.status, g.hw.handed_to), (GuestStatus::Stopped, None));
    assert!(!calls(&driver).iter().any(|c| c.starts_with("destroy")), "devralma hiçbir diske dokunmaz");
    assert!(page(&app, "/cluster").await.contains("Ortak depo"));

    // Paylaşım kopuksa hiçbir konuk kapatılmaz (karar verilemez).
    std::fs::remove_file(driver.dir.join(".redox-shared")).unwrap();
    state.shared_tick(3000);
    assert!(calls(&driver).is_empty());
    let _ = std::fs::remove_dir_all(&driver.dir);
}
