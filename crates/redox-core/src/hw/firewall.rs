//! Güvenlik duvarı kuralları.

use super::*;

/// Güvenlik duvarında izin verilen gelen trafik türü.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FwProto {
    Tcp,
    Udp,
    Icmp,
}

impl FwProto {
    pub fn as_str(self) -> &'static str {
        match self {
            FwProto::Tcp => "tcp",
            FwProto::Udp => "udp",
            FwProto::Icmp => "icmp",
        }
    }
}

/// Konuğa gelen trafiğe izin veren kural. Güvenlik duvarı açıkken kurallara uymayan gelen
/// bağlantılar düşürülür; konuğun başlattığı bağlantılar ve yanıtları serbesttir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FwRule {
    pub proto: FwProto,
    /// Kapı aralığı (dahil); `None` ise bütün kapılar. ICMP'de kullanılmaz.
    #[serde(default)]
    pub ports: Option<(u16, u16)>,
    /// Karşı uç: gelen kuralda kaynağın, giden kuralda hedefin IPv4 adresi, ağı (`10.0.0.0/8`) ya da adres
    /// kümesi (`@ofis`); `None` ise her yer.
    #[serde(default)]
    pub source: Option<String>,
}

pub const MAX_FW_RULES: usize = 32;

impl std::fmt::Display for FwRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.proto.as_str())?;
        match self.ports {
            Some((a, b)) if a == b => write!(f, "/{a}")?,
            Some((a, b)) => write!(f, "/{a}-{b}")?,
            None => {}
        }
        match &self.source {
            Some(source) => write!(f, " from {source}"),
            None => Ok(()),
        }
    }
}

/// IPv4 adresi ya da ağı: `192.168.1.10`, `10.0.0.0/8`.
pub fn valid_cidr(text: &str) -> bool {
    let (addr, prefix) = text.split_once('/').unwrap_or((text, "32"));
    let octets: Vec<&str> = addr.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|o| !o.is_empty() && o.len() <= 3 && o.bytes().all(|b| b.is_ascii_digit()) && o.parse::<u16>().is_ok_and(|v| v <= 255))
        && !prefix.is_empty()
        && prefix.len() <= 2
        && prefix.bytes().all(|b| b.is_ascii_digit())
        && prefix.parse::<u8>().is_ok_and(|p| p <= 32)
}

impl FwRule {
    pub fn valid(&self) -> bool {
        let ports_ok = match (self.proto, self.ports) {
            (FwProto::Icmp, ports) => ports.is_none(),
            (_, Some((a, b))) => a >= 1 && a <= b,
            (_, None) => true,
        };
        ports_ok && self.source.as_deref().is_none_or(|s| s.strip_prefix('@').map_or_else(|| valid_cidr(s), crate::fwsets::valid_fw_name))
    }

    /// Giden kuralın yazımı: `tcp/443 to 10.0.0.0/8`.
    pub fn outbound(&self) -> String {
        self.to_string().replacen(" from ", " to ", 1)
    }
}

/// "tcp/22, tcp/80-443 from 10.0.0.0/8, icmp" → kural listesi. Kurallar virgül ya da satırla ayrılır.
pub fn parse_fw_rules(text: &str) -> Result<Vec<FwRule>, String> {
    parse_directed(text, " from ", "tcp/22, udp/53, tcp/8000-8100 from 10.0.0.0/8, tcp/22 from @ofis, icmp")
}

/// Giden kurallar: "tcp/443, udp/53 to 10.0.0.1, tcp/25 to @posta".
pub fn parse_fw_out_rules(text: &str) -> Result<Vec<FwRule>, String> {
    parse_directed(text, " to ", "tcp/443, udp/53 to 10.0.0.1, tcp/25 to @posta, icmp")
}

fn parse_directed(text: &str, word: &str, examples: &str) -> Result<Vec<FwRule>, String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let bad = || format!("geçersiz güvenlik duvarı kuralı \"{item}\"; örnekler: {examples}");
            let (what, source) = match item.split_once(word) {
                Some((what, source)) => (what.trim(), Some(source.trim().to_string())),
                None => (item, None),
            };
            let (proto, ports) = what.split_once('/').map_or((what, None), |(p, range)| (p, Some(range)));
            let proto = match proto {
                "tcp" => FwProto::Tcp,
                "udp" => FwProto::Udp,
                "icmp" => FwProto::Icmp,
                _ => return Err(bad()),
            };
            let ports = match ports {
                None => None,
                Some(range) => {
                    let (a, b) = range.split_once('-').unwrap_or((range, range));
                    Some((a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?))
                }
            };
            let rule = FwRule { proto, ports, source };
            if rule.valid() { Ok(rule) } else { Err(bad()) }
        })
        .collect()
}
