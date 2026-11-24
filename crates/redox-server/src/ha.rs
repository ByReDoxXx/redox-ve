//! Kendiliğinden yeniden başlatma: "yeniden başlat" işaretli konuk beklenmedik biçimde kapanırsa açılır.
//!
//! Tek sunucudaki erişilebilirlik önlemidir. Sunucular arası kendiliğinden devralma bilerek yoktur: çöken
//! sunucunun gerçekten kapandığından emin olmadan (çitleme) konuğu başka yerde açmak, aynı diskin iki yerde
//! birden çalışmasına ve veri bozulmasına yol açar.

use std::collections::HashMap;

use redox_core::{Cluster, GuestStatus, VmId};

/// Bir konuk bu kadar saniye içinde biten bir görevin ardından durduysa, duruş istenmiş sayılır.
const EXPECTED_STOP_WINDOW: u64 = 30;
/// Bu süre içinde en çok `MAX_RESTARTS` kez yeniden başlatılır; aşılırsa vazgeçilir (açılıp açılıp çöken
/// konuk sonsuza dek denenmez).
pub const RESTART_WINDOW_SECS: u64 = 600;
pub const MAX_RESTARTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Restart(VmId),
    /// Sınır aşıldı: bu kapanış için yeniden başlatma denenmez.
    GiveUp(VmId),
}

#[derive(Default)]
pub struct Watch {
    /// İlk turda dolar: sunucu açılırken kapalı bulunan konuklar yeniden başlatılmaz (o iş açılış sırasınındır).
    started: bool,
    statuses: HashMap<VmId, GuestStatus>,
    /// Konuk başına yapılan yeniden başlatmaların anları.
    restarts: HashMap<VmId, Vec<u64>>,
}

impl Watch {
    /// Önceki turla karşılaştırır ve beklenmedik biçimde kapanan işaretli konuklar için karar verir.
    pub fn update(&mut self, c: &Cluster, now: u64) -> Vec<Decision> {
        let mut decisions = Vec::new();
        if self.started {
            for g in c.guests.iter().filter(|g| g.hw.restart && g.status == GuestStatus::Stopped && g.lock.is_none() && !g.hw.template && !g.hw.suspended) {
                let was_up = self.statuses.get(&g.vmid).is_some_and(|s| *s != GuestStatus::Stopped);
                // Yalnızca konuğu kapatabilecek görevler kapanışı açıklar; ayar kaydı ya da yedek açıklamaz.
                let stops = |t: &redox_core::Task| matches!(t.action, Some(redox_core::Action::Stop | redox_core::Action::Reboot)) || t.command.split(' ').nth(1) == Some("suspend");
                let explained = c.tasks.iter().any(|t| t.vmid == Some(g.vmid) && stops(t) && t.ended.is_some_and(|e| e + EXPECTED_STOP_WINDOW >= now));
                if !was_up || explained {
                    continue;
                }
                let history = self.restarts.entry(g.vmid).or_default();
                history.retain(|at| at + RESTART_WINDOW_SECS > now);
                if history.len() >= MAX_RESTARTS {
                    decisions.push(Decision::GiveUp(g.vmid));
                } else {
                    history.push(now);
                    decisions.push(Decision::Restart(g.vmid));
                }
            }
        }
        self.statuses = c.guests.iter().map(|g| (g.vmid, g.status)).collect();
        self.restarts.retain(|vmid, _| c.guests.iter().any(|g| g.vmid == *vmid));
        self.started = true;
        decisions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::Action;

    fn stop(c: &mut Cluster, vmid: VmId) {
        c.guest_mut_for_import(vmid).unwrap().status = GuestStatus::Stopped;
    }

    fn start(c: &mut Cluster, vmid: VmId) {
        c.guest_mut_for_import(vmid).unwrap().status = GuestStatus::Running;
    }

    #[test]
    fn unexpected_stops_are_restarted_with_a_limit() {
        let mut c = Cluster::demo(1000);
        let (a, b) = (100, 102);
        c.guest_mut_for_import(a).unwrap().hw.restart = true;
        let mut w = Watch::default();
        assert!(w.update(&c, 1000).is_empty());

        // İşaretli konuk kendiliğinden durdu: yeniden başlatılır. İşaretsiz konuk bırakılır.
        stop(&mut c, a);
        stop(&mut c, b);
        assert_eq!(w.update(&c, 1015), [Decision::Restart(a)]);
        assert!(w.update(&c, 1030).is_empty(), "aynı kapanış için ikinci karar verilmez");

        // Açılıp yeniden çöken konuk: pencere içinde en çok üç kez denenir, sonra vazgeçilir.
        for (i, at) in [1100u64, 1200].into_iter().enumerate() {
            start(&mut c, a);
            w.update(&c, at - 10);
            stop(&mut c, a);
            assert_eq!(w.update(&c, at), [Decision::Restart(a)], "{i}");
        }
        start(&mut c, a);
        w.update(&c, 1290);
        stop(&mut c, a);
        assert_eq!(w.update(&c, 1300), [Decision::GiveUp(a)]);
        // Pencere geçince sayaç boşalır.
        start(&mut c, a);
        w.update(&c, 1015 + RESTART_WINDOW_SECS + 300);
        stop(&mut c, a);
        assert_eq!(w.update(&c, 1015 + RESTART_WINDOW_SECS + 315), [Decision::Restart(a)]);
    }

    #[test]
    fn requested_stops_and_special_guests_are_left_alone() {
        let mut c = Cluster::demo(1000);
        for vmid in [100, 101, 102] {
            c.guest_mut_for_import(vmid).unwrap().hw.restart = true;
        }
        let mut w = Watch::default();
        w.update(&c, 1000);
        // Panelden kapatılan konuk: görev kaydı kapanışı açıklar.
        let task = c.begin_action(100, Action::Stop, "admin", 1010).unwrap();
        c.complete_task(task, Ok(()), 1012).unwrap();
        // Ayar kaydı gibi konuğu kapatmayan bir görev, çökmeyi açıklamaz.
        c.update_guest(100, &redox_core::GuestPatch { tags: Some(vec!["x".into()]), ..Default::default() }, "admin", 1013).ok();
        // Askıya alınan konuk ve üzerinde iş süren konuk.
        stop(&mut c, 101);
        c.guest_mut_for_import(101).unwrap().hw.suspended = true;
        stop(&mut c, 102);
        c.guest_mut_for_import(102).unwrap().lock = Some(99);
        assert!(w.update(&c, 1015).is_empty());

        // Sunucu açılırken kapalı bulunan konuk yeniden başlatılmaz.
        let mut fresh = Watch::default();
        let mut c2 = Cluster::demo(1000);
        c2.guest_mut_for_import(100).unwrap().hw.restart = true;
        stop(&mut c2, 100);
        assert!(fresh.update(&c2, 1000).is_empty() && fresh.update(&c2, 1015).is_empty());
        // Silinen konuğun kaydı tutulmaz.
        c2.guests.retain(|g| g.vmid != 100);
        fresh.update(&c2, 1030);
        assert!(fresh.restarts.is_empty() && !fresh.statuses.contains_key(&100));
    }
}
