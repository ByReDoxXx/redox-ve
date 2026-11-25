//! Çoğaltma: eş sunuculardaki konukların buradaki yedek kopyalarını düzenli aralıkla tazeler.

use super::*;

impl AppState {
    pub fn replica_put(&self, peer: &str, source: VmId, every_min: u32, user: &str) -> Result<(), Error> {
        if self.peers().peer(peer).is_none() {
            return Err(Error::InvalidSpec(format!("\"{peer}\" adında bir eş sunucu tanımlı değil")));
        }
        // Var olan çoğaltmanın yalnızca aralığı değişir; yeni çoğaltma disk imajı içe aktarabilen sürücü ister.
        let exists = self.lock().replicas.iter().any(|r| r.peer == peer && r.source == source);
        if !exists && self.driver().image_file("x.qcow2").is_none() {
            return Err(Error::InvalidSpec("bu sürücü disk imajı içe aktaramıyor (gerçek sürücü gerekir)".to_string()));
        }
        self.mutate(|c| {
            c.put_replica(peer, source, every_min)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("replica add {peer}/{source} {every_min}"), &Ok(()), now(), None);
            Ok(())
        })
    }

    pub fn replica_remove(&self, peer: &str, source: VmId, user: &str) -> Result<(), Error> {
        self.mutate(|c| {
            c.remove_replica(peer, source)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("replica remove {peer}/{source}"), &Ok(()), now(), None);
            Ok(())
        })
    }

    /// Çoğaltmayı aralığını beklemeden şimdi çalıştırır.
    pub fn replica_run(&self, peer: &str, source: VmId) -> Result<(), Error> {
        if !self.lock().replicas.iter().any(|r| r.peer == peer && r.source == source) {
            return Err(Error::InvalidSpec(format!("{peer}/{source} için tanımlı bir çoğaltma yok")));
        }
        match self.replicate(Some((peer, source))) {
            0 => Err(Error::InvalidSpec(format!("{peer}/{source} çoğaltması başlatılamadı: bir tur sürüyor ya da yedek kopya çalışıyor"))),
            _ => Ok(()),
        }
    }

    /// Sırası gelen çoğaltmaları başlatır (dakikada bir çağrılır); başlatılan tur sayısını döndürür.
    pub fn replica_tick(&self) -> usize {
        if self.in_maintenance() {
            return 0;
        }
        self.replicate(None)
    }

    fn replicate(&self, only: Option<(&str, VmId)>) -> usize {
        let due = self.mutate(|c| c.replicas_due(now(), only));
        let started = due.len();
        for replica in due {
            let me = self.clone();
            self.dispatch(move || {
                let (peer_name, source) = (replica.peer.clone(), replica.source);
                let outcome = (|| {
                    let peer = me.peers().peer(&peer_name).ok_or_else(|| format!("\"{peer_name}\" adında bir eş sunucu tanımlı değil"))?;
                    let image = format!("cogaltma-{peer_name}-{source}.qcow2");
                    let path = me.driver().image_file(&image).ok_or("bu sürücü disk imajı içe aktaramıyor")?;
                    let made = me.import_now(&peer, &peer_name, source, &image, &path, SCHEDULER, true);
                    let _ = std::fs::remove_file(&path);
                    made
                })();
                let old = me.mutate(|c| {
                    let old = c.finish_replica(&peer_name, source, &outcome, now());
                    let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                    c.log(&node, SCHEDULER, format!("vm replicate {peer_name}/{source}"), &outcome.as_ref().map(|_| ()).map_err(Clone::clone), now(), outcome.as_ref().ok().copied());
                    old
                });
                // Yeni kopya hazır: eskisi (hâlâ kapalı bir yedek kopyaysa) silinir.
                if let Some(old) = old {
                    let stale = me.snapshot().guest(old).is_ok_and(|g| g.status == redox_core::GuestStatus::Stopped && g.name.ends_with("-kopya"));
                    if stale {
                        if let Err(e) = me.delete_guest(old, SCHEDULER) {
                            eprintln!("uyarı: eski yedek kopya {old} silinemedi: {e}");
                        }
                    }
                }
            });
        }
        started
    }
}
