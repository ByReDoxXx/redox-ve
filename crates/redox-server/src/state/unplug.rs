//! Çalışan konuktan çekirdek ve bellek çıkarma.
//!
//! Konuk isteği süre dolduktan **sonra** da yerine getirebilir. Bu yüzden başarısız bir çıkarmadan sonra konuk
//! izlenir ve kayıt, konukta gerçekten takılı olana göre düzeltilir: askıdan dönüş ve taşıma kayda göre makine
//! kurar, kayıt yanlışsa bellek durumu yüklenmez.

use super::*;

impl AppState {
    fn pending_unplug(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<VmId>> {
        self.0.unplugging.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Kaydı konukta takılı olan çekirdek ve bellek parçalarıyla uyuşturur (yalnızca azaltır).
    pub fn reconcile_unplug(&self, vmid: VmId) {
        if !self.pending_unplug().contains(&vmid) {
            return;
        }
        let Ok(guest) = self.snapshot().guest(vmid).cloned() else {
            self.pending_unplug().remove(&vmid);
            return;
        };
        if guest.status == redox_core::GuestStatus::Stopped {
            self.pending_unplug().remove(&vmid);
            return;
        }
        let Some((cores, dimms)) = self.driver().hotplug_inventory(&guest) else { return };
        let released: Vec<usize> = (dimms..guest.hw.dimms.len()).rev().collect();
        for index in &released {
            self.driver().release_dimm(&guest, *index);
        }
        self.mutate(|c| {
            for _ in cores..guest.cores {
                c.note_core_removed(vmid);
            }
            for _ in &released {
                c.note_memory_removed(vmid);
            }
        });
        if cores < guest.cores || !released.is_empty() {
            let message = format!("vm unplug {vmid}: konuk sonradan bıraktı");
            self.mutate(|c| c.log(&guest.node, crate::state::SCHEDULER, message, &Ok(()), now(), Some(vmid)));
        }
    }

    /// İzleme turunda: yanıt bekleyen çıkarma istekleri denetlenir.
    pub(super) fn reconcile_pending_unplugs(&self) {
        let waiting: Vec<VmId> = self.pending_unplug().iter().copied().collect();
        for vmid in waiting {
            if self.snapshot().guest(vmid).map_or(true, |g| g.lock.is_none()) {
                self.reconcile_unplug(vmid);
            }
        }
    }

    pub(super) fn cores_remove(&self, vmid: VmId, target: u32, user: &str) -> Result<(), Error> {
        let (from, mut guest) = self.mutate(|c| Ok::<_, Error>((c.check_core_remove(vmid, target)?, c.guest(vmid)?.clone())))?;
        let mut outcome = Ok(());
        for _ in target..from {
            outcome = self.driver().hot_remove_core(&guest);
            if outcome.is_err() {
                self.pending_unplug().insert(vmid);
                break;
            }
            guest.cores -= 1;
            self.mutate(|c| c.note_core_removed(vmid));
        }
        self.mutate(|c| c.log(&guest.node, user, format!("vm cores {vmid} {target}"), &outcome, now(), Some(vmid)));
        outcome.map_err(Error::Driver)
    }

    pub(super) fn memory_remove(&self, vmid: VmId, target_mb: u64, user: &str) -> Result<(), Error> {
        let (parts, mut guest) = self.mutate(|c| Ok::<_, Error>((c.check_memory_remove(vmid, target_mb)?, c.guest(vmid)?.clone())))?;
        let mut outcome = Ok(());
        for _ in 0..parts {
            outcome = self.driver().hot_remove_memory(&guest);
            if outcome.is_err() {
                self.pending_unplug().insert(vmid);
                break;
            }
            if let Some(mb) = guest.hw.dimms.pop() {
                guest.mem_total_mb -= mb;
            }
            self.mutate(|c| c.note_memory_removed(vmid));
        }
        self.mutate(|c| c.log(&guest.node, user, format!("vm memory {vmid} {target_mb}"), &outcome, now(), Some(vmid)));
        outcome.map_err(Error::Driver)
    }
}
