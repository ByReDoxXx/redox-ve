//! Bellek balonu ve dinamik bellek.

use super::*;

impl Cluster {
    /// Balonla hedeflenen belleği kaydeder; önceki hedefi döndürür (sürücü uygulayamazsa geri konur).
    /// 0 ya da belleğin tamamı "balonu boşalt" demektir. Konuk çalışırken de değişir.
    pub fn set_balloon_target(&mut self, vmid: VmId, target_mb: u64) -> Result<u64, Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm || !g.hw.balloon {
            return Err(Error::InvalidSpec("bu konukta bellek balonu yok; önce Donanım sekmesinden açın (konuk kapalıyken)".to_string()));
        }
        if g.hw.mem_min_mb > 0 {
            return Err(Error::InvalidSpec("dinamik bellek açık: hedef kendiliğinden ayarlanıyor. Elle hedef vermek için önce dinamik belleği kapatın (en az bellek 0)".to_string()));
        }
        let total = g.mem_total_mb;
        if target_mb != 0 && !(crate::hw::MIN_BALLOON_MB..=total).contains(&target_mb) {
            return Err(Error::InvalidSpec(format!("bellek hedefi {} MB ile {total} MB arasında olmalı (0: tamamı)", crate::hw::MIN_BALLOON_MB)));
        }
        let g = self.guest_mut(vmid)?;
        let before = g.hw.balloon_mb;
        g.hw.balloon_mb = if target_mb >= total { 0 } else { target_mb };
        Ok(before)
    }

    /// Dinamik belleği açar (`min_mb` alt sınır) ya da kapatır (0). Konuk çalışırken de değişir.
    pub fn set_dynamic_memory(&mut self, vmid: VmId, min_mb: u64) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm || !g.hw.balloon {
            return Err(Error::InvalidSpec("bu konukta bellek balonu yok; önce Donanım sekmesinden açın (konuk kapalıyken)".to_string()));
        }
        let total = g.mem_total_mb;
        if min_mb != 0 && !(crate::hw::MIN_BALLOON_MB..=total).contains(&min_mb) {
            return Err(Error::InvalidSpec(format!("en az bellek {} MB ile {total} MB arasında olmalı (0: dinamik bellek kapalı)", crate::hw::MIN_BALLOON_MB)));
        }
        self.guest_mut(vmid)?.hw.mem_min_mb = min_mb;
        Ok(())
    }

    /// Dinamik belleğin hesapladığı hedefi kaydeder (sürücü uyguladıktan sonra).
    pub fn set_dynamic_target(&mut self, vmid: VmId, target_mb: u64) {
        if let Ok(g) = self.guest_mut(vmid) {
            if g.hw.mem_min_mb > 0 {
                g.hw.balloon_mb = if target_mb >= g.mem_total_mb { 0 } else { target_mb };
            }
        }
    }

    /// Uygulanamayan bellek hedefini eski değerine döndürür.
    pub fn restore_balloon_target(&mut self, vmid: VmId, target_mb: u64) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.balloon_mb = target_mb;
        }
    }
}
