//! Aygıt aktarımı, ek diskler ve donanım yazılımı seçimi.

use super::*;

pub const MAX_PASSTHROUGH: usize = 4;

pub(super) fn hex_fields(text: &str, separators: &[char], widths: &[usize]) -> bool {
    let parts: Vec<&str> = text.split(separators).collect();
    parts.len() == widths.len()
        && text.chars().filter(|c| separators.contains(c)).count() == widths.len() - 1
        && parts.iter().zip(widths).all(|(p, w)| p.len() == *w && p.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// USB aygıt kimliği: `046d:c52b`.
pub fn valid_usb_id(id: &str) -> bool {
    hex_fields(id, &[':'], &[4, 4])
}

/// PCI adresi: `0000:01:00.0` (alan:veri yolu:aygıt.işlev).
pub fn valid_pci_addr(addr: &str) -> bool {
    match addr.rsplit_once('.') {
        Some((rest, function)) => hex_fields(rest, &[':'], &[4, 2, 2]) && matches!(function, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7"),
        None => false,
    }
}

/// Ek veri diski. Numara kalıcıdır: aradaki bir disk silinse de diğerlerinin dosya adı değişmez.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraDisk {
    pub id: u8,
    pub size_gb: u64,
    /// Diskin kimliği: konuğa eklenen her diske artan bir sıra numarası verilir. Aynı numaraya (`id`)
    /// sonradan takılan disk başka bir disktir. (Eski kayıtlarda 0.)
    #[serde(default)]
    pub serial: u64,
}

pub const MAX_EXTRA_DISKS: usize = 8;
pub const MAX_EXTRA_NICS: usize = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Firmware {
    #[default]
    Bios,
    Uefi,
}

/// Formlardaki tek seçim kutusunun değeri: `bios`, `uefi` ya da `uefi-secure`.
pub fn parse_firmware_choice(choice: &str) -> Option<(Firmware, bool)> {
    match choice {
        "bios" => Some((Firmware::Bios, false)),
        "uefi" => Some((Firmware::Uefi, false)),
        "uefi-secure" => Some((Firmware::Uefi, true)),
        _ => None,
    }
}
