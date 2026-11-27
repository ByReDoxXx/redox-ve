//! Bakım kipi: sunucuyu güncellemek ya da yeniden başlatmak için konukları düzenli kapatıp sonra geri açma.
//!
//! Açıkken hiçbir konuk başlatılmaz (elle, açılışta, gözcüyle ya da zamanlamayla) ve zamanlanmış yedekler
//! alınmaz. Durum dosyada saklanır: sunucu bakım sırasında yeniden başlasa da konuklar kendiliğinden açılmaz.

use super::*;

impl AppState {
    pub fn in_maintenance(&self) -> bool {
        self.lock().maintenance
    }

    /// Bakıma alır. `drain`: çalışan konuklar açılış sırasının tersine kapatılır ve bakım bitince yeniden
    /// başlatılmak üzere kaydedilir. Kapatılmaya başlanan konuk sayısını döndürür.
    pub fn maintenance_on(&self, drain: bool, user: &str) -> usize {
        let plan = self.mutate(|c| {
            c.maintenance = true;
            let plan = if drain { c.maintenance_drain_plan() } else { Vec::new() };
            for vmid in &plan {
                if !c.maintenance_resume.contains(vmid) {
                    c.maintenance_resume.push(*vmid);
                }
            }
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("maintenance on{}", if drain { " --drain" } else { "" }), &Ok(()), now(), None);
            plan
        });
        self.stop_for_maintenance(plan, user)
    }

    /// Plandaki konukları kapatır; kapatılmaya başlanan konuk sayısını döndürür.
    fn stop_for_maintenance(&self, plan: Vec<VmId>, user: &str) -> usize {
        let mut stopped = 0;
        for vmid in plan {
            match self.run_action(vmid, Action::Stop, user) {
                Ok(_) => stopped += 1,
                Err(e) => {
                    // Kapatılamayan konuk bakım bitince "yeniden başlatılacaklar" arasında sayılmaz.
                    eprintln!("uyarı: bakım için {vmid} kapatılamadı: {e}");
                    self.mutate(|c| c.maintenance_resume.retain(|v| *v != vmid));
                }
            }
        }
        stopped
    }

    /// Bu sunucunun, eşin kendi eş listesindeki adı. Taşımayı eş yürütür (çekme düzeni) ve bu sunucuyu o adla
    /// arar. Ad verilmemişse eşin listesinde adresi bu sunucunun adına ya da adresine uyan tek kayıt seçilir.
    fn name_on_peer(&self, peer: &crate::peers::Peer, given: Option<&str>) -> Result<String, String> {
        let seen = peer.peers_seen()?;
        if let Some(name) = given {
            return if seen.iter().any(|(n, _, _)| n == name) { Ok(name.to_string()) } else { Err(format!("eş sunucu bu sunucuyu \"{name}\" adıyla tanımıyor (eşin --peers dosyasına bakın)")) };
        }
        let mine = [crate::host::hostname(), crate::host::primary_ip().unwrap_or_default()];
        let host_of = |url: &str| url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or_default().to_lowercase();
        let matches: Vec<&String> = seen.iter().filter(|(_, url, _)| mine.iter().any(|m| !m.is_empty() && *m == host_of(url))).map(|(name, _, _)| name).collect();
        match matches.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err("eş sunucunun eş listesinde bu sunucu bulunamadı: eşin --peers dosyasına bu sunucuyu ekleyin ya da oradaki adını --as ile verin".to_string()),
            _ => Err("eş sunucuda bu sunucunun adresiyle birden çok eş tanımlı; oradaki adını --as ile verin".to_string()),
        }
    }

    /// Bakıma alır ve çalışan sanal makineleri **kapatmadan** eş sunucuya taşır (arka planda, teker teker).
    /// Canlı taşınamayan konuklar yerinde kalır; `drain_rest` verilmişse onlar kapatılır ve bakım bitince yeniden
    /// başlatılır. Taşınmaya başlanacak konuk sayısını döndürür; sonuç görev günlüğüne yazılır. Taşınan konuklar
    /// bakım bitince kendiliğinden geri gelmez: eşte çalışmayı sürdürürler.
    pub fn maintenance_evacuate(&self, peer_name: &str, as_name: Option<&str>, drain_rest: bool, user: &str) -> Result<usize, Error> {
        let peer = self.peers().peer(peer_name).ok_or_else(|| Error::InvalidSpec(format!("\"{peer_name}\" adında bir eş sunucu tanımlı değil")))?;
        if !self.driver().live_migration() {
            return Err(Error::InvalidSpec("bu sürücü canlı taşımayı desteklemiyor (gerçek sürücü gerekir)".to_string()));
        }
        let me_name = self.name_on_peer(&peer, as_name).map_err(Error::InvalidSpec)?;
        let movable: Vec<VmId> = self.mutate(|c| {
            c.maintenance = true;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("maintenance on --evacuate {peer_name}"), &Ok(()), now(), None);
            c.maintenance_drain_plan().into_iter().filter(|vmid| c.guest(*vmid).is_ok_and(|g| g.status == redox_core::GuestStatus::Running && redox_core::migrate_blocker(g).is_none())).collect()
        });
        let count = movable.len();
        let (me, user, peer_name) = (self.clone(), user.to_string(), peer_name.to_string());
        self.dispatch(move || {
            let (mut moved, mut failed) = (0, Vec::new());
            for vmid in movable {
                match me.evacuate_one(&peer, &me_name, vmid) {
                    Ok(()) => moved += 1,
                    Err(e) => failed.push(format!("{vmid}: {e}")),
                }
            }
            if drain_rest {
                let plan = me.mutate(|c| {
                    let plan = c.maintenance_drain_plan();
                    for vmid in &plan {
                        if !c.maintenance_resume.contains(vmid) {
                            c.maintenance_resume.push(*vmid);
                        }
                    }
                    plan
                });
                me.stop_for_maintenance(plan, &user);
            }
            let outcome = if failed.is_empty() { Ok(()) } else { Err(format!("{moved} konuk taşındı; taşınamayanlar: {}", failed.join("; "))) };
            me.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, &user, format!("maintenance evacuate {peer_name} {moved}/{count}"), &outcome, now(), None);
            });
        });
        Ok(count)
    }

    /// Bir konuğu eşe taşıtır ve bitmesini bekler: konuk burada kapanıp "taşındı" işaretini alınca tamamdır.
    fn evacuate_one(&self, peer: &crate::peers::Peer, me_name: &str, vmid: VmId) -> Result<(), String> {
        let command = format!("vm migrate {me_name}/{vmid}");
        let before = peer.task_by_command(&command)?.map(|(id, _, _)| id);
        peer.migrate_pull(me_name, vmid)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(72 * 3600);
        loop {
            let here = self.lock().guest(vmid).ok().cloned();
            match here {
                None => return Ok(()),
                Some(g) if g.status == redox_core::GuestStatus::Stopped && g.lock.is_none() && g.tags.iter().any(|t| t == redox_core::MIGRATED_TAG) => return Ok(()),
                Some(_) => {}
            }
            if let Some((id, status, message)) = peer.task_by_command(&command)? {
                if Some(id) != before && status == "failed" {
                    return Err(message);
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("taşıma zamanında bitmedi".to_string());
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    }

    /// Bakımı bitirir ve bakıma girerken kapatılan konukları açılış sırasıyla yeniden başlatır; başlatılan
    /// konuk sayısını döndürür.
    pub fn maintenance_off(&self, user: &str) -> usize {
        let back = self.mutate(|c| {
            let back = c.maintenance_finish();
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, "maintenance off".to_string(), &Ok(()), now(), None);
            back
        });
        let mut started = 0;
        for vmid in back {
            match self.run_action(vmid, Action::Start, user) {
                Ok(_) => started += 1,
                Err(e) => eprintln!("uyarı: bakım sonrası {vmid} başlatılamadı: {e}"),
            }
        }
        started
    }
}
