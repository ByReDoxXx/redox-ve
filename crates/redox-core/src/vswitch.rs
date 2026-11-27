//! Dağıtık anahtar: birden çok sunucuda aynı adla bulunan ve aralarında VXLAN tüneliyle birleşen konuk ağı.
//!
//! Tanım her sunucuda birebir aynıdır: üye listesi bütün sunucuların adreslerini içerir, her sunucu listede
//! kendisine ait olan adresi yerel uç, kalanları tünelin karşı ucu sayar. Bu yüzden anahtar bir kez tanımlanır
//! ve değiştirilmeden eşlere gönderilir.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;

/// Anahtar köprülerinin zorunlu ad öneki. Ayrıcalıklı yardımcı yalnızca bu adları kurar; böylece yönetim
/// sunucusu ele geçirilse bile var olan bir köprü (ör. yönetim ağı) tünele bağlatılamaz.
pub const PREFIX: &str = "vds";
pub const MAX_SWITCHES: usize = 16;
pub const MAX_MEMBERS: usize = 16;
pub const MAX_VNI: u32 = 16_777_215;
pub const MIN_MTU: u32 = 1280;
pub const MAX_MTU: u32 = 9000;
/// 1500 baytlık bir ağda VXLAN başlığına yer kalan en büyük değer.
pub const DEFAULT_MTU: u32 = 1450;

fn default_mtu() -> u32 {
    DEFAULT_MTU
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VSwitch {
    /// Her sunucudaki köprünün adı (`vds` ile başlar); konukların ağ ayarında bu ad seçilir.
    pub name: String,
    /// VXLAN ağ kimliği.
    pub vni: u32,
    #[serde(default = "default_mtu")]
    pub mtu: u32,
    /// Anahtara üye bütün sunucuların tünel adresleri (bu sunucununki dahil).
    pub members: Vec<IpAddr>,
}

impl VSwitch {
    /// Tanım tek başına geçerli mi? Ayrıcalıklı yardımcı da komut üretmeden önce bunu çağırır.
    pub fn check(&self) -> Result<(), String> {
        let tail = self.name.strip_prefix(PREFIX).unwrap_or("");
        if self.name.len() > 15 || tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(format!("anahtar adı \"{PREFIX}\" ile başlamalı, ardından küçük harf, rakam ya da tire gelmeli ve toplam en çok 15 karakter olmalı: {}", self.name.escape_default()));
        }
        if !(1..=MAX_VNI).contains(&self.vni) {
            return Err(format!("VNI 1 ile {MAX_VNI} arasında olmalı"));
        }
        if !(MIN_MTU..=MAX_MTU).contains(&self.mtu) {
            return Err(format!("MTU {MIN_MTU} ile {MAX_MTU} arasında olmalı"));
        }
        if !(2..=MAX_MEMBERS).contains(&self.members.len()) {
            return Err(format!("anahtarın 2 ile {MAX_MEMBERS} arasında üye adresi olmalı"));
        }
        for (i, member) in self.members.iter().enumerate() {
            if member.is_loopback() || member.is_unspecified() || member.is_multicast() {
                return Err(format!("üye adresi olarak kullanılamaz: {member}"));
            }
            if member.is_ipv4() != self.members[0].is_ipv4() {
                return Err("üye adreslerinin hepsi aynı türde (IPv4 ya da IPv6) olmalı".to_string());
            }
            if self.members[..i].contains(member) {
                return Err(format!("üye adresi iki kez yazılmış: {member}"));
            }
        }
        Ok(())
    }
}

/// Listenin bütünü geçerli mi: her tanım tek başına, adlar ve VNI'ler de birbirine göre.
pub fn check_all(switches: &[VSwitch]) -> Result<(), String> {
    if switches.len() > MAX_SWITCHES {
        return Err(format!("en fazla {MAX_SWITCHES} dağıtık anahtar tanımlanabilir"));
    }
    for (i, switch) in switches.iter().enumerate() {
        switch.check()?;
        if switches[..i].iter().any(|s| s.name == switch.name) {
            return Err(format!("anahtar adı iki kez kullanılmış: {}", switch.name));
        }
        if let Some(other) = switches[..i].iter().find(|s| s.vni == switch.vni) {
            return Err(format!("VNI {} zaten {} anahtarında kullanılıyor", switch.vni, other.name));
        }
    }
    Ok(())
}

impl Cluster {
    /// Anahtarı ekler ya da aynı adlı olanı günceller. Değişiklik olduysa `true` döner.
    pub fn put_vswitch(&mut self, mut switch: VSwitch) -> Result<bool, Error> {
        // Sıra anlam taşımaz: her sunucudaki tanım aynı yazılsın.
        switch.members.sort();
        switch.check().map_err(Error::InvalidSpec)?;
        // İleti, eklenmek istenenin değil var olan anahtarın adını söylemelidir.
        if let Some(other) = self.vswitches.iter().find(|s| s.name != switch.name && s.vni == switch.vni) {
            return Err(Error::InvalidSpec(format!("VNI {} zaten {} anahtarında kullanılıyor", switch.vni, other.name)));
        }
        let mut next: Vec<VSwitch> = self.vswitches.iter().filter(|s| s.name != switch.name).cloned().collect();
        let changed = !self.vswitches.contains(&switch);
        next.push(switch);
        next.sort_by(|a, b| a.name.cmp(&b.name));
        check_all(&next).map_err(Error::InvalidSpec)?;
        self.vswitches = next;
        Ok(changed)
    }

    /// Anahtarı siler. Bu sunucuda onu kullanan konuk varken silinmez.
    pub fn remove_vswitch(&mut self, name: &str) -> Result<(), Error> {
        if !self.vswitches.iter().any(|s| s.name == name) {
            return Err(Error::InvalidSpec(format!("\"{name}\" adında bir dağıtık anahtar yok")));
        }
        let users = self.vswitch_users(name);
        if !users.is_empty() {
            let list = users.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            return Err(Error::InvalidSpec(format!("{name} anahtarını kullanan konuklar var: {list}")));
        }
        self.vswitches.retain(|s| s.name != name);
        Ok(())
    }

    /// Anahtarı birinci ya da ek ağ kartında kullanan konuklar.
    pub fn vswitch_users(&self, name: &str) -> Vec<crate::model::VmId> {
        let uses = |spec: &str| spec.split(':').next() == Some(name);
        self.guests.iter().filter(|g| uses(&g.bridge) || g.hw.nics.iter().any(|n| uses(n))).map(|g| g.vmid).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switch(name: &str, vni: u32, members: &[&str]) -> VSwitch {
        VSwitch { name: name.to_string(), vni, mtu: DEFAULT_MTU, members: members.iter().map(|m| m.parse().unwrap()).collect() }
    }

    #[test]
    fn only_well_formed_switches_are_accepted() {
        assert!(switch("vds-web", 100, &["10.0.0.1", "10.0.0.2"]).check().is_ok());
        assert!(switch("vds1", MAX_VNI, &["fd00::1", "fd00::2"]).check().is_ok());
        for (name, vni, members, needle) in [
            ("vmbr0", 1, &["10.0.0.1", "10.0.0.2"][..], "ile başlamalı"),
            ("vds", 1, &["10.0.0.1", "10.0.0.2"], "ile başlamalı"),
            ("vds Web", 1, &["10.0.0.1", "10.0.0.2"], "ile başlamalı"),
            ("vds;reboot", 1, &["10.0.0.1", "10.0.0.2"], "ile başlamalı"),
            ("vds-cok-uzun-bir-ad", 1, &["10.0.0.1", "10.0.0.2"], "15 karakter"),
            ("vds1", 0, &["10.0.0.1", "10.0.0.2"], "VNI"),
            ("vds1", MAX_VNI + 1, &["10.0.0.1", "10.0.0.2"], "VNI"),
            ("vds1", 1, &["10.0.0.1"], "üye adresi olmalı"),
            ("vds1", 1, &["10.0.0.1", "127.0.0.1"], "kullanılamaz"),
            ("vds1", 1, &["10.0.0.1", "224.0.0.1"], "kullanılamaz"),
            ("vds1", 1, &["10.0.0.1", "fd00::2"], "aynı türde"),
            ("vds1", 1, &["10.0.0.1", "10.0.0.1"], "iki kez"),
        ] {
            assert!(switch(name, vni, members).check().unwrap_err().contains(needle), "{name} {vni} {members:?}");
        }
        let mut small = switch("vds1", 1, &["10.0.0.1", "10.0.0.2"]);
        small.mtu = 576;
        assert!(small.check().unwrap_err().contains("MTU"));
        // Tanınmayan alan sessizce yutulmaz (ayrıcalıklı yardımcı dosyayı bu türle okur).
        assert!(serde_json::from_str::<VSwitch>(r#"{"name":"vds1","vni":1,"members":["10.0.0.1","10.0.0.2"],"komut":"x"}"#).is_err());
        assert!(serde_json::from_str::<VSwitch>(r#"{"name":"vds1","vni":1,"members":["10.0.0.1; reboot","10.0.0.2"]}"#).is_err());
        assert_eq!(serde_json::from_str::<VSwitch>(r#"{"name":"vds1","vni":1,"members":["10.0.0.1","10.0.0.2"]}"#).unwrap().mtu, DEFAULT_MTU);
    }

    #[test]
    fn names_and_vnis_are_unique_and_used_switches_stay() {
        let mut c = Cluster::demo(1000);
        assert!(c.put_vswitch(switch("vds-a", 10, &["10.0.0.2", "10.0.0.1"])).unwrap());
        assert_eq!(c.vswitches[0].members[0].to_string(), "10.0.0.1", "üyeler sıralı tutulur");
        assert!(!c.put_vswitch(switch("vds-a", 10, &["10.0.0.1", "10.0.0.2"])).unwrap(), "aynı tanım değişiklik sayılmaz");
        assert!(c.put_vswitch(switch("vds-b", 10, &["10.0.0.1", "10.0.0.2"])).unwrap_err().to_string().contains("zaten vds-a"));
        assert!(c.put_vswitch(switch("vds-0", 10, &["10.0.0.1", "10.0.0.2"])).unwrap_err().to_string().contains("zaten vds-a"), "ad sırası iletiyi değiştirmez");
        assert_eq!(c.vswitches.len(), 1, "reddedilen tanım eklenmez");
        c.put_vswitch(switch("vds-a", 11, &["10.0.0.1", "10.0.0.2"])).unwrap();
        assert_eq!((c.vswitches.len(), c.vswitches[0].vni), (1, 11));

        let vmid = c.guests[0].vmid;
        c.guest_mut_for_import(vmid).unwrap().hw.nics = vec!["vds-a:20".to_string()];
        assert_eq!(c.vswitch_users("vds-a"), [vmid]);
        assert!(c.vswitch_users("vds").is_empty());
        assert!(c.remove_vswitch("vds-a").unwrap_err().to_string().contains(&vmid.to_string()));
        c.guest_mut_for_import(vmid).unwrap().hw.nics.clear();
        c.remove_vswitch("vds-a").unwrap();
        assert!(c.remove_vswitch("vds-a").is_err());
    }
}
