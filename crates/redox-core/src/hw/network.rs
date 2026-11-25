//! Ağ: paylaşılan klasörler, ağ adı ve VLAN ayrıştırma, hız sınırları.

use super::*;

pub const MAX_VLAN: u16 = 4094;
pub const MAX_SHARES: usize = 4;

/// Paylaşılan klasör. Ad hem sunucudaki dizinin (`shares/<ad>`) hem konuktaki bağlama etiketinin adıdır;
/// aynı adı kullanan konuklar aynı klasörü görür.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub name: String,
    /// Konuk yalnızca okuyabilir.
    #[serde(default)]
    pub readonly: bool,
}

impl Share {
    /// Ad: küçük harf, rakam ve tire; 1-24 karakter; tireyle başlamaz. Yol ayırıcı ve nokta içeremez.
    pub fn valid(&self) -> bool {
        let n = &self.name;
        !n.is_empty() && n.len() <= 24 && !n.starts_with('-') && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }
}

/// Form ve komut satırı biçimi: `ortak` ya da salt okunur için `veri:ro`.
impl std::fmt::Display for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.name, if self.readonly { ":ro" } else { "" })
    }
}

/// Virgülle ya da boşlukla ayrılmış paylaşım listesi: `ortak, veri:ro`.
pub fn parse_shares(text: &str) -> Result<Vec<Share>, String> {
    text.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (name, readonly) = match item.split_once(':') {
                Some((name, "ro")) => (name, true),
                Some((name, "rw")) => (name, false),
                Some(_) => return Err(format!("geçersiz paylaşım \"{item}\": ad ya da salt okunur için ad:ro yazın")),
                None => (item, false),
            };
            let share = Share { name: name.to_string(), readonly };
            if share.valid() { Ok(share) } else { Err(format!("geçersiz paylaşım adı \"{name}\": küçük harf, rakam ve tire; en çok 24 karakter")) }
        })
        .collect()
}

/// Kullanıcının yazdığı ağ adını düzenler: `NAT` her yazımıyla `nat` olur; köprü adlarına dokunulmaz
/// (Linux'ta arayüz adları büyük/küçük harfe duyarlıdır).
pub fn normalize_network(spec: &str) -> String {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case(crate::model::NAT) { crate::model::NAT.to_string() } else { spec.to_string() }
}

/// Ek kart tanımını köprü (ya da `nat`) ve VLAN etiketine ayırır: `vmbr1` → `("vmbr1", 0)`, `vmbr1:20` → `("vmbr1", 20)`.
/// Geçersiz tanımda `None`. Arayüz adlarında `:` bulunmadığı için ayrım belirsiz değildir.
pub fn parse_nic(spec: &str) -> Option<(&str, u16)> {
    match spec.split_once(':') {
        None => crate::model::valid_bridge(spec).then_some((spec, 0)),
        Some((bridge, tag)) => {
            let vlan: u16 = tag.parse().ok().filter(|v| (1..=MAX_VLAN).contains(v) && tag.bytes().all(|b| b.is_ascii_digit()))?;
            (crate::model::valid_bridge(bridge) && bridge != crate::model::NAT).then_some((bridge, vlan))
        }
    }
}

pub const MAX_DISK_MBPS: u32 = 10_000;
pub const MAX_DISK_IOPS: u32 = 1_000_000;
pub const MAX_NET_MBPS: u32 = 100_000;
