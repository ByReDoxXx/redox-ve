//! Hata toleransının yönetimi: gerçek TCP kapılarında üç sunucu (birincil, yedek, hakem) ve davranışı testten
//! yönetilen bir sürücü. QEMU adımları `qemu_driver/real_ft.rs` ile gerçek QEMU'da sınanır.
//!
//! Her senaryonun sorduğu soru aynıdır: **konuk iki yerde birden çalışabilir mi?**

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use redox_core::{FtLink, FtSide, FtState, FtStatus};
use redox_server::state::ft::{DEAD_SECS, RETRY_SECS};

use super::*;

#[derive(Default)]
struct Twin {
    calls: Vec<String>,
    status: HashMap<u32, FtStatus>,
    protect_error: Option<String>,
}

#[derive(Clone, Default)]
struct FtDriver(Arc<Mutex<Twin>>);

impl FtDriver {
    fn note(&self, call: String) {
        self.0.lock().unwrap().calls.push(call);
    }

    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().calls)
    }

    fn set(&self, vmid: u32, status: Option<(bool, bool)>) {
        let mut d = self.0.lock().unwrap();
        match status {
            Some((engaged, paused)) => {
                d.status.insert(vmid, FtStatus { engaged, paused, ..Default::default() });
            }
            None => {
                d.status.remove(&vmid);
            }
        }
    }

    fn engaged(&self, vmid: u32) -> Option<bool> {
        self.0.lock().unwrap().status.get(&vmid).map(|s| s.engaged)
    }
}

impl Hypervisor for FtDriver {
    fn name(&self) -> &'static str {
        "qemu-kvm"
    }
    fn is_real(&self) -> bool {
        true
    }
    fn supports(&self, _: GuestKind) -> bool {
        true
    }
    fn bridges(&self) -> Vec<String> {
        vec!["vmbr0".into(), "vds-ft".into()]
    }
    fn apply(&self, g: &Guest, action: Action) -> Result<(), String> {
        match action {
            Action::Start => self.set(g.vmid, Some((false, false))),
            Action::Stop => self.set(g.vmid, None),
            _ => {}
        }
        Ok(())
    }
    fn probe(&self, g: &Guest) -> Option<Probe> {
        Some(Probe { status: g.status, cpu_pct: None, mem_used_mb: None })
    }
    fn fault_tolerance(&self) -> bool {
        true
    }
    fn ft_standby_start(&self, g: &Guest, disk_bytes: u64, link: &FtLink, key: &str) -> Result<(), String> {
        assert!(g.hw.ft && key.len() == 64 && disk_bytes == g.disk_gb << 30);
        self.note(format!("standby-start {} {}", g.vmid, link.primary_host));
        self.set(g.vmid, Some((false, true)));
        Ok(())
    }
    fn ft_standby_stop(&self, g: &Guest) {
        self.note(format!("standby-stop {}", g.vmid));
        self.set(g.vmid, None);
    }
    fn ft_protect(&self, g: &Guest, link: &FtLink, _: &str) -> Result<(), String> {
        if let Some(e) = self.0.lock().unwrap().protect_error.clone() {
            return Err(e);
        }
        assert!(link.secondary_host == "127.0.0.1" && link.nbd_port != link.migrate_port && link.local_ports.0 != link.mirror_port, "{link:?}");
        self.note(format!("protect {}", g.vmid));
        self.set(g.vmid, Some((true, false)));
        Ok(())
    }
    fn ft_status(&self, g: &Guest) -> Option<FtStatus> {
        self.0.lock().unwrap().status.get(&g.vmid).cloned()
    }
    fn ft_drop_secondary(&self, g: &Guest) -> Result<(), String> {
        self.note(format!("drop-secondary {}", g.vmid));
        if self.engaged(g.vmid).is_some() {
            self.set(g.vmid, Some((false, false)));
        }
        Ok(())
    }
    fn ft_take_over(&self, g: &Guest) -> Result<(), String> {
        self.note(format!("take-over {}", g.vmid));
        self.set(g.vmid, Some((false, false)));
        Ok(())
    }
    fn ft_hold(&self, g: &Guest, hold: bool) -> Result<(), String> {
        self.note(format!("hold {} {hold}", g.vmid));
        Ok(())
    }
    fn ft_fence(&self, g: &Guest) -> Result<(), String> {
        self.note(format!("fence {}", g.vmid));
        self.set(g.vmid, None);
        Ok(())
    }
}

struct Node {
    state: AppState,
    app: Router,
    twin: FtDriver,
    port: u16,
}

async fn node() -> Node {
    let twin = FtDriver::default();
    let auth = Auth::new(users(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(now()), Box::new(twin.clone()), Duration::ZERO, None, auth);
    let app = app(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    Node { state, app, twin, port }
}

/// Bir sunucunun eş tanımları; `None` kapalı bir kapıdır (o sunucuya ulaşılamıyor).
fn knows(node: &Node, peers: &[(&str, Option<u16>)]) {
    let text: String = peers.iter().map(|(name, port)| format!("{name} http://127.0.0.1:{} {ADMIN_TOKEN}\n", port.unwrap_or(1))).collect();
    node.state.peers().configure(redox_server::peers::parse_file(&text).unwrap());
}

struct World {
    primary: Node,
    standby: Node,
    witness: Node,
    vmid: u32,
}

impl World {
    fn connect(&self) {
        knows(&self.primary, &[("yedek", Some(self.standby.port)), ("hakem", Some(self.witness.port))]);
        knows(&self.standby, &[("hakem", Some(self.witness.port))]);
    }

    async fn tick(&self, node: &Node, at: u64) {
        let state = node.state.clone();
        tokio::task::spawn_blocking(move || state.ft_tick_at(at)).await.unwrap();
    }

    async fn act(&self, node: &Node, action: Action) -> Result<(), String> {
        let (state, vmid) = (node.state.clone(), self.vmid);
        tokio::task::spawn_blocking(move || state.run_action(vmid, action, "admin").map(drop).map_err(|e| e.to_string())).await.unwrap()
    }

    fn protection(&self) -> redox_core::Protection {
        self.primary.state.snapshot().protection(self.vmid).expect("koruma").clone()
    }

    fn guest(&self, node: &Node) -> Guest {
        node.state.snapshot().guest(self.vmid).expect("konuk").clone()
    }

    /// Korumayı kurar ve iki tarafın da eşzamanlı çalıştığını görür.
    async fn protect(&self, at: u64) {
        self.tick(&self.primary, at).await;
        let p = self.protection();
        assert_eq!((p.state, p.error.clone()), (FtState::Protected, None), "{p:?}");
        self.standby.twin.set(self.vmid, Some((true, false)));
        self.tick(&self.standby, at).await;
    }
}

/// Korunmaya hazır bir konukla üç sunucu.
async fn world() -> World {
    let (primary, standby, witness) = (node().await, node().await, node().await);
    let vmid = primary.state.snapshot().guests.iter().find(|g| g.kind == GuestKind::Vm && !g.hw.template).unwrap().vmid;
    primary.state.mutate(|c| {
        let g = c.guest_mut_for_import(vmid).unwrap();
        (g.status, g.started_at, g.bridge, g.iso, g.hw) = (GuestStatus::Stopped, None, "vds-ft".to_string(), None, Default::default());
    });
    // Yedek sunucuda aynı kimlik boştur.
    standby.state.mutate(|c| c.guests.retain(|g| g.vmid != vmid));
    let world = World { primary, standby, witness, vmid };
    world.connect();
    world
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
pub(super) async fn a_protected_guest_never_runs_in_two_places() {
    let w = world().await;
    let vmid = w.vmid;
    let origin = redox_server::state::mirrors::origin_name(&w.primary.state.snapshot());
    for (peer, witness, needle) in [("yok", "hakem", "tanımlı değil"), ("yedek", "yedek", "farklı bir sunucu")] {
        let (_, v) = api(&w.primary.app, post_json("/api/v1/ft/protections", &json!({ "vmid": vmid, "peer": peer, "witness": witness }))).await;
        assert!(v["error"].as_str().unwrap().contains(needle), "{v}");
    }
    let body = json!({ "vmid": vmid, "peer": "yedek", "witness": "hakem" });
    let viewer = as_viewer(Method::POST, "/api/v1/ft/protections").header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    assert_eq!(send(&w.primary.app, viewer).await.0, StatusCode::FORBIDDEN);
    assert_eq!(api(&w.primary.app, post_json("/api/v1/ft/protections", &body)).await.0, StatusCode::ACCEPTED);
    w.act(&w.primary, Action::Start).await.unwrap();

    // ---- Koruma kurulur.
    let mut at = now();
    w.protect(at).await;
    assert_eq!(w.standby.twin.calls(), [format!("standby-start {vmid} 127.0.0.1")]);
    assert_eq!(w.primary.twin.calls(), [format!("protect {vmid}")]);
    let record = w.standby.state.snapshot().standbys[0].clone();
    assert_eq!((record.origin.as_str(), record.source, record.vmid, record.generation), (origin.as_str(), vmid, vmid, w.protection().generation));
    // İkiz kilitlidir: yedek sunucuda kimse onu başlatamaz, silemez.
    assert!(w.guest(&w.standby).lock.is_some() && w.act(&w.standby, Action::Start).await.is_err());
    assert!(w.standby.state.delete_guest(vmid, "admin").is_err());
    // Birincilde sarılmış diske dokunan işlemler reddedilir.
    let (_, v) = api(&w.primary.app, post_json(&format!("/api/v1/guests/{vmid}/backups"), &json!({}))).await;
    assert!(v["error"].as_str().unwrap().contains("hata toleransıyla çalışırken"), "{v}");
    assert!(w.act(&w.primary, Action::Pause).await.unwrap_err().contains("duraklatılamaz"));

    // Bildirimler akarken kimse hakeme gitmez.
    for _ in 0..3 {
        at += DEAD_SECS - 1;
        w.tick(&w.primary, at).await;
        w.tick(&w.standby, at).await;
    }
    assert!(w.protection().state == FtState::Protected && w.witness.state.snapshot().ft_votes.is_empty());

    // Anlık yanıtsızlık ikizi sildirmez: yedek bekler, birincile "yok" denir ama karar hemen verilmez.
    w.standby.twin.0.lock().unwrap().status.insert(vmid, FtStatus { unresponsive: true, ..Default::default() });
    w.tick(&w.standby, at + DEAD_SECS * 3).await;
    assert_eq!(w.standby.state.snapshot().standbys.len(), 1);
    assert!(w.standby.twin.calls().is_empty());
    assert!(!w.standby.state.ft_standby_beat(&origin, vmid, w.protection().generation, true), "yanıt vermeyen ikiz birincile yok sayılır");
    w.standby.twin.set(vmid, Some((true, false)));
    assert!(w.standby.state.ft_standby_beat(&origin, vmid, w.protection().generation, true));
    w.tick(&w.standby, at).await;

    // ---- Beyin bölünmesi: birincil yedeğe ulaşamıyor; iki taraf da hakeme ulaşabiliyor.
    knows(&w.primary, &[("yedek", None), ("hakem", Some(w.witness.port))]);
    at += DEAD_SECS;
    w.tick(&w.primary, at).await;
    let p = w.protection();
    assert!(p.state == FtState::Unprotected && p.error.as_deref().unwrap().contains("haber alınamıyor"), "{p:?}");
    assert_eq!(w.primary.twin.calls(), [format!("drop-secondary {vmid}")]);
    // İkiz de sessizliği görüp hakeme gider; oy birincildedir, ikiz kendini kaldırır.
    w.tick(&w.standby, at).await;
    assert!(w.standby.twin.calls().contains(&format!("standby-stop {vmid}")));
    assert!(w.standby.state.snapshot().standbys.is_empty() && w.standby.state.snapshot().guest(vmid).is_err());
    assert_eq!(w.guest(&w.primary).status, GuestStatus::Running, "konuk yalnızca birincilde");

    // ---- Süre dolunca yeni kurulum numarasıyla yeniden korunur.
    w.connect();
    w.tick(&w.primary, at + RETRY_SECS - 1).await;
    assert_eq!(w.protection().state, FtState::Unprotected);
    at += RETRY_SECS;
    w.protect(at).await;
    assert_eq!(w.standby.state.snapshot().standbys[0].generation, p.generation + 1);
    w.primary.twin.calls();
    w.standby.twin.calls();

    // ---- Birincil sunucu ölür: bildirim kesilir; yedek hakemden oyu alıp devralır.
    w.tick(&w.standby, at + DEAD_SECS - 1).await;
    assert!(w.standby.twin.calls().is_empty(), "süre dolmadan devralınmaz");
    at += DEAD_SECS + 1;
    w.tick(&w.standby, at).await;
    assert_eq!(w.standby.twin.calls(), [format!("take-over {vmid}")]);
    let taken = w.guest(&w.standby);
    assert!(taken.status == GuestStatus::Running && taken.lock.is_none() && !taken.hw.ft, "{taken:?}");
    assert!(taken.tags.iter().any(|t| t == &format!("devralindi-{origin}")) && w.standby.state.snapshot().standbys.is_empty());

    // ---- Ölen birincil geri gelip konuğu başlatmak ister: hakem reddeder; kayıt devredilmiş olarak kalır.
    w.primary.state.mutate(|c| {
        let g = c.guest_mut_for_import(vmid).unwrap();
        (g.status, g.started_at) = (GuestStatus::Stopped, None);
    });
    let refused = w.act(&w.primary, Action::Start).await.unwrap_err();
    assert!(refused.contains("devralındı"), "{refused}");
    let old = w.guest(&w.primary);
    assert!(old.status == GuestStatus::Stopped && !old.onboot, "{old:?}");
    assert!(old.tags.iter().any(|t| t == "ft-devredildi"));
    assert_eq!(w.protection().state, FtState::HandedOver, "kayıt kalır");
    // Bekçi turları ve yeni denemeler reddi kaldırmaz; konuk silinemez de.
    w.tick(&w.primary, at + RETRY_SECS * 3).await;
    assert!(w.act(&w.primary, Action::Start).await.unwrap_err().contains("devralındı"));
    assert!(w.primary.state.delete_guest(vmid, "admin").is_err() && w.primary.twin.calls().iter().all(|c| !c.starts_with("protect")));
    // Yalnızca korumayı bilerek kapatmak kaydı serbest bırakır; devralan konuğa dokunulmaz.
    let state = w.primary.state.clone();
    tokio::task::spawn_blocking(move || state.ft_disable(vmid, "admin")).await.unwrap().unwrap();
    assert!(w.primary.state.snapshot().protections.is_empty() && !w.guest(&w.primary).hw.ft);
    assert_eq!(w.guest(&w.standby).status, GuestStatus::Running);
}

/// Geri gelen birincil, devralmayı kullanıcı bir şey yapmadan **önce** bekçisiyle öğrenir; elle başlatma yine reddedilir.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
pub(super) async fn a_returning_primary_learns_of_the_takeover_and_still_refuses_to_start() {
    let w = world().await;
    let vmid = w.vmid;
    w.primary.state.ft_enable(vmid, "yedek", "hakem", "admin").unwrap();
    w.act(&w.primary, Action::Start).await.unwrap();
    let mut at = now();
    w.protect(at).await;
    at += DEAD_SECS + 1;
    w.tick(&w.standby, at).await;
    assert_eq!(w.guest(&w.standby).status, GuestStatus::Running);
    // Birincil geri döner: QEMU'su ölmüş, kaydında koruma hâlâ "korunuyor".
    w.primary.twin.set(vmid, None);
    w.primary.state.mutate(|c| {
        let g = c.guest_mut_for_import(vmid).unwrap();
        (g.status, g.started_at) = (GuestStatus::Stopped, None);
    });
    for _ in 0..2 {
        at += 2;
        w.tick(&w.primary, at).await;
    }
    assert_eq!(w.protection().state, FtState::HandedOver, "{:?}", w.protection());
    assert!(w.act(&w.primary, Action::Start).await.unwrap_err().contains("devralındı"));
    assert_eq!(w.guest(&w.primary).status, GuestStatus::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
pub(super) async fn without_the_witness_nobody_takes_over_and_stopping_stays_safe() {
    let w = world().await;
    let vmid = w.vmid;
    w.primary.state.ft_enable(vmid, "yedek", "hakem", "admin").unwrap();
    w.act(&w.primary, Action::Start).await.unwrap();
    // Hakem yokken koruma kurulmaz (ikiz hiç açılmaz).
    knows(&w.primary, &[("yedek", Some(w.standby.port)), ("hakem", None)]);
    let mut at = now();
    w.tick(&w.primary, at).await;
    assert!(w.standby.state.snapshot().standbys.is_empty(), "hakemsiz ikiz açılmaz");
    assert!(w.primary.twin.calls().iter().all(|c| !c.starts_with("protect")));

    w.connect();
    at += RETRY_SECS * 2;
    // Önceki başarısız kurulum hakemsiz çözülemediği için konuk bekliyor olabilir: önce çözülür, sonra korunur.
    w.tick(&w.primary, at).await;
    at += RETRY_SECS;
    if w.protection().state != FtState::Protected {
        w.tick(&w.primary, at).await;
    }
    assert_eq!(w.protection().state, FtState::Protected, "{:?}", w.protection());
    w.standby.twin.set(vmid, Some((true, false)));
    w.tick(&w.standby, at).await;
    w.primary.twin.calls();
    w.standby.twin.calls();

    // ---- Ağ tümüyle bölünür: iki taraf da ne birbirine ne hakeme ulaşabiliyor.
    knows(&w.primary, &[("yedek", None), ("hakem", None)]);
    knows(&w.standby, &[("hakem", None)]);
    at += DEAD_SECS + 1;
    w.tick(&w.primary, at).await;
    w.tick(&w.standby, at).await;
    assert_eq!(w.protection().state, FtState::Suspended);
    assert_eq!(w.primary.twin.calls(), [format!("hold {vmid} true")], "birincil duraklar");
    assert!(w.standby.twin.calls().is_empty(), "ikiz hakemsiz devralmaz");
    assert_eq!(w.standby.state.snapshot().standbys.len(), 1);
    // Bu halde konuk durdurulamaz da: ikizin devralmayacağı doğrulanamaz.
    assert!(w.act(&w.primary, Action::Stop).await.unwrap_err().contains("işlem yapılmadı"));
    assert_eq!(w.guest(&w.primary).status, GuestStatus::Running);

    // Hakem önce birincile döner: birincil sürer, ikiz sonra öğrenip kapanır.
    knows(&w.primary, &[("yedek", None), ("hakem", Some(w.witness.port))]);
    at += 1;
    w.tick(&w.primary, at).await;
    assert_eq!(w.protection().state, FtState::Unprotected);
    assert!(w.primary.twin.calls().contains(&format!("drop-secondary {vmid}")));
    knows(&w.standby, &[("hakem", Some(w.witness.port))]);
    w.tick(&w.standby, at).await;
    assert!(w.standby.state.snapshot().standbys.is_empty() && !w.standby.twin.calls().contains(&format!("take-over {vmid}")));

    // ---- Korunan konuk olağan yoldan durdurulur: önce oy alınır, ikiz kaldırılır; sonradan devralamaz.
    w.connect();
    at += RETRY_SECS;
    w.protect(at).await;
    let generation = w.protection().generation;
    w.standby.twin.calls();
    w.act(&w.primary, Action::Stop).await.unwrap();
    let key = redox_core::ft_key(&redox_server::state::mirrors::origin_name(&w.primary.state.snapshot()), vmid, generation);
    assert_eq!(w.witness.state.ft_winner(&key), Some(FtSide::Primary));
    assert!(w.standby.state.snapshot().standbys.is_empty() && w.standby.twin.calls().contains(&format!("standby-stop {vmid}")));
    assert_eq!(w.protection().state, FtState::Waiting);
    // Hakem, geç gelen ikize hayır der.
    let (_, v) = api(&w.witness.app, post_json("/api/v1/ft/arbitrate", &json!({ "key": key, "side": "secondary" }))).await;
    assert_eq!(v["winner"], "primary");
    // Kapalı ve korunan konuk silinemez; koruma kapatılınca silinebilir.
    assert!(w.primary.state.delete_guest(vmid, "admin").unwrap_err().to_string().contains("hata toleransı"));
    let off = request(Method::DELETE, &format!("/api/v1/ft/protections/{vmid}")).body(Body::empty()).unwrap();
    assert_eq!(api(&w.primary.app, off).await.0, StatusCode::NO_CONTENT);
    assert!(!w.guest(&w.primary).hw.ft && w.primary.state.snapshot().protections.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
pub(super) async fn the_cluster_page_turns_fault_tolerance_on_and_off() {
    let w = world().await;
    let vmid = w.vmid;
    // Eş listesi okununca (iki eş) form görünür.
    let state = w.primary.state.clone();
    tokio::task::spawn_blocking(move || state.peers().poll(now())).await.unwrap();
    let shown = page(&w.primary.app, "/cluster?tab=koruma").await;
    assert!(shown.contains("Hata toleransı") && shown.contains("/cluster/ft") && shown.contains(&format!("option value=\"{vmid}\"")), "{shown}");
    for bad in [format!("op=on&vmid={vmid}&peer=yedek&witness=yedek"), "op=on&vmid=abc&peer=yedek&witness=hakem".to_string(), format!("op=bilinmez&vmid={vmid}")] {
        assert!(post_ok(&w.primary.app, "/cluster/ft", &bad).await.contains("error="), "{bad}");
    }
    let (status, _, _) = send(&w.primary.app, form_body(as_viewer(Method::POST, "/cluster/ft"), &format!("op=off&vmid={vmid}"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(post_ok(&w.primary.app, "/cluster/ft", &format!("op=on&vmid={vmid}&peer=yedek&witness=hakem")).await.contains("notice="));
    w.act(&w.primary, Action::Start).await.unwrap();
    w.protect(now()).await;
    let shown = page(&w.primary.app, "/cluster?tab=koruma").await;
    assert!(shown.contains("korunuyor") && shown.contains("value=\"off\""), "{shown}");
    let twin = page(&w.standby.app, "/cluster?tab=koruma").await;
    assert!(twin.contains("Buradaki ikiz"), "{twin}");
    assert!(post_ok(&w.primary.app, "/cluster/ft", &format!("op=off&vmid={vmid}")).await.contains("notice="));
    assert!(w.primary.state.snapshot().protections.is_empty() && w.standby.state.snapshot().standbys.is_empty());
    assert_eq!(w.guest(&w.primary).status, GuestStatus::Running, "kapatmak konuğu durdurmaz");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
pub(super) async fn the_standby_refuses_what_it_cannot_run_safely() {
    let w = world().await;
    let vmid = w.vmid;
    w.primary.state.ft_enable(vmid, "yedek", "hakem", "admin").unwrap();
    w.act(&w.primary, Action::Start).await.unwrap();
    // Yedek hakemi tanımıyor.
    knows(&w.standby, &[]);
    let mut at = now();
    w.tick(&w.primary, at).await;
    let p = w.protection();
    assert!(p.state == FtState::Unprotected && p.error.as_deref().unwrap().contains("hakem"), "{p:?}");
    // Yedekte kimlik dolu: oradaki konuğa dokunulmaz.
    w.connect();
    let mut other = w.guest(&w.primary);
    (other.status, other.name) = (GuestStatus::Stopped, "baskasinin".to_string());
    w.standby.state.mutate(|c| c.guests.insert(0, other));
    at += RETRY_SECS;
    w.tick(&w.primary, at).await;
    assert!(w.protection().error.unwrap().contains("kimliği dolu"));
    assert_eq!(w.guest(&w.standby).name, "baskasinin");
    assert!(w.standby.twin.calls().is_empty());
    // Kurulum yarıda kalırsa ikiz de temizlenir.
    w.standby.state.mutate(|c| c.guests.retain(|g| g.vmid != vmid));
    w.primary.twin.0.lock().unwrap().protect_error = Some("disk eşitlenemedi".to_string());
    at += RETRY_SECS;
    w.tick(&w.primary, at).await;
    assert!(w.protection().error.unwrap().contains("disk eşitlenemedi"));
    assert!(w.standby.state.snapshot().standbys.is_empty() && w.standby.state.snapshot().guest(vmid).is_err(), "yarım ikiz bırakılmaz");
}
