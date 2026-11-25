//! Çoğaltma: eş sunucudaki bir sanal makinenin kopyasını bu sunucuda düzenli aralıkla tazeleme.
//!
//! Kaynak sunucu ölürse buradaki yedek kopya (kapalı duran olağan bir konuk) elle başlatılır; kayıp, son
//! çoğaltmadan beri yazılanlardır. Her tur diskin tamamını kopyalar (artımlı değildir): aralık buna göre seçilmelidir.

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;
use crate::model::{GuestStatus, VmId};

pub const MAX_REPLICAS: usize = 32;
/// Çoğaltma aralığının sınırları (dakika): 15 dakika ile bir hafta.
pub const MIN_REPLICA_MIN: u32 = 15;
pub const MAX_REPLICA_MIN: u32 = 7 * 24 * 60;
/// Kendiliğinden devralma bekleme süresinin sınırları (dakika).
pub const MIN_FAILOVER_MIN: u32 = 5;
pub const MAX_FAILOVER_MIN: u32 = 24 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replica {
    /// Kaynak eş sunucunun adı ve oradaki konuğun kimliği.
    pub peer: String,
    pub source: VmId,
    pub every_min: u32,
    /// Buradaki yedek kopyanın kimliği; henüz hiç çoğaltılmadıysa `None`.
    #[serde(default)]
    pub standby: Option<VmId>,
    #[serde(default)]
    pub last_ok: Option<u64>,
    #[serde(default)]
    pub last_try: Option<u64>,
    /// Son denemenin hatası; başarılıysa `None`.
    #[serde(default)]
    pub error: Option<String>,
    /// Son turda ağdan indirilen ve toplam blok sayısı (artımlı çoğaltma); tam kopyada `None`.
    #[serde(default)]
    pub delta: Option<(u64, u64)>,
    /// Kendiliğinden devralma: kaynak sunucuya bu kadar dakika ulaşılamazsa yedek kopya başlatılır; `None` kapalı.
    #[serde(default)]
    pub failover_min: Option<u32>,
    /// Kaynak sunucuya ne zamandır ulaşılamıyor.
    #[serde(default)]
    pub down_since: Option<u64>,
    /// Yedek kopya kendiliğinden ne zaman başlatıldı; sonraki başarılı çoğaltma turuna dek bir daha devralınmaz.
    #[serde(default)]
    pub failed_over: Option<u64>,
    /// Devralmanın neden yapılmadığı (hakem engeli, kopya yok…).
    #[serde(default)]
    pub failover_note: Option<String>,
    /// Şu an bir tur sürüyor mu? Dosyaya yazılmaz: sunucu yeniden başlayınca yarım kalan tur unutulur.
    #[serde(skip)]
    pub running: bool,
}

impl Cluster {
    fn replica_mut(&mut self, peer: &str, source: VmId) -> Option<&mut Replica> {
        self.replicas.iter_mut().find(|r| r.peer == peer && r.source == source)
    }

    /// Çoğaltma ekler ya da var olanın aralığını değiştirir.
    pub fn put_replica(&mut self, peer: &str, source: VmId, every_min: u32) -> Result<(), Error> {
        if !(MIN_REPLICA_MIN..=MAX_REPLICA_MIN).contains(&every_min) {
            return Err(Error::InvalidSpec(format!("çoğaltma aralığı {MIN_REPLICA_MIN} ile {MAX_REPLICA_MIN} dakika arasında olmalı")));
        }
        let full = self.replicas.len() >= MAX_REPLICAS;
        match self.replica_mut(peer, source) {
            Some(r) => r.every_min = every_min,
            None if full => return Err(Error::InvalidSpec(format!("en fazla {MAX_REPLICAS} çoğaltma tanımlanabilir"))),
            None => {
                self.replicas.push(Replica { peer: peer.to_string(), source, every_min, standby: None, last_ok: None, last_try: None, error: None, delta: None, failover_min: None, down_since: None, failed_over: None, failover_note: None, running: false });
                self.replicas.sort_by(|a, b| (&a.peer, a.source).cmp(&(&b.peer, b.source)));
            }
        }
        Ok(())
    }

    /// Kendiliğinden devralmayı açar (`Some(dakika)`) ya da kapatır.
    pub fn set_replica_failover(&mut self, peer: &str, source: VmId, minutes: Option<u32>) -> Result<(), Error> {
        if minutes.is_some_and(|m| !(MIN_FAILOVER_MIN..=MAX_FAILOVER_MIN).contains(&m)) {
            return Err(Error::InvalidSpec(format!("devralma süresi {MIN_FAILOVER_MIN} ile {MAX_FAILOVER_MIN} dakika arasında olmalı")));
        }
        let r = self.replica_mut(peer, source).ok_or_else(|| Error::InvalidSpec(format!("{peer}/{source} için tanımlı bir çoğaltma yok")))?;
        r.failover_min = minutes;
        r.down_since = None;
        r.failover_note = None;
        Ok(())
    }

    /// Kaynak sunucusuna yeterince uzun süredir ulaşılamayan ve yedek kopyası başlatılabilecek çoğaltmalar:
    /// (eş, kaynak konuk, yedek kopya). `online`: eş sunucuya şu an ulaşılıyor mu. Karar burada bitmez:
    /// çağıran hakem sunuculara danışır ve sonucu `note_failover` ile bildirir.
    pub fn failover_due(&mut self, now: u64, online: &dyn Fn(&str) -> bool) -> Vec<(String, VmId, VmId)> {
        let idle: Vec<VmId> = self.guests.iter().filter(|g| g.status == GuestStatus::Stopped && g.lock.is_none()).map(|g| g.vmid).collect();
        let mut due = Vec::new();
        for r in self.replicas.iter_mut() {
            let Some(minutes) = r.failover_min else { continue };
            if online(&r.peer) {
                r.down_since = None;
                r.failover_note = None;
                continue;
            }
            let since = *r.down_since.get_or_insert(now);
            if r.failed_over.is_some() || r.running || now.saturating_sub(since) < u64::from(minutes) * 60 {
                continue;
            }
            match r.standby {
                Some(id) if idle.contains(&id) => due.push((r.peer.clone(), r.source, id)),
                Some(_) => {}
                None => r.failover_note = Some("yedek kopya henüz yok: devralınamadı".to_string()),
            }
        }
        due
    }

    /// Devralma denemesinin sonucunu işler; durum değiştiyse (günlüğe yazılmaya değerse) `true`.
    pub fn note_failover(&mut self, peer: &str, source: VmId, outcome: &Result<(), String>, now: u64) -> bool {
        let Some(r) = self.replica_mut(peer, source) else { return false };
        match outcome {
            Ok(()) => {
                r.failed_over = Some(now);
                r.failover_note = None;
                true
            }
            Err(e) => r.failover_note.replace(e.clone()).as_ref() != Some(e),
        }
    }

    /// Çoğaltmayı kaldırır; buradaki yedek kopya olağan bir konuk olarak kalır.
    pub fn remove_replica(&mut self, peer: &str, source: VmId) -> Result<Replica, Error> {
        let at = self.replicas.iter().position(|r| r.peer == peer && r.source == source).ok_or_else(|| Error::InvalidSpec(format!("{peer}/{source} için tanımlı bir çoğaltma yok")))?;
        Ok(self.replicas.remove(at))
    }

    /// Sırası gelen çoğaltmaları "sürüyor" diye işaretleyip döndürür. `force`: aralığa bakılmaz (elle başlatma).
    /// Yedek kopya çalışıyorsa (devralınmışsa) çoğaltma yapılmaz: çalışan kopyanın üzerine yazılmaz.
    pub fn replicas_due(&mut self, now: u64, only: Option<(&str, VmId)>) -> Vec<Replica> {
        let running: Vec<VmId> = self.guests.iter().filter(|g| g.status != GuestStatus::Stopped || g.lock.is_some()).map(|g| g.vmid).collect();
        let mut due = Vec::new();
        for r in self.replicas.iter_mut().filter(|r| !r.running) {
            let wanted = match only {
                Some((peer, source)) => r.peer == peer && r.source == source,
                None => r.last_try.is_none_or(|t| now.saturating_sub(t) >= u64::from(r.every_min) * 60),
            };
            if !wanted {
                continue;
            }
            r.last_try = Some(now);
            if r.standby.is_some_and(|id| running.contains(&id)) {
                r.error = Some("yedek kopya çalışıyor (devralınmış): üzerine yazılmadı. Çoğaltmayı kaldırın ya da kopyayı kapatın".to_string());
                continue;
            }
            r.running = true;
            due.push(r.clone());
        }
        due
    }

    /// Sürmekte olan turda bir diskin artımlı aktarım sayılarını ekler; `reset` turun başında sayaçları sıfırlar.
    pub fn note_replica_delta(&mut self, peer: &str, source: VmId, fetched: u64, total: u64, reset: bool) {
        if let Some(r) = self.replica_mut(peer, source) {
            let (f, t) = if reset { (0, 0) } else { r.delta.unwrap_or((0, 0)) };
            r.delta = if reset && total == 0 { None } else { Some((f + fetched, t + total)) };
        }
    }

    /// Turun sonucunu işler. Başarılıysa yeni kopya yedek kopya olur ve **eski kopyanın kimliği** döner
    /// (çağıran onu siler).
    pub fn finish_replica(&mut self, peer: &str, source: VmId, outcome: &Result<VmId, String>, now: u64) -> Option<VmId> {
        let r = self.replica_mut(peer, source)?;
        r.running = false;
        match outcome {
            Ok(new) => {
                r.error = None;
                r.last_ok = Some(now);
                // Kaynak geri geldi ve yeni bir kopya alındı: devralma yeniden kurulur.
                r.failed_over = None;
                r.standby.replace(*new).filter(|old| old != new)
            }
            Err(e) => {
                r.error = Some(e.clone());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;

    #[test]
    fn replicas_run_on_their_interval_and_never_overwrite_a_running_standby() {
        let mut c = Cluster::demo(NOW);
        assert!(c.put_replica("ankara", 500, 5).is_err() && c.put_replica("ankara", 500, MAX_REPLICA_MIN + 1).is_err());
        c.put_replica("ankara", 500, 60).unwrap();
        c.put_replica("ankara", 500, 30).unwrap();
        assert_eq!((c.replicas.len(), c.replicas[0].every_min), (1, 30), "aynı kaynak yinelenmez; aralık güncellenir");

        // İlk tur hemen; sürerken ve aralık dolmadan yeniden başlamaz.
        let due = c.replicas_due(NOW, None);
        assert_eq!(due.len(), 1);
        assert!(c.replicas_due(NOW + 3600, None).is_empty(), "süren tur bitmeden yenisi başlamaz");
        assert_eq!(c.finish_replica("ankara", 500, &Ok(103), NOW + 100), None, "ilk kopyada silinecek eski kopya yok");
        assert_eq!((c.replicas[0].standby, c.replicas[0].last_ok, c.replicas[0].running), (Some(103), Some(NOW + 100), false));
        assert!(c.replicas_due(NOW + 29 * 60, None).is_empty());
        assert_eq!(c.replicas_due(NOW + 29 * 60, Some(("ankara", 500))).len(), 1, "elle başlatma aralığa bakmaz");
        // Başarısız tur eski kopyayı korur ve hatayı saklar.
        assert_eq!(c.finish_replica("ankara", 500, &Err("eş sunucuya ulaşılamadı".into()), NOW + 30 * 60), None);
        assert_eq!((c.replicas[0].standby, c.replicas[0].error.as_deref()), (Some(103), Some("eş sunucuya ulaşılamadı")));
        // Sonraki başarılı tur: eski kopya silinmek üzere döner.
        assert_eq!(c.replicas_due(NOW + 3 * 3600, None).len(), 1);
        assert_eq!(c.finish_replica("ankara", 500, &Ok(105), NOW + 3 * 3600 + 50), Some(103));

        // Yedek kopya çalışıyorsa (100 demo kümede çalışır) üzerine yazılmaz.
        c.replicas[0].standby = Some(100);
        assert!(c.replicas_due(NOW + 9 * 3600, None).is_empty());
        assert!(c.replicas[0].error.as_deref().is_some_and(|e| e.contains("devralınmış")) && !c.replicas[0].running);

        // Sunucu yeniden başlayınca yarım kalan tur unutulur.
        c.replicas[0].running = true;
        let loaded: Cluster = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(!loaded.replicas[0].running);
        assert_eq!(c.remove_replica("ankara", 500).unwrap().source, 500);
        assert!(c.remove_replica("ankara", 500).is_err());
    }

    #[test]
    fn failover_waits_for_the_peer_to_stay_down_and_happens_once() {
        let mut c = Cluster::demo(NOW);
        c.put_replica("ankara", 500, 60).unwrap();
        assert!(c.set_replica_failover("ankara", 500, Some(MIN_FAILOVER_MIN - 1)).is_err() && c.set_replica_failover("yok", 1, Some(10)).is_err());
        let (up, down) = (|_: &str| true, |_: &str| false);
        // Kapalıyken eş ölse de hiçbir şey olmaz.
        assert!(c.failover_due(NOW, &down).is_empty() && c.replicas[0].down_since.is_none());
        c.set_replica_failover("ankara", 500, Some(10)).unwrap();
        // Kopya yokken süre dolsa da devralınmaz; nedeni yazılır.
        assert!(c.failover_due(NOW, &down).is_empty());
        assert!(c.failover_due(NOW + 600, &down).is_empty());
        assert!(c.replicas[0].failover_note.as_deref().unwrap().contains("henüz yok"));
        // Eş geri gelince sayaç ve not sıfırlanır.
        assert!(c.failover_due(NOW + 700, &up).is_empty());
        assert_eq!((c.replicas[0].down_since, c.replicas[0].failover_note.clone()), (None, None));

        let standby = c.guests.iter().find(|g| g.status == GuestStatus::Stopped).unwrap().vmid;
        c.replicas[0].standby = Some(standby);
        assert!(c.failover_due(NOW + 1000, &down).is_empty(), "kesinti yeni başladı");
        assert!(c.failover_due(NOW + 1000 + 599, &down).is_empty(), "süre dolmadı");
        assert_eq!(c.failover_due(NOW + 1000 + 600, &down), [("ankara".to_string(), 500, standby)]);
        // Hakem engeli: not bir kez "değişti" sayılır, aynı neden yinelenince sayılmaz.
        let blocked = Err("hakem engeli".to_string());
        assert!(c.note_failover("ankara", 500, &blocked, NOW) && !c.note_failover("ankara", 500, &blocked, NOW));
        assert_eq!(c.failover_due(NOW + 2000, &down).len(), 1, "engel kalkana dek yeniden denenir");
        assert!(c.note_failover("ankara", 500, &Ok(()), NOW + 2000));
        assert!(c.failover_due(NOW + 3000, &down).is_empty(), "bir kez devralınır");
        // Başarılı bir çoğaltma turu devralmayı yeniden kurar.
        c.finish_replica("ankara", 500, &Ok(standby), NOW + 4000);
        assert_eq!(c.replicas[0].failed_over, None);
    }
}
