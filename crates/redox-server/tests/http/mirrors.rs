//! Eşzamanlı disk yansıtmanın yönetimi: gerçek TCP kapılarında iki sunucu ve davranışı testten yönetilen bir
//! sürücü. Diskleri gerçekten yansıtan kısım `qemu_driver/real_mirror.rs` ile gerçek QEMU'da sınanır.

use std::sync::{Arc, Mutex};

use redox_core::{MirrorProbe, MirrorState};
use redox_server::state::mirrors::{HEARTBEAT_SECS, RETRY_SECS, STALL_SECS};

use super::*;

#[derive(Default)]
struct Disks {
    /// Sürücüye yapılan yansıtma ve kopya çağrıları, sırayla.
    calls: Vec<String>,
    probe: MirrorProbe,
    start_error: Option<String>,
}

#[derive(Clone, Default)]
struct Mirroring(Arc<Mutex<Disks>>);

impl Mirroring {
    fn note(&self, call: String) {
        self.0.lock().unwrap().calls.push(call);
    }

    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().calls)
    }

    fn set(&self, jobs: usize, synced: bool, done: u64, total: u64) {
        let mut d = self.0.lock().unwrap();
        d.probe = MirrorProbe { disks: d.probe.disks, jobs, synced, done, total };
    }
}

impl Hypervisor for Mirroring {
    fn name(&self) -> &'static str {
        "qemu-kvm"
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
    fn probe(&self, g: &Guest) -> Option<Probe> {
        Some(Probe { status: g.status, cpu_pct: None, mem_used_mb: None })
    }
    fn disk_mirroring(&self) -> bool {
        true
    }
    fn live_disk_ops(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm
    }
    fn mirror_disk_sizes(&self, g: &Guest) -> Result<Vec<u64>, String> {
        let mut d = self.0.lock().unwrap();
        d.probe.disks = 1 + g.hw.disks.len();
        Ok(vec![1 << 30; d.probe.disks])
    }
    fn mirror_start(&self, g: &Guest, host: &str, port: u16, key: &str) -> Result<(), String> {
        let mut d = self.0.lock().unwrap();
        if let Some(e) = d.start_error.clone() {
            return Err(e);
        }
        assert_eq!(key.len(), 64);
        d.calls.push(format!("start {} {host}:{port}", g.vmid));
        d.probe.jobs = d.probe.disks;
        (d.probe.synced, d.probe.done, d.probe.total) = (false, 0, 100);
        Ok(())
    }
    fn mirror_probe(&self, g: &Guest) -> Option<MirrorProbe> {
        let mut d = self.0.lock().unwrap();
        d.probe.disks = 1 + g.hw.disks.len();
        Some(d.probe)
    }
    fn mirror_stop(&self, g: &Guest) -> Result<(), String> {
        let mut d = self.0.lock().unwrap();
        d.calls.push(format!("stop {}", g.vmid));
        d.probe.jobs = 0;
        Ok(())
    }
    fn replica_serve(&self, id: &str, sizes: &[u64], _: &str, port: u16, _: &str) -> Result<(), String> {
        self.note(format!("serve {id} {} {port}", sizes.len()));
        Ok(())
    }
    fn replica_release(&self, id: &str) -> Result<(), String> {
        self.note(format!("release {id}"));
        Ok(())
    }
    fn replica_delete(&self, id: &str) -> Result<(), String> {
        self.note(format!("delete {id}"));
        Ok(())
    }
    fn replica_adopt(&self, id: &str, g: &Guest) -> Result<(), String> {
        self.note(format!("adopt {id} {}", g.vmid));
        Ok(())
    }
}

struct Server {
    state: AppState,
    app: Router,
    disks: Mirroring,
    port: u16,
}

async fn server() -> Server {
    let disks = Mirroring::default();
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(disks.clone()), Duration::ZERO, None, auth);
    let app = app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    Server { state, app, disks, port }
}

async fn tick(server: &Server, at: u64) {
    let state = server.state.clone();
    tokio::task::spawn_blocking(move || state.mirror_tick_at(at)).await.unwrap();
}

fn record(server: &Server, vmid: u32) -> redox_core::Mirror {
    server.state.snapshot().mirror(vmid).expect("yansıtma kaydı").clone()
}

fn delete(uri: &str) -> Request<Body> {
    request(Method::DELETE, uri).body(Body::empty()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(super) async fn a_mirror_is_followed_rescued_when_stuck_and_taken_over_on_the_peer() {
    let (source, target) = (server().await, server().await);
    source.state.peers().configure(redox_server::peers::parse_file(&format!("yedek http://127.0.0.1:{} {ADMIN_TOKEN}\n", target.port)).unwrap());
    let guest = source.state.snapshot().guests.iter().find(|g| g.status == GuestStatus::Running && redox_core::mirror_blocker(g).is_none()).unwrap().clone();
    let vmid = guest.vmid;
    let origin = redox_server::state::mirrors::origin_name(&source.state.snapshot());

    // Tanımsız eş ve yetkisiz kullanıcı.
    let (status, v) = api(&source.app, post_json("/api/v1/mirrors", &json!({ "vmid": vmid, "peer": "yok" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let viewer = as_viewer(Method::POST, "/api/v1/mirrors").header(CONTENT_TYPE, "application/json").body(Body::from(json!({ "vmid": vmid, "peer": "yedek" }).to_string())).unwrap();
    assert_eq!(send(&source.app, viewer).await.0, StatusCode::FORBIDDEN);
    assert_eq!(api(&source.app, post_json("/api/v1/mirrors", &json!({ "vmid": vmid, "peer": "yedek" }))).await.0, StatusCode::ACCEPTED);

    // İlk tur: hedefte kuşak açılır, yerel işler kurulur.
    let mut at = now();
    tick(&source, at).await;
    let m = record(&source, vmid);
    assert_eq!((m.state, m.generation, m.error.clone()), (MirrorState::Syncing, 1, None), "{m:?}");
    let served = target.disks.calls();
    assert_eq!(served.len(), 1, "{served:?}");
    assert!(served[0].starts_with(&format!("serve {origin}-{vmid}-1 {} ", 1 + guest.hw.disks.len())), "{served:?}");
    let started = source.disks.calls();
    assert!(started.len() == 1 && started[0].starts_with(&format!("start {vmid} 127.0.0.1:")), "adres eşin yönetim adresinden alınır: {started:?}");
    // Eşitlenmeden devralınamaz.
    let (_, v) = api(&target.app, post_json(&format!("/api/v1/mirror-stores/{origin}/{vmid}/promote"), &json!({}))).await;
    assert!(v["error"].as_str().unwrap().contains("eksiksiz bir kopya yok"), "{v}");

    // Eşitlenir: hedefe bildirilir, kuşak eksiksiz sayılır.
    source.disks.set(1, true, 100, 100);
    at += 1;
    tick(&source, at).await;
    assert_eq!(record(&source, vmid).state, MirrorState::InSync);
    let held = || target.state.snapshot().mirror_stores[0].clone();
    assert_eq!((held().complete, held().in_sync), (Some(1), true));
    // Kaynak bildirimi sürerken devralma reddedilir (konuk orada çalışıyor).
    let (status, v) = api(&target.app, post_json(&format!("/api/v1/mirror-stores/{origin}/{vmid}/promote"), &json!({}))).await;
    assert!(status == StatusCode::UNPROCESSABLE_ENTITY && v["error"].as_str().unwrap().contains("hâlâ yansıtıyor"), "{v}");
    // Hedef kilit kipinde olsa da durum bildirimi kabul edilir.
    target.state.lockdown_set(true, "admin");
    let before = held().last_in_sync;
    target.state.mutate(|c| c.mirror_stores[0].last_in_sync = Some(1));
    at += HEARTBEAT_SECS;
    tick(&source, at).await;
    assert!(held().last_in_sync >= before, "kilitliyken de bildirim işlenir");
    target.state.lockdown_set(false, "admin");

    // Yazma sürerken (yetişmemiş ama ilerliyor) yansıtmaya dokunulmaz.
    source.disks.calls();
    for (i, done) in [150u64, 180, 190].into_iter().enumerate() {
        source.disks.set(1, true, done, 200);
        tick(&source, at + (i as u64 + 1) * STALL_SECS).await;
    }
    at += 3 * STALL_SECS;
    assert!(source.disks.calls().is_empty() && record(&source, vmid).state == MirrorState::InSync);
    // İlerleme takılırsa (donan kopya) süre dolmadan beklenir, dolunca yansıtma durdurulur.
    tick(&source, at + STALL_SECS - 1).await;
    assert!(source.disks.calls().is_empty());
    at += STALL_SECS;
    tick(&source, at).await;
    assert_eq!(source.disks.calls(), [format!("stop {vmid}")]);
    let m = record(&source, vmid);
    assert!(m.state == MirrorState::Broken && m.error.as_deref().unwrap().contains("yanıt vermiyor") && m.last_in_sync.is_some(), "{m:?}");
    assert!(!held().in_sync, "hedefe artık eşit olmadığı bildirilir");
    assert_eq!(held().complete, Some(1), "eksiksiz kuşak korunur");
    assert!(source.state.snapshot().tasks.iter().any(|t| t.command == format!("vm mirror broken {vmid} yedek")));

    // Deneme aralığı dolmadan yeniden başlamaz; dolunca YENİ kuşağa eşitlenir, eskisi o tamamlanana dek kalır.
    target.disks.calls();
    tick(&source, at + RETRY_SECS - 1).await;
    assert!(target.disks.calls().is_empty());
    at += RETRY_SECS;
    tick(&source, at).await;
    assert_eq!((record(&source, vmid).state, record(&source, vmid).generation), (MirrorState::Syncing, 2));
    let calls = target.disks.calls();
    assert!(calls.contains(&format!("release {origin}-{vmid}-1")) && calls.iter().any(|c| c.starts_with(&format!("serve {origin}-{vmid}-2 "))) && !calls.iter().any(|c| c.starts_with("delete")), "{calls:?}");
    source.disks.set(1, true, 100, 100);
    at += 1;
    tick(&source, at).await;
    assert_eq!(held().complete, Some(2));
    assert!(target.disks.calls().contains(&format!("delete {origin}-{vmid}-1")), "yenisi tamamlanınca eskisi silinir");

    // Anlık görüntü yansıtmayı sürücüde duraklatır: bu kopma sayılmaz ve deneme aralığı beklenmeden yeniden eşitlenir.
    at += RETRY_SECS;
    tick(&source, at).await;
    source.disks.set(1, true, 100, 100);
    at += 1;
    tick(&source, at).await;
    assert_eq!(record(&source, vmid).state, MirrorState::InSync);
    let generation = record(&source, vmid).generation;
    // Görevler gerçek saatle biter; testin ileri sardığı saat kayıttan geri alınır.
    source.state.mutate(|c| c.mirrors[0].since = now());
    let (status, v) = api(&source.app, post_json(&format!("/api/v1/guests/{vmid}/snapshots"), &json!({ "name": "aynali" }))).await;
    assert!(status.is_success(), "{v}");
    source.disks.set(0, false, 0, 0);
    at += 1;
    tick(&source, at).await;
    let m = record(&source, vmid);
    assert!(m.state == MirrorState::Waiting && m.error.as_deref().unwrap().contains("duraklatıldı"), "{m:?}");
    assert!(!held().in_sync);
    at += 1;
    tick(&source, at).await;
    assert_eq!((record(&source, vmid).state, record(&source, vmid).generation), (MirrorState::Syncing, generation + 1));
    source.disks.set(1, true, 100, 100);
    at += 1;
    tick(&source, at).await;
    let taken_generation = record(&source, vmid).generation;

    // Bağlantı koparsa işler kendiliğinden kaybolur: bu da kopmadır.
    source.disks.calls();
    source.disks.set(0, false, 0, 0);
    at += 1;
    tick(&source, at).await;
    assert!(record(&source, vmid).error.unwrap().contains("koptu"));

    // Yansıtılan konuk canlı taşınmaz.
    assert!(source.state.migrate_out(vmid, "10.0.0.1", 49152, 49153, &"a".repeat(64), "admin").unwrap_err().to_string().contains("canlı taşın"));

    // Devralma: kaynak artık eşit bildirmiyor. Kimlik hedefte dolu (aynı demo kümesi): yeni kimlik verilir.
    target.disks.calls();
    let listed = api(&target.app, get("/api/v1/mirror-stores")).await.1;
    assert!(listed[0]["origin"] == origin && listed[0]["complete"] == taken_generation && listed[0]["in_sync"] == false, "{listed}");
    let (status, v) = api(&target.app, post_json(&format!("/api/v1/mirror-stores/{origin}/{vmid}/promote"), &json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let new = v["vmid"].as_u64().unwrap() as u32;
    assert_ne!(new, vmid);
    let taken = target.state.snapshot().guest(new).unwrap().clone();
    assert_eq!((taken.status, taken.name.as_str(), taken.cores, taken.mem_total_mb, taken.hw.disks.len()), (GuestStatus::Stopped, guest.name.as_str(), guest.cores, guest.mem_total_mb, guest.hw.disks.len()));
    assert!(taken.tags.iter().any(|t| t == &format!("devralindi-{origin}")), "{:?}", taken.tags);
    assert!(target.disks.calls().contains(&format!("adopt {origin}-{vmid}-{taken_generation} {new}")));
    assert!(target.state.snapshot().mirror_stores.is_empty());
    // Devralınan kopyaya kaynak artık yazamaz.
    assert!(target.state.mirror_store_report(&origin, vmid, taken_generation, true).is_err());

    // Başlatma hatası görünür ve yeniden denenir; kapatınca kayıt kalkar.
    source.disks.0.lock().unwrap().start_error = Some("kapı kapalı".to_string());
    at += RETRY_SECS;
    tick(&source, at).await;
    let m = record(&source, vmid);
    assert!(m.state == MirrorState::Broken && m.error.as_deref() == Some("kapı kapalı"), "{m:?}");
    assert_eq!(api(&source.app, delete(&format!("/api/v1/mirrors/{vmid}?delete_copy=true"))).await.0, StatusCode::NO_CONTENT);
    assert!(source.state.snapshot().mirrors.is_empty() && target.state.snapshot().mirror_stores.is_empty(), "hedefteki kopya da silinir");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(super) async fn the_cluster_page_manages_mirrors_and_held_copies() {
    let (source, target) = (server().await, server().await);
    source.state.peers().configure(redox_server::peers::parse_file(&format!("yedek http://127.0.0.1:{} {ADMIN_TOKEN}\n", target.port)).unwrap());
    let vmid = source.state.snapshot().guests.iter().find(|g| g.status == GuestStatus::Running && redox_core::mirror_blocker(g).is_none()).unwrap().vmid;
    let origin = redox_server::state::mirrors::origin_name(&source.state.snapshot());
    assert!(page(&source.app, "/cluster").await.contains("Disk yansıtma"));
    for bad in ["op=add&vmid=abc&peer=yedek".to_string(), format!("op=add&vmid={vmid}&peer=yok"), format!("op=bilinmez&vmid={vmid}")] {
        assert!(post_ok(&source.app, "/cluster/mirror", &bad).await.contains("error="), "{bad}");
    }
    assert!(post_ok(&source.app, "/cluster/mirror", &format!("op=add&vmid={vmid}&peer=yedek")).await.contains("notice="));
    tick(&source, 100).await;
    source.disks.set(1, true, 100, 100);
    tick(&source, 101).await;
    let shown = page(&source.app, "/cluster").await;
    assert!(shown.contains("pill ok") && shown.contains(">eşit<") || shown.contains("eşit"), "{shown}");
    let held = page(&target.app, "/cluster").await;
    assert!(held.contains("Tutulan kopyalar") && held.contains(&origin) && held.contains("/cluster/mirror"), "{held}");
    // Kaynak yaşarken devralma formdan da reddedilir.
    assert!(post_ok(&target.app, "/cluster/mirror", &format!("op=promote&origin={origin}&vmid={vmid}")).await.contains("error="));
    let (status, _, _) = send(&target.app, form_body(as_viewer(Method::POST, "/cluster/mirror"), &format!("op=delete&origin={origin}&vmid={vmid}"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Kopyayı bırakarak kapatma: hedefe "eşit değil" denir ve devralınabilir.
    assert!(post_ok(&source.app, "/cluster/mirror", &format!("op=off&vmid={vmid}")).await.contains("notice="));
    assert!(!target.state.snapshot().mirror_stores[0].in_sync);
    let done = post_ok(&target.app, "/cluster/mirror", &format!("op=promote&origin={origin}&vmid={vmid}")).await;
    assert!(done.contains("notice=") && target.state.snapshot().mirror_stores.is_empty(), "{done}");
}
