//! Hata toleransı (QEMU COLO): çalışan sanal makinenin eş sunucuda **eşzamanlı çalışan** bir ikizi vardır.
//! Birincil ölürse ikiz, konuk yeniden başlamadan kaldığı yerden sürer.
//!
//! En büyük tehlike beyin bölünmesidir: iki sunucunun arası koptuğunda her biri ötekini ölmüş sanır. Bu yüzden
//! devralma kararı üçüncü bir sunucudaki **hakeme** aittir: aynı koruma için ilk oy isteyen taraf kazanır,
//! kaybeden kendi konuğunu durdurur. Hakeme ulaşamayan taraf hiçbir şeyi devralmaz.

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;
use crate::model::{Guest, GuestKind, VmId, NAT};

pub const MAX_PROTECTED: usize = 8;
/// Hakemin sakladığı karar sayısı.
pub const MAX_VOTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FtSide {
    Primary,
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FtState {
    /// Konuk çalışınca ya da sıradaki denemede koruma kurulacak.
    #[default]
    Waiting,
    /// İkiz açıldı; diskler eşitleniyor ve bellek aktarılıyor. Konuk henüz korunmuyor.
    Preparing,
    /// İkiz eşzamanlı çalışıyor.
    Protected,
    /// İkiz kayboldu; konuk tek başına sürüyor.
    Unprotected,
    /// Hakeme ulaşılamadı: aynı konuk iki yerde çalışmasın diye konuk duraklatıldı.
    Suspended,
    /// İkiz devraldı. Buradaki kayıt eskidir; koruma bilerek kapatılmadan başlatılamaz.
    HandedOver,
}

impl FtState {
    pub fn label(self) -> &'static str {
        match self {
            FtState::Waiting => "bekliyor",
            FtState::Preparing => "hazırlanıyor",
            FtState::Protected => "korunuyor",
            FtState::Unprotected => "korumasız",
            FtState::Suspended => "hakem bekliyor (duraklatıldı)",
            FtState::HandedOver => "yedek sunucu devraldı",
        }
    }
}

/// Birincil taraf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protection {
    pub vmid: VmId,
    /// İkizi çalıştıran eş sunucu ve hakem (iki sunucuda da aynı adla tanımlı).
    pub peer: String,
    pub witness: String,
    #[serde(default)]
    pub state: FtState,
    #[serde(default)]
    pub since: u64,
    #[serde(default)]
    pub error: Option<String>,
    /// Her kurulumda artar; hakem kararı kuruluma özgüdür.
    #[serde(default)]
    pub generation: u32,
}

/// İkincil taraf: eş sunucudaki konuğun burada çalışan ikizi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standby {
    pub origin: String,
    /// Kaynaktaki ve buradaki kimlik.
    pub source: VmId,
    pub vmid: VmId,
    pub witness: String,
    pub generation: u32,
    /// Birincilden en son ne zaman haber alındı.
    pub last_beat: u64,
}

/// Hakemin bir korumaya verdiği karar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FtVote {
    pub key: String,
    pub winner: FtSide,
    pub at: u64,
}

/// Hakem anahtarı: kaynak, konuk ve kurulum numarası.
pub fn ft_key(origin: &str, vmid: VmId, generation: u32) -> String {
    format!("{origin}-{vmid}-{generation}")
}

/// Konuk hata toleransına uygun mu? Değilse nedeni. Kapsam bilerek dardır: yalnızca sınanmış donanım.
pub fn ft_blocker(g: &Guest) -> Option<&'static str> {
    if g.kind != GuestKind::Vm {
        return Some("hata toleransı yalnızca sanal makinelerde kullanılır");
    }
    if g.hw.template {
        return Some("şablon korunmaz");
    }
    if !g.hw.disks.is_empty() {
        return Some("hata toleransı tek diskli konukta kullanılır; ek diskleri kaldırın");
    }
    if g.bridge == NAT || !g.hw.nics.is_empty() {
        return Some("hata toleransı köprüye bağlı tek ağ kartı ister (NAT ve ek kart desteklenmez)");
    }
    if g.hw.vlan > 0 || g.hw.firewall || g.hw.nic_guard || g.hw.net_mbps > 0 {
        return Some("hata toleransında VLAN etiketi, konuk güvenlik duvarı, kart koruması ve ağ hız sınırı kullanılamaz");
    }
    if g.hw.max_cores > 0 || g.hw.max_mem_mb > 0 || g.hw.balloon || g.hw.balloon_mb > 0 || g.hw.mem_min_mb > 0 {
        return Some("hata toleransında çalışırken çekirdek ya da bellek ekleme ve bellek balonu kullanılamaz");
    }
    if g.hw.firmware == crate::hw::Firmware::Uefi || g.hw.tpm || !g.hw.usb.is_empty() || !g.hw.pci.is_empty() || !g.hw.shares.is_empty() {
        return Some("hata toleransında UEFI, sanal TPM, aygıt aktarımı ve paylaşılan klasör kullanılamaz");
    }
    if g.iso.is_some() || g.hw.cloud_init() || g.hw.base.is_some() || g.hw.instant.is_some() || g.hw.disk_mbps > 0 || g.hw.disk_iops > 0 {
        return Some("hata toleransında kurulum kalıbı, cloud-init, bağlı disk ve disk hız sınırı kullanılamaz");
    }
    if g.hw.pinned_memory() || !g.hw.cpu_affinity.is_empty() {
        return Some("hata toleransında çekirdek sabitleme, NUMA bağı ve büyük sayfalar kullanılamaz");
    }
    if g.hw.uses_compat() || g.hw.media_changed {
        return Some("hata toleransında disk ve ağ kartı virtio olmalı, ikinci kalıp takılı olmamalı");
    }
    None
}

impl Cluster {
    pub fn protection(&self, vmid: VmId) -> Option<&Protection> {
        self.protections.iter().find(|p| p.vmid == vmid)
    }

    pub fn standby_of(&self, vmid: VmId) -> Option<&Standby> {
        self.standbys.iter().find(|s| s.vmid == vmid)
    }

    pub fn standby(&self, origin: &str, source: VmId) -> Option<&Standby> {
        self.standbys.iter().find(|s| s.origin == origin && s.source == source)
    }

    /// Korumayı açar. Konuk kapalı olmalıdır: diski korumaya uygun biçimde ancak açılışta sarılır.
    pub fn ft_enable(&mut self, vmid: VmId, peer: &str, witness: &str, now: u64) -> Result<(), Error> {
        if peer == witness {
            return Err(Error::InvalidSpec("hakem, ikizi çalıştıran sunucudan farklı bir sunucu olmalı".to_string()));
        }
        if self.protection(vmid).is_some() {
            return Err(Error::InvalidSpec(format!("{vmid} zaten korunuyor")));
        }
        if self.protections.len() >= MAX_PROTECTED {
            return Err(Error::InvalidSpec(format!("en fazla {MAX_PROTECTED} konuk korunabilir")));
        }
        if self.mirror(vmid).is_some() {
            return Err(Error::InvalidSpec(format!("{vmid} için disk yansıtması açık; hata toleransı diski kendisi eşitler, önce yansıtmayı kapatın")));
        }
        let g = self.guest(vmid)?;
        if let Some(why) = ft_blocker(g) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        if g.lock.is_some() || g.status != crate::model::GuestStatus::Stopped || g.hw.suspended {
            return Err(Error::InvalidSpec(format!("{vmid} kapalı olmalı: hata toleransı konuk sonraki açılışında devreye girer")));
        }
        self.guest_mut(vmid)?.hw.ft = true;
        // Eski bir korumadan kalan numara yinelenmesin: hakemdeki eski kararlar yeni kuruluma uygulanmamalı.
        let generation = (now % u64::from(u32::MAX / 2)) as u32;
        self.protections.push(Protection { vmid, peer: peer.to_string(), witness: witness.to_string(), state: FtState::Waiting, since: now, error: None, generation });
        self.protections.sort_by_key(|p| p.vmid);
        Ok(())
    }

    /// Korumayı kaldırır. Konuk bir sonraki açılışında olağan disk düzenine döner.
    pub fn ft_disable(&mut self, vmid: VmId) -> Result<Protection, Error> {
        let at = self.protections.iter().position(|p| p.vmid == vmid).ok_or_else(|| Error::InvalidSpec(format!("{vmid} korunmuyor")))?;
        if let Ok(g) = self.guest_mut(vmid) {
            g.hw.ft = false;
        }
        Ok(self.protections.remove(at))
    }

    pub fn ft_set(&mut self, vmid: VmId, state: FtState, error: Option<String>, now: u64) -> bool {
        let Some(p) = self.protections.iter_mut().find(|p| p.vmid == vmid) else { return false };
        let changed = p.state != state;
        if changed {
            p.since = now;
        }
        (p.state, p.error) = (state, error);
        changed
    }

    /// Yeni bir kurulum: numara artar.
    pub fn ft_next_generation(&mut self, vmid: VmId) -> Result<u32, Error> {
        let p = self.protections.iter_mut().find(|p| p.vmid == vmid).ok_or_else(|| Error::InvalidSpec(format!("{vmid} korunmuyor")))?;
        p.generation = p.generation.wrapping_add(1);
        Ok(p.generation)
    }

    /// Hakem: `key` için kazananı döndürür. İlk soran kazanır; sonraki sorular aynı yanıtı alır.
    pub fn ft_arbitrate(&mut self, key: &str, side: FtSide, now: u64) -> Result<FtSide, Error> {
        if key.is_empty() || key.len() > 64 || !key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(Error::InvalidSpec("geçersiz hakem anahtarı".to_string()));
        }
        if let Some(vote) = self.ft_votes.iter().find(|v| v.key == key) {
            return Ok(vote.winner);
        }
        self.ft_votes.insert(0, FtVote { key: key.to_string(), winner: side, at: now });
        self.ft_votes.truncate(MAX_VOTES);
        Ok(side)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GuestStatus;

    fn plain_vm(c: &mut Cluster) -> VmId {
        let vmid = c.guests.iter().find(|g| g.kind == GuestKind::Vm && !g.hw.template).unwrap().vmid;
        let g = c.guest_mut(vmid).unwrap();
        g.status = GuestStatus::Stopped;
        g.bridge = "vds-ft".to_string();
        g.iso = None;
        g.hw = Default::default();
        vmid
    }

    #[test]
    fn only_a_stopped_vm_with_the_tested_hardware_can_be_protected() {
        let mut c = Cluster::demo(1000);
        let vmid = plain_vm(&mut c);
        assert_eq!(ft_blocker(c.guest(vmid).unwrap()), None);
        assert!(c.ft_enable(vmid, "b", "b", 1).unwrap_err().to_string().contains("farklı bir sunucu"));
        c.guest_mut(vmid).unwrap().status = GuestStatus::Running;
        assert!(c.ft_enable(vmid, "b", "h", 1).unwrap_err().to_string().contains("kapalı olmalı"));
        c.guest_mut(vmid).unwrap().status = GuestStatus::Stopped;
        let base = c.guest(vmid).unwrap().clone();
        let changed = |f: &dyn Fn(&mut Guest)| {
            let mut g = base.clone();
            f(&mut g);
            ft_blocker(&g).unwrap_or("")
        };
        for (needle, reason) in [
            ("tek diskli", changed(&|g| g.hw.disks.push(crate::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 }))),
            ("köprüye bağlı tek ağ kartı", changed(&|g| g.bridge = NAT.to_string())),
            ("VLAN etiketi", changed(&|g| g.hw.vlan = 5)),
            ("bellek balonu", changed(&|g| g.hw.balloon = true)),
            ("bellek balonu", changed(&|g| g.hw.max_cores = 8)),
            ("UEFI", changed(&|g| g.hw.tpm = true)),
            ("kurulum kalıbı", changed(&|g| g.iso = Some("a.iso".into()))),
        ] {
            assert!(reason.contains(needle), "{needle}: {reason}");
        }
        c.ft_enable(vmid, "b", "h", 100).unwrap();
        assert!(c.guest(vmid).unwrap().hw.ft && c.protection(vmid).unwrap().state == FtState::Waiting);
        assert!(c.ft_enable(vmid, "b", "h", 100).unwrap_err().to_string().contains("zaten"));
        assert!(c.mirror_add(vmid, "b", 1).unwrap_err().to_string().contains("hata toleransı"), "korunan konuk yansıtılmaz");
        let first = c.ft_next_generation(vmid).unwrap();
        assert_eq!(c.ft_next_generation(vmid).unwrap(), first + 1);
        c.ft_disable(vmid).unwrap();
        assert!(!c.guest(vmid).unwrap().hw.ft && c.ft_disable(vmid).is_err());
        c.standbys.push(Standby { origin: "a".into(), source: vmid, vmid, witness: "h".into(), generation: 1, last_beat: 0 });
        assert!(c.mirror_add(vmid, "b", 1).unwrap_err().to_string().contains("ikizi"), "ikiz yansıtılmaz");
    }

    #[test]
    fn the_witness_lets_exactly_one_side_win() {
        let mut c = Cluster::demo(1000);
        let key = ft_key("pve-a", 100, 7);
        assert_eq!(c.ft_arbitrate(&key, FtSide::Secondary, 1).unwrap(), FtSide::Secondary);
        // Geç kalan birincil kaybettiğini öğrenir; yineleyen kazanan aynı yanıtı alır.
        assert_eq!(c.ft_arbitrate(&key, FtSide::Primary, 2).unwrap(), FtSide::Secondary);
        assert_eq!(c.ft_arbitrate(&key, FtSide::Secondary, 3).unwrap(), FtSide::Secondary);
        // Yeni kurulum yeni bir karardır.
        assert_eq!(c.ft_arbitrate(&ft_key("pve-a", 100, 8), FtSide::Primary, 4).unwrap(), FtSide::Primary);
        for bad in ["", "A", "a b", "../x"] {
            assert!(c.ft_arbitrate(bad, FtSide::Primary, 5).is_err(), "{bad}");
        }
        for i in 0..MAX_VOTES + 5 {
            c.ft_arbitrate(&format!("k-{i}"), FtSide::Primary, 9).unwrap();
        }
        assert_eq!(c.ft_votes.len(), MAX_VOTES);
    }
}
