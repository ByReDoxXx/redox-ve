//! Sunucunun panelden kurulan ağları: bond, VLAN alt arayüzü, köprü ve DHCP/DNS'li konuk ağı.
//!
//! Ağı kuran kök yardımcısı bu tanımları yeniden doğrular. Kurallar bir yanlış tanımın sunucuyu erişilmez
//! bırakmasını engelleyecek biçimde dardır: yardımcı yalnızca kendi kurduğu aygıtları değiştirir ve üzerinde
//! adres ya da varsayılan yol olan bir arayüzü hiçbir yere bağlamaz.

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

use crate::{Cluster, Error, NAT};

pub const MAX_HOST_NETS: usize = 32;
pub const MAX_PORTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BondMode {
    #[serde(rename = "active-backup")]
    ActiveBackup,
    #[serde(rename = "802.3ad")]
    Lacp,
    #[serde(rename = "balance-xor")]
    BalanceXor,
}

impl BondMode {
    pub const ALL: [BondMode; 3] = [BondMode::ActiveBackup, BondMode::Lacp, BondMode::BalanceXor];

    pub fn as_str(self) -> &'static str {
        match self {
            BondMode::ActiveBackup => "active-backup",
            BondMode::Lacp => "802.3ad",
            BondMode::BalanceXor => "balance-xor",
        }
    }

    pub fn parse(text: &str) -> Option<BondMode> {
        Self::ALL.into_iter().find(|m| m.as_str() == text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum HostNet {
    /// Birkaç fiziksel kartı tek arayüz yapar.
    Bond { name: String, mode: BondMode, ports: Vec<String> },
    /// Bir arayüzün 802.1Q etiketli alt arayüzü.
    Vlan { name: String, parent: String, id: u16 },
    /// Konukların bağlandığı köprü; `ports` ile fiziksel ağa açılır.
    Bridge {
        name: String,
        #[serde(default)]
        ports: Vec<String>,
        /// Konuk kartlarında VLAN etiketi kullanılabilsin.
        #[serde(default)]
        vlan_aware: bool,
    },
    /// Sunucunun yönettiği konuk ağı: sunucu ağ geçididir; DHCP ve DNS dağıtır, istenirse NAT ile dışarı çıkarır.
    Guest {
        name: String,
        /// Sunucunun bu ağdaki adresi ve ön eki: `10.10.0.1/24`.
        address: String,
        /// DHCP aralığı; ikisi de boşsa DHCP kapalıdır.
        #[serde(default)]
        dhcp_start: Option<Ipv4Addr>,
        #[serde(default)]
        dhcp_end: Option<Ipv4Addr>,
        /// Konuklar sunucunun adresiyle dış ağa çıkabilsin. Kapalıysa ağ yalıtılmıştır.
        #[serde(default)]
        nat: bool,
    },
}

/// Linux arayüz adı: en çok 15 karakter; harf, rakam, nokta, tire, alt çizgi; tire ya da noktayla başlamaz.
pub fn valid_ifname(name: &str) -> bool {
    !name.is_empty() && name.len() <= 15 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) && !name.starts_with(['-', '.'])
}

/// `a.b.c.d/önek` → `(adres, önek)`.
pub fn parse_subnet(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (ip, prefix) = text.split_once('/')?;
    let prefix: u8 = prefix.parse().ok()?;
    Some((ip.parse().ok()?, prefix))
}

/// Adresin ağ numarası.
pub fn network_of(ip: Ipv4Addr, prefix: u8) -> u32 {
    u32::from(ip) & u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0)
}

impl HostNet {
    pub fn name(&self) -> &str {
        match self {
            HostNet::Bond { name, .. } | HostNet::Vlan { name, .. } | HostNet::Bridge { name, .. } | HostNet::Guest { name, .. } => name,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            HostNet::Bond { .. } => "bond",
            HostNet::Vlan { .. } => "vlan",
            HostNet::Bridge { .. } => "köprü",
            HostNet::Guest { .. } => "konuk ağı",
        }
    }

    /// Bu tanımın altına aldığı (kendine bağladığı ya da üzerine kurulduğu) arayüzler.
    pub fn uses(&self) -> Vec<&str> {
        match self {
            HostNet::Bond { ports, .. } | HostNet::Bridge { ports, .. } => ports.iter().map(String::as_str).collect(),
            HostNet::Vlan { parent, .. } => vec![parent],
            HostNet::Guest { .. } => Vec::new(),
        }
    }

    /// Konukların ağ olarak seçebileceği bir köprü mü?
    pub fn is_bridge(&self) -> bool {
        matches!(self, HostNet::Bridge { .. } | HostNet::Guest { .. })
    }

    pub fn check(&self) -> Result<(), String> {
        let name = self.name();
        let fail = |why: String| Err(format!("{name}: {why}"));
        if !valid_ifname(name) || name == NAT || name == "lo" {
            return Err(format!("geçersiz arayüz adı: \"{name}\" (en çok 15 karakter; harf, rakam, nokta, tire, alt çizgi)"));
        }
        // `vds` dağıtık anahtarların, `vx` onların tünellerinin ön ekidir.
        if name.starts_with("vds") || name.starts_with("vx") {
            return fail("vds ve vx ön ekleri dağıtık anahtarlara ayrılmıştır".to_string());
        }
        if let Some(bad) = self.uses().into_iter().find(|p| !valid_ifname(p) || *p == name || *p == "lo") {
            return fail(format!("geçersiz alt arayüz: \"{bad}\""));
        }
        let mut ports = self.uses();
        ports.sort_unstable();
        ports.dedup();
        if ports.len() != self.uses().len() || ports.len() > MAX_PORTS {
            return fail(format!("alt arayüzler tekrarsız ve en çok {MAX_PORTS} olmalı"));
        }
        match self {
            HostNet::Bond { ports, .. } => {
                if !name.starts_with("bond") {
                    return fail("bond adı \"bond\" ile başlamalı".to_string());
                }
                if ports.is_empty() {
                    return fail("bond en az bir arayüz ister".to_string());
                }
            }
            HostNet::Vlan { parent, id, .. } => {
                if *id == 0 || *id > crate::hw::MAX_VLAN {
                    return fail(format!("VLAN etiketi 1 ile {} arasında olmalı", crate::hw::MAX_VLAN));
                }
                if name != format!("{parent}.{id}") {
                    return fail(format!("VLAN arayüzünün adı \"{parent}.{id}\" olmalı (en çok 15 karakter)"));
                }
            }
            HostNet::Bridge { .. } => {
                if !crate::valid_bridge(name) {
                    return fail("köprü adı konuk ağ ayarında kullanılabilir olmalı".to_string());
                }
            }
            HostNet::Guest { address, dhcp_start, dhcp_end, .. } => {
                if !crate::valid_bridge(name) {
                    return fail("ağ adı konuk ağ ayarında kullanılabilir olmalı".to_string());
                }
                let Some((ip, prefix)) = parse_subnet(address).filter(|(ip, p)| (16..=30).contains(p) && ip.is_private()) else {
                    return fail("adres özel bir IPv4 adresi ve /16 ile /30 arası ön ek olmalı (örn. 10.10.0.1/24)".to_string());
                };
                let network = network_of(ip, prefix);
                let broadcast = network | !network_of(Ipv4Addr::BROADCAST, prefix);
                let host = |a: Ipv4Addr| network_of(a, prefix) == network && u32::from(a) != network && u32::from(a) != broadcast;
                if !host(ip) {
                    return fail("sunucunun adresi ağ ya da yayın adresi olamaz".to_string());
                }
                match (dhcp_start, dhcp_end) {
                    (None, None) => {}
                    (Some(a), Some(b)) => {
                        if !host(*a) || !host(*b) || a > b || (*a..=*b).contains(&ip) {
                            return fail("DHCP aralığı ağın içinde, sıralı olmalı ve sunucunun adresini içermemeli".to_string());
                        }
                    }
                    _ => return fail("DHCP aralığının başı ve sonu birlikte verilmeli".to_string()),
                }
            }
        }
        Ok(())
    }
}

/// Tanımlar tek tek ve birlikte geçerli mi: adlar tekil, bir arayüz tek yerde, alt ağlar çakışmıyor, döngü yok.
pub fn check_all(nets: &[HostNet]) -> Result<(), String> {
    if nets.len() > MAX_HOST_NETS {
        return Err(format!("en fazla {MAX_HOST_NETS} sunucu ağı tanımlanabilir"));
    }
    for (i, net) in nets.iter().enumerate() {
        net.check()?;
        for other in &nets[..i] {
            if other.name() == net.name() {
                return Err(format!("{}: aynı ad iki kez tanımlanamaz", net.name()));
            }
            // Bir arayüz ya tek bir bond/köprünün üyesidir ya da üzerine VLAN alt arayüzleri kurulur.
            let enslaves = |n: &HostNet| if matches!(n, HostNet::Vlan { .. }) { Vec::new() } else { n.uses().into_iter().map(str::to_string).collect::<Vec<_>>() };
            if let Some(port) = enslaves(net).into_iter().find(|p| other.uses().contains(&p.as_str())) {
                return Err(format!("{}: {port} arayüzü {} tanımında kullanılıyor", net.name(), other.name()));
            }
            if let Some(port) = enslaves(other).into_iter().find(|p| net.uses().contains(&p.as_str())) {
                return Err(format!("{}: {port} arayüzü {} tanımında kullanılıyor", net.name(), other.name()));
            }
            if let (HostNet::Guest { address: a, .. }, HostNet::Guest { address: b, .. }) = (net, other) {
                let (Some((a, pa)), Some((b, pb))) = (parse_subnet(a), parse_subnet(b)) else { continue };
                let wider = pa.min(pb);
                if network_of(a, wider) == network_of(b, wider) {
                    return Err(format!("{}: alt ağı {} ile çakışıyor", net.name(), other.name()));
                }
            }
        }
    }
    // Tanımlar birbirinin üzerine döngü kuramaz.
    for net in nets {
        let mut pending: Vec<&str> = net.uses();
        for _ in 0..nets.len() {
            pending = pending.iter().filter_map(|p| nets.iter().find(|n| n.name() == *p)).flat_map(HostNet::uses).collect();
            if pending.contains(&net.name()) {
                return Err(format!("{}: tanımlar birbirine döngü kuruyor", net.name()));
            }
        }
    }
    Ok(())
}

impl Cluster {
    pub fn host_net(&self, name: &str) -> Option<&HostNet> {
        self.host_nets.iter().find(|n| n.name() == name)
    }

    pub fn put_host_net(&mut self, net: HostNet) -> Result<(), Error> {
        if self.vswitches.iter().any(|s| s.name == net.name()) {
            return Err(Error::InvalidSpec(format!("{}: bu adda bir dağıtık anahtar var", net.name())));
        }
        let mut next = self.host_nets.clone();
        match next.iter_mut().find(|n| n.name() == net.name()) {
            Some(old) => *old = net,
            None => next.push(net),
        }
        next.sort_by(|a, b| a.name().cmp(b.name()));
        check_all(&next).map_err(Error::InvalidSpec)?;
        self.host_nets = next;
        Ok(())
    }

    /// Kullanımdaki ağ silinmez: konuğun bağlı olduğu köprü ya da başka bir tanımın üzerine kurulduğu arayüz.
    pub fn remove_host_net(&mut self, name: &str) -> Result<(), Error> {
        if self.host_net(name).is_none() {
            return Err(Error::InvalidSpec(format!("\"{name}\" adında bir sunucu ağı yok")));
        }
        let guests: Vec<String> = self.vswitch_users(name).into_iter().map(|v| v.to_string()).collect();
        if !guests.is_empty() {
            return Err(Error::InvalidSpec(format!("{name} ağını kullanan konuklar var: {}", guests.join(", "))));
        }
        if let Some(upper) = self.host_nets.iter().find(|n| n.uses().contains(&name)) {
            return Err(Error::InvalidSpec(format!("{name} üzerine {} kurulu; önce onu silin", upper.name())));
        }
        self.host_nets.retain(|n| n.name() != name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge(name: &str, ports: &[&str]) -> HostNet {
        HostNet::Bridge { name: name.into(), ports: ports.iter().map(|p| p.to_string()).collect(), vlan_aware: false }
    }

    fn guest(name: &str, address: &str, range: Option<(&str, &str)>) -> HostNet {
        HostNet::Guest { name: name.into(), address: address.into(), dhcp_start: range.map(|r| r.0.parse().unwrap()), dhcp_end: range.map(|r| r.1.parse().unwrap()), nat: true }
    }

    #[test]
    fn definitions_reject_names_and_values_that_cannot_work() {
        assert!(bridge("vmbr1", &["enp2s0"]).check().is_ok());
        for bad in ["nat", "lo", "a b", "x;reboot", "-x", "onaltikarakterlik", "vds-a", "vx100"] {
            assert!(bridge(bad, &[]).check().is_err(), "{bad}");
        }
        assert!(bridge("vmbr1", &["vmbr1"]).check().is_err() && bridge("vmbr1", &["e0", "e0"]).check().is_err() && bridge("vmbr1", &["e0;x"]).check().is_err());
        let bond = |name: &str, ports: &[&str]| HostNet::Bond { name: name.into(), mode: BondMode::Lacp, ports: ports.iter().map(|p| p.to_string()).collect() };
        assert!(bond("bond0", &["e0", "e1"]).check().is_ok() && bond("team0", &["e0"]).check().is_err() && bond("bond0", &[]).check().is_err());
        let vlan = |name: &str, parent: &str, id| HostNet::Vlan { name: name.into(), parent: parent.into(), id };
        assert!(vlan("bond0.20", "bond0", 20).check().is_ok() && vlan("vlan20", "bond0", 20).check().is_err() && vlan("e0.0", "e0", 0).check().is_err() && vlan("e0.4095", "e0", 4095).check().is_err());
        assert!(guest("vnet1", "10.10.0.1/24", Some(("10.10.0.50", "10.10.0.99"))).check().is_ok());
        for (address, range) in [("8.8.8.1/24", None), ("10.0.0.1/8", None), ("10.0.0.0/24", None), ("10.0.0.255/24", None), ("10.0.0.1/24", Some(("10.0.1.5", "10.0.1.9"))), ("10.0.0.1/24", Some(("10.0.0.1", "10.0.0.9"))), ("10.0.0.1/24", Some(("10.0.0.9", "10.0.0.5")))] {
            assert!(guest("vnet1", address, range).check().is_err(), "{address} {range:?}");
        }
        assert_eq!(serde_json::to_value(bond("bond0", &["e0"])).unwrap()["mode"], "802.3ad");
        assert!(serde_json::from_str::<HostNet>(r#"{"kind":"bridge","name":"br9","script":"x"}"#).is_err(), "bilinmeyen alan kabul edilmez");
    }

    #[test]
    fn definitions_cannot_share_ports_overlap_or_loop() {
        let vlan = HostNet::Vlan { name: "e0.5".into(), parent: "e0".into(), id: 5 };
        assert!(check_all(&[vlan.clone(), bridge("br1", &["e0.5"]), guest("vnet1", "10.1.0.1/24", None)]).is_ok());
        assert!(check_all(&[bridge("br1", &["e0"]), bridge("br2", &["e0"])]).unwrap_err().contains("kullanılıyor"));
        assert!(check_all(&[vlan, bridge("br1", &["e0"])]).unwrap_err().contains("kullanılıyor"), "köprüye bağlı karta VLAN kurulmaz");
        assert!(check_all(&[guest("a1", "10.1.0.1/16", None), guest("b1", "10.1.5.1/24", None)]).unwrap_err().contains("çakışıyor"));
        assert!(check_all(&[bridge("br1", &["br2"]), bridge("br2", &["br1"])]).is_err());
    }

    #[test]
    fn networks_in_use_are_not_removed() {
        let mut c = Cluster::demo(0);
        c.put_host_net(HostNet::Bond { name: "bond0".into(), mode: BondMode::ActiveBackup, ports: vec!["e1".into()] }).unwrap();
        c.put_host_net(bridge("vmbr7", &["bond0"])).unwrap();
        assert!(c.remove_host_net("bond0").unwrap_err().to_string().contains("vmbr7"));
        c.guests[0].bridge = "vmbr7".into();
        assert!(c.remove_host_net("vmbr7").unwrap_err().to_string().contains("kullanan konuklar"));
        c.guests[0].bridge = "nat".into();
        c.remove_host_net("vmbr7").unwrap();
        c.remove_host_net("bond0").unwrap();
        assert!(c.remove_host_net("bond0").is_err());
    }
}
