//! Çalışan konuğa disk, ağ kartı ve çekirdek ekleme.

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
    let added = hmp_text(d, g, &format!("drive_add 0 file={},if=none,id={drive},format=qcow2,discard=unmap{}", crate::qemu::opt(&d.cfg.extra_disk_of(g, id)), crate::qemu::disk_throttle(g)))?;
    if added != "OK" {
        return Err(format!("disk konuğa tanıtılamadı: {}", added.lines().last().unwrap_or("bilinmeyen hata")));
    }
    let device = serde_json::json!({ "execute": "device_add", "arguments": { "driver": "virtio-blk-pci", "drive": drive, "id": format!("xdev{id}"), "bus": crate::qemu::disk_port(id) } });
    qmp_values(&sock, &[device]).map(|_| ()).map_err(|e| {
        let _ = hmp_text(d, g, &format!("drive_del {drive}"));
        format!("disk konuğa takılamadı: {e}")
    })
}

/// Çalışan konuğun CD sürücüsündeki kalıbı değiştirir. Sürücü, konuk açılırken `cd0` / `cd1` adıyla kurulur;
/// o açılışta kalıbı olmayan konukta bu adda sürücü yoktur.
pub fn change_media(d: &QemuDriver, g: &Guest, drive: u8, iso: Option<&str>) -> Result<(), String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let id = format!("cd{}", drive.saturating_sub(1));
    let blocks = qmp(&sock, &["query-block"])?;
    if !blocks.first().and_then(Value::as_array).is_some_and(|list| list.iter().any(|b| b["device"] == id.as_str())) {
        return Err(format!("konuk bu açılışta {drive}. CD sürücüsü olmadan başlamış; kapatın, kalıbı ayarlardan takıp yeniden açın"));
    }
    // Konuk tepsiyi kilitlemiş olabilir; değiştirme bilerek zorlanır.
    let command = match iso {
        Some(name) if redox_core::valid_iso_name(name) => {
            let path = d.cfg.iso_dir().join(name).display().to_string();
            serde_json::json!({ "execute": "blockdev-change-medium", "arguments": { "device": id, "filename": path, "format": "raw", "read-only-mode": "read-only", "force": true } })
        }
        Some(name) => return Err(format!("geçersiz ISO adı: {name}")),
        None => serde_json::json!({ "execute": "eject", "arguments": { "device": id, "force": true } }),
    };
    qmp_values(&sock, &[command]).map(|_| ()).map_err(|e| format!("kalıp değiştirilemedi: {e}"))
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
        "driver": g.hw.nic_model.device(), "netdev": netdev, "id": format!("xnic{index}"), "mac": redox_core::mac_for_nic(g.vmid, index), "bus": crate::qemu::nic_port(index) } });
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

pub(super) const NO_SLOTS: &str = "bu konuk boş çekirdek yuvası olmadan açılmış; bir kez kapatıp açın, sonra yeniden deneyin";

/// Çalışan konuğa bir çekirdek ekler: boş yuvaların en küçük numaralısı doldurulur. Böylece sonradan eklenen
/// çekirdekler, konuk aynı sayıyla yeniden açıldığında (ya da askıdan döndüğünde) aynı yerde durur.
pub fn hot_add_core(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    let Some(pid) = live_pid(d, g.vmid) else {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    };
    if !std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains(",maxcpus=")) {
        return Err(NO_SLOTS.to_string());
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let reply = qmp(&sock, &["query-hotpluggable-cpus"])?;
    let slots = reply.first().and_then(Value::as_array).ok_or("konuğun çekirdek yuvaları QEMU'dan okunamadı")?;
    let place = |slot: &Value| ["socket-id", "die-id", "cluster-id", "module-id", "core-id", "thread-id"].map(|key| slot["props"][key].as_i64().unwrap_or(0));
    let free = slots.iter().filter(|slot| slot.get("qom-path").is_none()).min_by_key(|slot| place(slot)).ok_or("konukta boş çekirdek yuvası kalmadı")?;
    let present = slots.iter().filter(|slot| slot.get("qom-path").is_some()).count();
    let mut arguments = free["props"].clone();
    if !arguments.is_object() || !free["type"].is_string() {
        return Err("konuğun çekirdek yuvaları QEMU'dan okunamadı".to_string());
    }
    arguments["driver"] = free["type"].clone();
    arguments["id"] = format!("xcpu{present}").into();
    qmp_values(&sock, &[serde_json::json!({ "execute": "device_add", "arguments": arguments })]).map(|_| ()).map_err(|e| format!("çekirdek konuğa eklenemedi: {e}"))
}

/// Sonradan eklenen çekirdekleri konuk ajanıyla etkinleştirir. Konuk yeni çekirdeği hemen görmeyebilir: ajan
/// beklenen sayıyı bildirene kadar birkaç kez sorulur.
pub fn online_cores(d: &QemuDriver, g: &Guest) -> bool {
    let qga = d.cfg.qga_path(g.vmid);
    for attempt in 0..8 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        let Ok(reply) = crate::qga::call(&qga, "guest-get-vcpus", Duration::from_secs(3)) else {
            return false;
        };
        let Some(list) = reply.as_array() else { return false };
        if list.len() < g.cores as usize {
            continue;
        }
        let offline: Vec<Value> = list.iter().filter(|cpu| cpu["online"] == false).map(|cpu| serde_json::json!({ "logical-id": cpu["logical-id"], "online": true })).collect();
        return offline.is_empty() || crate::qga::call_with(&qga, "guest-set-vcpus", Some(serde_json::json!({ "vcpus": offline })), Duration::from_secs(5)).is_ok();
    }
    false
}

pub(super) const NO_MEM_SLOTS: &str = "bu konuk boş bellek yuvası olmadan açılmış; bir kez kapatıp açın, sonra yeniden deneyin";

/// Çalışan konuğa `mb` MB'lık bir bellek parçası takar. Parçanın sırası konuğun kaydındaki parça sayısıdır;
/// aynı sıra ve yuva, konuk askıdan dönerken komut satırında yeniden kurulur.
pub fn hot_add_memory(d: &QemuDriver, g: &Guest, mb: u64) -> Result<(), String> {
    let Some(pid) = live_pid(d, g.vmid) else {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    };
    if !std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains(",maxmem=")) {
        return Err(NO_MEM_SLOTS.to_string());
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let index = g.hw.dimms.len();
    let (backend, dimm) = (crate::qemu::dimm_backend(index), crate::qemu::dimm_device(index));
    let object = serde_json::json!({ "execute": "object-add", "arguments": { "qom-type": "memory-backend-ram", "id": backend, "size": mb << 20 } });
    qmp_values(&sock, &[object]).map_err(|e| format!("bellek konuğa tanıtılamadı: {e}"))?;
    let device = serde_json::json!({ "execute": "device_add", "arguments": { "driver": "pc-dimm", "id": dimm, "memdev": backend, "slot": index } });
    qmp_values(&sock, &[device]).map(|_| ()).map_err(|e| {
        let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "object-del", "arguments": { "id": backend } })]);
        format!("bellek konuğa eklenemedi: {e}")
    })
}

/// Sonradan eklenen belleği konuk ajanıyla etkinleştirir ve konuğun belleğin tamamını kullandığını doğrular.
/// Konuk yeni belleği hemen görmeyebilir: birkaç kez sorulur.
pub fn online_memory(d: &QemuDriver, g: &Guest) -> bool {
    let qga = d.cfg.qga_path(g.vmid);
    let Ok(info) = crate::qga::call(&qga, "guest-get-memory-block-info", Duration::from_secs(3)) else {
        return false;
    };
    let block_mb = info["size"].as_u64().unwrap_or(0) >> 20;
    for attempt in 0..8 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        let Ok(reply) = crate::qga::call(&qga, "guest-get-memory-blocks", Duration::from_secs(3)) else {
            return false;
        };
        let Some(list) = reply.as_array() else { return false };
        let offline: Vec<Value> = list.iter().filter(|block| block["online"] == false).map(|block| serde_json::json!({ "phys-index": block["phys-index"], "online": true })).collect();
        if !offline.is_empty() {
            if crate::qga::call_with(&qga, "guest-set-memory-blocks", Some(serde_json::json!({ "mem-blks": offline })), Duration::from_secs(10)).is_err() {
                return false;
            }
            continue;
        }
        // Blok boyu bilinmiyorsa, kapalı blok kalmaması yeterli sayılır.
        if block_mb == 0 || list.len() as u64 * block_mb >= g.mem_total_mb {
            return true;
        }
    }
    false
}
