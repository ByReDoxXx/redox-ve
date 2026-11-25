//! Dört göz ilkesi: geri alınamayan işlemler (yedek ve konuk silme) ikinci bir yöneticinin onayını bekler.
//!
//! Tek bir yönetici hesabı ele geçirilse bile yedekler tek başına silinemez. İstek kaydedilir; **başka** bir
//! yönetici onaylayınca işlem yürür. İstek sahibi kendi isteğini onaylayamaz, ama geri çekebilir.

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;

/// Onaylanmayan istek bu süre sonunda düşer.
pub const APPROVAL_TTL_SECS: u64 = 24 * 3600;
pub const MAX_APPROVALS: usize = 64;

pub const BACKUP_DELETE: &str = "backup-delete";
pub const GUEST_DELETE: &str = "guest-delete";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    /// Hiç yeniden kullanılmayan sıra numarası: eski bir onay bağlantısı başka bir isteğe uygulanamaz.
    pub id: u64,
    /// `backup-delete` ya da `guest-delete`.
    pub action: String,
    /// Yedeğin ya da konuğun kimliği.
    pub target: String,
    /// İsteği yapan yönetici.
    pub by: String,
    pub at: u64,
}

impl Approval {
    pub fn describe(&self) -> String {
        match self.action.as_str() {
            BACKUP_DELETE => format!("{} yedeğini silmek", self.target),
            GUEST_DELETE => format!("{} kimlikli konuğu silmek", self.target),
            other => format!("{other} {}", self.target),
        }
    }
}

impl Cluster {
    fn expire_approvals(&mut self, now: u64) {
        self.approvals.retain(|a| a.at + APPROVAL_TTL_SECS > now);
    }

    /// Onay bekleyen istekler (süresi dolmamış olanlar), eskiden yeniye.
    pub fn pending_approvals(&self, now: u64) -> Vec<&Approval> {
        self.approvals.iter().filter(|a| a.at + APPROVAL_TTL_SECS > now).collect()
    }

    /// İstek kaydeder; aynı işlem için bekleyen istek varsa onun numarasını döndürür.
    pub fn request_approval(&mut self, action: &str, target: &str, by: &str, now: u64) -> Result<u64, Error> {
        self.expire_approvals(now);
        if let Some(existing) = self.approvals.iter().find(|a| a.action == action && a.target == target) {
            return Ok(existing.id);
        }
        if self.approvals.len() >= MAX_APPROVALS {
            return Err(Error::InvalidSpec(format!("onay bekleyen istek sayısı sınırda ({MAX_APPROVALS}); önce bekleyenleri sonuçlandırın")));
        }
        self.approval_seq += 1;
        self.approvals.push(Approval { id: self.approval_seq, action: action.to_string(), target: target.to_string(), by: by.to_string(), at: now });
        Ok(self.approval_seq)
    }

    /// İsteği onaylayıp kayıttan alır; işlemi çağıran yürütür. İstek sahibi kendi isteğini onaylayamaz.
    pub fn take_approval(&mut self, id: u64, approver: &str, now: u64) -> Result<Approval, Error> {
        self.expire_approvals(now);
        let at = self.approvals.iter().position(|a| a.id == id).ok_or_else(|| Error::InvalidSpec(format!("{id} numaralı onay isteği yok (sonuçlanmış ya da süresi dolmuş olabilir)")))?;
        if self.approvals[at].by == approver {
            return Err(Error::InvalidSpec("kendi isteğinizi onaylayamazsınız: başka bir yönetici onaylamalı".to_string()));
        }
        Ok(self.approvals.remove(at))
    }

    /// İsteği reddeder ya da geri çeker (isteği yapan da yapabilir).
    pub fn reject_approval(&mut self, id: u64, now: u64) -> Result<Approval, Error> {
        self.expire_approvals(now);
        let at = self.approvals.iter().position(|a| a.id == id).ok_or_else(|| Error::InvalidSpec(format!("{id} numaralı onay isteği yok (sonuçlanmış ya da süresi dolmuş olabilir)")))?;
        Ok(self.approvals.remove(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;

    #[test]
    fn a_second_admin_must_approve_and_numbers_are_never_reused() {
        let mut c = Cluster::demo(NOW);
        let first = c.request_approval(BACKUP_DELETE, "vm-103-1", "ayse", NOW).unwrap();
        assert_eq!(c.request_approval(BACKUP_DELETE, "vm-103-1", "mehmet", NOW + 5).unwrap(), first, "aynı işlem iki kez kuyruğa girmez");
        let second = c.request_approval(GUEST_DELETE, "103", "ayse", NOW).unwrap();
        assert_eq!((first, second), (1, 2));
        assert_eq!(c.pending_approvals(NOW).len(), 2);
        assert_eq!(c.approvals[0].describe(), "vm-103-1 yedeğini silmek");

        assert!(c.take_approval(first, "ayse", NOW).unwrap_err().to_string().contains("kendi isteğinizi onaylayamazsınız"));
        assert_eq!(c.take_approval(first, "mehmet", NOW).unwrap().target, "vm-103-1");
        assert!(c.take_approval(first, "mehmet", NOW).is_err(), "onay bir kez kullanılır");
        // İstek sahibi geri çekebilir; numara yeniden kullanılmaz.
        assert_eq!(c.reject_approval(second, NOW).unwrap().by, "ayse");
        assert_eq!(c.request_approval(GUEST_DELETE, "104", "ayse", NOW).unwrap(), 3);
        // Süresi dolan istek onaylanamaz.
        assert!(c.pending_approvals(NOW + APPROVAL_TTL_SECS).is_empty());
        assert!(c.take_approval(3, "mehmet", NOW + APPROVAL_TTL_SECS).unwrap_err().to_string().contains("süresi dolmuş"));
    }
}
