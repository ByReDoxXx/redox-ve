//! Konuk güvenlik duvarı: kurallar nftables `bridge` ailesinde, konuğun MAC adresine göre uygulanır.
//!
//! Güvenlik duvarı açık konuğa gelen trafik yalnızca izin verilen kurallara uyuyorsa geçer;
//! konuğun başlattığı bağlantılar ve yanıtları serbesttir. Kurallar köprü üzerinden geçen
//! trafiğe (başka makinelerden ve sunucunun kendisinden) uygulanır; kullanıcı kipinde NAT'taki
//! sanal makineye dışarıdan zaten yalnızca yönlendirilen kapılardan erişilir.

use std::io::Write;
use std::process::{Command, Stdio};

use redox_core::hw::FwProto;
use redox_core::{mac_for, Guest, GuestKind, NAT};

pub const TABLE: &str = "redox";

/// Bu konuğun kuralları yüklenir mi? NAT'taki sanal makine köprüden geçmez.
pub fn enforced(g: &Guest) -> bool {
    g.hw.firewall && (g.kind == GuestKind::Ct || g.bridge != NAT)
}

/// Bütün konukların kurallarını içeren nftables betiği. Tablo silinip yeniden kurulduğu ve betik
/// tek işlemde yüklendiği için arada korumasız bir an oluşmaz.
pub fn ruleset(guests: &[Guest]) -> String {
    ruleset_with(guests, &[])
}

/// Ağ kartı koruması bu konuk için kurulur mu? Yalnızca çalışan sanal makinenin köprüdeki kartlarında anlamlıdır.
pub fn guarded(g: &Guest) -> bool {
    g.kind == GuestKind::Vm && g.hw.nic_guard && g.status != redox_core::GuestStatus::Stopped
}

/// Konuğun bütün kartlarının MAC adresleri.
fn macs(g: &Guest) -> Vec<String> {
    std::iter::once(mac_for(g.vmid)).chain((1..=g.hw.nics.len() as u8).map(|n| redox_core::mac_for_nic(g.vmid, n))).collect()
}

/// Ağ kartı korumaları: konuğun sunucudaki arayüzlerinden (`taps`) giren trafik köprüye ulaşmadan süzülür.
/// Kaynak MAC konuğun kendi adreslerinden biri değilse, ya da konuk DHCP sunucusu / yönlendirici gibi
/// davranıyorsa çerçeve düşürülür.
fn guards(guests: &[Guest], taps: &[(redox_core::VmId, Vec<String>)]) -> String {
    let mut jumps = String::new();
    let mut chains = String::new();
    for g in guests.iter().filter(|g| guarded(g)) {
        let names: Vec<String> = taps
            .iter()
            .filter(|(vmid, _)| *vmid == g.vmid)
            .flat_map(|(_, names)| names)
            .filter(|n| crate::netlimit::valid_ifname(n))
            .map(|n| format!("\"{n}\""))
            .collect();
        if names.is_empty() {
            continue;
        }
        jumps.push_str(&format!("        iifname {{ {} }} jump s{}\n", names.join(", "), g.vmid));
        chains.push_str(&format!("    chain s{} {{\n", g.vmid));
        chains.push_str(&format!("        ether saddr != {{ {} }} drop\n", macs(g).join(", ")));
        chains.push_str("        udp sport 67 udp dport 68 drop\n");
        chains.push_str("        udp sport 547 udp dport 546 drop\n");
        chains.push_str("        icmpv6 type { nd-router-advert, nd-redirect } drop\n");
        chains.push_str("    }\n");
    }
    if jumps.is_empty() {
        return String::new();
    }
    format!("    chain prerouting {{\n        type filter hook prerouting priority -300; policy accept;\n{jumps}    }}\n{chains}")
}

/// `taps`: korunan konukların sunucudaki arayüz adları.
pub fn ruleset_with(guests: &[Guest], taps: &[(redox_core::VmId, Vec<String>)]) -> String {
    let guards = guards(guests, taps);
    let mut jumps = String::new();
    let mut chains = String::new();
    for g in guests.iter().filter(|g| enforced(g) && g.hw.fw_rules.iter().all(|r| r.valid())) {
        jumps.push_str(&format!("        ether daddr {} jump g{}\n", mac_for(g.vmid), g.vmid));
        chains.push_str(&format!("    chain g{} {{\n", g.vmid));
        // Kurulu bağlantıların yanıtları, adres çözümleme ve DHCP yanıtı her zaman geçer.
        chains.push_str("        ct state established,related accept\n");
        chains.push_str("        ether type arp accept\n");
        chains.push_str("        udp sport 67 udp dport 68 accept\n");
        chains.push_str("        icmpv6 type { nd-neighbor-solicit, nd-neighbor-advert, nd-router-advert } accept\n");
        for rule in &g.hw.fw_rules {
            let source = rule.source.as_ref().map(|s| format!("ip saddr {s} ")).unwrap_or_default();
            let line = match (rule.proto, rule.ports) {
                (FwProto::Icmp, _) => format!("{source}ip protocol icmp accept"),
                (proto, Some((a, b))) if a == b => format!("{source}{} dport {a} accept", proto.as_str()),
                (proto, Some((a, b))) => format!("{source}{} dport {a}-{b} accept", proto.as_str()),
                (proto, None) => format!("{source}meta l4proto {} accept", proto.as_str()),
            };
            chains.push_str(&format!("        {line}\n"));
        }
        chains.push_str("        drop\n    }\n");
    }
    // Önce tabloyu (boş da olsa) tanımla ki silme komutu ilk çalıştırmada hata vermesin.
    format!(
        "table bridge {TABLE}\ndelete table bridge {TABLE}\ntable bridge {TABLE} {{\n    chain forward {{\n        type filter hook forward priority 0; policy accept;\n{jumps}    }}\n    chain output {{\n        type filter hook output priority 0; policy accept;\n{jumps}    }}\n{chains}{guards}}}\n"
    )
}

/// Betiği `nft -f -` ile yükler.
pub fn apply(nft_bin: &str, script: &str) -> Result<(), String> {
    let mut child = Command::new(nft_bin)
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{nft_bin} çalıştırılamadı: {e} (nftables kurulu mu?)"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).map_err(|e| format!("kurallar nft'ye yazılamadı: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("nft beklenemedi: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&out.stderr);
    let first = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("bilinmeyen hata").trim().to_string();
    Err(format!("güvenlik duvarı kuralları yüklenemedi: {first}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::hw::parse_fw_rules;
    use redox_core::Cluster;

    fn guest(vmid: u32, bridge: &str, rules: &str) -> Guest {
        let mut g = Cluster::demo(0).guests[0].clone();
        g.vmid = vmid;
        g.kind = GuestKind::Vm;
        g.bridge = bridge.to_string();
        g.hw.firewall = true;
        g.hw.fw_rules = parse_fw_rules(rules).unwrap();
        g
    }

    #[test]
    fn ruleset_isolates_each_guest_by_mac() {
        let web = guest(100, "vmbr0", "tcp/22 from 10.0.0.0/8, tcp/80-443, udp, icmp");
        let script = ruleset(&[web]);
        assert!(script.starts_with("table bridge redox\ndelete table bridge redox\ntable bridge redox {\n"), "{script}");
        assert_eq!(script.matches("ether daddr 52:54:00:00:00:64 jump g100").count(), 2, "hem geçen hem sunucudan çıkan trafik");
        let chain = script.split("chain g100 {").nth(1).unwrap();
        let lines: Vec<&str> = chain.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        assert_eq!(
            lines,
            [
                "ct state established,related accept",
                "ether type arp accept",
                "udp sport 67 udp dport 68 accept",
                "icmpv6 type { nd-neighbor-solicit, nd-neighbor-advert, nd-router-advert } accept",
                "ip saddr 10.0.0.0/8 tcp dport 22 accept",
                "tcp dport 80-443 accept",
                "meta l4proto udp accept",
                "ip protocol icmp accept",
                "drop",
                "}",
                "}",
            ]
        );
    }

    #[test]
    fn only_enforceable_guests_get_rules() {
        let mut off = guest(101, "vmbr0", "tcp/22");
        off.hw.firewall = false;
        let nat_vm = guest(102, NAT, "tcp/22");
        let mut nat_ct = guest(103, NAT, "");
        nat_ct.kind = GuestKind::Ct;
        assert!(!enforced(&off) && !enforced(&nat_vm) && enforced(&nat_ct), "konteyner NAT'ta da bir köprüdedir");
        let script = ruleset(&[off, nat_vm, nat_ct]);
        assert!(!script.contains("g101") && !script.contains("g102") && script.contains("chain g103 {"));
        // Kuralsız ama açık güvenlik duvarı: gelen her şey düşer.
        assert!(script.split("chain g103 {").nth(1).unwrap().contains("accept\n        drop\n"));

        // Hiç kural yokken de tablo yeniden kurulur: eski kurallar temizlenir.
        let empty = ruleset(&[]);
        assert!(empty.contains("chain forward {") && !empty.contains("jump"));
    }

    #[test]
    fn nic_guard_filters_what_enters_from_the_guests_own_interfaces() {
        let mut g = guest(100, "vmbr0", "");
        g.hw.firewall = false;
        g.hw.nic_guard = true;
        g.status = redox_core::GuestStatus::Running;
        g.hw.nics = vec!["vmbr1".to_string()];
        let taps = vec![(100, vec!["tap3".to_string(), "rx100n1".to_string(), "kötü ad; drop".to_string()])];
        let script = ruleset_with(std::slice::from_ref(&g), &taps);
        assert!(script.contains("type filter hook prerouting priority -300; policy accept;\n        iifname { \"tap3\", \"rx100n1\" } jump s100\n"), "{script}");
        let chain = script.split("chain s100 {").nth(1).unwrap();
        assert_eq!(
            chain.lines().map(str::trim).filter(|l| !l.is_empty() && *l != "}").collect::<Vec<_>>(),
            [
                "ether saddr != { 52:54:00:00:00:64, 52:54:01:00:00:64 } drop",
                "udp sport 67 udp dport 68 drop",
                "udp sport 547 udp dport 546 drop",
                "icmpv6 type { nd-router-advert, nd-redirect } drop",
            ]
        );
        assert!(!script.contains("kötü"), "geçersiz arayüz adı betiğe girmez");

        // Arayüzü bilinmeyen, kapalı ya da koruması kapalı konuk için kural kurulmaz; betik eskisiyle aynıdır.
        assert_eq!(ruleset_with(std::slice::from_ref(&g), &[]), ruleset(std::slice::from_ref(&g)));
        assert!(!ruleset(std::slice::from_ref(&g)).contains("prerouting"));
        g.status = redox_core::GuestStatus::Stopped;
        assert!(!ruleset_with(std::slice::from_ref(&g), &taps).contains("prerouting"));
        g.status = redox_core::GuestStatus::Running;
        g.hw.nic_guard = false;
        assert!(!ruleset_with(&[g], &taps).contains("prerouting"));
    }

    #[test]
    fn forged_rules_never_reach_nft() {
        let mut g = guest(104, "vmbr0", "tcp/22");
        g.hw.fw_rules[0].source = Some("1.2.3.4 accept; flush ruleset".into());
        assert!(!ruleset(&[g]).contains("flush ruleset"), "bozuk kurallı konuk betiğe girmez");
    }

    #[test]
    fn missing_nft_is_a_clear_error() {
        let err = apply("redox-olmayan-nft", "table bridge redox\n").unwrap_err();
        assert!(err.contains("nftables kurulu mu"), "{err}");
    }
}
