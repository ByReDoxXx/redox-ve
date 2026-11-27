//! Sunucu ağlarının kök yardımcısı (`redox net apply <dosya>`).
//!
//! Yetkisiz yönetim sunucusu yalnızca tanım dosyasını yazar. Yardımcı dosyaya güvenmez: yeniden doğrular,
//! sunucunun o anki ağını okur, saf planlayıcıyla komutları üretir ve kabuk kullanmadan çalıştırır. Yalnızca
//! `redox-net` işaretli aygıtları değiştirir; adres ya da varsayılan yol taşıyan arayüzü hiçbir yere bağlamaz.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use redox_core::hostnet::{check_all, network_of, parse_subnet, HostNet};
use serde_json::Value;

/// Yardımcının kurduğu aygıtlara konan işaret (`ifalias`).
pub const MARK: &str = "redox-net";
pub const NFT_TABLE: &str = "redox_net";
const MAX_SPEC_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    /// `bond`, `vlan`, `bridge`, … ; fiziksel kartta boş.
    pub kind: String,
    pub mark: String,
    pub up: bool,
    pub master: Option<String>,
    pub parent: Option<String>,
    pub vlan_id: Option<u16>,
    pub bond_mode: Option<String>,
    pub vlan_filtering: bool,
    /// IPv4 ve genel kapsamlı IPv6 adresleri (`10.0.0.1/24`). Bağlantı-yerel adresler sayılmaz.
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    pub links: Vec<Link>,
    /// Varsayılan yolun geçtiği arayüzler.
    pub default_route: Vec<String>,
}

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

/// `ip -j -d addr show` ve `ip -j route show default` çıktıları.
pub fn parse_observed(links: &str, routes: &str) -> Result<Observed, String> {
    let list: Vec<Value> = serde_json::from_str(links).map_err(|e| format!("ip çıktısı okunamadı: {e}"))?;
    let text = |v: &Value| v.as_str().map(str::to_string);
    let links = list
        .iter()
        .filter_map(|l| {
            let info = &l["linkinfo"];
            Some(Link {
                name: text(&l["ifname"])?,
                kind: text(&info["info_kind"]).unwrap_or_default(),
                mark: text(&l["ifalias"]).unwrap_or_default(),
                up: l["flags"].as_array().is_some_and(|f| f.iter().any(|x| x == "UP")),
                master: text(&l["master"]),
                parent: text(&l["link"]),
                vlan_id: info["info_data"]["id"].as_u64().and_then(|v| u16::try_from(v).ok()),
                bond_mode: text(&info["info_data"]["mode"]),
                vlan_filtering: info["info_data"]["vlan_filtering"].as_u64() == Some(1),
                addresses: l["addr_info"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|a| a["scope"] != "link" && a["scope"] != "host")
                    .filter_map(|a| Some(format!("{}/{}", a["local"].as_str()?, a["prefixlen"].as_u64()?)))
                    .collect(),
            })
        })
        .collect();
    let routes: Vec<Value> = serde_json::from_str(if routes.trim().is_empty() { "[]" } else { routes }).map_err(|e| format!("ip çıktısı okunamadı: {e}"))?;
    Ok(Observed { links, default_route: routes.iter().filter_map(|r| text(&r["dev"])).collect() })
}

/// Bir arayüz bir köprüye ya da bond'a bağlanabilir mi? Bağlanması sunucunun erişimini kesebilecekse hayır.
pub fn port_is_free(port: &str, master: &str, observed: &Observed) -> Result<(), String> {
    let Some(link) = observed.links.iter().find(|l| l.name == port) else {
        return Err(format!("{master}: {port} arayüzü sunucuda yok"));
    };
    if observed.default_route.iter().any(|d| d == port) {
        return Err(format!("{master}: {port} sunucunun varsayılan yolunu taşıyor; bağlanırsa sunucuya erişim kesilir"));
    }
    if !link.addresses.is_empty() {
        return Err(format!("{master}: {port} arayüzünde adres var ({}); bağlanırsa o adres çalışmaz olur", link.addresses.join(", ")));
    }
    if let Some(current) = link.master.as_deref().filter(|m| *m != master) {
        // Yalnızca kendi kurduğumuz bir üst aygıttan alınabilir.
        if !observed.links.iter().any(|l| l.name == current && l.mark == MARK) {
            return Err(format!("{master}: {port} arayüzü {current} aygıtına bağlı; dokunulmadı"));
        }
    }
    Ok(())
}

fn order(net: &HostNet, all: &[HostNet]) -> usize {
    // Alttakiler önce kurulur.
    1 + net.uses().iter().filter_map(|p| all.iter().find(|n| n.name() == *p)).map(|lower| order(lower, all)).max().unwrap_or(0)
}

/// Ağları istenen hale getiren komutlar. Her şey zaten istenen haldeyse liste boştur.
pub fn plan(nets: &[HostNet], observed: &Observed) -> Result<Vec<Step>, String> {
    check_all(nets)?;
    let find = |name: &str| observed.links.iter().find(|l| l.name == name);
    // Önce bütün güvenlik denetimleri: biri bile başarısızsa hiçbir komut üretilmez.
    for net in nets {
        let kind = match net {
            HostNet::Bond { .. } => "bond",
            HostNet::Vlan { .. } => "vlan",
            HostNet::Bridge { .. } | HostNet::Guest { .. } => "bridge",
        };
        if let Some(link) = find(net.name()) {
            if link.mark != MARK || link.kind != kind {
                return Err(format!("{}: bu adda bir aygıt zaten var ve RedoX VE tarafından kurulmamış; dokunulmadı", net.name()));
            }
        }
        for port in net.uses() {
            let defined = nets.iter().any(|n| n.name() == port);
            match net {
                HostNet::Vlan { .. } if !defined && find(port).is_none() => return Err(format!("{}: {port} arayüzü sunucuda yok", net.name())),
                HostNet::Vlan { .. } => {}
                _ if defined && find(port).is_none() => {}
                _ => port_is_free(port, net.name(), observed)?,
            }
        }
        if let HostNet::Guest { address, .. } = net {
            let Some((ip_wanted, prefix)) = parse_subnet(address) else { continue };
            let clash = observed.links.iter().filter(|l| l.name != net.name()).flat_map(|l| l.addresses.iter().map(move |a| (l, a))).find(|(_, a)| {
                parse_subnet(a).is_some_and(|(other, p)| network_of(other, p.min(prefix)) == network_of(ip_wanted, p.min(prefix)))
            });
            if let Some((link, other)) = clash {
                return Err(format!("{}: alt ağ {} arayüzündeki {other} adresiyle çakışıyor", net.name(), link.name));
            }
        }
    }
    let mut ordered: Vec<&HostNet> = nets.iter().collect();
    ordered.sort_by_key(|n| (order(n, nets), n.name().to_string()));
    let mut steps = cleanup(nets, observed);
    for net in ordered {
        let name = net.name();
        let mut current = find(name);
        // Etiket ve bond kipi sonradan değiştirilemez: farklıysa aygıt yeniden kurulur.
        let rebuild = current.is_some_and(|l| match net {
            HostNet::Vlan { parent, id, .. } => l.vlan_id != Some(*id) || l.parent.as_deref() != Some(parent),
            HostNet::Bond { mode, .. } => l.bond_mode.as_deref() != Some(mode.as_str()),
            _ => false,
        });
        if rebuild {
            steps.push(ip(&["link", "del", name]));
            current = None;
        }
        if current.is_none() {
            match net {
                HostNet::Bond { mode, .. } => steps.push(ip(&["link", "add", name, "type", "bond", "mode", mode.as_str(), "miimon", "100"])),
                HostNet::Vlan { parent, id, .. } => steps.push(ip(&["link", "add", "link", parent, "name", name, "type", "vlan", "id", &id.to_string()])),
                HostNet::Bridge { .. } | HostNet::Guest { .. } => steps.push(ip(&["link", "add", name, "type", "bridge"])),
            }
            steps.push(ip(&["link", "set", name, "alias", MARK]));
        }
        if let HostNet::Bridge { vlan_aware, .. } = net {
            if current.map_or(*vlan_aware, |l| l.vlan_filtering != *vlan_aware) {
                steps.push(ip(&["link", "set", name, "type", "bridge", "vlan_filtering", if *vlan_aware { "1" } else { "0" }]));
            }
        }
        if matches!(net, HostNet::Bond { .. } | HostNet::Bridge { .. }) {
            let ports = net.uses();
            // Tanımdan çıkarılan üyeler bırakılır.
            for link in observed.links.iter().filter(|l| l.master.as_deref() == Some(name) && !ports.contains(&l.name.as_str())) {
                // Çalışan konukların TAP arayüzleri köprüde kalır.
                if !(link.kind == "tun" || link.name.starts_with("tap") || link.name.starts_with("rx") || link.name.starts_with("veth")) {
                    steps.push(ip(&["link", "set", &link.name, "nomaster"]));
                }
            }
            for port in ports {
                let attached = !rebuild && find(port).is_some_and(|l| l.master.as_deref() == Some(name));
                if !attached {
                    // Bond üyesi kapalıyken eklenir.
                    if matches!(net, HostNet::Bond { .. }) {
                        steps.push(ip(&["link", "set", port, "down"]));
                    }
                    steps.push(ip(&["link", "set", port, "master", name]));
                }
                if !attached || find(port).is_some_and(|l| !l.up) {
                    steps.push(ip(&["link", "set", port, "up"]));
                }
            }
        }
        if let HostNet::Guest { address, .. } = net {
            let have: &[String] = current.map_or(&[], |l| &l.addresses);
            for old in have.iter().filter(|a| *a != address && a.contains('.')) {
                steps.push(ip(&["addr", "del", old, "dev", name]));
            }
            if !have.contains(address) {
                steps.push(ip(&["addr", "add", address, "broadcast", "+", "dev", name]));
            }
        }
        if current.is_none_or(|l| !l.up) {
            steps.push(ip(&["link", "set", name, "up"]));
        }
    }
    Ok(steps)
}

/// Artık tanımlı olmayan, yardımcının kurduğu aygıtları silen komutlar (üsttekiler önce).
fn cleanup(nets: &[HostNet], observed: &Observed) -> Vec<Step> {
    let mut stale: Vec<&Link> = observed.links.iter().filter(|l| l.mark == MARK && !nets.iter().any(|n| n.name() == l.name)).collect();
    let rank = |kind: &str| match kind {
        "bridge" => 0,
        "vlan" => 1,
        _ => 2,
    };
    stale.sort_by_key(|l| (rank(&l.kind), l.name.clone()));
    stale.iter().map(|l| ip(&["link", "del", &l.name])).collect()
}

/// Konuk ağlarının nftables tablosu: NAT'lı ağ dışarıya sunucunun adresiyle çıkar; NAT'sız ağ yalıtılır
/// (başka arayüzlerle arasında yönlendirme yapılmaz).
pub fn nft_script(nets: &[HostNet]) -> String {
    let (mut nat, mut forward) = (String::new(), String::new());
    for net in nets.iter().filter(|n| n.check().is_ok()) {
        let HostNet::Guest { name, address, nat: out, .. } = net else { continue };
        let Some((gateway, prefix)) = parse_subnet(address) else { continue };
        let subnet = format!("{}/{prefix}", Ipv4Addr::from(network_of(gateway, prefix)));
        if *out {
            nat.push_str(&format!("        ip saddr {subnet} oifname != \"{name}\" masquerade\n"));
        } else {
            forward.push_str(&format!("        iifname \"{name}\" oifname != \"{name}\" drop\n        oifname \"{name}\" iifname != \"{name}\" drop\n"));
        }
    }
    format!(
        "table inet {NFT_TABLE}\ndelete table inet {NFT_TABLE}\ntable inet {NFT_TABLE} {{\n    chain forward {{\n        type filter hook forward priority 0; policy accept;\n{forward}    }}\n    chain postrouting {{\n        type nat hook postrouting priority 100; policy accept;\n{nat}    }}\n}}\n"
    )
}

/// DHCP'li konuk ağının dnsmasq ayarı. dnsmasq yalnızca bu ağda dinler; sunucunun ad çözümleyicisine iletir.
pub fn dnsmasq_conf(net: &HostNet, lease_dir: &Path) -> Option<String> {
    let HostNet::Guest { name, address, dhcp_start: Some(start), dhcp_end: Some(end), nat } = net else { return None };
    net.check().ok()?;
    let (gateway, prefix) = parse_subnet(address)?;
    let mask = Ipv4Addr::from(network_of(Ipv4Addr::BROADCAST, prefix));
    // Yalıtılmış ağda ağ geçidi dağıtılmaz: konuklar dışarı yol aramaz.
    let router = if *nat { format!("dhcp-option=option:router,{gateway}\n") } else { "dhcp-option=option:router\n".to_string() };
    Some(format!(
        "# RedoX VE yazar, elle düzenlemeyin.\ninterface={name}\nbind-interfaces\nexcept-interface=lo\nlisten-address={gateway}\ndhcp-range={start},{end},{mask},12h\n{router}dhcp-authoritative\ndomain-needed\nbogus-priv\ndhcp-leasefile={}\n",
        lease_dir.join(format!("{name}.leases")).display()
    ))
}

/// `qemu-bridge-helper` izin listesi.
pub fn bridge_conf(nets: &[HostNet]) -> String {
    let mut out = String::from("# RedoX VE sunucu ağları; `redox net apply` yazar, elle düzenlemeyin.\n");
    for net in nets.iter().filter(|n| n.is_bridge() && n.check().is_ok()) {
        out.push_str(&format!("allow {}\n", net.name()));
    }
    out
}

pub fn write_spec(path: &Path, nets: &[HostNet]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(nets).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.yeni");
    std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path)).map_err(|e| format!("{} yazılamadı: {e}", path.display()))
}

/// Yardımcı için okur: sembolik bağ kabul edilmez, boyut sınırlıdır, tanımlar yeniden doğrulanır.
pub fn read_spec(path: &Path) -> Result<Vec<HostNet>, String> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!("{} olağan bir dosya değil", path.display()));
    }
    let mut text = String::new();
    std::fs::File::open(path).and_then(|f| f.take(MAX_SPEC_BYTES + 1).read_to_string(&mut text)).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
    if text.len() as u64 > MAX_SPEC_BYTES {
        return Err(format!("{} çok büyük", path.display()));
    }
    let nets: Vec<HostNet> = serde_json::from_str(&text).map_err(|e| format!("{} ağ tanımı değil (satır {})", path.display(), e.line()))?;
    check_all(&nets)?;
    Ok(nets)
}

/// Yardımcının kullandığı programlar ve dizinler.
#[derive(Debug, Clone)]
pub struct Options {
    pub ip: String,
    pub nft: String,
    pub sysctl: String,
    pub systemctl: Option<String>,
    pub bridge_conf: Option<PathBuf>,
    /// dnsmasq ayarlarının yazıldığı dizin (köke ait).
    pub dns_dir: PathBuf,
    pub dry_run: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { ip: "ip".into(), nft: "nft".into(), sysctl: "sysctl".into(), systemctl: Some("systemctl".into()), bridge_conf: None, dns_dir: PathBuf::from("/var/lib/redox-dnsmasq"), dry_run: false }
    }
}

fn run(program: &str, args: &[String], input: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program} çalıştırılamadı: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("bilinmeyen hata")))
}

pub fn observe(ip_bin: &str) -> Result<Observed, String> {
    let words = |text: &str| text.split(' ').map(str::to_string).collect::<Vec<_>>();
    parse_observed(&run(ip_bin, &words("-j -d addr show"), None)?, &run(ip_bin, &words("-j route show default"), None)?)
}

/// Tanım dosyasını uygular; yapılan (ya da `dry_run`da yapılacak) işleri döndürür.
pub fn apply(spec: &Path, options: &Options) -> Result<Vec<String>, String> {
    let nets = read_spec(spec)?;
    let steps = plan(&nets, &observe(&options.ip)?)?;
    let mut done: Vec<String> = steps.iter().map(Step::line).collect();
    let has_guest = nets.iter().any(|n| matches!(n, HostNet::Guest { .. }));
    let routed = nets.iter().any(|n| matches!(n, HostNet::Guest { nat: true, .. }));
    if options.dry_run {
        return Ok(done);
    }
    for step in &steps {
        run(&options.ip, &step.args, None)?;
    }
    // Güvenlik tablosu, yönlendirme açılmadan önce yüklenir: yalıtılmış ağ hiçbir an açık kalmaz.
    // Son yüklenen tablo saklanır: tablo her uygulamada yeniden yüklenir (yeniden açılışta kaybolur), ama
    // yalnızca değiştiğinde bildirilir.
    let loaded = options.dns_dir.join("nft.son");
    if has_guest || loaded.exists() {
        let script = nft_script(&nets);
        let changed = std::fs::read_to_string(&loaded).ok().as_deref() != Some(script.as_str());
        run(&options.nft, &["-f".to_string(), "-".to_string()], Some(&script))?;
        std::fs::create_dir_all(&options.dns_dir).map_err(|e| format!("{} oluşturulamadı: {e}", options.dns_dir.display()))?;
        if has_guest {
            std::fs::write(&loaded, &script).map_err(|e| e.to_string())?;
        } else {
            let _ = std::fs::remove_file(&loaded);
        }
        if changed {
            done.push("nft tablosu yüklendi".to_string());
        }
    }
    if routed && std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").map_or(true, |v| v.trim() != "1") {
        run(&options.sysctl, &["-w".to_string(), "net.ipv4.ip_forward=1".to_string()], None)?;
        done.push("IPv4 yönlendirme açık".to_string());
    }
    if let Some(path) = &options.bridge_conf {
        let temp = path.with_extension("conf.yeni");
        std::fs::write(&temp, bridge_conf(&nets))
            .and_then(|()| {
                #[cfg(unix)]
                std::fs::set_permissions(&temp, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
                std::fs::rename(&temp, path)
            })
            .map_err(|e| format!("{} yazılamadı: {e}", path.display()))?;
    }
    done.extend(apply_dns(&nets, options)?);
    Ok(done)
}

/// DHCP ayarlarını yazar; değişen ağların dnsmasq birimini (yeniden) başlatır, kalkanlarınkini durdurur.
fn apply_dns(nets: &[HostNet], options: &Options) -> Result<Vec<String>, String> {
    let wanted: Vec<(String, String)> = nets.iter().filter_map(|n| Some((n.name().to_string(), dnsmasq_conf(n, &options.dns_dir)?))).collect();
    let existing: Vec<String> = std::fs::read_dir(&options.dns_dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".conf").map(str::to_string)).collect();
    if wanted.is_empty() && existing.is_empty() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(&options.dns_dir).map_err(|e| format!("{} oluşturulamadı: {e}", options.dns_dir.display()))?;
    let unit = |verb: &str, name: &str| -> Result<(), String> {
        match &options.systemctl {
            Some(systemctl) => run(systemctl, &[verb.to_string(), format!("redox-dnsmasq@{name}.service")], None).map(|_| ()),
            None => Ok(()),
        }
    };
    let mut done = Vec::new();
    for name in existing.iter().filter(|n| !wanted.iter().any(|(w, _)| w == *n)) {
        unit("stop", name)?;
        let _ = std::fs::remove_file(options.dns_dir.join(format!("{name}.conf")));
        done.push(format!("DHCP durduruldu: {name}"));
    }
    for (name, text) in wanted {
        let file = options.dns_dir.join(format!("{name}.conf"));
        let changed = std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str());
        std::fs::write(&file, &text).map_err(|e| format!("{} yazılamadı: {e}", file.display()))?;
        // Değişmeyen ağda birim yalnızca (açılıştan sonra) çalışmıyorsa başlatılır.
        unit(if changed { "restart" } else { "start" }, &name)?;
        if changed {
            done.push(format!("DHCP ayarlandı: {name}"));
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::hostnet::BondMode;

    fn link(name: &str) -> Link {
        Link { name: name.into(), up: true, ..Default::default() }
    }

    fn managed(name: &str, kind: &str) -> Link {
        Link { kind: kind.into(), mark: MARK.into(), ..link(name) }
    }

    fn lines(steps: &[Step]) -> Vec<String> {
        steps.iter().map(Step::line).collect()
    }

    fn server() -> Observed {
        let mut eth0 = link("eth0");
        eth0.addresses = vec!["192.168.1.10/24".into()];
        Observed { links: vec![eth0, link("eth1"), link("eth2")], default_route: vec!["eth0".into()] }
    }

    fn net(json: &str) -> HostNet {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_management_interface_is_never_touched() {
        let steal = net(r#"{"kind":"bridge","name":"vmbr1","ports":["eth0"]}"#);
        assert!(plan(&[steal], &server()).unwrap_err().contains("varsayılan yolunu taşıyor"));
        let mut observed = server();
        observed.default_route.clear();
        let steal = HostNet::Bond { name: "bond0".into(), mode: BondMode::Lacp, ports: vec!["eth1".into(), "eth0".into()] };
        assert!(plan(&[steal], &observed).unwrap_err().contains("adres var (192.168.1.10/24)"), "tek bir sakıncalı üye bütün planı durdurur");
        let mut foreign = server();
        foreign.links.push(link("vmbr0"));
        foreign.links[1].master = Some("vmbr0".into());
        assert!(plan(&[net(r#"{"kind":"bridge","name":"vmbr1","ports":["eth1"]}"#)], &foreign).unwrap_err().contains("vmbr0 aygıtına bağlı"));
        assert!(plan(&[net(r#"{"kind":"bridge","name":"vmbr0"}"#)], &foreign).unwrap_err().contains("kurulmamış; dokunulmadı"));
        assert!(plan(&[net(r#"{"kind":"bridge","name":"vmbr1","ports":["eth9"]}"#)], &server()).unwrap_err().contains("sunucuda yok"));
        assert!(plan(&[net(r#"{"kind":"guest","name":"vnet1","address":"192.168.1.1/24"}"#)], &server()).unwrap_err().contains("çakışıyor"));
    }

    #[test]
    fn a_stack_is_built_bottom_up_and_left_alone_once_it_matches() {
        let nets = [
            net(r#"{"kind":"bridge","name":"vmbr1","ports":["bond0.20"],"vlan_aware":true}"#),
            net(r#"{"kind":"vlan","name":"bond0.20","parent":"bond0","id":20}"#),
            net(r#"{"kind":"bond","name":"bond0","mode":"active-backup","ports":["eth1","eth2"]}"#),
        ];
        let built = lines(&plan(&nets, &server()).unwrap());
        let at = |text: &str| built.iter().position(|l| l == text).unwrap_or_else(|| panic!("{text}\n{built:#?}"));
        assert!(at("ip link add bond0 type bond mode active-backup miimon 100") < at("ip link add link bond0 name bond0.20 type vlan id 20"));
        assert!(at("ip link add link bond0 name bond0.20 type vlan id 20") < at("ip link set bond0.20 master vmbr1"));
        assert!(at("ip link set eth1 down") < at("ip link set eth1 master bond0"));
        assert!(built.contains(&"ip link set vmbr1 type bridge vlan_filtering 1".to_string()));
        assert!(built.iter().all(|l| !l.contains("eth0")));

        let mut after = server();
        after.links[1].master = Some("bond0".into());
        after.links[2].master = Some("bond0".into());
        after.links.push(Link { bond_mode: Some("active-backup".into()), ..managed("bond0", "bond") });
        after.links.push(Link { vlan_id: Some(20), parent: Some("bond0".into()), master: Some("vmbr1".into()), ..managed("bond0.20", "vlan") });
        after.links.push(Link { vlan_filtering: true, ..managed("vmbr1", "bridge") });
        after.links.push(Link { master: Some("vmbr1".into()), ..link("tap0") });
        assert_eq!(lines(&plan(&nets, &after).unwrap()), Vec::<String>::new(), "değişiklik yok");
        // Tanımlar kalkınca yalnızca kendi aygıtları, üstten alta silinir.
        assert_eq!(lines(&plan(&[], &after).unwrap()), ["ip link del vmbr1", "ip link del bond0.20", "ip link del bond0"]);
        // Bond kipi değişirse yeniden kurulur ve üyeler geri bağlanır.
        let mut changed = nets.to_vec();
        changed[2] = HostNet::Bond { name: "bond0".into(), mode: BondMode::Lacp, ports: vec!["eth1".into()] };
        let steps = lines(&plan(&changed, &after).unwrap());
        assert!(steps.contains(&"ip link del bond0".to_string()) && steps.contains(&"ip link set eth1 master bond0".to_string()) && steps.contains(&"ip link set eth2 nomaster".to_string()), "{steps:?}");
    }

    #[test]
    fn guest_networks_get_an_address_nat_or_isolation_and_dhcp() {
        let nets = [
            net(r#"{"kind":"guest","name":"vnet1","address":"10.10.0.1/24","dhcp_start":"10.10.0.50","dhcp_end":"10.10.0.99","nat":true}"#),
            net(r#"{"kind":"guest","name":"vnet2","address":"10.20.0.1/24"}"#),
        ];
        let steps = lines(&plan(&nets, &server()).unwrap());
        assert!(steps.contains(&"ip addr add 10.10.0.1/24 broadcast + dev vnet1".to_string()), "{steps:?}");
        let script = nft_script(&nets);
        assert!(script.contains("ip saddr 10.10.0.0/24 oifname != \"vnet1\" masquerade") && !script.contains("10.20.0.0/24"));
        assert!(script.contains("iifname \"vnet2\" oifname != \"vnet2\" drop") && !script.contains("iifname \"vnet1\""));
        let conf = dnsmasq_conf(&nets[0], Path::new("/l")).unwrap();
        for line in ["interface=vnet1", "bind-interfaces", "listen-address=10.10.0.1", "dhcp-range=10.10.0.50,10.10.0.99,255.255.255.0,12h", "dhcp-option=option:router,10.10.0.1"] {
            assert!(conf.lines().any(|l| l == line), "{line}\n{conf}");
        }
        assert!(dnsmasq_conf(&nets[1], Path::new("/l")).is_none());
        assert_eq!(bridge_conf(&nets).lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>(), ["allow vnet1", "allow vnet2"]);
    }

    #[test]
    fn real_ip_output_shapes_are_parsed() {
        let links = r#"[{"ifname":"lo","flags":["LOOPBACK","UP"],"addr_info":[{"local":"127.0.0.1","prefixlen":8,"scope":"host"}]},
          {"ifname":"eth0","flags":["UP"],"addr_info":[{"local":"fe80::1","prefixlen":64,"scope":"link"},{"local":"192.168.1.10","prefixlen":24,"scope":"global"}]},
          {"ifname":"bond0.20","link":"bond0","ifalias":"redox-net","flags":[],"master":"vmbr1","linkinfo":{"info_kind":"vlan","info_data":{"id":20}},"addr_info":[]}]"#;
        let observed = parse_observed(links, r#"[{"dst":"default","dev":"eth0"}]"#).unwrap();
        assert_eq!(observed.default_route, ["eth0"]);
        assert_eq!(observed.links[0].addresses, Vec::<String>::new());
        assert_eq!(observed.links[1].addresses, ["192.168.1.10/24"]);
        let v = &observed.links[2];
        assert_eq!((v.kind.as_str(), v.mark.as_str(), v.vlan_id, v.parent.as_deref(), v.master.as_deref(), v.up), ("vlan", MARK, Some(20), Some("bond0"), Some("vmbr1"), false));
        assert!(parse_observed("x", "").is_err());
    }
}
