//! Canlı taşıma: çalışan sanal makinenin başka bir sunucuya geçmesinin kuralları.

use super::*;

/// Taşıma anahtarı: 32 baytın onaltılık yazımı. İki QEMU arasındaki bağlantıyı şifreler; her taşıma için yenisi üretilir.
pub fn valid_migrate_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Taşıma bağlantısının adresi: bir IP adresi ya da sunucu adı. QEMU'ya argüman olarak gider; yalnızca adres
/// karakterleri kabul edilir.
pub fn valid_migrate_host(host: &str) -> bool {
    !host.is_empty() && host.len() <= 253 && !host.starts_with('-') && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
}

/// Konuk canlı taşınabilir mi? Taşınamıyorsa nedeni. Hedef QEMU konuğun kaydından aynı donanımla açılır;
/// kayıttan yeniden kurulamayan ya da sunucuya bağlı olan donanım taşımayı engeller.
pub fn migrate_blocker(g: &Guest) -> Option<&'static str> {
    if g.kind != GuestKind::Vm {
        return Some("yalnızca sanal makineler canlı taşınır");
    }
    if g.hw.template {
        return Some("şablon canlı taşınmaz");
    }
    if g.hw.firmware == crate::hw::Firmware::Uefi || g.hw.tpm {
        return Some("UEFI ya da sanal TPM'li konuk canlı taşınamaz (donanım yazılımının durumu disklerle birlikte gitmez); kapatıp öyle taşıyın");
    }
    if !g.hw.usb.is_empty() || !g.hw.pci.is_empty() {
        return Some("sunucu aygıtı aktarılmış konuk canlı taşınamaz; kapatıp öyle taşıyın");
    }
    if !g.hw.shares.is_empty() {
        return Some("paylaşılan klasörü olan konuk canlı taşınamaz; kapatıp öyle taşıyın");
    }
    if g.iso.is_some() {
        return Some("kurulum kalıbı takılı konuk canlı taşınamaz; önce kalıbı çıkarın (konuk kapalıyken)");
    }
    if g.hw.cloud_init() {
        return Some("cloud-init sürücüsü takılı konuk canlı taşınamaz; kapatıp öyle taşıyın");
    }
    if g.hw.base.is_some() || g.hw.instant.is_some() {
        return Some("diski başka bir diske ya da yedeğe bağlı konuk canlı taşınamaz; önce bağımsız hale getirin");
    }
    None
}

impl Cluster {
    /// Konuk bu sunucudan canlı taşınabilir mi?
    pub fn check_migrate_out(&self, vmid: VmId) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        if let Some(task) = g.lock {
            return Err(Error::Locked { vmid, task });
        }
        if let Some(why) = migrate_blocker(g) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        if g.status != GuestStatus::Running {
            return Err(Error::InvalidSpec(format!("{vmid} çalışmıyor; canlı taşıma çalışan konuk içindir (kapalı konuk olağan yolla kopyalanır)")));
        }
        Ok(())
    }

    /// Konuk başka sunucuya geçti: burada kapalıdır ve kendiliğinden yeniden açılmamalıdır (aynı konuk iki
    /// sunucuda birden çalışmasın). Kayıt ve diskler, hedefte her şeyin yolunda olduğu görülene kadar durur.
    pub fn after_migrated_out(&mut self, vmid: VmId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.status = GuestStatus::Stopped;
            g.started_at = None;
            g.cpu_pct = 0;
            g.mem_used_mb = 0;
            g.onboot = false;
            g.hw.restart = false;
            g.hw.dimms.clear();
            if !g.tags.iter().any(|t| t == MIGRATED_TAG) && g.tags.len() < MAX_TAGS {
                g.tags.push(MIGRATED_TAG.to_string());
            }
        }
    }
}

/// Canlı taşınmış konuğun kaynakta kalan kaydına konan etiket.
pub const MIGRATED_TAG: &str = "tasindi";
