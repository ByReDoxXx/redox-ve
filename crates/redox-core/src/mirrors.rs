//! Eşzamanlı disk yansıtma: çalışan sanal makinenin her disk yazması aynı anda eş sunucudaki kopyaya da gider.
//!
//! İki taraf vardır. **Kaynak** (`Mirror`) konuğun çalıştığı sunucudur ve yansıtmayı yürütür. **Hedef**
//! (`MirrorStore`) kopyayı tutar. Kopya **kuşaklar** halindedir: yeniden eşitleme yeni dosyalara yapılır ve eski
//! eksiksiz kuşak, yenisi tam eşitlenene kadar silinmez; kaynak eşitleme sırasında ölse bile elde tutarlı bir
//! kopya kalır.

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;
use crate::model::{Guest, GuestKind, VmId};

pub const MAX_MIRRORS: usize = 16;
/// Hedefte kopyadan konuk açmak için kaynağın en az bu kadar saniyedir "eşit" bildirmemiş olması gerekir:
/// bildirimi süren bir yansıtma, konuğun kaynakta çalıştığını gösterir.
pub const PROMOTE_QUIET_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MirrorState {
    /// Konuk çalışınca ya da sıradaki denemede başlayacak.
    #[default]
    Waiting,
    /// İlk kopya sürüyor; hedefteki yeni kuşak henüz kullanılamaz.
    Syncing,
    /// Her yazma iki yere birden gidiyor.
    InSync,
    /// Yansıtma koptu: konuk çalışmayı sürdürüyor ama hedefteki kopya artık geride.
    Broken,
}

impl MirrorState {
    pub fn label(self) -> &'static str {
        match self {
            MirrorState::Waiting => "bekliyor",
            MirrorState::Syncing => "eşitleniyor",
            MirrorState::InSync => "eşit",
            MirrorState::Broken => "koptu",
        }
    }
}

/// Kaynak taraf: bu sunucudaki bir konuğun yansıtması.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mirror {
    pub vmid: VmId,
    /// Kopyayı tutan eş sunucunun buradaki adı.
    pub peer: String,
    #[serde(default)]
    pub state: MirrorState,
    /// Durum en son ne zaman değişti.
    #[serde(default)]
    pub since: u64,
    /// Kopyanın en son eşit görüldüğü an.
    #[serde(default)]
    pub last_in_sync: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    /// Hedefteki kuşağın numarası (0: henüz yok).
    #[serde(default)]
    pub generation: u32,
    /// İlk kopyanın ilerlemesi (bayt).
    #[serde(default)]
    pub done: u64,
    #[serde(default)]
    pub total: u64,
}

/// Hedef taraf: eş sunucudaki bir konuğun burada tutulan disk kopyası.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MirrorStore {
    /// Kaynağın kendini tanıttığı ad.
    pub origin: String,
    pub vmid: VmId,
    /// Konuğun kaynaktaki kaydı: kopyadan konuk açılırken donanımı buradan kurulur.
    pub guest: Guest,
    /// Şu an yazılan kuşak.
    pub generation: u32,
    /// Eksiksiz olduğu bildirilen en yeni kuşak; hiç yoksa kopyadan konuk açılamaz.
    #[serde(default)]
    pub complete: Option<u32>,
    /// Yazılan kuşak şu an kaynakla eşit mi?
    #[serde(default)]
    pub in_sync: bool,
    #[serde(default)]
    pub last_in_sync: Option<u64>,
    /// Disk kapısı.
    pub port: u16,
}

impl MirrorStore {
    /// Kuşağın sürücüdeki adı (dosya dizini). `origin` doğrulanmış olduğu için yol ayracı içeremez.
    pub fn replica_id(&self, generation: u32) -> String {
        format!("{}-{}-{}", self.origin, self.vmid, generation)
    }
}

pub fn valid_origin(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Konuğun diskleri yansıtılabilir mi? Yansıtılamıyorsa nedeni. Kopyadan açılacak konuk kayıttan yeniden
/// kurulabilmelidir.
pub fn mirror_blocker(g: &Guest) -> Option<&'static str> {
    if g.kind != GuestKind::Vm {
        return Some("yalnızca sanal makinelerin diskleri yansıtılır");
    }
    if g.hw.template {
        return Some("şablonun diskleri yansıtılmaz");
    }
    if g.hw.firmware == crate::hw::Firmware::Uefi || g.hw.tpm {
        return Some("UEFI ya da sanal TPM'li konuk yansıtılamaz (donanım yazılımının durumu disklerle birlikte gitmez)");
    }
    if g.hw.base.is_some() || g.hw.instant.is_some() {
        return Some("diski başka bir diske ya da yedeğe bağlı konuk yansıtılamaz; önce bağımsız hale getirin");
    }
    None
}

impl Cluster {
    pub fn mirror_add(&mut self, vmid: VmId, peer: &str, now: u64) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        if let Some(why) = mirror_blocker(g) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        if self.mirrors.iter().any(|m| m.vmid == vmid) {
            return Err(Error::InvalidSpec(format!("{vmid} zaten yansıtılıyor")));
        }
        if self.mirrors.len() >= MAX_MIRRORS {
            return Err(Error::InvalidSpec(format!("en fazla {MAX_MIRRORS} konuk yansıtılabilir")));
        }
        self.mirrors.push(Mirror { vmid, peer: peer.to_string(), state: MirrorState::Waiting, since: now, last_in_sync: None, error: None, generation: 0, done: 0, total: 0 });
        self.mirrors.sort_by_key(|m| m.vmid);
        Ok(())
    }

    pub fn mirror(&self, vmid: VmId) -> Option<&Mirror> {
        self.mirrors.iter().find(|m| m.vmid == vmid)
    }

    pub fn mirror_remove(&mut self, vmid: VmId) -> Result<Mirror, Error> {
        let at = self.mirrors.iter().position(|m| m.vmid == vmid).ok_or_else(|| Error::InvalidSpec(format!("{vmid} yansıtılmıyor")))?;
        Ok(self.mirrors.remove(at))
    }

    /// Yansıtmanın durumunu yazar. Durum değiştiyse `true` döner (görev günlüğüne yazılır).
    pub fn mirror_set(&mut self, vmid: VmId, state: MirrorState, error: Option<String>, now: u64) -> bool {
        let Some(m) = self.mirrors.iter_mut().find(|m| m.vmid == vmid) else { return false };
        let changed = m.state != state;
        if changed {
            m.since = now;
        }
        m.state = state;
        m.error = error;
        if state == MirrorState::InSync {
            m.last_in_sync = Some(now);
        }
        changed
    }

    pub fn mirror_progress(&mut self, vmid: VmId, generation: u32, done: u64, total: u64) {
        if let Some(m) = self.mirrors.iter_mut().find(|m| m.vmid == vmid) {
            (m.generation, m.done, m.total) = (generation, done, total);
        }
    }

    pub fn mirror_store(&self, origin: &str, vmid: VmId) -> Option<&MirrorStore> {
        self.mirror_stores.iter().find(|s| s.origin == origin && s.vmid == vmid)
    }

    fn store_index(&self, origin: &str, vmid: VmId) -> Result<usize, Error> {
        self.mirror_stores.iter().position(|s| s.origin == origin && s.vmid == vmid).ok_or_else(|| Error::InvalidSpec(format!("{origin}/{vmid} için tutulan bir kopya yok")))
    }

    /// Hedef: yeni bir kuşak açar. `(yeni kuşak, artık silinebilecek yarım kuşak)` döner; eksiksiz kuşak korunur.
    pub fn store_prepare(&mut self, origin: &str, guest: Guest, port: u16) -> Result<(u32, Option<u32>), Error> {
        if !valid_origin(origin) {
            return Err(Error::InvalidSpec("kaynak adı geçersiz (küçük harf, rakam ve tire; en çok 32 karakter)".to_string()));
        }
        if let Some(why) = mirror_blocker(&guest) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        let vmid = guest.vmid;
        match self.store_index(origin, vmid) {
            Ok(at) => {
                let store = &mut self.mirror_stores[at];
                let stale = Some(store.generation).filter(|g| store.complete != Some(*g));
                store.generation += 1;
                store.guest = guest;
                store.port = port;
                store.in_sync = false;
                Ok((store.generation, stale))
            }
            Err(_) => {
                if self.mirror_stores.len() >= MAX_MIRRORS {
                    return Err(Error::InvalidSpec(format!("en fazla {MAX_MIRRORS} kopya tutulabilir")));
                }
                self.mirror_stores.push(MirrorStore { origin: origin.to_string(), vmid, guest, generation: 1, complete: None, in_sync: false, last_in_sync: None, port });
                Ok((1, None))
            }
        }
    }

    /// Hedef: kaynağın durum bildirimi. Kuşak eşitlendiyse eksiksiz sayılır; yerini aldığı eski kuşağın numarası döner.
    pub fn store_report(&mut self, origin: &str, vmid: VmId, generation: u32, in_sync: bool, now: u64) -> Result<Option<u32>, Error> {
        let at = self.store_index(origin, vmid)?;
        let store = &mut self.mirror_stores[at];
        if store.generation != generation {
            return Err(Error::InvalidSpec(format!("{origin}/{vmid}: bildirilen kuşak eski")));
        }
        store.in_sync = in_sync;
        if !in_sync {
            return Ok(None);
        }
        store.last_in_sync = Some(now);
        Ok(store.complete.replace(generation).filter(|old| *old != generation))
    }

    /// Hedef: kopyadan konuk açılabilir mi? Açılabiliyorsa kullanılacak eksiksiz kuşak.
    pub fn store_promotable(&self, origin: &str, vmid: VmId, now: u64) -> Result<u32, Error> {
        let store = &self.mirror_stores[self.store_index(origin, vmid)?];
        let complete = store.complete.ok_or_else(|| Error::InvalidSpec(format!("kopya {origin}/{vmid}: henüz eksiksiz bir kopya yok (ilk eşitleme tamamlanmadı)")))?;
        if store.in_sync && store.last_in_sync.is_some_and(|at| now < at.saturating_add(PROMOTE_QUIET_SECS)) {
            return Err(Error::InvalidSpec(format!("kopya {origin}/{vmid}: kaynak hâlâ yansıtıyor, yani konuk orada çalışıyor. Aynı konuk iki yerde açılmasın diye reddedildi")));
        }
        Ok(complete)
    }

    pub fn store_remove(&mut self, origin: &str, vmid: VmId) -> Result<MirrorStore, Error> {
        let at = self.store_index(origin, vmid)?;
        Ok(self.mirror_stores.remove(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(c: &Cluster) -> VmId {
        c.guests.iter().find(|g| mirror_blocker(g).is_none()).unwrap().vmid
    }

    #[test]
    fn only_plain_virtual_machines_are_mirrored_and_only_once() {
        let mut c = Cluster::demo(1000);
        let vmid = vm(&c);
        c.mirror_add(vmid, "ankara", 10).unwrap();
        assert!(c.mirror_add(vmid, "izmir", 10).unwrap_err().to_string().contains("zaten"));
        let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
        assert!(c.mirror_add(ct, "ankara", 10).unwrap_err().to_string().contains("yalnızca sanal makinelerin"));
        assert!(c.mirror_add(9999, "ankara", 10).is_err());

        assert!(c.mirror_set(vmid, MirrorState::InSync, None, 20), "durum değişti");
        assert!(!c.mirror_set(vmid, MirrorState::InSync, None, 25));
        let m = c.mirror(vmid).unwrap();
        assert_eq!((m.since, m.last_in_sync), (20, Some(25)));
        c.mirror_set(vmid, MirrorState::Broken, Some("koptu".into()), 30);
        assert_eq!(c.mirror(vmid).unwrap().last_in_sync, Some(25), "kopunca son eşit an korunur");
        c.mirror_remove(vmid).unwrap();
        assert!(c.mirror_remove(vmid).is_err());
    }

    #[test]
    fn a_complete_generation_survives_until_the_next_one_is_complete() {
        let mut c = Cluster::demo(1000);
        let guest = c.guest(vm(&c)).unwrap().clone();
        let vmid = guest.vmid;
        assert!(c.store_prepare("../etc", guest.clone(), 1).is_err(), "kaynak adı dosya yolu olamaz");
        assert_eq!(c.store_prepare("ankara", guest.clone(), 49152).unwrap(), (1, None));
        assert!(c.store_promotable("ankara", vmid, 100).unwrap_err().to_string().contains("eksiksiz bir kopya yok"));
        // İlk kuşak yarım kaldı: yenisi açılınca silinebilir.
        assert_eq!(c.store_prepare("ankara", guest.clone(), 49152).unwrap(), (2, Some(1)));
        assert!(c.store_report("ankara", vmid, 1, true, 100).is_err(), "eski kuşağın bildirimi sayılmaz");
        assert_eq!(c.store_report("ankara", vmid, 2, true, 100).unwrap(), None);
        // Bildirimi süren yansıtmadan konuk açılmaz; susunca ya da "geride" deyince açılabilir.
        assert!(c.store_promotable("ankara", vmid, 100 + PROMOTE_QUIET_SECS - 1).unwrap_err().to_string().contains("hâlâ yansıtıyor"));
        assert_eq!(c.store_promotable("ankara", vmid, 100 + PROMOTE_QUIET_SECS).unwrap(), 2);
        c.store_report("ankara", vmid, 2, false, 101).unwrap();
        assert_eq!(c.store_promotable("ankara", vmid, 102).unwrap(), 2);
        // Yeniden eşitleme: eksiksiz 2. kuşak, 3. tamamlanana kadar kalır.
        assert_eq!(c.store_prepare("ankara", guest.clone(), 49152).unwrap(), (3, None));
        assert_eq!(c.store_promotable("ankara", vmid, 500).unwrap(), 2, "yarım kuşak kullanılmaz");
        assert_eq!(c.store_prepare("ankara", guest, 49152).unwrap(), (4, Some(3)));
        assert_eq!(c.store_report("ankara", vmid, 4, true, 600).unwrap(), Some(2), "yenisi tamamlanınca eskisi silinebilir");
        assert_eq!(c.mirror_store("ankara", vmid).unwrap().replica_id(4), format!("ankara-{vmid}-4"));
        c.store_remove("ankara", vmid).unwrap();
        assert!(c.store_remove("ankara", vmid).is_err());
    }
}
