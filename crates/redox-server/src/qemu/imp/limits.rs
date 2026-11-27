//! İşlemci payı ve bellek garantisi: konuğun QEMU süreci kendi denetim grubuna (cgroup v2) alınır.
//!
//! Pay (`cpu.weight`), sunucunun işlemcisi yetmediğinde konukların ona hangi oranda ortak olacağını belirler:
//! 200 paylı konuk, 100 paylı konuğun iki katı işlemci alır; işlemci boşken kimse kısılmaz. Garanti
//! (`memory.min`), sunucunun belleği daraldığında çekirdeğin o konuktan geri almayacağı bellektir.
//!
//! Bunun için hizmetin kendi denetim grubunda alt grup açabilmesi gerekir (systemd biriminde `Delegate=`).
//! Yetki yoksa ve konukta pay ya da garanti istenmişse konuk başlatılmaz: ayar sessizce yok sayılmaz.

use super::*;

pub(super) const NO_CGROUP: &str = "işlemci payı ve bellek garantisi için hizmetin denetim grubu yetkisi gerekir: systemd biriminde \"Delegate=cpu memory\" ve \"ProtectControlGroups=false\" olmalı (deploy/redox-qemu.service). Yetki verilemiyorsa konuğun payını ve garantisini sıfırlayın";

fn group_name(vmid: VmId) -> String {
    format!("vm-{vmid}")
}

/// Hizmetin kendi denetim grubu (`/proc/self/cgroup`, tek hiyerarşi).
fn own_group() -> Option<std::path::PathBuf> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = text.lines().find_map(|line| line.strip_prefix("0::"))?.trim();
    Some(Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')))
}

/// Dizinde alt grup açabiliyor muyuz?
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!("redox-yoklama-{}", std::process::id()));
    let made = std::fs::create_dir(&probe).is_ok();
    let _ = std::fs::remove_dir(&probe);
    made
}

/// Konuk gruplarının açılacağı dizin. Sırasıyla:
/// 1. Ayarla verilen dizin (testler).
/// 2. Hizmet bir alt grupta çalışıyorsa (`DelegateSubgroup=`) üst grup: içinde doğrudan süreç yoktur.
/// 3. Hizmetin kendi grubu: içindeki süreçler önce bir yaprak gruba alınır (alt grubu olan grupta süreç duramaz).
fn base(d: &QemuDriver) -> Result<std::path::PathBuf, String> {
    if let Some(base) = &d.cfg.cgroup_base {
        return Ok(base.clone());
    }
    let own = own_group().ok_or(NO_CGROUP)?;
    if let Some(parent) = own.parent().filter(|p| *p != Path::new("/sys/fs/cgroup")) {
        let empty = std::fs::read_to_string(parent.join("cgroup.procs")).is_ok_and(|procs| procs.trim().is_empty());
        if empty && writable(parent) {
            return Ok(parent.to_path_buf());
        }
    }
    if !writable(&own) {
        return Err(NO_CGROUP.to_string());
    }
    let main = own.join("redox-main");
    std::fs::create_dir_all(&main).map_err(|e| format!("{} oluşturulamadı: {e}", main.display()))?;
    for pid in std::fs::read_to_string(own.join("cgroup.procs")).unwrap_or_default().lines() {
        let _ = std::fs::write(main.join("cgroup.procs"), pid);
    }
    Ok(own)
}

/// Konuğun payını ve garantisini çalışan QEMU sürecine uygular. Konuk çalışmıyorsa yapılacak iş yoktur:
/// ayarlar başlatılırken uygulanır. İkisi de sıfırsa ve konuğun grubu hiç açılmamışsa dokunulmaz.
pub fn apply_limits(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    let Some(pid) = live_pid(d, g.vmid) else { return Ok(()) };
    apply_affinity(d, g, pid)?;
    let wanted = g.hw.cpu_shares > 0 || g.hw.mem_reserve_mb > 0;
    let base = match base(d) {
        Ok(base) => base,
        Err(_) if !wanted => return Ok(()),
        Err(e) => return Err(e),
    };
    let dir = base.join(group_name(g.vmid));
    if !wanted && !dir.is_dir() {
        return Ok(());
    }
    // Alt gruplarda işlemci ve bellek denetimi açılır (zaten açıksa yazma etkisizdir; olmayan denetleyici atlanır).
    for controller in ["+cpu", "+memory"] {
        let _ = std::fs::write(base.join("cgroup.subtree_control"), controller);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("konuğun denetim grubu açılamadı ({}): {e}; systemd biriminde \"Delegate=cpu memory\" ve \"ProtectControlGroups=false\" olmalı", dir.display()))?;
    std::fs::write(dir.join("cgroup.procs"), pid.to_string()).map_err(|e| format!("konuk denetim grubuna alınamadı: {e}"))?;
    std::fs::write(dir.join("cpu.weight"), redox_core::hw::cpu_weight(g.hw.cpu_shares).to_string()).map_err(|e| format!("işlemci payı uygulanamadı: {e}"))?;
    std::fs::write(dir.join("memory.min"), (g.hw.mem_reserve_mb << 20).to_string()).map_err(|e| format!("bellek garantisi uygulanamadı: {e}"))
}

/// Sürecin çalışabileceği çekirdekler (`/proc/<pid>/status`).
fn allowed_cpus(pid: &str) -> Option<String> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|l| l.strip_prefix("Cpus_allowed_list:")).map(|v| v.trim().to_string())
}

/// QEMU'nun bütün iş parçacıklarını istenen çekirdeklere sabitler; sonradan açılan iş parçacıkları (eklenen
/// sanal çekirdekler) bunu devralır. Sabitleme kaldırılmışsa süreç, hizmetin kendi çekirdek kümesine döner.
fn apply_affinity(d: &QemuDriver, g: &Guest, pid: u32) -> Result<(), String> {
    let host = crate::host::performance();
    let list = match crate::qemu::perf::wanted_cpus(g, &host) {
        Some(cpus) => redox_core::hw::format_cpu_list(&cpus),
        None => match (allowed_cpus("self"), allowed_cpus(&pid.to_string())) {
            (Some(own), Some(current)) if own != current => own,
            _ => return Ok(()),
        },
    };
    tool(&d.cfg.taskset_bin, &["-a", "-p", "-c", &list, &pid.to_string()]).map_err(|e| format!("konuk {list} çekirdeklerine sabitlenemedi: {e}"))
}

/// Kapanan konuğun grubunu kaldırır (içi boşalmış olmalıdır; değilse çekirdek reddeder ve grup sonraki
/// başlatmada yeniden kullanılır).
pub(super) fn drop_limits(d: &QemuDriver, vmid: VmId) {
    let own = own_group();
    let candidates = [d.cfg.cgroup_base.clone(), own.as_ref().and_then(|o| o.parent().map(Path::to_path_buf)), own.clone()];
    for base in candidates.into_iter().flatten() {
        let _ = std::fs::remove_dir(base.join(group_name(vmid)));
    }
}
