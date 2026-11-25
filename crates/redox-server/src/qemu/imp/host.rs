//! Sunucu tarafı hazırlık: sanal TPM süreci, VLAN arayüzleri, ağ hız sınırı.

use super::*;

/// Konuğun sanal TPM öykünücüsünü başlatır ve soketinin açılmasını bekler.
pub(super) fn start_tpm(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    if !g.hw.tpm {
        return Ok(());
    }
    let (dir, sock, pid) = (d.cfg.tpm_dir(g.vmid), d.cfg.tpm_sock(g.vmid), d.cfg.tpm_pid(g.vmid));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    // --terminate: QEMU bağlantıyı kapatınca (konuk durunca) öykünücü de çıkar.
    let out = std::process::Command::new(&d.cfg.swtpm_bin)
        .args(["socket", "--tpm2", "--terminate", "--daemon"])
        .arg("--tpmstate")
        .arg(format!("dir={}", dir.display()))
        .arg("--ctrl")
        .arg(format!("type=unixio,path={}", sock.display()))
        .arg("--pid")
        .arg(format!("file={}", pid.display()))
        .output()
        .map_err(|e| format!("{} çalıştırılamadı: {e} (sudo apt install swtpm)", d.cfg.swtpm_bin))?;
    if !out.status.success() {
        return Err(format!("sanal TPM başlatılamadı: {}", last_line(&out.stderr)));
    }
    if !wait_until(Duration::from_secs(3), || sock.exists()) {
        stop_tpm(d, g.vmid);
        return Err("sanal TPM başladı ama soketi açılmadı".to_string());
    }
    Ok(())
}

/// Artakalan TPM öykünücüsünü durdurur. Kimlik dosyasındaki süreç gerçekten bu konuğun öykünücüsüyse öldürülür.
pub(super) fn stop_tpm(d: &QemuDriver, vmid: VmId) {
    let (sock, pid_file) = (d.cfg.tpm_sock(vmid), d.cfg.tpm_pid(vmid));
    if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        if String::from_utf8_lossy(&cmdline).contains(&sock.display().to_string()) {
            let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        }
    }
    let _ = std::fs::remove_file(pid_file);
    let _ = std::fs::remove_file(sock);
}

/// Bir VLAN kartının TAP arayüzünü kurar: arayüz bu kullanıcıya ait açılır, köprüye bağlanır ve yalnızca
/// kendi VLAN'ına (etiketsiz, varsayılan olarak) üye yapılır.
pub(super) fn vlan_tap(d: &QemuDriver, name: &str, bridge: &str, vlan: u16) -> Result<(), String> {
    let filtering = std::fs::read_to_string(d.cfg.sys_net.join(bridge).join("bridge/vlan_filtering")).map(|s| s.trim() == "1").unwrap_or(false);
    if !filtering {
        return Err(format!("{bridge} köprüsünde VLAN süzme kapalı (ya da köprü yok); açmak için: ip link set {bridge} type bridge vlan_filtering 1"));
    }
    let (ip, br) = (d.cfg.ip_bin.as_str(), d.cfg.bridge_bin.as_str());
    if d.cfg.sys_net.join(name).exists() {
        let _ = tool(ip, &["link", "del", name]);
    }
    let uid = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").map(|m| m.uid().to_string()).unwrap_or_else(|_| "0".to_string())
    };
    tool(ip, &["tuntap", "add", "dev", name, "mode", "tap", "user", &uid])?;
    tool(ip, &["link", "set", "dev", name, "master", bridge])?;
    // Köprünün varsayılan VLAN'ı (1) kaldırılır; yoksa hata önemsizdir.
    let _ = tool(br, &["vlan", "del", "dev", name, "vid", "1"]);
    tool(br, &["vlan", "add", "dev", name, "vid", &vlan.to_string(), "pvid", "untagged"])?;
    tool(ip, &["link", "set", "dev", name, "up"])
}

pub(super) const VLAN_HINT: &str = "etiketi kaldırın ya da hizmete CAP_NET_ADMIN yetkisi verin (iproute2 kurulu ve köprüde VLAN süzme açık olmalı)";

/// Konuğun VLAN etiketli bütün kartlarının arayüzlerini kurar.
pub(super) fn prepare_vlan_taps(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    for (index, bridge, vlan) in crate::qemu::vlan_nics(g) {
        vlan_tap(d, &crate::qemu::tap_name(g.vmid, index), &bridge, vlan).map_err(|e| format!("VLAN arayüzü hazırlanamadı: {e}; {VLAN_HINT}"))?;
    }
    Ok(())
}

/// Konuğun bizim kurduğumuz TAP arayüzlerini kaldırır (konuk kapanınca ya da açılamayınca).
pub(super) fn remove_vlan_taps(d: &QemuDriver, vmid: VmId) {
    for index in 0..=redox_core::hw::MAX_EXTRA_NICS as u8 {
        let name = crate::qemu::tap_name(vmid, index);
        if d.cfg.sys_net.join(&name).exists() {
            let _ = tool(&d.cfg.ip_bin, &["link", "del", &name]);
        }
    }
}

pub(in crate::qemu) fn taps(d: &QemuDriver, vmid: VmId) -> Vec<String> {
    match live_pid(d, vmid) {
        Some(pid) => crate::netlimit::taps_of(Path::new(&format!("/proc/{pid}/fdinfo"))),
        None => Vec::new(),
    }
}

pub(super) fn limit_network(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    if g.hw.net_mbps == 0 {
        return Ok(());
    }
    let hint = "sınırı 0 yapın ya da hizmete CAP_NET_ADMIN yetkisi verin (iproute2 kurulu olmalı)";
    let taps = live_pid(d, g.vmid).map(|pid| crate::netlimit::taps_of(Path::new(&format!("/proc/{pid}/fdinfo")))).unwrap_or_default();
    if taps.is_empty() {
        return Err(format!("ağ hız sınırı uygulanamadı: konuğun köprü arayüzü (TAP) bulunamadı; {hint}"));
    }
    crate::netlimit::apply(&d.cfg.tc_bin, &taps, g.hw.net_mbps).map_err(|e| format!("ağ hız sınırı uygulanamadı: {e}; {hint}"))
}
