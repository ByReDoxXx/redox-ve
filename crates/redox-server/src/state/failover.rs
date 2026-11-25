//! Kendiliğinden devralma: kaynak sunucu ölünce yedek kopyayı hakem onayıyla başlatır.
//!
//! Bu sunucu, "eş sunucu öldü" ile "benim ağım koptu" durumlarını tek başına ayıramaz. Bu yüzden karar bir
//! hakeme, yani tanımlı başka bir eş sunucuya sorulur: hiçbir hakeme ulaşılamıyorsa kopan büyük olasılıkla
//! bu sunucudur; bir hakem kaynağı çalışır görüyorsa kopan yalnızca aradaki bağlantıdır. İki durumda da
//! devralınmaz, yoksa aynı konuk iki sunucuda birden çalışırdı.

use super::*;

impl AppState {
    pub fn replica_failover_set(&self, peer: &str, source: VmId, minutes: Option<u32>, user: &str) -> Result<(), Error> {
        if minutes.is_some() && self.peers().views().iter().filter(|v| v.name != peer).count() == 0 {
            return Err(Error::InvalidSpec("kendiliğinden devralma için hakem olacak ikinci bir eş sunucu tanımlı olmalı (--peers)".to_string()));
        }
        self.mutate(|c| {
            c.set_replica_failover(peer, source, minutes)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("replica failover {peer}/{source} {}", minutes.map_or("off".to_string(), |m| m.to_string())), &Ok(()), now(), None);
            Ok(())
        })
    }

    /// Dakikada bir çağrılır; başlatılan yedek kopya sayısını döndürür.
    pub fn failover_tick(&self) -> usize {
        self.failover_tick_at(now())
    }

    pub fn failover_tick_at(&self, now: u64) -> usize {
        if self.in_maintenance() {
            return 0;
        }
        let views = self.peers().views();
        // Tanımı kaldırılmış ya da henüz hiç yoklanmamış eş "ölü" sayılmaz.
        let online = |name: &str| views.iter().find(|v| v.name == name).is_none_or(|v| v.checked.is_none() || v.error.is_none());
        let due = self.mutate(|c| c.failover_due(now, &online));
        let mut started = 0;
        for (peer, source, standby) in due {
            let outcome = self.witness_allows(&peer, &views).and_then(|()| self.run_action(standby, Action::Start, SCHEDULER).map(|_| ()).map_err(|e| e.to_string()));
            started += usize::from(outcome.is_ok());
            self.mutate(|c| {
                if c.note_failover(&peer, source, &outcome, now) {
                    let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                    c.log(&node, SCHEDULER, format!("vm failover {peer}/{source}"), &outcome, now, Some(standby));
                }
            });
        }
        started
    }

    /// Hakem oylaması: en az bir hakeme ulaşılmalı ve ulaşılan hiçbir hakem kaynağı çalışır görmemeli.
    fn witness_allows(&self, source: &str, views: &[crate::peers::PeerView]) -> Result<(), String> {
        let source_url = views.iter().find(|v| v.name == source).map(|v| v.url.clone()).unwrap_or_default();
        let mut reached = 0;
        for view in views.iter().filter(|v| v.name != source) {
            let Some(witness) = self.peers().peer(&view.name) else { continue };
            let Ok(seen) = witness.peers_seen() else { continue };
            reached += 1;
            if seen.iter().any(|(name, url, online)| *online && (*url == source_url || name == source)) {
                return Err(format!("hakem {} kaynak sunucuyu çalışır görüyor: devralınmadı (yalnızca aradaki bağlantı kopmuş olabilir)", view.name));
            }
        }
        match reached {
            0 => Err("hiçbir hakem sunucuya ulaşılamıyor: bu sunucunun ağı kopmuş olabilir, devralınmadı".to_string()),
            _ => Ok(()),
        }
    }
}
