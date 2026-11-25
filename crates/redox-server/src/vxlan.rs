//! VXLAN: sunucular arasında ortak bir konuk ağı için kurulum komutlarını üretir.
//!
//! RedoX VE ağ aygıtı oluşturmaz (yetkisiz çalışır); konuklar yöneticinin kurduğu köprülere bağlanır. Bu modül,
//! birkaç sunucudaki aynı adlı köprüyü VXLAN tüneliyle birleştiren `ip` komutlarını yazar; yönetici bunları her
//! sunucuda root olarak çalıştırır. Sonrasında köprü, konuklarda olağan bir köprü gibi seçilir.

use std::net::IpAddr;

pub const PORT: u16 = 4789;

/// Kurulum komutları: (VNI, eş sayısı, komutlar).
fn commands(bridge: &str, vni: &str, local: &str, peers: &str) -> Result<(u32, usize, Vec<String>), String> {
    if !redox_core::valid_bridge(bridge) || bridge == redox_core::NAT || bridge.len() > 15 {
        return Err(format!("geçersiz köprü adı \"{bridge}\""));
    }
    let vni: u32 = vni.parse().ok().filter(|v| (1..=16_777_215).contains(v)).ok_or("VNI 1 ile 16777215 arasında bir sayı olmalı")?;
    let address = |text: &str| text.trim().parse::<IpAddr>().map_err(|_| format!("geçersiz adres \"{}\"", text.trim()));
    let local = address(local)?;
    let peers = peers.split(',').filter(|p| !p.trim().is_empty()).map(address).collect::<Result<Vec<_>, _>>()?;
    if peers.is_empty() {
        return Err("en az bir eş sunucu adresi gerekir".to_string());
    }
    if peers.iter().any(|p| *p == local || p.is_ipv4() != local.is_ipv4()) {
        return Err("eş adresleri yerel adresten farklı ve aynı türde (IPv4 ya da IPv6) olmalı".to_string());
    }
    let device = format!("vx{vni}");
    let mut out = vec![format!("ip link add {device} type vxlan id {vni} local {local} dstport {PORT}")];
    out.extend(peers.iter().map(|peer| format!("bridge fdb append 00:00:00:00:00:00 dev {device} dst {peer}")));
    out.push(format!("ip link show {bridge} >/dev/null 2>&1 || ip link add {bridge} type bridge"));
    out.push(format!("ip link set {device} master {bridge}"));
    out.push(format!("ip link set {device} up"));
    out.push(format!("ip link set {bridge} up"));
    Ok((vni, peers.len(), out))
}

const MTU_NOTE: &str = "# Tünel 50 bayt yer kaplar: konuklarda MTU 1450 kullanın ya da sunucular arası ağda MTU'yu 1550 yapın.";

/// Elle çalıştırılacak komutlar. `peers`: virgülle ayrılmış eş sunucu adresleri.
pub fn plan(bridge: &str, vni: &str, local: &str, peers: &str) -> Result<String, String> {
    let (vni, count, commands) = commands(bridge, vni, local, peers)?;
    let mut out = vec![format!("# VXLAN {vni}: {bridge} köprüsünü {count} eş sunucuyla birleştirir. Bu sunucuda root olarak çalıştırın."), MTU_NOTE.to_string()];
    out.extend(commands);
    Ok(out.join("\n") + "\n")
}

/// Aynı kurulumu her açılışta yapan systemd birimi (`/etc/systemd/system/redox-vxlan-<vni>.service`).
pub fn unit(bridge: &str, vni: &str, local: &str, peers: &str) -> Result<String, String> {
    let (vni, _, commands) = commands(bridge, vni, local, peers)?;
    let mut out = vec![
        format!("# /etc/systemd/system/redox-vxlan-{vni}.service dosyasına kaydedin; sonra: systemctl daemon-reload && systemctl enable --now redox-vxlan-{vni}"),
        MTU_NOTE.to_string(),
        "[Unit]".to_string(),
        format!("Description=RedoX VE VXLAN {vni} ({bridge})"),
        "After=network-online.target".to_string(),
        "Wants=network-online.target".to_string(),
        String::new(),
        "[Service]".to_string(),
        "Type=oneshot".to_string(),
        "RemainAfterExit=yes".to_string(),
    ];
    // Komutlarda tırnak ve yüzde imi yoktur (girdiler doğrulanmıştır); kabuk yalnızca yol araması ve `||` içindir.
    out.extend(commands.iter().map(|command| format!("ExecStart=/bin/sh -c \"{command}\"")));
    out.push(format!("ExecStop=/bin/sh -c \"ip link del vx{vni}\""));
    out.extend([String::new(), "[Install]".to_string(), "WantedBy=multi-user.target".to_string()]);
    Ok(out.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_lists_one_flood_entry_per_peer_and_validates_input() {
        let script = plan("vmbr100", "100", "10.0.0.1", "10.0.0.2, 10.0.0.3").unwrap();
        let commands: Vec<&str> = script.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(
            commands,
            [
                "ip link add vx100 type vxlan id 100 local 10.0.0.1 dstport 4789",
                "bridge fdb append 00:00:00:00:00:00 dev vx100 dst 10.0.0.2",
                "bridge fdb append 00:00:00:00:00:00 dev vx100 dst 10.0.0.3",
                "ip link show vmbr100 >/dev/null 2>&1 || ip link add vmbr100 type bridge",
                "ip link set vx100 master vmbr100",
                "ip link set vx100 up",
                "ip link set vmbr100 up",
            ]
        );
        assert!(plan("vmbr1", "5", "fd00::1", "fd00::2").unwrap().contains("local fd00::1"));
        let unit = unit("vmbr100", "100", "10.0.0.1", "10.0.0.2").unwrap();
        for needle in [
            "redox-vxlan-100.service",
            "[Unit]\nDescription=RedoX VE VXLAN 100 (vmbr100)",
            "Type=oneshot\nRemainAfterExit=yes",
            "ExecStart=/bin/sh -c \"ip link add vx100 type vxlan id 100 local 10.0.0.1 dstport 4789\"",
            "ExecStart=/bin/sh -c \"ip link show vmbr100 >/dev/null 2>&1 || ip link add vmbr100 type bridge\"",
            "ExecStop=/bin/sh -c \"ip link del vx100\"",
            "[Install]\nWantedBy=multi-user.target",
        ] {
            assert!(unit.contains(needle), "{needle}\n{unit}");
        }
        assert_eq!(unit.matches("ExecStart=").count(), 6);
        assert!(super::unit("nat", "1", "10.0.0.1", "10.0.0.2").is_err());
        for (bridge, vni, local, peers, needle) in [
            ("vmbr1; rm -rf /", "5", "10.0.0.1", "10.0.0.2", "köprü adı"),
            ("nat", "5", "10.0.0.1", "10.0.0.2", "köprü adı"),
            ("vmbr1", "0", "10.0.0.1", "10.0.0.2", "VNI"),
            ("vmbr1", "16777216", "10.0.0.1", "10.0.0.2", "VNI"),
            ("vmbr1", "5", "10.0.0.1; reboot", "10.0.0.2", "geçersiz adres"),
            ("vmbr1", "5", "10.0.0.1", "$(reboot)", "geçersiz adres"),
            ("vmbr1", "5", "10.0.0.1", "", "en az bir"),
            ("vmbr1", "5", "10.0.0.1", "10.0.0.1", "farklı"),
            ("vmbr1", "5", "10.0.0.1", "fd00::2", "aynı türde"),
        ] {
            assert!(plan(bridge, vni, local, peers).unwrap_err().contains(needle), "{bridge} {vni} {local} {peers}");
        }
    }
}
