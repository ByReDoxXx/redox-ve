//! Ortak depoda disk sahipliği devredilen konuğun kaydı üzerindeki kurallar.

use crate::{Cluster, Error, Guest, GuestKind, GuestStatus, VmId};

/// Konuk ortak depoda başka sunucuya devredilebilir mi? Devredilemiyorsa nedeni. Yalnızca paylaşımda duran
/// diskler geçer; bu sunucunun veri dizininde kalan her şey devri engeller.
pub fn handover_blocker(g: &Guest) -> Option<&'static str> {
    if g.kind != GuestKind::Vm {
        return Some("ortak depoda yalnızca sanal makineler devredilir");
    }
    if g.hw.template || g.hw.base.is_some() || g.hw.instant.is_some() {
        return Some("şablon, bağlı klon ya da yedeğe bağlı konuk devredilemez");
    }
    if g.hw.suspended {
        return Some("askıdaki konuk devredilemez: bellek durumu bu sunucuda durur; önce sürdürün ya da askı durumunu atın");
    }
    if g.hw.firmware == crate::hw::Firmware::Uefi || g.hw.tpm {
        return Some("UEFI ya da sanal TPM'li konuk devredilemez: donanım yazılımının durumu bu sunucuda durur");
    }
    if !g.hw.usb.is_empty() || !g.hw.pci.is_empty() || !g.hw.shares.is_empty() {
        return Some("sunucuya bağlı aygıtı ya da paylaşılan klasörü olan konuk devredilemez");
    }
    None
}

impl Cluster {
    pub fn not_handed_over(&self, vmid: VmId) -> Result<(), Error> {
        match self.guest(vmid)?.hw.handed_to.as_deref() {
            None => Ok(()),
            Some(to) => Err(Error::InvalidSpec(format!("{vmid} diskleri {to} sunucusuna devredildi: burada yalnızca kaydı duruyor; kaydı silebilirsiniz"))),
        }
    }

    /// Devredilecek konuk: kapalı, boşta ve devre engel bir ayarı olmayan.
    pub fn check_release(&self, vmid: VmId) -> Result<&Guest, Error> {
        self.not_handed_over(vmid)?;
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if g.status != GuestStatus::Stopped {
            return Err(Error::RequiresStopped { vmid, what: "disk sahipliğini devretmek".to_string() });
        }
        if let Some(why) = handover_blocker(g) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        if self.protection(vmid).is_some() || self.mirror(vmid).is_some() {
            return Err(Error::InvalidSpec(format!("{vmid} için hata toleransı ya da disk yansıtma açık; önce kapatın")));
        }
        self.no_linked_clones(vmid, "devredilemez")?;
        Ok(g)
    }

    /// Diskler `to` sunucusuna geçti. Kayıt kapalı kalır ve kendiliğinden hiçbir şey yapmaz.
    pub fn mark_handed_over(&mut self, vmid: VmId, to: &str) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.handed_to = Some(to.to_string());
            g.onboot = false;
            g.hw.restart = false;
            g.hw.schedules.clear();
            g.backup_time = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, GuestPatch};

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn a_handed_over_record_can_only_be_stopped_or_deleted() {
        let mut c = Cluster::demo(NOW);
        let vm = 103;
        assert!(c.check_release(vm).is_ok());
        assert!(c.check_release(100).unwrap_err().to_string().contains("sahipliğini devretmek"), "çalışan konuk devredilmez");
        c.guest_mut(vm).unwrap().onboot = true;
        c.mark_handed_over(vm, "ankara");
        assert!(c.check_release(vm).unwrap_err().to_string().contains("ankara"));
        assert!(!c.guest(vm).unwrap().onboot);
        for (status, action) in [(GuestStatus::Stopped, Action::Start), (GuestStatus::Paused, Action::Resume)] {
            c.guest_mut(vm).unwrap().status = status;
            assert!(c.begin_action(vm, action, "u", NOW).unwrap_err().to_string().contains("ankara"), "{action:?}");
        }
        assert!(c.begin_op(vm, "vm backup".into(), "u", NOW, None).unwrap_err().to_string().contains("devredildi"));
        // Yanlışlıkla çalışır görünen kayıt kapatılabilir.
        c.guest_mut(vm).unwrap().status = GuestStatus::Running;
        assert!(c.begin_action(vm, Action::Stop, "u", NOW).is_ok());
        let mut c = Cluster::demo(NOW);
        c.mark_handed_over(vm, "ankara");
        c.update_guest(vm, &GuestPatch { tags: Some(vec!["eski".into()]), ..Default::default() }, "u", NOW).unwrap();
        c.delete_guest(vm, "u", NOW).unwrap();
    }

    #[test]
    fn only_guests_whose_whole_state_is_on_the_share_can_move() {
        let base = Cluster::demo(0).guests.iter().find(|g| g.vmid == 103).unwrap().clone();
        assert!(handover_blocker(&base).is_none());
        for (label, change) in [
            ("suspended", Box::new(|g: &mut Guest| g.hw.suspended = true) as Box<dyn Fn(&mut Guest)>),
            ("tpm", Box::new(|g: &mut Guest| g.hw.tpm = true)),
            ("template", Box::new(|g: &mut Guest| g.hw.template = true)),
            ("usb", Box::new(|g: &mut Guest| g.hw.usb = vec!["046d:c52b".into()])),
            ("ct", Box::new(|g: &mut Guest| g.kind = GuestKind::Ct)),
        ] {
            let mut g = base.clone();
            change(&mut g);
            assert!(handover_blocker(&g).is_some(), "{label}");
        }
    }

    #[test]
    fn creation_cannot_claim_a_guest_was_handed_over() {
        let mut c = Cluster::demo(NOW);
        let spec = crate::GuestSpec { vmid: None, name: "a".into(), kind: GuestKind::Vm, node: c.nodes[0].id.clone(), cores: 1, memory_mb: 256, disk_gb: 1, storage: "local-zfs".into(), os: "x".into(), bridge: "nat".into(), onboot: false, iso: None, hw: crate::Hardware { handed_to: Some("x".into()), ..Default::default() } };
        let id = c.create_guest(&spec, "u", NOW).unwrap();
        assert!(c.guest(id).unwrap().hw.handed_to.is_none());
    }
}
