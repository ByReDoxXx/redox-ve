//! Çalışan sanal makinenin CD sürücüsünde kalıp değiştirme.

use super::*;

/// Konuğun CD sürücüsü sayısı (1: asıl kalıp, 2: ikinci kalıp).
pub const CD_DRIVES: u8 = 2;

impl Cluster {
    /// Kalıp çalışırken değiştirilebilir mi? `name` boşsa sürücü boşaltılır.
    pub fn check_media_change(&self, vmid: VmId, drive: u8, name: &str) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.kind != GuestKind::Vm {
            return Err(Error::InvalidSpec("ISO yalnızca sanal makinelere takılabilir".to_string()));
        }
        if !(1..=CD_DRIVES).contains(&drive) {
            return Err(Error::InvalidSpec(format!("CD sürücüsü 1 ile {CD_DRIVES} arasında olmalı")));
        }
        if !name.is_empty() && !valid_iso_name(name) {
            return Err(Error::InvalidSpec(format!("geçersiz ISO adı: {name}")));
        }
        if !matches!(g.status, GuestStatus::Running | GuestStatus::Paused) {
            return Err(Error::InvalidSpec(format!("{vmid} çalışmıyor; kapalı konukta kalıbı ayarlardan değiştirin")));
        }
        Ok(())
    }

    /// Sürücü kalıbı değiştirdikten sonra kayda işler.
    pub fn note_media_changed(&mut self, vmid: VmId, drive: u8, name: &str) {
        if let Ok(g) = self.guest_mut(vmid) {
            let value = Some(name.to_string()).filter(|n| !n.is_empty());
            match drive {
                1 => g.iso = value,
                _ => g.hw.iso2 = value,
            }
            g.hw.media_changed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate_blocker;

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn media_changes_only_in_a_running_vm_and_only_valid_names() {
        let mut c = Cluster::demo(NOW);
        let vm = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running && g.lock.is_none()).unwrap().vmid;
        assert!(c.check_media_change(vm, 1, "virtio-win.iso").is_ok());
        assert!(c.check_media_change(vm, 2, "").is_ok(), "boş ad sürücüyü boşaltır");
        assert!(c.check_media_change(vm, 3, "a.iso").unwrap_err().to_string().contains("1 ile 2"));
        assert!(c.check_media_change(vm, 1, "../a.iso").unwrap_err().to_string().contains("geçersiz ISO"));
        let stopped = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).unwrap().vmid;
        assert!(c.check_media_change(stopped, 1, "a.iso").unwrap_err().to_string().contains("çalışmıyor"));
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
        assert!(c.check_media_change(ct, 1, "a.iso").is_err());

        c.note_media_changed(vm, 2, "virtio-win.iso");
        let g = c.guest(vm).unwrap();
        assert_eq!((g.hw.iso2.as_deref(), g.hw.media_changed), (Some("virtio-win.iso"), true));
        assert!(migrate_blocker(g).is_some(), "kalıbı değişen konuk canlı taşınmaz");
        c.note_media_changed(vm, 2, "");
        assert!(c.guest(vm).unwrap().hw.iso2.is_none() && migrate_blocker(c.guest(vm).unwrap()).unwrap().contains("çalışırken değiştirildi"));
    }
}
