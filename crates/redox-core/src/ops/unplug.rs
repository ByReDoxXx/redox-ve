//! Çalışan sanal makineden çekirdek ve bellek çıkarma kuralları.

use super::*;

impl Cluster {
    fn check_unplug(&self, vmid: VmId) -> Result<&Guest, Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm {
            return Err(Error::InvalidSpec("çalışırken çekirdek ve bellek çıkarma yalnızca sanal makinelerde kullanılır".to_string()));
        }
        if g.status != GuestStatus::Running {
            return Err(Error::InvalidSpec("konuk çalışmıyor; kapalı konuğun çekirdek ve belleği Donanım sekmesinden değişir".to_string()));
        }
        Ok(g)
    }

    /// Çekirdek sayısı `target` değerine indirilebilir mi? Şimdiki sayıyı döndürür.
    pub fn check_core_remove(&self, vmid: VmId, target: u32) -> Result<u32, Error> {
        let g = self.check_unplug(vmid)?;
        if g.hw.max_cores == 0 {
            return Err(Error::InvalidSpec("bu konuk boş çekirdek yuvası olmadan açılmış; çekirdek çıkarmak için konuğu kapatın".to_string()));
        }
        if target == 0 || target >= g.cores {
            return Err(Error::InvalidSpec(format!("çıkarma için 1 ile {} arasında bir sayı verin", g.cores.saturating_sub(1))));
        }
        Ok(g.cores)
    }

    /// Bellek `target_mb` değerine indirilebilir mi? Sondan kaç parçanın çıkarılacağını döndürür. Yalnızca
    /// çalışırken eklenen parçalar, eklendikleri sıranın tersiyle ve bütün olarak çıkar.
    pub fn check_memory_remove(&self, vmid: VmId, target_mb: u64) -> Result<usize, Error> {
        let g = self.check_unplug(vmid)?;
        if g.hw.dimms.is_empty() {
            return Err(Error::InvalidSpec("yalnızca çalışırken eklenen bellek çıkarılabilir; bu konuğa eklenen bellek yok (belleği azaltmak için balonu kullanın)".to_string()));
        }
        if g.hw.balloon_mb > 0 || g.hw.mem_min_mb > 0 {
            return Err(Error::InvalidSpec("bellek çıkarmadan önce balon hedefini ve dinamik belleği kapatın (0)".to_string()));
        }
        if target_mb < g.hw.mem_reserve_mb {
            return Err(Error::InvalidSpec(format!("bellek, konuğun bellek garantisinin ({} MB) altına inemez", g.hw.mem_reserve_mb)));
        }
        let mut total = g.mem_total_mb;
        let mut reachable = Vec::new();
        for (count, part) in g.hw.dimms.iter().rev().enumerate() {
            total = total.saturating_sub(*part);
            if total == target_mb {
                return Ok(count + 1);
            }
            reachable.push(total.to_string());
        }
        Err(Error::InvalidSpec(format!("bellek yalnızca eklenen parçalar halinde çıkar; inilebilecek değerler: {} MB", reachable.join(", "))))
    }

    /// Sürücünün çıkardığı çekirdeği kayda işler.
    pub fn note_core_removed(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.cores = g.cores.saturating_sub(1).max(1);
        }
    }

    /// Sürücünün çıkardığı son bellek parçasını kayda işler.
    pub fn note_memory_removed(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            if let Some(mb) = g.hw.dimms.pop() {
                g.mem_total_mb = g.mem_total_mb.saturating_sub(mb);
            }
        }
    }
}
