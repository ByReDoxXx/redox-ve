//! Güvenlik duvarı kurallarının **gerçek çekirdekte, gerçek paketlerle** sınanması.
//!
//! Sürücünün ürettiği nftables betiği, QEMU'da açılan bir Linux'un içinde yüklenir; orada ayrı ağ ad alanları
//! bir köprüyle bağlanır: biri güvenlik duvarlı "konuk" (konuğun gerçek MAC adresleriyle, iki kartlı), biri
//! "öbür makine"dir. Başlangıç dosya sisteminde `/fw` altında `nft`, `ip`, gerekli çekirdek modülleri ve küçük
//! bir bağlantı sınama aracı bulunmalıdır (`REDOX_REAL_INITRD_FW`).

use super::real_qemu::{real_firewall, Real};
use super::*;

use redox_core::hw::{parse_fw_out_rules, parse_fw_rules};
use redox_core::{mac_for, mac_for_nic};
use redox_server::firewall;

const HOST: &str = "10.7.0.1";
const OTHER: &str = "10.7.0.2";
const GUEST: &str = "10.7.0.10";
const GUEST_SECOND: &str = "10.7.1.10";

fn load(r: &Real, script: &str) {
    r.ask(": > /tmp/rules");
    for line in script.lines() {
        assert!(!line.contains('\''), "{line}");
        r.ask(&format!("echo '{line}' >> /tmp/rules"));
    }
    let out = r.ask("/fw/nft -f /tmp/rules && echo YUKLENDI");
    assert!(out.iter().any(|l| l == "YUKLENDI"), "kurallar gerçek çekirdeğe yüklenemedi: {out:?}");
}

/// `from` ad alanından (boşsa sunucunun kendisinden) `ip:port` açılabiliyor mu?
fn open(r: &Real, from: &str, ip: &str, port: u16) -> bool {
    let prefix = if from.is_empty() { String::new() } else { format!("/fw/ip netns exec {from} ") };
    let out = r.ask(&format!("{prefix}/fw/probe connect {ip} {port}"));
    assert!(out.iter().any(|l| l == "ACIK" || l == "KAPALI"), "{out:?}");
    out.iter().any(|l| l == "ACIK")
}

#[test]
fn real_kernel_enforces_inbound_outbound_groups_and_every_network_card() {
    let Some(r) = real_firewall() else { return };
    let mut cluster = Cluster::demo(0);
    let mut spec = cluster.guest(100).map(|g| g.clone()).unwrap();
    spec.vmid = 100;
    spec.kind = GuestKind::Vm;
    spec.bridge = "br0".into();
    spec.hw = Default::default();
    spec.hw.nics = vec!["br0".into()];

    // Düz bir QEMU konuğu yalnızca çekirdek vermek için açılır.
    let driver = QemuDriver::new(r.cfg.clone()).unwrap();
    let mut vm = spec.clone();
    (vm.name, vm.bridge, vm.hw.nics, vm.cores, vm.mem_total_mb, vm.disk_gb) = ("fw-cekirdek".into(), "nat".into(), Vec::new(), 2, 768, 1);
    driver.create(&vm).unwrap();
    driver.apply(&vm, Action::Start).unwrap();
    r.boot();
    let loaded = r.ask("for m in $(cat /fw/order); do insmod /fw/mod/$m 2>/dev/null || { xz -dc /fw/mod/$m > /tmp/m.ko && insmod /tmp/m.ko; } 2>/dev/null; done; /fw/nft add table bridge deneme && /fw/nft delete table bridge deneme && echo KOPRU-DUVARI-VAR");
    assert!(loaded.iter().any(|l| l == "KOPRU-DUVARI-VAR"), "{loaded:?}");

    r.ask("mkdir -p /run/netns /var/run; /fw/ip link set lo up; /fw/ip link add br0 type bridge && /fw/ip addr add 10.7.0.1/16 dev br0 && /fw/ip link set br0 up");
    r.ask("/fw/ip netns add obur; /fw/ip netns add konuk");
    let card = |ns: &str, host: &str, inner: &str, mac: &str, ip: &str| {
        format!("/fw/ip link add {host} type veth peer name {inner} && /fw/ip link set {host} master br0 up && /fw/ip link set {inner} netns {ns} && /fw/ip netns exec {ns} /fw/ip link set {inner} address {mac} && /fw/ip netns exec {ns} /fw/ip addr add {ip}/24 dev {inner} && /fw/ip netns exec {ns} /fw/ip link set {inner} up && echo KART")
    };
    for command in [card("obur", "ho", "eo", "02:00:00:00:00:02", OTHER), card("konuk", "h0", "e0", &mac_for(100), GUEST), card("konuk", "h1", "e1", &mac_for_nic(100, 1), GUEST_SECOND)] {
        assert!(r.ask(&command).iter().any(|l| l == "KART"), "{command}");
    }
    // İki ağ da aynı köprüde: öbür makineye ikinci ağdan da bir adres ve yollar.
    r.ask("/fw/ip netns exec obur /fw/ip addr add 10.7.1.2/24 dev eo; /fw/ip addr add 10.7.1.1/24 dev br0");
    r.ask("/fw/ip netns exec konuk sysctl -w net.ipv4.conf.all.arp_ignore=1 >/dev/null 2>&1; for p in 22 80 8080; do /fw/ip netns exec konuk /fw/probe listen $p; done; for p in 443 25 8000; do /fw/ip netns exec obur /fw/probe listen $p; /fw/probe listen $p; done; sleep 1");

    // Duvar yokken her şey açık: sınama düzeneği çalışıyor.
    assert!(open(&r, "obur", GUEST, 22) && open(&r, "obur", GUEST, 8080) && open(&r, "obur", GUEST_SECOND, 22));
    assert!(open(&r, "konuk", OTHER, 443) && open(&r, "konuk", OTHER, 25) && open(&r, "konuk", HOST, 8000));

    // 1) Yalnızca gelen: kurala uyan kapılar açık, gerisi iki kartta da kapalı; giden serbest.
    cluster.ip_sets = vec![redox_core::fwsets::IpSet { name: "yonetim".into(), entries: vec![OTHER.into()] }];
    cluster.fw_groups = vec![redox_core::fwsets::FwGroup { name: "web".into(), rules: parse_fw_rules("tcp/80").unwrap(), out: parse_fw_out_rules("tcp/443 to @yonetim").unwrap() }];
    let mut g = spec.clone();
    g.hw.firewall = true;
    g.hw.fw_rules = parse_fw_rules("tcp/22 from @yonetim").unwrap();
    g.hw.fw_groups = vec!["web".into()];
    load(&r, &firewall::ruleset(&[cluster.effective_firewall(&g)]));
    assert!(open(&r, "obur", GUEST, 22), "kümedeki kaynaktan ssh");
    assert!(open(&r, "obur", GUEST, 80), "grup kuralı");
    assert!(!open(&r, "obur", GUEST, 8080), "kuralsız kapı kapalı");
    assert!(!open(&r, "", GUEST, 22), "küme dışındaki kaynak (sunucunun kendisi) ssh açamaz");
    assert!(open(&r, "", GUEST, 80), "grup kuralı her kaynağa açık");
    assert!(open(&r, "obur", "10.7.1.10", 80) && !open(&r, "obur", "10.7.1.10", 8080), "ikinci kart da süzülür");
    assert!(open(&r, "konuk", OTHER, 443) && open(&r, "konuk", OTHER, 25) && open(&r, "konuk", HOST, 8000), "giden kısıtlı değilken serbest");

    // 2) Giden de kısıtlı: yalnızca grubun giden kuralı geçer; izinli gelen bağlantının yanıtı geçer.
    g.hw.fw_out_restrict = true;
    load(&r, &firewall::ruleset(&[cluster.effective_firewall(&g)]));
    assert!(open(&r, "konuk", OTHER, 443), "izinli hedef ve kapı");
    assert!(!open(&r, "konuk", OTHER, 25), "kuralsız giden kapı kapalı");
    assert!(!open(&r, "konuk", HOST, 443), "izinli kapı ama küme dışındaki hedef (sunucu) kapalı");
    assert!(!open(&r, "konuk", HOST, 8000), "sunucuya giden trafik de süzülür");
    assert!(open(&r, "obur", GUEST, 22) && open(&r, "obur", GUEST, 80), "gelen bağlantının yanıtı giden kısıtına takılmaz");

    // 3) Konuğun kendi kuralı da eklenir.
    g.hw.fw_out = parse_fw_out_rules("tcp/8000 to 10.7.0.1").unwrap();
    load(&r, &firewall::ruleset(&[cluster.effective_firewall(&g)]));
    assert!(open(&r, "konuk", HOST, 8000) && !open(&r, "konuk", OTHER, 25) && !open(&r, "konuk", OTHER, 8000));

    // 4) Duvar kapatılınca kurallar kalkar.
    g.hw.firewall = false;
    load(&r, &firewall::ruleset(&[cluster.effective_firewall(&g)]));
    assert!(open(&r, "obur", GUEST, 8080) && open(&r, "konuk", OTHER, 25));
    driver.apply(&vm, Action::Stop).unwrap();
    eprintln!("gerçek çekirdekte güvenlik duvarı: tamam");
}
