//! İstenen anahtarlarla sunucunun o anki ağını karşılaştırıp çalıştırılacak komutları üretir. Saf işlevdir:
//! hiçbir şey okumaz ve çalıştırmaz.

use std::net::IpAddr;

use redox_core::VSwitch;

/// Yardımcının kurduğu aygıtlara konan işaret (`ifalias`). İşaretsiz aygıta dokunulmaz.
pub const MARK: &str = "redox-vds";
pub const PORT: u16 = crate::vxlan::PORT;
const FLOOD: &str = "00:00:00:00:00:00";

/// Sunucuda var olan bir ağ aygıtı.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub kind: String,
    pub mark: String,
    pub mtu: u32,
    pub up: bool,
    pub master: Option<String>,
    pub vni: Option<u32>,
    pub local: Option<IpAddr>,
    pub port: Option<u16>,
    /// VXLAN aygıtında taşkın trafiğinin gönderildiği karşı uçlar.
    pub peers: Vec<IpAddr>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// Sunucunun arayüzlerindeki adresler.
    pub addresses: Vec<IpAddr>,
    pub links: Vec<Link>,
}

/// Çalıştırılacak tek komut: program adı ve ayrı argümanlar (kabuk yoktur).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub program: &'static str,
    pub args: Vec<String>,
}

impl Step {
    pub fn line(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }
}

fn ip(args: &[&str]) -> Step {
    Step { program: "ip", args: args.iter().map(|a| a.to_string()).collect() }
}

fn bridge(args: &[&str]) -> Step {
    Step { program: "bridge", args: args.iter().map(|a| a.to_string()).collect() }
}

pub fn device_name(vni: u32) -> String {
    format!("vx{vni}")
}

/// Bu sunucunun üye listesindeki adresi: tam olarak biri bu sunucuda tanımlı olmalıdır.
pub fn local_member(switch: &VSwitch, addresses: &[IpAddr]) -> Result<IpAddr, String> {
    let mine: Vec<&IpAddr> = switch.members.iter().filter(|m| addresses.contains(m)).collect();
    match mine.as_slice() {
        [one] => Ok(**one),
        [] => Err(format!("{}: üye adreslerinden hiçbiri bu sunucuda tanımlı değil", switch.name)),
        _ => Err(format!("{}: üye adreslerinden birden fazlası bu sunucuda tanımlı", switch.name)),
    }
}

/// Bir anahtarı istenen hale getiren komutlar. Aygıtlar zaten istenen haldeyse liste boştur.
pub fn switch_steps(switch: &VSwitch, observed: &Observed) -> Result<Vec<Step>, String> {
    switch.check()?;
    let local = local_member(switch, &observed.addresses)?;
    let name = switch.name.as_str();
    let device = device_name(switch.vni);
    let device = device.as_str();
    let find = |wanted: &str| observed.links.iter().find(|l| l.name == wanted);
    for (wanted, kind) in [(name, "bridge"), (device, "vxlan")] {
        if let Some(link) = find(wanted).filter(|l| l.mark != MARK || l.kind != kind) {
            return Err(format!("{name}: {} aygıtı zaten var ve RedoX VE tarafından kurulmamış; dokunulmadı", link.name));
        }
    }
    let mtu = switch.mtu.to_string();
    let mut steps = Vec::new();
    let have_bridge = find(name);
    if have_bridge.is_none() {
        steps.push(ip(&["link", "add", name, "type", "bridge"]));
        steps.push(ip(&["link", "set", name, "alias", MARK]));
    }
    // VNI, yerel adres ve kapı sonradan değiştirilemez: farklıysa aygıt silinip yeniden kurulur.
    let mut have_device = find(device);
    if have_device.is_some_and(|l| l.vni != Some(switch.vni) || l.local != Some(local) || l.port != Some(PORT)) {
        steps.push(ip(&["link", "del", device]));
        have_device = None;
    }
    if have_device.is_none() {
        steps.push(ip(&["link", "add", device, "type", "vxlan", "id", &switch.vni.to_string(), "local", &local.to_string(), "dstport", &PORT.to_string()]));
        steps.push(ip(&["link", "set", device, "alias", MARK]));
    }
    if have_device.is_none_or(|l| l.mtu != switch.mtu) {
        steps.push(ip(&["link", "set", device, "mtu", &mtu]));
    }
    if have_device.is_none_or(|l| l.master.as_deref() != Some(name)) {
        steps.push(ip(&["link", "set", device, "master", name]));
    }
    let known: &[IpAddr] = have_device.map_or(&[], |l| &l.peers);
    let wanted: Vec<IpAddr> = switch.members.iter().copied().filter(|m| *m != local).collect();
    for peer in wanted.iter().filter(|p| !known.contains(p)) {
        steps.push(bridge(&["fdb", "append", FLOOD, "dev", device, "dst", &peer.to_string()]));
    }
    for peer in known.iter().filter(|p| !wanted.contains(p)) {
        steps.push(bridge(&["fdb", "del", FLOOD, "dev", device, "dst", &peer.to_string()]));
    }
    if have_device.is_none_or(|l| !l.up) {
        steps.push(ip(&["link", "set", device, "up"]));
    }
    // Köprünün MTU'su, kartı bağlandıktan sonra açıkça yazılır.
    if have_bridge.is_none_or(|l| l.mtu != switch.mtu) {
        steps.push(ip(&["link", "set", name, "mtu", &mtu]));
    }
    if have_bridge.is_none_or(|l| !l.up) {
        steps.push(ip(&["link", "set", name, "up"]));
    }
    Ok(steps)
}

/// Artık tanımlı olmayan anahtarların, yardımcının kurduğu aygıtlarını silen komutlar.
pub fn cleanup_steps(switches: &[VSwitch], observed: &Observed) -> Vec<Step> {
    let wanted = |link: &Link| match link.kind.as_str() {
        "bridge" => switches.iter().any(|s| s.name == link.name),
        _ => switches.iter().any(|s| device_name(s.vni) == link.name),
    };
    // Önce tünel aygıtları, sonra köprüler.
    let mut stale: Vec<&Link> = observed.links.iter().filter(|l| l.mark == MARK && matches!(l.kind.as_str(), "bridge" | "vxlan") && !wanted(l)).collect();
    stale.sort_by_key(|l| (l.kind != "vxlan", l.name.clone()));
    stale.iter().map(|l| ip(&["link", "del", &l.name])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn switch() -> VSwitch {
        VSwitch { name: "vds-web".to_string(), vni: 100, mtu: 1450, members: ["10.0.0.1", "10.0.0.2", "10.0.0.3"].iter().map(|m| address(m)).collect() }
    }

    fn lines(steps: &[Step]) -> Vec<String> {
        steps.iter().map(Step::line).collect()
    }

    /// `switch()` kurulduktan sonra 10.0.0.2 adresli sunucunun göreceği ağ.
    fn built() -> Observed {
        let bridge = Link { name: "vds-web".to_string(), kind: "bridge".to_string(), mark: MARK.to_string(), mtu: 1450, up: true, ..Default::default() };
        let device = Link {
            name: "vx100".to_string(),
            kind: "vxlan".to_string(),
            mark: MARK.to_string(),
            mtu: 1450,
            up: true,
            master: Some("vds-web".to_string()),
            vni: Some(100),
            local: Some(address("10.0.0.2")),
            port: Some(PORT),
            peers: vec![address("10.0.0.1"), address("10.0.0.3")],
        };
        Observed { addresses: vec![address("10.0.0.2")], links: vec![bridge, device] }
    }

    #[test]
    fn a_missing_switch_is_built_with_this_servers_own_address() {
        let observed = Observed { addresses: vec![address("192.168.1.5"), address("10.0.0.2")], links: Vec::new() };
        assert_eq!(
            lines(&switch_steps(&switch(), &observed).unwrap()),
            [
                "ip link add vds-web type bridge",
                "ip link set vds-web alias redox-vds",
                "ip link add vx100 type vxlan id 100 local 10.0.0.2 dstport 4789",
                "ip link set vx100 alias redox-vds",
                "ip link set vx100 mtu 1450",
                "ip link set vx100 master vds-web",
                "bridge fdb append 00:00:00:00:00:00 dev vx100 dst 10.0.0.1",
                "bridge fdb append 00:00:00:00:00:00 dev vx100 dst 10.0.0.3",
                "ip link set vx100 up",
                "ip link set vds-web mtu 1450",
                "ip link set vds-web up",
            ]
        );
    }

    #[test]
    fn only_what_differs_is_changed() {
        assert!(switch_steps(&switch(), &built()).unwrap().is_empty(), "kurulu anahtarda yapılacak iş yoktur");
        let mut changed = switch();
        changed.mtu = 1400;
        changed.members = vec![address("10.0.0.2"), address("10.0.0.3"), address("10.0.0.4")];
        assert_eq!(
            lines(&switch_steps(&changed, &built()).unwrap()),
            [
                "ip link set vx100 mtu 1400",
                "bridge fdb append 00:00:00:00:00:00 dev vx100 dst 10.0.0.4",
                "bridge fdb del 00:00:00:00:00:00 dev vx100 dst 10.0.0.1",
                "ip link set vds-web mtu 1400",
            ]
        );
        // Yerel adres değişince tünel aygıtı yeniden kurulur, köprü (ve ona bağlı konuklar) kalır.
        let mut moved = built();
        moved.links[1].local = Some(address("10.0.0.9"));
        let steps = lines(&switch_steps(&switch(), &moved).unwrap());
        assert!(steps[0] == "ip link del vx100" && steps.iter().any(|s| s.starts_with("ip link add vx100")) && !steps.iter().any(|s| s.contains("add vds-web")), "{steps:?}");
    }

    #[test]
    fn foreign_devices_and_wrong_hosts_are_refused() {
        for index in [0, 1] {
            let mut observed = built();
            observed.links[index].mark = String::new();
            assert!(switch_steps(&switch(), &observed).unwrap_err().contains("dokunulmadı"), "{index}");
        }
        let mut wrong_kind = built();
        wrong_kind.links[0].kind = "dummy".to_string();
        assert!(switch_steps(&switch(), &wrong_kind).is_err());
        let mut observed = built();
        observed.addresses = vec![address("10.0.0.9")];
        assert!(switch_steps(&switch(), &observed).unwrap_err().contains("hiçbiri"));
        observed.addresses = vec![address("10.0.0.1"), address("10.0.0.2")];
        assert!(switch_steps(&switch(), &observed).unwrap_err().contains("birden fazlası"));
        // Doğrulanmamış bir tanımdan komut üretilmez.
        let mut bad = switch();
        bad.name = "vmbr0".to_string();
        assert!(switch_steps(&bad, &built()).is_err());
    }

    #[test]
    fn cleanup_removes_only_marked_leftovers() {
        let mut observed = built();
        observed.links.push(Link { name: "vmbr0".to_string(), kind: "bridge".to_string(), ..Default::default() });
        observed.links.push(Link { name: "eth0".to_string(), kind: String::new(), ..Default::default() });
        assert!(cleanup_steps(&[switch()], &observed).is_empty());
        assert_eq!(lines(&cleanup_steps(&[], &observed)), ["ip link del vx100", "ip link del vds-web"]);
        // VNI değişince eski tünel aygıtı artık istenmiyordur.
        let mut renumbered = switch();
        renumbered.vni = 200;
        assert_eq!(lines(&cleanup_steps(&[renumbered], &observed)), ["ip link del vx100"]);
    }
}
