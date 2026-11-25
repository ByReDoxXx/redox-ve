//! Uyarılar: kümenin durumundan türetilen koşullar (dolan depolama, düşen düğüm) ve
//! olaylar (başarısız görev, kendiliğinden kapanan konuk). İstenirse bir webhook'a gönderilir.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use redox_core::{Cluster, GuestStatus, NodeStatus, TaskId, TaskStatus, VmId};
use serde_json::json;

pub const STORAGE_WARN_PCT: u8 = 80;
pub const STORAGE_CRIT_PCT: u8 = 90;
/// Kaynak uyarıları: düğümün belleği ve işlemcisi, konuğun işlemcisi.
pub const MEM_WARN_PCT: u8 = 90;
pub const MEM_CRIT_PCT: u8 = 97;
pub const CPU_WARN_PCT: u8 = 90;
pub const GUEST_CPU_WARN_PCT: u8 = 95;
/// Kaynak uyarısı ancak koşul bu kadar saniye kesintisiz sürerse bildirilir (anlık sıçramalar bildirim üretmesin).
pub const LOAD_HOLD_SECS: u64 = 300;
/// Bir konuk bu kadar saniye içinde biten bir görevin ardından durduysa, duruş beklenen sayılır.
const EXPECTED_STOP_WINDOW: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Critical,
}

/// Sürmekte olan bir koşul.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// Koşulu tanımlayan kararlı anahtar (aynı uyarı yinelenmesin diye).
    pub key: String,
    pub severity: Severity,
    pub message: String,
    /// Bildirilmeden önce koşulun kesintisiz sürmesi gereken süre (saniye); 0 ise hemen bildirilir.
    pub hold_secs: u64,
}

/// Kümenin şu anki durumundaki koşullar. Saf işlev: sayfalar ve bildirimler aynı listeyi kullanır.
pub fn evaluate(c: &Cluster) -> Vec<Alert> {
    let mut alerts: Vec<Alert> = Vec::new();
    let mut add = |key: String, severity, message: String| alerts.push(Alert { key, severity, message, hold_secs: 0 });
    for n in c.nodes.iter().filter(|n| n.status == NodeStatus::Offline) {
        add(format!("node:{}", n.id), Severity::Critical, format!("{} düğümü çevrimdışı", n.id));
    }
    if c.nodes.len() > 1 && !c.summary().quorate {
        add("quorum".to_string(), Severity::Critical, "Kümede çoğunluk yok".to_string());
    }
    for s in &c.storages {
        let pct = s.used_pct();
        let place = s.node.as_deref().map(|n| format!(" ({n})")).unwrap_or_default();
        let key = format!("storage:{}:{}", s.id, s.node.as_deref().unwrap_or(""));
        if pct >= STORAGE_CRIT_PCT {
            add(key, Severity::Critical, format!("{}{place} deposu %{pct} dolu", s.id));
        } else if pct >= STORAGE_WARN_PCT {
            add(key, Severity::Warning, format!("{}{place} deposu %{pct} dolu", s.id));
        }
    }
    // Kaynak kullanımı: sayfada hemen görünür; bildirim için koşulun sürmesi beklenir.
    let mut load = |key: String, severity, message: String| alerts.push(Alert { key, severity, message, hold_secs: LOAD_HOLD_SECS });
    for n in c.nodes.iter().filter(|n| n.status == NodeStatus::Online) {
        let mem = redox_core::fmt::pct(n.mem_used_mb, n.mem_total_mb);
        if mem >= MEM_CRIT_PCT {
            load(format!("mem:{}", n.id), Severity::Critical, format!("{} düğümünün belleği %{mem} dolu", n.id));
        } else if mem >= MEM_WARN_PCT {
            load(format!("mem:{}", n.id), Severity::Warning, format!("{} düğümünün belleği %{mem} dolu", n.id));
        }
        if n.cpu_pct >= CPU_WARN_PCT {
            load(format!("cpu:{}", n.id), Severity::Warning, format!("{} düğümünün işlemcisi %{} kullanımda", n.id, n.cpu_pct));
        }
    }
    for g in c.guests.iter().filter(|g| g.status == GuestStatus::Running && g.cpu_pct >= GUEST_CPU_WARN_PCT) {
        load(format!("guest-cpu:{}", g.vmid), Severity::Warning, format!("{} ({}) konuğunun işlemcisi %{} kullanımda", g.vmid, g.name, g.cpu_pct));
    }
    alerts
}

/// Bildirilecek değişiklik.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Raised(Alert),
    Cleared(Alert),
    TaskFailed { command: String, user: String, message: String },
    GuestStopped { vmid: VmId, name: String },
}

impl Event {
    pub fn text(&self, cluster: &str) -> String {
        match self {
            Event::Raised(a) => {
                let level = if a.severity == Severity::Critical { "KRİTİK" } else { "UYARI" };
                format!("[{cluster}] {level}: {}", a.message)
            }
            Event::Cleared(a) => format!("[{cluster}] DÜZELDİ: {}", a.message),
            Event::TaskFailed { command, user, message } => format!("[{cluster}] GÖREV BAŞARISIZ: redox {command} ({user}): {message}"),
            Event::GuestStopped { vmid, name } => format!("[{cluster}] BEKLENMEDİK KAPANMA: {vmid} ({name}) kendiliğinden durdu"),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Event::Raised(_) => "raised",
            Event::Cleared(_) => "cleared",
            Event::TaskFailed { .. } => "task_failed",
            Event::GuestStopped { .. } => "guest_stopped",
        }
    }
}

#[derive(Default)]
struct Seen {
    /// İlk turda dolar: sunucu açılırken var olan koşullar ve eski görevler bildirilmez.
    started: bool,
    active: BTreeMap<String, Alert>,
    last_task: TaskId,
    /// `last_task` sonrasında bildirilmiş başarısız görevler (yinelenmesin diye).
    reported: Vec<TaskId>,
    statuses: HashMap<VmId, GuestStatus>,
    /// Süresi dolmayı bekleyen kaynak koşulları: anahtar → ilk görüldüğü an.
    pending: HashMap<String, u64>,
}

#[derive(Default)]
pub struct Alerts {
    webhook: Mutex<Option<String>>,
    email: Mutex<Option<crate::mail::Config>>,
    seen: Mutex<Seen>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Alerts {
    pub fn set_webhook(&self, url: &str) -> Result<(), String> {
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(char::is_whitespace) {
            return Err(format!("webhook adresi http:// ya da https:// ile başlamalı: {url}"));
        }
        *lock(&self.webhook) = Some(url.to_string());
        Ok(())
    }

    pub fn set_email(&self, config: crate::mail::Config) {
        *lock(&self.email) = Some(config);
    }

    /// E-posta ayarı (yedek raporu da bunu kullanır).
    pub fn email(&self) -> Option<crate::mail::Config> {
        lock(&self.email).clone()
    }

    /// Bir turun olaylarını tek e-postada gönderir. Gönderilen ileti sayısını döndürür (0 ya da 1).
    pub fn deliver_email(&self, cluster: &str, events: &[Event]) -> Result<usize, String> {
        let Some(config) = lock(&self.email).clone() else { return Ok(0) };
        if events.is_empty() {
            return Ok(0);
        }
        let subject = match events {
            [only] => only.text(cluster),
            many => format!("[{cluster}] {} uyarı", many.len()),
        };
        let body: String = events.iter().map(|e| format!("{}\n", e.text(cluster))).collect();
        config.send(&subject, &body).map(|()| 1)
    }

    pub fn webhook(&self) -> Option<String> {
        lock(&self.webhook).clone()
    }

    /// Önceki turla karşılaştırır; yeni başlayan ve biten koşulları, yeni başarısız görevleri
    /// ve beklenmedik konuk duruşlarını döndürür.
    pub fn update(&self, c: &Cluster, now: u64) -> Vec<Event> {
        let mut seen = lock(&self.seen);
        // Bekleme süresi olan koşul, o süre boyunca her turda görülürse etkin sayılır; arada bir kez
        // kaybolursa sayaç sıfırlanır. Etkinleşmiş koşul, sürdüğü sürece etkin kalır.
        let mut current: BTreeMap<String, Alert> = BTreeMap::new();
        let mut pending = HashMap::new();
        for alert in evaluate(c) {
            if alert.hold_secs > 0 {
                let since = seen.pending.get(&alert.key).copied().unwrap_or(now);
                pending.insert(alert.key.clone(), since);
                if now.saturating_sub(since) < alert.hold_secs && !seen.active.contains_key(&alert.key) {
                    continue;
                }
            }
            current.insert(alert.key.clone(), alert);
        }
        seen.pending = pending;
        let newest_task = c.tasks.iter().map(|t| t.id).max().unwrap_or(0);
        let statuses: HashMap<VmId, GuestStatus> = c.guests.iter().map(|g| (g.vmid, g.status)).collect();
        let mut events = Vec::new();

        if seen.started {
            for (key, alert) in &current {
                match seen.active.get(key) {
                    None => events.push(Event::Raised(alert.clone())),
                    // Uyarıdan kritiğe yükselme de bildirilir.
                    Some(old) if old.severity != alert.severity && alert.severity == Severity::Critical => events.push(Event::Raised(alert.clone())),
                    _ => {}
                }
            }
            for (key, alert) in &seen.active {
                if !current.contains_key(key) {
                    events.push(Event::Cleared(alert.clone()));
                }
            }
            let fresh: Vec<_> = c.tasks.iter().rev().filter(|t| t.id > seen.last_task && t.status == TaskStatus::Failed && !seen.reported.contains(&t.id)).collect();
            for t in fresh {
                seen.reported.push(t.id);
                events.push(Event::TaskFailed {
                    command: t.command.clone(),
                    user: t.user.clone(),
                    message: t.message.clone().unwrap_or_default(),
                });
            }
            for g in c.guests.iter().filter(|g| g.status == GuestStatus::Stopped) {
                let was_running = seen.statuses.get(&g.vmid).is_some_and(|s| *s != GuestStatus::Stopped);
                let explained = c.tasks.iter().any(|t| t.vmid == Some(g.vmid) && t.ended.is_some_and(|e| e + EXPECTED_STOP_WINDOW >= now));
                if was_running && !explained {
                    events.push(Event::GuestStopped { vmid: g.vmid, name: g.name.clone() });
                }
            }
        }
        // Süren görev sonradan başarısız olabilir: yalnızca bitmişlerin kimliğine kadar ilerle.
        let oldest_running = c.tasks.iter().filter(|t| t.status == TaskStatus::Running).map(|t| t.id).min();
        seen.last_task = oldest_running.map(|id| id - 1).unwrap_or(newest_task).max(seen.last_task);
        let floor = seen.last_task;
        seen.reported.retain(|id| *id > floor);
        seen.active = current;
        seen.statuses = statuses;
        seen.started = true;
        events
    }

    /// Olayları webhook'a gönderir (ağ çağrısı; bloklayan iş parçacığından çağrılmalı).
    /// Gövde Slack, Mattermost ve Discord'un anladığı alanları birlikte taşır.
    pub fn deliver(&self, cluster: &str, events: &[Event]) -> Result<usize, String> {
        let Some(url) = self.webhook() else { return Ok(0) };
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
        for event in events {
            let text = event.text(cluster);
            agent
                .post(&url)
                .send_json(json!({ "text": text, "content": text, "event": event.kind(), "cluster": cluster }))
                .map_err(|e| format!("webhook gönderilemedi: {e}"))?;
        }
        Ok(events.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::Action;

    fn keys(alerts: &[Alert]) -> Vec<(&str, Severity)> {
        alerts.iter().map(|a| (a.key.as_str(), a.severity)).collect()
    }

    #[test]
    fn evaluates_storage_nodes_and_quorum() {
        let mut c = Cluster::demo(0);
        // Örnek kümede nfs-yedek %93 dolu (7400/8000).
        assert_eq!(keys(&evaluate(&c)), [("storage:nfs-yedek:", Severity::Critical)]);
        assert_eq!(evaluate(&c)[0].message, "nfs-yedek deposu %93 dolu");

        c.storages[1].used_gb = 1500; // 1500/1800 = %83
        c.nodes[0].status = NodeStatus::Offline;
        let a = evaluate(&c);
        assert_eq!(
            keys(&a),
            [("node:pve-ist-01", Severity::Critical), ("storage:local-zfs:pve-ist-02", Severity::Warning), ("storage:nfs-yedek:", Severity::Critical)]
        );
        assert_eq!(a[1].message, "local-zfs (pve-ist-02) deposu %83 dolu");

        c.nodes[1].status = NodeStatus::Offline;
        assert!(evaluate(&c).iter().any(|x| x.key == "quorum"), "üçte bir çoğunluk değildir");
        // Tek düğümlü kurulumda çoğunluk uyarısı anlamsızdır.
        c.nodes.truncate(1);
        assert!(evaluate(&c).iter().all(|x| x.key != "quorum"));
    }

    #[test]
    fn only_changes_after_startup_are_reported() {
        let alerts = Alerts::default();
        let mut c = Cluster::demo(0);
        let t = c.begin_action(103, Action::Start, "u", 0).unwrap();
        c.complete_task(t, Err("eski hata".into()), 0).unwrap();
        assert!(alerts.update(&c, 100).is_empty(), "açılışta var olan koşullar ve eski görevler bildirilmez");
        assert!(alerts.update(&c, 110).is_empty(), "değişiklik yoksa sessiz kalır");

        c.storages[0].used_gb = 1500; // %83 uyarı
        assert_eq!(alerts.update(&c, 120).len(), 1);
        c.storages[0].used_gb = 1700; // %94 kritik
        let events = alerts.update(&c, 130);
        assert!(matches!(&events[..], [Event::Raised(a)] if a.severity == Severity::Critical), "yükselme bildirilir: {events:?}");
        assert!(alerts.update(&c, 140).is_empty());
        c.storages[0].used_gb = 100;
        let events = alerts.update(&c, 150);
        assert!(matches!(&events[..], [Event::Cleared(a)] if a.key == "storage:local-zfs:pve-ist-01"));
        assert_eq!(events[0].text("ist"), "[ist] DÜZELDİ: local-zfs (pve-ist-01) deposu %94 dolu");
    }

    #[test]
    fn failed_tasks_are_reported_once_even_if_they_finish_late() {
        let alerts = Alerts::default();
        let mut c = Cluster::demo(0);
        alerts.update(&c, 0);

        let slow = c.begin_action(103, Action::Start, "ayse", 10).unwrap();
        let quick = c.begin_action(100, Action::Pause, "u", 10).unwrap();
        c.complete_task(quick, Ok(()), 11).unwrap();
        assert!(alerts.update(&c, 12).is_empty(), "süren ve başarılı görevler olay değildir");

        c.complete_task(slow, Err("QEMU başlatılamadı".into()), 40).unwrap();
        let events = alerts.update(&c, 41);
        assert_eq!(events, [Event::TaskFailed { command: "vm start 103".into(), user: "ayse".into(), message: "QEMU başlatılamadı".into() }]);
        assert_eq!(events[0].text("ist"), "[ist] GÖREV BAŞARISIZ: redox vm start 103 (ayse): QEMU başlatılamadı");
        assert!(alerts.update(&c, 42).is_empty(), "aynı görev ikinci kez bildirilmez");

        // Daha eski bir görev hâlâ sürerken biten başarısız görev de yalnızca bir kez bildirilir.
        let old = c.begin_action(202, Action::Start, "u", 50).unwrap();
        let newer = c.begin_action(100, Action::Resume, "u", 50).unwrap();
        c.complete_task(newer, Err("x".into()), 51).unwrap();
        assert_eq!(alerts.update(&c, 52).len(), 1);
        assert!(alerts.update(&c, 53).is_empty());
        c.complete_task(old, Ok(()), 54).unwrap();
        assert!(alerts.update(&c, 55).is_empty());
    }

    #[test]
    fn unexpected_guest_stop_is_distinguished_from_a_requested_one() {
        let alerts = Alerts::default();
        let mut c = Cluster::demo(1000);
        alerts.update(&c, 1000);

        // İstenen kapatma: az önce biten bir görev var.
        let t = c.begin_action(100, Action::Stop, "u", 1000).unwrap();
        c.complete_task(t, Ok(()), 1001).unwrap();
        assert!(alerts.update(&c, 1005).is_empty());

        // Kendiliğinden duran konuk (sürücü gözlemi), açıklayan görev yok.
        let stopped = redox_core::Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None };
        c.observe_guest(102, &stopped, 2000);
        assert_eq!(alerts.update(&c, 2000), [Event::GuestStopped { vmid: 102, name: "db-postgres".into() }]);
        assert!(alerts.update(&c, 2005).is_empty(), "bir kez bildirilir");
    }

    #[test]
    fn webhook_url_is_validated() {
        let alerts = Alerts::default();
        for bad in ["", "ftp://x", "javascript:alert(1)", "example.com/hook", "http://a b"] {
            assert!(alerts.set_webhook(bad).is_err(), "{bad}");
        }
        assert_eq!(alerts.deliver("ist", &[Event::GuestStopped { vmid: 1, name: "x".into() }]), Ok(0), "adres yoksa gönderim yapılmaz");
        alerts.set_webhook("https://hooks.example.com/abc").unwrap();
        assert_eq!(alerts.webhook().as_deref(), Some("https://hooks.example.com/abc"));
    }

    #[test]
    fn delivers_json_to_the_webhook() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut received = Vec::new();
            let mut buf = [0u8; 4096];
            // Başlıklar ve gövdenin tamamı gelene kadar oku.
            loop {
                let n = socket.read(&mut buf).unwrap_or(0);
                received.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&received).to_string();
                if n == 0 || text.split("\r\n\r\n").nth(1).is_some_and(|body| body.trim_end().ends_with('}')) {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    return text;
                }
            }
        });
        let alerts = Alerts::default();
        alerts.set_webhook(&format!("http://127.0.0.1:{port}/hook")).unwrap();
        let event = Event::Raised(Alert { key: "storage:x:".into(), severity: Severity::Critical, message: "x deposu %95 dolu".into(), hold_secs: 0 });
        assert_eq!(alerts.deliver("istanbul-01", &[event]), Ok(1));

        let request = server.join().unwrap();
        assert!(request.starts_with("POST /hook HTTP/1.1"), "{request}");
        let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap().trim()).unwrap();
        assert_eq!(body["text"], "[istanbul-01] KRİTİK: x deposu %95 dolu");
        assert_eq!((body["content"].as_str(), body["event"].as_str(), body["cluster"].as_str()), (body["text"].as_str(), Some("raised"), Some("istanbul-01")));
    }

    #[test]
    fn unreachable_webhook_is_an_error_not_a_panic() {
        let alerts = Alerts::default();
        alerts.set_webhook("http://127.0.0.1:1/kapali").unwrap();
        assert!(alerts.deliver("x", &[Event::GuestStopped { vmid: 1, name: "a".into() }]).unwrap_err().contains("webhook gönderilemedi"));
    }

    #[test]
    fn resource_alerts_are_reported_only_when_sustained() {
        let mut c = Cluster::demo(1000);
        assert!(evaluate(&c).iter().all(|a| a.hold_secs == 0), "örnek kümede kaynak uyarısı yok");
        let alerts = Alerts::default();
        assert!(alerts.update(&c, 1000).is_empty());

        // Bellek %92: sayfada hemen görünür, bildirim beş dakika sürerse gelir.
        c.nodes[0].mem_used_mb = c.nodes[0].mem_total_mb * 92 / 100;
        let node = c.nodes[0].id.clone();
        let now = evaluate(&c);
        let mem = now.iter().find(|a| a.key == format!("mem:{node}")).unwrap();
        assert_eq!((mem.severity, mem.hold_secs), (Severity::Warning, LOAD_HOLD_SECS));
        assert!(mem.message.contains("belleği %92 dolu"), "{}", mem.message);
        assert!(alerts.update(&c, 1015).is_empty() && alerts.update(&c, 1015 + LOAD_HOLD_SECS - 1).is_empty());
        let events = alerts.update(&c, 1015 + LOAD_HOLD_SECS);
        assert!(matches!(&events[..], [Event::Raised(a)] if a.key == format!("mem:{node}")), "{events:?}");
        assert!(alerts.update(&c, 2000).is_empty(), "süren koşul yinelenmez");
        // Kritiğe yükselme hemen bildirilir (koşul zaten etkin).
        c.nodes[0].mem_used_mb = c.nodes[0].mem_total_mb * 98 / 100;
        assert!(matches!(&alerts.update(&c, 2015)[..], [Event::Raised(a)] if a.severity == Severity::Critical));
        c.nodes[0].mem_used_mb = c.nodes[0].mem_total_mb / 2;
        assert!(matches!(&alerts.update(&c, 2030)[..], [Event::Cleared(a)] if a.key == format!("mem:{node}")));

        // Anlık sıçrama: süre dolmadan düşen koşul bildirilmez ve sayaç sıfırlanır.
        let running = c.guests.iter().position(|g| g.status == GuestStatus::Running).unwrap();
        let key = format!("guest-cpu:{}", c.guests[running].vmid);
        c.guests[running].cpu_pct = 99;
        c.nodes[1].cpu_pct = 95;
        assert_eq!(evaluate(&c).iter().filter(|a| a.hold_secs > 0).count(), 2);
        assert!(alerts.update(&c, 3000).is_empty() && alerts.update(&c, 3200).is_empty());
        c.guests[running].cpu_pct = 10;
        assert!(alerts.update(&c, 3250).is_empty(), "bildirilmemiş koşulun düzelmesi de bildirilmez");
        c.guests[running].cpu_pct = 99;
        let events = alerts.update(&c, 3300);
        assert!(matches!(&events[..], [Event::Raised(a)] if a.key == format!("cpu:{}", c.nodes[1].id)), "düğüm işlemcisi 300 sn sürdü: {events:?}");
        assert!(alerts.update(&c, 3250 + LOAD_HOLD_SECS).is_empty(), "konuk sayacı 3300'de yeniden başladı");
        let events = alerts.update(&c, 3300 + LOAD_HOLD_SECS);
        assert!(matches!(&events[..], [Event::Raised(a)] if a.key == key && a.message.contains("işlemcisi %99")), "{events:?}");
        // Çevrimdışı düğüm ve kapalı konuk için kaynak uyarısı üretilmez.
        c.guests[running].status = GuestStatus::Stopped;
        c.nodes[1].status = NodeStatus::Offline;
        assert!(evaluate(&c).iter().all(|a| a.hold_secs == 0));
    }
}
