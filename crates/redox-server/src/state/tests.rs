use super::*;
use crate::auth::{Cost, UserFile};
use redox_core::{GuestKind, GuestStatus, Probe, SimDriver};
use std::sync::atomic::{AtomicBool, Ordering};

fn auth() -> Auth {
    Auth::new(UserFile::default(), None, false, Cost::Test)
}

fn spec(name: &str, kind: GuestKind) -> GuestSpec {
    GuestSpec {
        vmid: None,
        name: name.into(),
        kind,
        node: "pve-ist-01".into(),
        cores: 1,
        memory_mb: 512,
        disk_gb: 10,
        storage: "local-zfs".into(),
        os: "linux".into(),
        bridge: "nat".into(),
        onboot: false,
        iso: None,
        hw: Default::default(),
    }
}

/// Gerçek sürücü gibi davranan, ayarlanabilir sahte sürücü.
#[derive(Default, Clone)]
struct FakeReal {
    fail_create: bool,
    fail_destroy: bool,
    reported: Arc<Mutex<Option<Probe>>>,
    destroyed: Arc<AtomicBool>,
}

impl Hypervisor for FakeReal {
    fn name(&self) -> &'static str {
        "sahte"
    }
    fn is_real(&self) -> bool {
        true
    }
    fn supports(&self, kind: GuestKind) -> bool {
        kind == GuestKind::Vm
    }
    fn isos(&self) -> Vec<String> {
        vec!["debian.iso".into()]
    }
    fn bridges(&self) -> Vec<String> {
        vec!["vmbr0".into()]
    }
    fn templates(&self) -> Vec<String> {
        vec!["alpine-3.20".into()]
    }
    fn guest_ip(&self, g: &Guest) -> Option<String> {
        Some(format!("10.0.0.{}", g.vmid % 250))
    }
    fn configure(&self, _: &Guest) -> Result<(), String> {
        if self.fail_create { Err("yapılandırma yazılamadı".into()) } else { Ok(()) }
    }
    fn create(&self, _: &Guest) -> Result<(), String> {
        if self.fail_create { Err("disk yazılamadı".into()) } else { Ok(()) }
    }
    fn destroy(&self, _: &Guest) -> Result<(), String> {
        if self.fail_destroy {
            return Err("dosya kilitli".into());
        }
        self.destroyed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
        Ok(())
    }
    fn probe(&self, _: &Guest) -> Option<Probe> {
        self.reported.lock().unwrap().clone()
    }
}

fn real(fake: FakeReal) -> (AppState, FakeReal) {
    let state = AppState::new(Cluster::demo(1000), Box::new(fake.clone()), Duration::ZERO, None, auth());
    (state, fake)
}

#[test]
fn recover_fails_stale_tasks_and_unlocks_guests() {
    let mut c = Cluster::demo(1000);
    let t = c.begin_action(103, Action::Start, "u", 1000).unwrap();
    assert_eq!(recover(&mut c, 2000), 1);
    assert_eq!(c.task(t).unwrap().status, TaskStatus::Failed);
    let g = c.guest(103).unwrap();
    assert_eq!((g.status, g.lock), (GuestStatus::Stopped, None));
    assert_eq!(recover(&mut c, 2000), 0, "ikinci kez yapılacak iş kalmaz");
}

#[test]
fn state_file_round_trip_and_errors() {
    let dir = std::env::temp_dir().join(format!("redox-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.json");
    assert_eq!(load_state(&path), Ok(None), "dosya yoksa hata değil");

    let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::default()), Duration::ZERO, Some(path.clone()), auth());
    state.run_action(103, Action::Start, "admin").unwrap();
    let loaded = load_state(&path).unwrap().expect("değişiklikten sonra dosya yazılmalı");
    assert_eq!(loaded, state.snapshot());
    assert_eq!(loaded.guest(103).unwrap().status, GuestStatus::Running);

    std::fs::write(&path, b"{ bozuk").unwrap();
    assert!(load_state(&path).is_err(), "bozuk dosya sessizce yok sayılmamalı");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn driver_failure_marks_task_failed() {
    let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::failing_on(103)), Duration::ZERO, None, auth());
    let t = state.run_action(103, Action::Start, "admin").unwrap();
    let c = state.snapshot();
    assert_eq!(c.task(t).unwrap().status, TaskStatus::Failed);
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);
}

#[test]
fn create_checks_driver_support_and_iso() {
    let (state, _) = real(FakeReal::default());
    let before = state.snapshot();
    let err = state.create_guest(&spec("kutu", GuestKind::Ct), "admin").unwrap_err();
    assert!(matches!(&err, Error::InvalidSpec(m) if m.contains("konteyner türünü desteklemiyor")), "{err}");
    let missing = GuestSpec { iso: Some("yok.iso".into()), ..spec("vm", GuestKind::Vm) };
    assert_eq!(state.create_guest(&missing, "admin"), Err(Error::InvalidSpec("ISO bulunamadı: yok.iso".into())));
    assert_eq!(state.snapshot(), before);

    let ok = GuestSpec { iso: Some("debian.iso".into()), ..spec("vm", GuestKind::Vm) };
    let id = state.create_guest(&ok, "admin").unwrap();
    assert_eq!(state.snapshot().guest(id).unwrap().iso.as_deref(), Some("debian.iso"));
}

#[test]
fn failed_disk_creation_is_rolled_back() {
    let (state, _) = real(FakeReal { fail_create: true, ..Default::default() });
    let storages = state.snapshot().storages;
    assert_eq!(state.create_guest(&spec("vm", GuestKind::Vm), "admin"), Err(Error::Driver("disk yazılamadı".into())));
    let c = state.snapshot();
    assert_eq!(c.guests.len(), 8, "yarım kalan konuk listede kalmamalı");
    assert_eq!(c.storages, storages);
    assert_eq!((c.tasks[0].status, c.tasks[0].message.as_deref()), (TaskStatus::Failed, Some("disk yazılamadı")));
}

#[test]
fn delete_removes_driver_resources_only_when_allowed() {
    let (state, fake) = real(FakeReal::default());
    assert_eq!(state.delete_guest(100, "admin"), Err(Error::NotStopped(100)));
    assert!(!fake.destroyed.load(Ordering::SeqCst), "çalışan konuğun diski silinmemeli");
    state.delete_guest(103, "admin").unwrap();
    assert!(fake.destroyed.load(Ordering::SeqCst));
    assert!(state.snapshot().guest(103).is_err());

    let (state, _) = real(FakeReal { fail_destroy: true, ..Default::default() });
    assert_eq!(state.delete_guest(103, "admin"), Err(Error::Driver("dosya kilitli".into())));
    assert!(state.snapshot().guest(103).is_ok(), "sürücü silemediyse kayıt da kalır");
}

#[test]
fn refresh_applies_probes_and_records_bounded_history() {
    let (state, fake) = real(FakeReal::default());
    *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Running, cpu_pct: Some(42), mem_used_mb: Some(256) });
    let host = HostSample { cpu_pct: 17, mem_used_mb: 4096, mem_total_mb: 8192, uptime_secs: 99 };

    state.refresh(Some(&host), Some((30, 100)));
    let c = state.snapshot();
    assert!(c.guests.iter().all(|g| g.status == GuestStatus::Running && g.cpu_pct == 42));
    let n = &c.nodes[0];
    assert_eq!((n.cpu_pct, n.mem_used_mb, n.mem_total_mb, n.uptime_secs), (17, 4096, 8192, 99));

    for _ in 0..HISTORY_LEN + 30 {
        state.refresh(Some(&host), None);
    }
    let h = state.history();
    assert_eq!(h["node:cpu"].len(), HISTORY_LEN, "geçmiş sınırlı kalmalı");
    assert_eq!(h["node:cpu"].last(), Some(&17.0));
    assert_eq!(h["node:mem"].last(), Some(&50.0));
    assert_eq!(h["guest:103:cpu"].last(), Some(&42.0));

    state.delete_guest(202, "admin").unwrap_err(); // çalışıyor göründüğü için silinemez
    *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None });
    state.refresh(None, None);
    state.delete_guest(202, "admin").unwrap();
    state.refresh(None, None);
    assert!(!state.history().contains_key("guest:202:cpu"), "silinen konuğun geçmişi atılır");
}

#[test]
fn real_driver_validates_network_settings() {
    let (state, _) = real(FakeReal::default());
    let bad = GuestSpec { bridge: "br-yok".into(), ..spec("vm", GuestKind::Vm) };
    assert!(matches!(state.create_guest(&bad, "admin"), Err(Error::InvalidSpec(m)) if m.contains("adında bir köprü yok") && m.contains("vmbr0")));
    let id = state.create_guest(&GuestSpec { bridge: "vmbr0".into(), ..spec("vm", GuestKind::Vm) }, "admin").unwrap();
    assert_eq!(state.snapshot().guest(id).unwrap().bridge, "vmbr0");

    // Köprüdeki konukta port yönlendirme anlamsızdır; NAT'a geçince kabul edilir.
    let forwards = Some(vec![redox_core::PortForward { host_port: 8080, guest_port: 80 }]);
    let err = state.update_guest(id, &GuestPatch { forwards: forwards.clone(), ..Default::default() }, "admin").unwrap_err();
    assert!(matches!(&err, Error::InvalidSpec(m) if m.contains("yalnızca NAT")), "{err}");
    state.update_guest(id, &GuestPatch { bridge: Some("nat".into()), forwards, ..Default::default() }, "admin").unwrap();
    let err = state.update_guest(id, &GuestPatch { bridge: Some("vmbr0".into()), ..Default::default() }, "admin").unwrap_err();
    assert!(matches!(&err, Error::InvalidSpec(m) if m.contains("yalnızca NAT")), "yönlendirme varken köprüye geçilemez: {err}");
    assert!(matches!(state.update_guest(id, &GuestPatch { bridge: Some("br-yok".into()), ..Default::default() }, "admin"), Err(Error::InvalidSpec(_))));
}

#[test]
fn refresh_tracks_addresses_of_running_guests_only() {
    let (state, fake) = real(FakeReal::default());
    *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Running, cpu_pct: None, mem_used_mb: None });
    state.refresh(None, None);
    assert_eq!(state.snapshot().guest(103).unwrap().ip.as_deref(), Some("10.0.0.103"));
    *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None });
    state.refresh(None, None);
    assert_eq!(state.snapshot().guest(103).unwrap().ip, None, "duran konuğun eski adresi silinir");
}

#[test]
fn driver_reconfigure_failure_is_reported() {
    let (state, _) = real(FakeReal { fail_create: true, ..Default::default() });
    let err = state.update_guest(103, &GuestPatch { cores: Some(2), ..Default::default() }, "admin").unwrap_err();
    assert_eq!(err, Error::Driver("yapılandırma yazılamadı".into()));
}

#[test]
fn exec_needs_a_running_guest_and_hides_the_command() {
    let (state, _) = real(FakeReal::default());
    assert!(matches!(state.exec(103, "id", "admin"), Err(Error::InvalidSpec(m)) if m.contains("çalışmıyor")));
    assert_eq!(state.exec(999, "id", "admin"), Err(Error::GuestNotFound(999)));
    // Sahte sürücü komut çalıştırmayı desteklemez: hata sürücüden gelir ve görev günlüğüne düşer.
    assert!(matches!(state.exec(200, "cat /root/gizli-parola", "ayse"), Err(Error::Driver(_))));
    let t = state.snapshot().tasks[0].clone();
    assert_eq!((t.command.as_str(), t.user.as_str(), t.status), ("ct exec 200", "ayse", TaskStatus::Failed));
    assert!(!format!("{:?}", state.snapshot().tasks).contains("gizli-parola"), "komut metni günlüğe yazılmaz");
}

#[test]
fn tcp_probe_needs_data_from_the_service_not_just_a_connection() {
    use std::io::{Read, Write};
    let serve = |handler: fn(std::net::TcpStream)| {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                handler(stream);
            }
        });
        port
    };
    let quick = Duration::from_millis(100);
    // SSH gibi: bağlanınca karşılama gönderir.
    let banner = serve(|mut s| {
        let _ = s.write_all(b"SSH-2.0-sinama\r\n");
    });
    probe_tcp(banner, quick).unwrap();
    // HTTP gibi: istek gelince yanıt verir.
    let http = serve(|mut s| {
        let mut buf = [0u8; 64];
        if s.read(&mut buf).is_ok_and(|n| n > 0) {
            let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
        }
    });
    probe_tcp(http, quick).unwrap();
    // Bağlantıyı kabul edip hemen kapatan (konuk kapıyı reddetti) ve hiç dinlenmeyen kapı.
    let closing = serve(drop);
    assert!(probe_tcp(closing, quick).unwrap_err().contains("veri gelmeden kapandı"));
    let unused = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    assert!(probe_tcp(unused, quick).unwrap_err().contains("bağlanılamadı"));
}

#[test]
fn refresh_is_a_no_op_for_the_simulator() {
    let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::default()), Duration::ZERO, None, auth());
    let before = state.snapshot();
    state.refresh(Some(&HostSample { cpu_pct: 99, mem_used_mb: 1, mem_total_mb: 2, uptime_secs: 3 }), Some((1, 2)));
    assert_eq!(state.snapshot(), before);
    assert!(state.history().is_empty());
}

#[test]
fn api_rate_limit_counts_per_user_and_minute() {
    let st = AppState::new(Cluster::demo(now()), Box::new(SimDriver::default()), Duration::ZERO, None, auth());
    let t = 1_700_000_040 + 40; // dakikanın 40. saniyesi
    assert!((0..1000).all(|_| st.api_allowed("ayse", t).is_ok()), "kapalıyken sınır yok");
    st.set_api_rate(3);
    assert!((0..3).all(|_| st.api_allowed("ayse", t).is_ok()));
    assert_eq!(st.api_allowed("ayse", t + 5), Err(15), "dakika dolana kadar beklenir");
    assert!(st.api_allowed("mehmet", t).is_ok(), "başka kullanıcı etkilenmez");
    assert!(st.api_allowed("ayse", t + 20).is_ok(), "yeni dakikada sayaç sıfırlanır");
}

#[test]
fn a_filling_backup_storage_is_forecast_and_alerted() {
    let mut c = Cluster::demo(now());
    assert!(c.backup_forecast(now()).is_none(), "yedek yokken tahmin yok");
    let (storage, free_gb) = c.storages.iter().find(|s| s.content.iter().any(|k| k == "yedek")).map(|s| (s.id.clone(), s.free_gb())).expect("yedek alanı");
    // Beş günde, boş yerin tamamı kadar veri: bu hızla beş günde dolar.
    let per_backup_mb = free_gb * 1024 / 5;
    for day in 1..=5u64 {
        c.add_backup(redox_core::Backup { id: format!("vm-103-{day}"), vmid: 103, guest_name: "x".into(), created: now() - day * 86_400 + 60, size_mb: per_backup_mb, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    }
    let (name, f) = c.backup_forecast(now()).unwrap();
    assert_eq!((name.as_str(), f.days_left), (storage.as_str(), Some(5)));
    let alert = crate::alerts::evaluate(&c).into_iter().find(|a| a.key == "backup-forecast").expect("uyarı");
    assert!(alert.severity == crate::alerts::Severity::Warning && alert.message.contains("5 gün içinde dolacak"), "{}", alert.message);
}
