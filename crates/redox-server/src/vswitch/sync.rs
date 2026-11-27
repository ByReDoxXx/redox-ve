//! Ayrıcalıklı yardımcı: ağın o anki halini `ip -j` ile okur ve planlanan komutları çalıştırır.

use std::net::IpAddr;
use std::process::Command;

use redox_core::VSwitch;
use serde_json::Value;

use super::plan::{cleanup_steps, device_name, switch_steps, Link, Observed, Step};

/// `ip -j addr show` çıktısındaki adresler.
pub fn parse_addresses(json: &str) -> Result<Vec<IpAddr>, String> {
    let all: Vec<Value> = serde_json::from_str(json).map_err(|e| format!("ip çıktısı anlaşılamadı: {e}"))?;
    Ok(all.iter().flat_map(|l| l["addr_info"].as_array().cloned().unwrap_or_default()).filter_map(|a| a["local"].as_str()?.parse().ok()).collect())
}

/// `ip -d -j link show` çıktısındaki aygıtlar.
pub fn parse_links(json: &str) -> Result<Vec<Link>, String> {
    let all: Vec<Value> = serde_json::from_str(json).map_err(|e| format!("ip çıktısı anlaşılamadı: {e}"))?;
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    Ok(all
        .iter()
        .map(|l| {
            let info = &l["linkinfo"]["info_data"];
            Link {
                name: text(&l["ifname"]),
                kind: text(&l["linkinfo"]["info_kind"]),
                mark: text(&l["ifalias"]),
                mtu: l["mtu"].as_u64().unwrap_or(0) as u32,
                up: l["flags"].as_array().is_some_and(|f| f.iter().any(|x| x == "UP")),
                master: l["master"].as_str().map(str::to_string),
                vni: info["id"].as_u64().map(|v| v as u32),
                local: info["local"].as_str().or_else(|| info["local6"].as_str()).and_then(|a| a.parse().ok()),
                port: info["port"].as_u64().map(|v| v as u16),
                peers: Vec::new(),
            }
        })
        .collect())
}

/// `bridge -j fdb show dev <vxlan>` çıktısındaki taşkın hedefleri.
pub fn parse_peers(json: &str) -> Result<Vec<IpAddr>, String> {
    let all: Vec<Value> = serde_json::from_str(json).map_err(|e| format!("bridge çıktısı anlaşılamadı: {e}"))?;
    Ok(all.iter().filter(|e| e["mac"] == "00:00:00:00:00:00").filter_map(|e| e["dst"].as_str()?.parse().ok()).collect())
}

fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("{program} çalıştırılamadı: {e} (iproute2 kurulu mu?)"))?;
    if !out.status.success() {
        return Err(format!("{program} {} başarısız: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn observe() -> Result<Observed, String> {
    let addresses = parse_addresses(&output("ip", &["-j", "addr", "show"])?)?;
    let mut links = parse_links(&output("ip", &["-d", "-j", "link", "show"])?)?;
    for link in links.iter_mut().filter(|l| l.kind == "vxlan" && l.mark == super::plan::MARK) {
        link.peers = parse_peers(&output("bridge", &["-j", "fdb", "show", "dev", &link.name])?)?;
    }
    Ok(Observed { addresses, links })
}

fn execute(step: &Step) -> Result<(), String> {
    let args: Vec<&str> = step.args.iter().map(String::as_str).collect();
    output(step.program, &args).map(|_| ())
}

/// Ağı tanımlara uydurur. Bir anahtardaki hata ötekileri durdurmaz. `(yapılanlar, hatalar)` döner.
/// `dry_run` doğruysa hiçbir şey değiştirilmez, komutlar yalnızca listelenir.
pub fn sync(switches: &[VSwitch], dry_run: bool) -> Result<(Vec<String>, Vec<String>), String> {
    redox_core::vswitch::check_all(switches)?;
    let (mut done, mut errors) = (Vec::new(), Vec::new());
    let mut run = |steps: Vec<Step>, errors: &mut Vec<String>| {
        for step in steps {
            if !dry_run {
                if let Err(e) = execute(&step) {
                    errors.push(e);
                    return false;
                }
            }
            done.push(step.line());
        }
        true
    };
    // Önce artıklar silinir: VNI'si değişen bir anahtarın eski tünel aygıtı yenisiyle çakışmasın.
    let observed = observe()?;
    let stale = cleanup_steps(switches, &observed);
    run(stale, &mut errors);
    let observed = if dry_run { observed } else { observe()? };
    for switch in switches {
        match switch_steps(switch, &observed) {
            Ok(steps) => {
                // Yeni kurulumda yarım kalan anahtar geri alınır: yarım bir ağ "hazır" görünmesin.
                let fresh = !observed.links.iter().any(|l| l.name == switch.name);
                if !run(steps, &mut errors) && fresh {
                    for name in [device_name(switch.vni), switch.name.clone()] {
                        let _ = execute(&Step { program: "ip", args: vec!["link".into(), "del".into(), name] });
                    }
                }
            }
            Err(e) => errors.push(e),
        }
    }
    Ok((done, errors))
}

#[cfg(test)]
mod tests {
    use super::*;

    // iproute2'nin gerçek çıktısından kısaltılmıştır.
    const LINKS: &str = r#"[
        {"ifname":"lo","flags":["LOOPBACK","UP"],"mtu":65536},
        {"ifname":"vds-web","flags":["BROADCAST","MULTICAST"],"mtu":1450,"linkinfo":{"info_kind":"bridge","info_data":{"stp_state":0}},"ifalias":"redox-vds"},
        {"ifname":"vx100","flags":["BROADCAST","UP","LOWER_UP"],"mtu":1450,"master":"vds-web","linkinfo":{"info_kind":"vxlan","info_data":{"id":100,"local":"10.0.0.2","port":4789},"info_slave_kind":"bridge"},"ifalias":"redox-vds"},
        {"ifname":"vx6","flags":[],"mtu":1450,"linkinfo":{"info_kind":"vxlan","info_data":{"id":6,"local6":"fd00::2","port":4789}}}
    ]"#;

    #[test]
    fn iproute_output_is_parsed() {
        let links = parse_links(LINKS).unwrap();
        assert_eq!((links[1].kind.as_str(), links[1].mark.as_str(), links[1].up, links[1].mtu), ("bridge", "redox-vds", false, 1450));
        let device = &links[2];
        assert_eq!((device.vni, device.port, device.master.as_deref(), device.up), (Some(100), Some(4789), Some("vds-web"), true));
        assert_eq!(device.local, "10.0.0.2".parse().ok());
        assert_eq!(links[3].local, "fd00::2".parse().ok());
        assert_eq!((links[0].kind.as_str(), links[0].mark.as_str()), ("", ""));
        let addresses = parse_addresses(r#"[{"addr_info":[{"family":"inet","local":"10.0.0.2"},{"family":"inet6","local":"fe80::1"}]},{"addr_info":[]}]"#).unwrap();
        assert_eq!(addresses.len(), 2);
        let peers = parse_peers(r#"[{"mac":"aa:bb:cc:dd:ee:ff","master":"vds-web"},{"mac":"00:00:00:00:00:00","dst":"10.0.0.1"},{"mac":"aa:bb:cc:dd:ee:01","dst":"10.0.0.3"}]"#).unwrap();
        assert_eq!(peers, ["10.0.0.1".parse::<IpAddr>().unwrap()], "yalnızca taşkın girdileri sayılır");
        assert!(parse_links("bozuk").is_err());
    }
}
