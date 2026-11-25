//! Çalışan sanal makineye çekirdek ekleme.

use super::*;

impl Cluster {
    /// Çalışan sanal makinenin çekirdek sayısı `target` değerine çıkarılabilir mi? Şimdiki sayıyı döndürür.
    /// Çalışırken yalnızca artırılır ve konuğun "en fazla çekirdek" değerini geçemez; azaltmak için konuk kapatılır.
    pub fn check_core_add(&self, vmid: VmId, target: u32) -> Result<u32, Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm {
            return Err(Error::InvalidSpec("çalışırken çekirdek ekleme yalnızca sanal makinelerde kullanılır".to_string()));
        }
        if g.status != GuestStatus::Running {
            return Err(Error::InvalidSpec("konuk çalışmıyor; kapalı konuğun çekirdek sayısı Donanım sekmesinden değişir".to_string()));
        }
        if g.hw.max_cores == 0 {
            return Err(Error::InvalidSpec("bu konukta çalışırken çekirdek ekleme kapalı; konuk kapalıyken Donanım sekmesinden \"en fazla çekirdek\" değerini verin".to_string()));
        }
        if target <= g.cores {
            return Err(Error::InvalidSpec(format!("çalışırken çekirdek yalnızca artırılır (şimdi {}); azaltmak için konuğu kapatın", g.cores)));
        }
        if target > g.hw.max_cores {
            return Err(Error::InvalidSpec(format!("bu konuk en fazla {} çekirdeğe çıkabilir", g.hw.max_cores)));
        }
        if let Some(pool) = g.pool.as_deref() {
            self.pool_allows(pool, Some(vmid), target, g.mem_total_mb, g.total_disk_gb())?;
        }
        Ok(g.cores)
    }

    /// Sürücünün eklediği bir çekirdeği kayda işler.
    pub fn note_core_added(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.cores += 1;
        }
    }
}
