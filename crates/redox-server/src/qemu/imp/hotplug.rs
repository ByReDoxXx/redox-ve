//! Çalışan konuğa disk ve ağ kartı takma.

use super::*;

/// Konuk, çalışırken takma bağlantı noktalarıyla mı açılmış?
pub(super) fn has_ports(d: &QemuDriver, vmid: VmId) -> bool {
    live_pid(d, vmid).and_then(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok()).is_some_and(|c| String::from_utf8_lossy(&c).contains("pcie-root-port,id=hp0"))
}

/// Çalışan konuğun açıldığı donanım yerleşimi (komut satırından okunur).
pub(super) fn running_layout(d: &QemuDriver, vmid: VmId) -> u8 {
    let cmdline = live_pid(d, vmid).and_then(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok()).map(|c| String::from_utf8_lossy(&c).into_owned()).unwrap_or_default();
    if !cmdline.contains("pcie-root-port,id=hp0") {
        crate::qemu::LAYOUT_LEGACY
    } else if cmdline.contains("usb-tablet") {
        crate::qemu::LAYOUT_CURRENT
    } else {
        crate::qemu::LAYOUT_PORTS
    }
}

pub(super) const NO_PORTS: &str = "bu konuk çalışırken takmayı desteklemeyen eski yerleşimle açılmış; bir kez kapatıp açın, sonra yeniden deneyin";

pub fn hot_add_disk(d: &QemuDriver, g: &Guest, id: u8) -> Result<(), String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    if !has_ports(d, g.vmid) {
        return Err(NO_PORTS.to_string());
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let drive = format!("xd{id}");
    let added = hmp_text(d, g, &format!("drive_add 0 file={},if=none,id={drive},format=qcow2{}", crate::qemu::opt(&d.cfg.extra_disk_of(g, id)), crate::qemu::disk_throttle(g)))?;
    if added != "OK" {
        return Err(format!("disk konuğa tanıtılamadı: {}", added.lines().last().unwrap_or("bilinmeyen hata")));
    }
    let device = serde_json::json!({ "execute": "device_add", "arguments": { "driver": "virtio-blk-pci", "drive": drive, "id": format!("xdev{id}"), "bus": crate::qemu::disk_port(id) } });
    qmp_values(&sock, &[device]).map(|_| ()).map_err(|e| {
        let _ = hmp_text(d, g, &format!("drive_del {drive}"));
        format!("disk konuğa takılamadı: {e}")
    })
}

pub fn hot_add_nic(d: &QemuDriver, g: &Guest, index: u8, network: &str) -> Result<(), String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    if !has_ports(d, g.vmid) {
        return Err(NO_PORTS.to_string());
    }
    let Some((network, vlan)) = redox_core::hw::parse_nic(network) else {
        return Err(format!("geçersiz ağ ayarı: {network}"));
    };
    let sock = d.cfg.qmp_path(g.vmid);
    let fdinfo = live_pid(d, g.vmid).map(|pid| format!("/proc/{pid}/fdinfo")).unwrap_or_default();
    let before = crate::netlimit::taps_of(Path::new(&fdinfo));
    let netdev = format!("net{index}");
    let tap = crate::qemu::tap_name(g.vmid, index);
    let backend = if network == crate::qemu::NAT {
        serde_json::json!({ "execute": "netdev_add", "arguments": { "type": "user", "id": netdev } })
    } else if vlan > 0 {
        vlan_tap(d, &tap, network, vlan).map_err(|e| format!("VLAN arayüzü hazırlanamadı: {e}; {VLAN_HINT}"))?;
        serde_json::json!({ "execute": "netdev_add", "arguments": { "type": "tap", "id": netdev, "ifname": tap, "script": "no", "downscript": "no" } })
    } else {
        serde_json::json!({ "execute": "netdev_add", "arguments": { "type": "bridge", "id": netdev, "br": network } })
    };
    let drop_tap = || {
        if vlan > 0 {
            let _ = tool(&d.cfg.ip_bin, &["link", "del", &tap]);
        }
    };
    qmp_values(&sock, &[backend]).map_err(|e| {
        drop_tap();
        format!("ağ kartı konuğa tanıtılamadı: {e}")
    })?;
    let remove = || {
        let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "netdev_del", "arguments": { "id": netdev } })]);
        drop_tap();
    };
    let device = serde_json::json!({ "execute": "device_add", "arguments": {
        "driver": "virtio-net-pci", "netdev": netdev, "id": format!("xnic{index}"), "mac": redox_core::mac_for_nic(g.vmid, index), "bus": crate::qemu::nic_port(index) } });
    if let Err(e) = qmp_values(&sock, &[device]) {
        remove();
        return Err(format!("ağ kartı konuğa takılamadı: {e}"));
    }
    // Ağ hız sınırı yeni kartın arayüzüne de uygulanır; uygulanamazsa kart sınırsız bırakılmaz.
    if g.hw.net_mbps > 0 && network != crate::qemu::NAT {
        let fresh: Vec<String> = crate::netlimit::taps_of(Path::new(&fdinfo)).into_iter().filter(|t| !before.contains(t)).collect();
        let limited = if fresh.is_empty() { Err("yeni kartın köprü arayüzü (TAP) bulunamadı".to_string()) } else { crate::netlimit::apply(&d.cfg.tc_bin, &fresh, g.hw.net_mbps) };
        if let Err(e) = limited {
            let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "device_del", "arguments": { "id": format!("xnic{index}") } })]);
            remove();
            return Err(format!("ağ hız sınırı uygulanamadı: {e}; kart takılmadı"));
        }
    }
    Ok(())
}
