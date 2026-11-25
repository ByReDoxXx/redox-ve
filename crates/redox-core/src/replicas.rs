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
                self.replicas.push(Replica { peer: peer.to_string(), source, every_min, standby: None, last_ok: None, last_try: None, error: None, running: false });
                self.replicas.sort_by(|a, b| (&a.peer, a.source).cmp(&(&b.peer, b.source)));
            }
        }
        Ok(())
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

    /// Turun sonucunu işler. Başarılıysa yeni kopya yedek kopya olur ve **eski kopyanın kimliği** döner
    /// (çağıran onu siler).
    pub fn finish_replica(&mut self, peer: &str, source: VmId, outcome: &Result<VmId, String>, now: u64) -> Option<VmId> {
        let r = self.replica_mut(peer, source)?;
        r.running = false;
        match outcome {
            Ok(new) => {
                r.error = None;
                r.last_ok = Some(now);
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
}
