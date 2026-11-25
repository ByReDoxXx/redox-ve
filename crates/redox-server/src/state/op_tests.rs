use super::*;
use crate::auth::{Cost, UserFile};
use redox_core::{GuestStatus, PortForward, SimDriver};
use std::sync::atomic::{AtomicBool, Ordering};

/// Disk işlemleri istenince başarısız olabilen sürücü.
#[derive(Default, Clone)]
struct Flaky {
    fail: Arc<AtomicBool>,
}

impl Flaky {
    fn outcome<T: Default>(&self) -> Result<T, String> {
        if self.fail.load(Ordering::SeqCst) { Err("disk hatası".into()) } else { Ok(T::default()) }
    }
}

impl Hypervisor for Flaky {
    fn name(&self) -> &'static str {
        "oynak"
    }
    fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
        Ok(())
    }
    fn snapshot(&self, _: &Guest, _: SnapshotOp<'_>) -> Result<(), String> {
        self.outcome()
    }
    fn resize_disk(&self, _: &Guest, _: u64) -> Result<(), String> {
        self.outcome()
    }
    fn clone_disk(&self, _: &Guest, _: &Guest) -> Result<(), String> {
        self.outcome()
    }
    fn backup(&self, _: &Guest, _: &str) -> Result<redox_core::BackupSize, String> {
        self.outcome().map(|()| redox_core::BackupSize::full(42))
    }
    fn restore(&self, _: &Guest, _: &str, _: &[u8]) -> Result<(), String> {
        self.outcome()
    }
    fn delete_backup(&self, _: &str) -> Result<(), String> {
        self.outcome()
    }
}

fn setup() -> (AppState, Flaky) {
    let driver = Flaky::default();
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    (AppState::new(Cluster::demo(1000), Box::new(driver.clone()), Duration::ZERO, None, auth), driver)
}

fn last_task(state: &AppState) -> (String, TaskStatus, Option<String>) {
    let t = state.snapshot().tasks[0].clone();
    (t.command, t.status, t.message)
}

#[test]
fn snapshot_lifecycle_and_failures() {
    let (state, driver) = setup();
    assert!(matches!(state.snapshot_create(100, "x", "u"), Err(Error::RequiresStopped { .. })), "çalışan konukta alınmaz");
    assert!(matches!(state.snapshot_create(103, "Kötü", "u"), Err(Error::InvalidSpec(_))));

    state.snapshot_create(103, "temiz", "ayse").unwrap();
    assert_eq!(state.snapshot().guest(103).unwrap().snapshots[0].name, "temiz");
    assert_eq!(last_task(&state), ("vm snapshot 103 temiz".into(), TaskStatus::Ok, None));
    assert_eq!(state.snapshot_create(103, "temiz", "u"), Err(Error::SnapshotExists("temiz".into())));
    assert_eq!(state.snapshot_rollback(103, "yok", "u"), Err(Error::SnapshotNotFound("yok".into())));
    state.snapshot_rollback(103, "temiz", "u").unwrap();

    // Sürücü başarısız olursa kayıt değişmez, görev hatayı taşır, kilit açılır.
    driver.fail.store(true, Ordering::SeqCst);
    state.snapshot_create(103, "ikinci", "u").unwrap();
    state.snapshot_delete(103, "temiz", "u").unwrap();
    let g = state.snapshot().guest(103).unwrap().clone();
    assert_eq!((g.snapshots.len(), g.lock), (1, None));
    assert_eq!(last_task(&state), ("vm delsnapshot 103 temiz".into(), TaskStatus::Failed, Some("disk hatası".into())));

    driver.fail.store(false, Ordering::SeqCst);
    state.snapshot_delete(103, "temiz", "u").unwrap();
    assert!(state.snapshot().guest(103).unwrap().snapshots.is_empty());
}

#[test]
fn resize_applies_only_on_success() {
    let (state, driver) = setup();
    assert!(matches!(state.resize_disk(103, 100, "u"), Err(Error::InvalidSpec(_))));
    driver.fail.store(true, Ordering::SeqCst);
    state.resize_disk(103, 300, "u").unwrap();
    assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 200, "başarısız büyütme kaydı değiştirmez");
    driver.fail.store(false, Ordering::SeqCst);
    state.resize_disk(103, 300, "u").unwrap();
    assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 300);
    assert_eq!(last_task(&state).0, "vm resize 103 300G");
}

#[test]
fn backup_restore_and_delete() {
    let (state, driver) = setup();
    assert!(matches!(state.backup_create(100, "u"), Err(Error::RequiresStopped { .. })));
    state.snapshot_create(103, "s", "u").unwrap();

    let id = state.backup_create(103, "ayse").unwrap();
    assert!(id.starts_with("vm-103-"), "{id}");
    let c = state.snapshot();
    let b = c.backup(&id).unwrap();
    assert_eq!((b.vmid, b.guest_name.as_str(), b.size_mb), (103, "win-build", 42));
    assert!(matches!(state.backup_create(103, "u"), Err(Error::InvalidSpec(_))), "aynı saniyede ikinci yedek kimliği çakışır");

    assert_eq!(state.backup_restore("yok", "u"), Err(Error::BackupNotFound("yok".into())));
    state.backup_restore(&id, "u").unwrap();
    assert!(state.snapshot().guest(103).unwrap().snapshots.is_empty(), "geri yükleme anlık görüntü listesini sıfırlar");

    driver.fail.store(true, Ordering::SeqCst);
    assert_eq!(state.backup_delete(&id, "u"), Err(Error::Driver("disk hatası".into())));
    assert!(state.snapshot().backup(&id).is_ok(), "dosya silinemediyse kayıt da kalır");
    driver.fail.store(false, Ordering::SeqCst);
    state.backup_delete(&id, "u").unwrap();
    assert!(state.snapshot().backups.is_empty());
    assert_eq!(last_task(&state).0, format!("backup delete {id}"));
    assert_eq!(state.backup_delete(&id, "u"), Err(Error::BackupNotFound(id)));
}

#[test]
fn verification_outcome_is_logged() {
    let (state, _) = setup();
    assert_eq!(state.backup_verify("yok", "u"), Err(Error::BackupNotFound("yok".into())));
    let id = state.backup_create(103, "ayse").unwrap();
    state.backup_verify(&id, "ayse").unwrap();
    let (command, status, message) = last_task(&state);
    // Bu sürücü doğrulamayı desteklemez: sessizce "sağlam" denmez, başarısız görev olarak yazılır.
    assert_eq!((command, status), (format!("backup verify {id}"), TaskStatus::Failed));
    assert!(message.unwrap().contains("doğrulama yok"));
}

#[test]
fn scheduler_takes_daily_backups_and_prunes_old_ones() {
    let (state, driver) = setup();
    let schedule = GuestPatch { backup_time: Some("03:30".into()), backup_keep: Some(2), ..Default::default() };
    state.update_guest(103, &schedule, "admin").unwrap();

    assert_eq!(state.scheduler_tick("2026-11-24", 200), 0, "saati gelmedi");
    assert_eq!(state.scheduler_tick("2026-11-24", 215), 1);
    assert_eq!(state.scheduler_tick("2026-11-24", 216), 0, "aynı gün ikinci kez alınmaz");
    let c = state.snapshot();
    assert_eq!((c.backups.len(), c.backups[0].auto, c.backups[0].vmid), (1, true, 103));
    assert!(c.tasks.iter().any(|t| t.command == "vm backup 103" && t.user == SCHEDULER));

    // Elle alınan yedek saklama sayısına dahil değildir.
    std::thread::sleep(Duration::from_millis(1100));
    let manual = state.backup_create(103, "ayse").unwrap();
    for day in ["2026-11-25", "2026-11-26", "2026-11-27"] {
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(state.scheduler_tick(day, 300), 1, "{day}");
    }
    state.scheduler_tick("2026-11-27", 301);
    let c = state.snapshot();
    let autos: Vec<&str> = c.backups.iter().filter(|b| b.auto).map(|b| b.id.as_str()).collect();
    assert_eq!(autos.len(), 2, "yalnızca en yeni iki otomatik yedek kalır: {autos:?}");
    assert!(c.backup(&manual).is_ok(), "elle alınan yedek silinmez");
    assert!(c.tasks.iter().any(|t| t.command.starts_with("backup delete vm-103-") && t.user == SCHEDULER));

    // Çalışan konuk (bu sürücüde canlı yedek yok): hata günlüğe düşer, gün içinde yinelenmez.
    state.update_guest(100, &schedule, "admin").unwrap();
    assert_eq!(state.scheduler_tick("2026-11-28", 300), 2);
    let c = state.snapshot();
    let failed = c.tasks.iter().find(|t| t.command == "vm backup 100").expect("başarısız deneme kaydedilmeli");
    assert_eq!((failed.status, failed.user.as_str()), (TaskStatus::Failed, SCHEDULER));
    assert!(failed.message.as_deref().unwrap().contains("önce konuğu kapatın"));
    assert_eq!(state.scheduler_tick("2026-11-28", 400), 0);

    // Sürücü hatasında da yeniden deneme fırtınası olmaz.
    driver.fail.store(true, Ordering::SeqCst);
    assert_eq!(state.scheduler_tick("2026-11-29", 300), 2);
    assert_eq!(state.scheduler_tick("2026-11-29", 301), 0);
}

#[test]
fn local_time_helper_is_well_formed() {
    let (day, minute) = local_day_minute();
    assert_eq!(day.len(), 10);
    assert!(day.as_bytes()[4] == b'-' && day.as_bytes()[7] == b'-' && minute < 1440);
}

#[test]
fn clone_creates_or_cleans_up() {
    let (state, driver) = setup();
    assert!(matches!(state.clone_guest(100, "kopya", "u"), Err(Error::RequiresStopped { .. })));
    assert_eq!(state.clone_guest(103, "Kötü", "u"), Err(Error::InvalidName("Kötü".into())));
    assert_eq!(state.snapshot().guest(103).unwrap().lock, None, "reddedilen klon kaynağı kilitli bırakmaz");

    let id = state.clone_guest(103, "win-build-2", "u").unwrap();
    let c = state.snapshot();
    assert_eq!((c.guest(id).unwrap().name.as_str(), c.guest(id).unwrap().lock, c.guest(103).unwrap().lock), ("win-build-2", None, None));

    driver.fail.store(true, Ordering::SeqCst);
    let storages = state.snapshot().storages;
    let failed = state.clone_guest(103, "ucuncu", "u").unwrap();
    let c = state.snapshot();
    assert!(c.guest(failed).is_err(), "disk kopyalanamadıysa hedef kayıt silinir");
    assert_eq!(c.storages, storages);
    assert_eq!(c.tasks[0].status, TaskStatus::Failed);
}

#[test]
fn update_and_console_delegation() {
    let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
    let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::default()), Duration::ZERO, None, auth);
    let patch = GuestPatch { tags: Some(vec!["ci".into()]), forwards: Some(vec![PortForward { host_port: 8080, guest_port: 80 }]), ..Default::default() };
    state.update_guest(103, &patch, "u").unwrap();
    let g = state.snapshot().guest(103).unwrap().clone();
    assert_eq!((g.tags, g.forwards.len(), g.status), (vec!["ci".to_string()], 1, GuestStatus::Stopped));
    // Simülasyon sürücüsünde konsol yoktur; hata sürücü hatası olarak döner.
    assert!(matches!(state.screenshot(103), Err(Error::Driver(_))));
    assert!(matches!(state.send_keys(103, &[]), Err(Error::Driver(_))));
    assert_eq!(state.screenshot(999), Err(Error::GuestNotFound(999)));
}
