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

    /// Ek disk silme (veriyle birlikte); dört göz ilkesi açıksa onay isteği açar.
    pub fn request_disk_remove(&self, vmid: VmId, disk: u8, user: &str) -> Result<Option<u64>, Error> {
        if !self.four_eyes() {
            return self.disk_remove(vmid, disk, user).map(|()| None);
        }
        self.mutate(|c| {
            if !c.guest(vmid)?.hw.disks.iter().any(|d| d.id == disk) {
                return Err(Error::InvalidSpec(format!("{vmid} kimlikli konukta {disk} numaralı disk yok")));
            }
            self.open_approval(c, redox_core::approvals::DISK_REMOVE, &format!("{vmid}:{disk}"), user).map(Some)
        })
    }

    /// Yedek işini silme (işi silen, yedeklemeyi sessizce durdurabilir); dört göz ilkesi açıksa onay isteği açar.
    pub fn request_job_remove(&self, name: &str, user: &str) -> Result<Option<u64>, Error> {
        if !self.four_eyes() {
            return self.job_remove(name).map(|()| None);
        }
        self.mutate(|c| {
            c.job(name)?;
            self.open_approval(c, redox_core::approvals::JOB_REMOVE, name, user).map(Some)
        })
    }

    /// Kullanıcı silme; dört göz ilkesi açıksa onay isteği açar.
    pub fn request_user_remove(&self, name: &str, user: &str) -> Result<Option<u64>, Error> {
        if !self.four_eyes() {
            return self.auth().edit(|u| u.remove_user(name)).map(|()| None).map_err(Error::InvalidSpec);
        }
        if !self.auth().list().iter().any(|u| u.name == name) {
            return Err(Error::InvalidSpec(format!("{name} adında bir kullanıcı yok")));
        }
        self.mutate(|c| self.open_approval(c, redox_core::approvals::USER_REMOVE, name, user).map(Some))
    }

    /// Rol değiştirme. Dört göz ilkesi açıkken birini yönetici yapmak onay ister: yoksa tek bir yönetici kendine
    /// ikinci bir yönetici hesabı açıp kendi silme isteklerini onunla onaylayabilirdi.
    pub fn request_user_role(&self, name: &str, role: crate::auth::Role, user: &str) -> Result<Option<u64>, Error> {
        use crate::auth::Role;
        let current = self.auth().list().iter().find(|u| u.name == name).map(|u| u.role).ok_or_else(|| Error::InvalidSpec(format!("kullanıcı bulunamadı: {name}")))?;
        if !(self.four_eyes() && role == Role::Admin && current != Role::Admin) {
            return self.auth().edit(|u| u.set_role(name, role)).map(|()| None).map_err(Error::InvalidSpec);
        }
        self.mutate(|c| self.open_approval(c, redox_core::approvals::USER_PROMOTE, name, user).map(Some))
    }

    /// Dört göz ilkesi açıkken bir yönetici, başka bir yöneticinin kimliğine panelden bürünemez: onun parolasını
    /// ya da ikinci adımını sıfırlayamaz, onun adına token üretemez. (Kurtarma yolu sunucudaki `redox user` komutlarıdır.)
    pub fn guard_other_admin(&self, target: &str, actor: &str) -> Result<(), String> {
        let admin = self.auth().list().iter().any(|u| u.name == target && u.role == crate::auth::Role::Admin);
        if self.four_eyes() && admin && target != actor {
            return Err(format!("dört göz ilkesi açıkken başka bir yöneticinin ({target}) parolası, ikinci adımı ve token'ları panelden değiştirilemez; sunucuda redox user komutlarını kullanın"));
        }
        Ok(())
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
            redox_core::approvals::DISK_REMOVE => {
                let parsed = approval.target.split_once(':').and_then(|(vmid, disk)| Some((vmid.parse().ok()?, disk.parse().ok()?)));
                let (vmid, disk) = parsed.ok_or_else(|| Error::InvalidSpec(format!("geçersiz disk hedefi: {}", approval.target)))?;
                self.disk_remove(vmid, disk, &who)?
            }
            redox_core::approvals::JOB_REMOVE => self.job_remove(&approval.target)?,
            // İkinci yönetici kendi hesabının silinmesini de onaylayabilir; son yöneticiyi silmeyi kullanıcı dosyası reddeder.
            redox_core::approvals::USER_REMOVE => self.auth().edit(|u| u.remove_user(&approval.target)).map_err(Error::InvalidSpec)?,
            redox_core::approvals::USER_PROMOTE => self.auth().edit(|u| u.set_role(&approval.target, crate::auth::Role::Admin)).map_err(Error::InvalidSpec)?,
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
