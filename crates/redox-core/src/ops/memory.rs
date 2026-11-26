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

impl Cluster {
    /// Çalışan sanal makinenin belleği `target_mb` değerine çıkarılabilir mi? Eklenecek miktarı (MB) döndürür.
    /// Çalışırken yalnızca artırılır, konuğun "en fazla bellek" değerini geçemez ve belirli adımlarla eklenir.
    pub fn check_memory_add(&self, vmid: VmId, target_mb: u64) -> Result<u64, Error> {
        use crate::hw::{MEM_SLOTS, MEM_STEP_MB};
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm {
            return Err(Error::InvalidSpec("çalışırken bellek ekleme yalnızca sanal makinelerde kullanılır".to_string()));
        }
        if g.status != GuestStatus::Running {
            return Err(Error::InvalidSpec("konuk çalışmıyor; kapalı konuğun belleği Donanım sekmesinden değişir".to_string()));
        }
        let (total, max) = (g.mem_total_mb, g.hw.max_mem_mb);
        if max == 0 {
            return Err(Error::InvalidSpec("bu konukta çalışırken bellek ekleme kapalı; konuk kapalıyken Donanım sekmesinden \"en fazla bellek\" değerini verin".to_string()));
        }
        if target_mb <= total {
            return Err(Error::InvalidSpec(format!("çalışırken bellek yalnızca artırılır (şimdi {total} MB); azaltmak için konuğu kapatın ya da bellek balonunu kullanın")));
        }
        if target_mb > max {
            return Err(Error::InvalidSpec(format!("bu konuk en fazla {max} MB belleğe çıkabilir")));
        }
        let add = target_mb - total;
        if !add.is_multiple_of(MEM_STEP_MB) {
            let (lower, upper) = (total + add / MEM_STEP_MB * MEM_STEP_MB, total + (add / MEM_STEP_MB + 1) * MEM_STEP_MB);
            let near = if upper <= max { upper } else { lower };
            return Err(Error::InvalidSpec(if near > total {
                format!("bellek {MEM_STEP_MB} MB'lık adımlarla eklenir; en yakın geçerli değer {near} MB")
            } else {
                format!("bellek {MEM_STEP_MB} MB'lık adımlarla eklenir; bu konukta {max} MB sınırına kadar bir adımlık yer kalmadı")
            }));
        }
        if g.hw.dimms.len() >= MEM_SLOTS {
            return Err(Error::InvalidSpec(format!("konuğa bir açılışta en fazla {MEM_SLOTS} kez bellek eklenir; yeniden eklemek için konuğu bir kez kapatıp açın")));
        }
        if let Some(pool) = g.pool.as_deref() {
            self.pool_allows(pool, Some(vmid), g.cores, target_mb, g.total_disk_gb())?;
        }
        Ok(add)
    }

    /// Sürücünün eklediği bellek parçasını kayda işler. Elle verilmiş balon hedefi aynı miktarda yükselir:
    /// balonun konuktan aldığı bellek değişmediği için konuk eklenen belleğin tamamını kullanır.
    pub fn note_memory_added(&mut self, vmid: VmId, mb: u64) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.mem_total_mb += mb;
            g.hw.dimms.push(mb);
            if g.hw.balloon_mb > 0 {
                g.hw.balloon_mb += mb;
            }
        }
    }
}
