//! Dört göz ilkesi: silme istekleri, onay ve ret.

use super::*;

impl AppState {
    pub fn set_four_eyes(&self, on: bool) {
        self.0.four_eyes.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn four_eyes(&self) -> bool {
        self.0.four_eyes.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Kullanıcının istediği yedek silme. Dört göz ilkesi açıksa silmez: onay isteği açar ve numarasını
    /// döndürür. (Zamanlayıcının budaması ve iç temizlikler bu yoldan geçmez.)
    pub fn request_backup_delete(&self, id: &str, user: &str) -> Result<Option<u64>, Error> {
        if !self.four_eyes() {
            return self.backup_delete(id, user).map(|()| None);
        }
        self.mutate(|c| {
            c.backup_deletable(id, now())?;
            self.open_approval(c, redox_core::approvals::BACKUP_DELETE, id, user).map(Some)
        })
    }

    /// Kullanıcının istediği konuk silme; dört göz ilkesi açıksa onay isteği açar.
    pub fn request_guest_delete(&self, vmid: VmId, user: &str) -> Result<Option<u64>, Error> {
        if !self.four_eyes() {
            return self.delete_guest(vmid, user).map(|()| None);
        }
        self.mutate(|c| {
            c.check_deletable(vmid)?;
            self.open_approval(c, redox_core::approvals::GUEST_DELETE, &vmid.to_string(), user).map(Some)
        })
    }

    pub(super) fn open_approval(&self, c: &mut Cluster, action: &str, target: &str, user: &str) -> Result<u64, Error> {
        let known = c.approvals.len();
        let id = c.request_approval(action, target, user, now())?;
        if c.approvals.len() > known {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("approval request #{id} {action} {target}"), &Ok(()), now(), None);
        }
        Ok(id)
    }

    /// Bekleyen isteği onaylar ve işlemi yürütür; ne yapıldığını döndürür. Onaylayan, isteği yapandan başka
    /// biri olmalıdır. İşlem iki yöneticinin adıyla günlüğe geçer.
    pub fn approve(&self, id: u64, user: &str) -> Result<String, Error> {
        let approval = self.mutate(|c| c.take_approval(id, user, now()))?;
        let who = format!("{}+{user}", approval.by);
        match approval.action.as_str() {
            redox_core::approvals::BACKUP_DELETE => self.backup_delete(&approval.target, &who)?,
            redox_core::approvals::GUEST_DELETE => {
                let vmid = approval.target.parse().map_err(|_| Error::InvalidSpec(format!("geçersiz konuk kimliği: {}", approval.target)))?;
                self.delete_guest(vmid, &who)?
            }
            other => return Err(Error::InvalidSpec(format!("bilinmeyen onay türü: {other}"))),
        }
        Ok(format!("onaylandı: {}", approval.describe()))
    }

    /// Bekleyen isteği reddeder (ya da isteği yapan geri çeker).
    pub fn reject(&self, id: u64, user: &str) -> Result<(), Error> {
        self.mutate(|c| {
            let approval = c.reject_approval(id, now())?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("approval reject #{id} {} {}", approval.action, approval.target), &Ok(()), now(), None);
            Ok(())
        })
    }
}
