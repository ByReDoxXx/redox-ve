//! Windows ve dışarıdan aktarılan konuklar için aygıt tipleri.
//!
//! Olağan seçim virtio'dur (en hızlısı). Windows kurulumu ve başka sanallaştırıcıdan alınan diskler, konukta
//! virtio sürücüsü yokken yalnızca öykünülen aygıtları (SATA, Intel ağ kartı) görür.

use super::*;

/// Asıl diskin bağlandığı denetleyici. Ek veri diskleri her zaman virtio'dur (çalışırken takılabilsin diye).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiskBus {
    #[default]
    Virtio,
    VirtioScsi,
    Sata,
}

impl DiskBus {
    pub const ALL: [DiskBus; 3] = [DiskBus::Virtio, DiskBus::VirtioScsi, DiskBus::Sata];

    pub fn as_str(self) -> &'static str {
        match self {
            DiskBus::Virtio => "virtio",
            DiskBus::VirtioScsi => "virtio-scsi",
            DiskBus::Sata => "sata",
        }
    }

    pub fn parse(text: &str) -> Option<DiskBus> {
        Self::ALL.into_iter().find(|b| b.as_str() == text)
    }
}

/// Konuğun bütün ağ kartlarının modeli.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NicModel {
    #[default]
    Virtio,
    E1000e,
    E1000,
}

impl NicModel {
    pub const ALL: [NicModel; 3] = [NicModel::Virtio, NicModel::E1000e, NicModel::E1000];

    pub fn as_str(self) -> &'static str {
        match self {
            NicModel::Virtio => "virtio",
            NicModel::E1000e => "e1000e",
            NicModel::E1000 => "e1000",
        }
    }

    pub fn parse(text: &str) -> Option<NicModel> {
        Self::ALL.into_iter().find(|m| m.as_str() == text)
    }

    /// QEMU aygıt adı.
    pub fn device(self) -> &'static str {
        match self {
            NicModel::Virtio => "virtio-net-pci",
            NicModel::E1000e => "e1000e",
            NicModel::E1000 => "e1000",
        }
    }
}

impl Hardware {
    /// Olağan dışı aygıt tipi ya da ikinci kalıp kullanılıyor mu?
    pub fn uses_compat(&self) -> bool {
        self.disk_bus != DiskBus::Virtio || self.nic_model != NicModel::Virtio || self.iso2.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GuestKind;

    #[test]
    fn names_round_trip_and_match_the_stored_form() {
        for bus in DiskBus::ALL {
            assert_eq!(DiskBus::parse(bus.as_str()), Some(bus));
            assert_eq!(serde_json::to_value(bus).unwrap(), bus.as_str());
        }
        for model in NicModel::ALL {
            assert_eq!(NicModel::parse(model.as_str()), Some(model));
            assert_eq!(serde_json::to_value(model).unwrap(), model.as_str());
        }
        assert_eq!((DiskBus::parse("ide"), NicModel::parse("rtl8139")), (None, None));
    }

    #[test]
    fn old_records_use_virtio_and_containers_cannot_choose() {
        let old: Hardware = serde_json::from_str("{}").unwrap();
        assert!(!old.uses_compat() && !old.media_changed);
        let sata = Hardware { disk_bus: DiskBus::Sata, ..Default::default() };
        assert!(sata.validate(GuestKind::Vm).is_ok());
        assert!(sata.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        let bad = Hardware { iso2: Some("../x.iso".into()), ..Default::default() };
        assert!(bad.validate(GuestKind::Vm).unwrap_err().contains("geçersiz ISO adı"));
    }
}
