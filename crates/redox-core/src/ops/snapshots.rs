//! Anlık görüntü kuralları.

use super::*;

impl Cluster {
    /// Yeni anlık görüntü alınabilir mi?
    pub fn check_snapshot_new(&self, vmid: VmId, name: &str) -> Result<(), Error> {
        let guest = self.guest(vmid)?;
        if !valid_snapshot_name(name) {
            return Err(Error::InvalidSpec(format!(
                "geçersiz anlık görüntü adı \"{name}\": 1-32 karakter; küçük harf, rakam, tire ve alt çizgi kullanın"
            )));
        }
        if guest.snapshots.iter().any(|s| s.name == name) {
            return Err(Error::SnapshotExists(name.to_string()));
        }
        if guest.snapshots.len() >= MAX_SNAPSHOTS {
            return Err(Error::InvalidSpec(format!(
                "bir konukta en fazla {MAX_SNAPSHOTS} anlık görüntü tutulabilir; önce eskilerden silin"
            )));
        }
        Ok(())
    }

    pub fn check_snapshot(&self, vmid: VmId, name: &str) -> Result<(), Error> {
        if self.guest(vmid)?.snapshots.iter().any(|s| s.name == name) {
            Ok(())
        } else {
            Err(Error::SnapshotNotFound(name.to_string()))
        }
    }

    pub fn add_snapshot(&mut self, vmid: VmId, name: &str, now: u64) {
        self.add_snapshot_kind(vmid, name, now, false, false);
    }

    /// `disk_only`: bellek durumu olmadan alınmış (üretim) denetim noktası; `quiesced`: dosya sistemleri dondurulmuştu.
    pub fn add_snapshot_kind(&mut self, vmid: VmId, name: &str, now: u64, disk_only: bool, quiesced: bool) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.snapshots.push(Snapshot { name: name.to_string(), created: now, disk_only, quiesced });
        }
    }

    /// Belleksiz görüntüye, konuk çalışırken dönülemez: QEMU'nun yükleyeceği bir bellek durumu yoktur.
    pub fn check_rollback_state(&self, vmid: VmId, name: &str) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        let disk_only = g.snapshots.iter().any(|s| s.name == name && s.disk_only);
        if disk_only && g.status != GuestStatus::Stopped {
            return Err(Error::RequiresStopped { vmid, what: "bellek durumu olmayan (üretim) denetim noktasına dönmek".to_string() });
        }
        Ok(())
    }

    pub fn remove_snapshot(&mut self, vmid: VmId, name: &str) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.snapshots.retain(|s| s.name != name);
        }
    }
}
