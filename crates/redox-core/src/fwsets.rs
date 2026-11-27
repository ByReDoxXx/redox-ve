//! Yeniden kullanılan güvenlik duvarı tanımları: adres kümeleri (`@ofis`) ve güvenlik grupları.
//!
//! Sürücü bunları bilmez: kurallar uygulanmadan önce her konuk için düz kurallara açılır. Bulunamayan bir
//! küme ya da grup hiçbir şeye izin vermez (kural düşer, genişlemez).

use serde::{Deserialize, Serialize};

use crate::hw::{valid_cidr, FwRule, MAX_FW_RULES};
use crate::{Cluster, Error, Guest, VmId};

pub const MAX_IP_SETS: usize = 16;
pub const MAX_SET_ENTRIES: usize = 64;
pub const MAX_FW_GROUPS: usize = 16;
/// Bir konuğa en fazla bu kadar grup verilebilir.
pub const MAX_GUEST_GROUPS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpSet {
    pub name: String,
    /// IPv4 adresleri ve ağları.
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FwGroup {
    pub name: String,
    /// İzin verilen gelen trafik.
    pub rules: Vec<FwRule>,
    /// İzin verilen giden trafik (yalnızca giden trafiği kısıtlanan konukta etkilidir).
    pub out: Vec<FwRule>,
}

/// Küme ve grup adı: küçük harfle başlar; küçük harf, rakam ve tire; en çok 24 karakter.
pub fn valid_fw_name(name: &str) -> bool {
    name.len() <= 24 && name.starts_with(|c: char| c.is_ascii_lowercase()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn set_refs(rules: &[FwRule]) -> impl Iterator<Item = &str> {
    rules.iter().filter_map(|r| r.source.as_deref().and_then(|s| s.strip_prefix('@')))
}

impl Cluster {
    pub fn ip_set(&self, name: &str) -> Option<&IpSet> {
        self.ip_sets.iter().find(|s| s.name == name)
    }

    pub fn fw_group(&self, name: &str) -> Option<&FwGroup> {
        self.fw_groups.iter().find(|g| g.name == name)
    }

    /// Kurallarda ve grup listesinde adı geçen her tanım var mı?
    pub fn check_fw_refs(&self, rules: &[FwRule], out: &[FwRule], groups: &[String]) -> Result<(), Error> {
        if let Some(missing) = set_refs(rules).chain(set_refs(out)).find(|n| self.ip_set(n).is_none()) {
            return Err(Error::InvalidSpec(format!("\"@{missing}\" adında bir adres kümesi yok")));
        }
        if let Some(missing) = groups.iter().find(|n| self.fw_group(n).is_none()) {
            return Err(Error::InvalidSpec(format!("\"{missing}\" adında bir güvenlik grubu yok")));
        }
        Ok(())
    }

    pub fn put_ip_set(&mut self, set: IpSet) -> Result<(), Error> {
        if !valid_fw_name(&set.name) {
            return Err(Error::InvalidName(set.name));
        }
        if set.entries.is_empty() || set.entries.len() > MAX_SET_ENTRIES {
            return Err(Error::InvalidSpec(format!("adres kümesinde 1 ile {MAX_SET_ENTRIES} arasında adres olmalı")));
        }
        if let Some(bad) = set.entries.iter().find(|e| !valid_cidr(e)) {
            return Err(Error::InvalidSpec(format!("geçersiz adres \"{bad}\"; örnekler: 192.168.1.10, 10.0.0.0/8")));
        }
        match self.ip_sets.iter_mut().find(|s| s.name == set.name) {
            Some(old) => *old = set,
            None => {
                if self.ip_sets.len() >= MAX_IP_SETS {
                    return Err(Error::InvalidSpec(format!("en fazla {MAX_IP_SETS} adres kümesi tanımlanabilir")));
                }
                let at = self.ip_sets.partition_point(|s| s.name < set.name);
                self.ip_sets.insert(at, set);
            }
        }
        Ok(())
    }

    pub fn put_fw_group(&mut self, group: FwGroup) -> Result<(), Error> {
        if !valid_fw_name(&group.name) {
            return Err(Error::InvalidName(group.name));
        }
        if group.rules.len() > MAX_FW_RULES || group.out.len() > MAX_FW_RULES {
            return Err(Error::InvalidSpec(format!("en fazla {MAX_FW_RULES} güvenlik duvarı kuralı eklenebilir")));
        }
        if let Some(bad) = group.rules.iter().chain(&group.out).find(|r| !r.valid()) {
            return Err(Error::InvalidSpec(format!("geçersiz güvenlik duvarı kuralı: {bad}")));
        }
        self.check_fw_refs(&group.rules, &group.out, &[])?;
        match self.fw_groups.iter_mut().find(|g| g.name == group.name) {
            Some(old) => *old = group,
            None => {
                if self.fw_groups.len() >= MAX_FW_GROUPS {
                    return Err(Error::InvalidSpec(format!("en fazla {MAX_FW_GROUPS} güvenlik grubu tanımlanabilir")));
                }
                let at = self.fw_groups.partition_point(|g| g.name < group.name);
                self.fw_groups.insert(at, group);
            }
        }
        Ok(())
    }

    /// Kümeyi kullanan konuklar ve gruplar.
    pub fn ip_set_users(&self, name: &str) -> (Vec<VmId>, Vec<String>) {
        let uses = |rules: &[FwRule]| set_refs(rules).any(|n| n == name);
        (
            self.guests.iter().filter(|g| uses(&g.hw.fw_rules) || uses(&g.hw.fw_out)).map(|g| g.vmid).collect(),
            self.fw_groups.iter().filter(|g| uses(&g.rules) || uses(&g.out)).map(|g| g.name.clone()).collect(),
        )
    }

    pub fn fw_group_users(&self, name: &str) -> Vec<VmId> {
        self.guests.iter().filter(|g| g.hw.fw_groups.iter().any(|n| n == name)).map(|g| g.vmid).collect()
    }

    pub fn remove_ip_set(&mut self, name: &str) -> Result<(), Error> {
        if self.ip_set(name).is_none() {
            return Err(Error::InvalidSpec(format!("\"@{name}\" adında bir adres kümesi yok")));
        }
        let (guests, groups) = self.ip_set_users(name);
        if !guests.is_empty() || !groups.is_empty() {
            let who = guests.iter().map(ToString::to_string).chain(groups).collect::<Vec<_>>().join(", ");
            return Err(Error::InvalidSpec(format!("adres kümesi kullanımda ({who}); önce oradaki kuralları değiştirin")));
        }
        self.ip_sets.retain(|s| s.name != name);
        Ok(())
    }

    pub fn remove_fw_group(&mut self, name: &str) -> Result<(), Error> {
        if self.fw_group(name).is_none() {
            return Err(Error::InvalidSpec(format!("\"{name}\" adında bir güvenlik grubu yok")));
        }
        let users = self.fw_group_users(name);
        if !users.is_empty() {
            let who = users.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            return Err(Error::InvalidSpec(format!("güvenlik grubu kullanımda ({who}); önce konuklardan çıkarın")));
        }
        self.fw_groups.retain(|g| g.name != name);
        Ok(())
    }

    /// Kümeleri düz adreslere açar.
    fn expand(&self, rules: &[FwRule]) -> Vec<FwRule> {
        let mut out = Vec::new();
        for rule in rules {
            match rule.source.as_deref().and_then(|s| s.strip_prefix('@')) {
                None => out.push(rule.clone()),
                Some(name) => {
                    for entry in self.ip_set(name).map(|s| s.entries.as_slice()).unwrap_or_default() {
                        out.push(FwRule { source: Some(entry.clone()), ..rule.clone() });
                    }
                }
            }
        }
        out
    }

    /// Sürücüye verilecek konuk: kendi kuralları ve gruplarınınkiler, kümeleri açılmış olarak.
    pub fn effective_firewall(&self, guest: &Guest) -> Guest {
        let mut rules = guest.hw.fw_rules.clone();
        let mut out = guest.hw.fw_out.clone();
        for group in guest.hw.fw_groups.iter().filter_map(|n| self.fw_group(n)) {
            rules.extend(group.rules.iter().cloned());
            out.extend(group.out.iter().cloned());
        }
        let mut g = guest.clone();
        g.hw.fw_rules = self.expand(&rules);
        g.hw.fw_out = self.expand(&out);
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::{parse_fw_out_rules, parse_fw_rules};
    use crate::GuestPatch;

    const NOW: u64 = 1_800_000_000;

    fn set(name: &str, entries: &[&str]) -> IpSet {
        IpSet { name: name.into(), entries: entries.iter().map(|e| e.to_string()).collect() }
    }

    #[test]
    fn definitions_are_validated_and_cannot_be_removed_while_used() {
        let mut c = Cluster::demo(NOW);
        assert!(c.put_ip_set(set("Ofis", &["10.0.0.1"])).is_err());
        assert!(c.put_ip_set(set("ofis", &[])).is_err());
        assert!(c.put_ip_set(set("ofis", &["10.0.0.1; drop"])).unwrap_err().to_string().contains("geçersiz adres"));
        c.put_ip_set(set("ofis", &["10.0.0.0/24", "192.168.1.5"])).unwrap();
        let web = FwGroup { name: "web".into(), rules: parse_fw_rules("tcp/443, tcp/22 from @ofis").unwrap(), out: parse_fw_out_rules("udp/53 to @ofis").unwrap() };
        c.put_fw_group(web.clone()).unwrap();
        let bad = FwGroup { name: "db".into(), rules: parse_fw_rules("tcp/5432 from @yok").unwrap(), ..Default::default() };
        assert!(c.put_fw_group(bad).unwrap_err().to_string().contains("@yok"));
        assert!(c.remove_ip_set("ofis").unwrap_err().to_string().contains("web"));

        let patch = |groups: &[&str], rules: &str| GuestPatch { fw_groups: Some(groups.iter().map(|g| g.to_string()).collect()), fw_rules: Some(parse_fw_rules(rules).unwrap()), ..Default::default() };
        assert!(c.update_guest(103, &patch(&["yok"], ""), "u", NOW).unwrap_err().to_string().contains("güvenlik grubu yok"));
        assert!(c.update_guest(103, &patch(&["web"], "tcp/1 from @yok"), "u", NOW).is_err());
        c.update_guest(103, &patch(&["web"], "tcp/8080"), "u", NOW).unwrap();
        assert!(c.remove_fw_group("web").unwrap_err().to_string().contains("103"));

        let effective = c.effective_firewall(c.guest(103).unwrap());
        let shown: Vec<String> = effective.hw.fw_rules.iter().map(ToString::to_string).collect();
        assert_eq!(shown, ["tcp/8080", "tcp/443", "tcp/22 from 10.0.0.0/24", "tcp/22 from 192.168.1.5"]);
        assert_eq!(effective.hw.fw_out.len(), 2);
        assert!(effective.hw.fw_rules.iter().chain(&effective.hw.fw_out).all(FwRule::valid));
        c.update_guest(103, &patch(&[], ""), "u", NOW).unwrap();
        c.remove_fw_group("web").unwrap();
        c.remove_ip_set("ofis").unwrap();
    }

    #[test]
    fn a_missing_definition_allows_nothing_instead_of_everything() {
        let mut c = Cluster::demo(NOW);
        let mut g = c.guest(103).unwrap().clone();
        g.hw.fw_rules = parse_fw_rules("tcp/22 from @silinmis").unwrap();
        g.hw.fw_groups = vec!["silinmis".into()];
        assert!(c.effective_firewall(&g).hw.fw_rules.is_empty());
        c.ip_sets.push(set("silinmis", &[]));
        assert!(c.effective_firewall(&g).hw.fw_rules.is_empty(), "boş küme de bir şeye izin vermez");
    }
}
