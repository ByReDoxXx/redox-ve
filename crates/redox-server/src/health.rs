//! Disk sağlığı (smartctl), donanım algılayıcıları (ipmitool) ve bekleyen paket güncellemeleri.
//!
//! `redox host health` ve `redox host update-apply` **kök olarak**, ayrı kısa ömürlü systemd birimlerinden
//! çalışır. Sonuçlar köke ait bir dizine yazılır; yetkisiz yönetim sunucusu yalnızca okur.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use redox_core::health::{DiskHealth, HostHealth, PackageUpdate, Sensor, UpdateRun};
use serde_json::Value;

/// Köke ait sonuç dizini.
pub const HEALTH_DIR: &str = "/var/lib/redox-health";
const MAX_FILE: u64 = 512 * 1024;

/// `smartctl --json -a <aygıt>` çıktısı.
pub fn parse_smart(device: &str, json: &str) -> Option<DiskHealth> {
    let v: Value = serde_json::from_str(json).ok()?;
    let attribute = |id: u64| v["ata_smart_attributes"]["table"].as_array()?.iter().find(|a| a["id"].as_u64() == Some(id))?["raw"]["value"].as_u64();
    let nvme = &v["nvme_smart_health_information_log"];
    let disk = DiskHealth {
        device: device.to_string(),
        model: v["model_name"].as_str().unwrap_or("").chars().take(64).collect(),
        passed: v["smart_status"]["passed"].as_bool(),
        temperature_c: v["temperature"]["current"].as_i64(),
        power_on_hours: v["power_on_time"]["hours"].as_u64(),
        // 5: yeniden eşlenen sektörler.
        bad_sectors: attribute(5).or_else(|| nvme["media_errors"].as_u64()),
        wear_pct: nvme["percentage_used"].as_u64(),
    };
    // Hiçbir bilgi vermeyen aygıt (sanal disk, USB köprüsü) listelenmez.
    (disk.passed.is_some() || disk.temperature_c.is_some() || !disk.model.is_empty()).then_some(disk)
}

/// `smartctl --json --scan` çıktısındaki aygıtlar.
pub fn parse_scan(json: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    v["devices"].as_array().into_iter().flatten().filter_map(|d| d["name"].as_str()).filter(|n| n.starts_with("/dev/") && n.len() < 64 && !n.contains("..")).take(64).map(str::to_string).collect()
}

/// `ipmitool sensor` çıktısı: `ad | değer | birim | durum | eşikler…`. Değeri olmayan satırlar atlanır.
pub fn parse_sensors(text: &str) -> Vec<Sensor> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('|').map(str::trim).collect();
            let (name, value, unit, status) = (f.first()?, f.get(1)?, f.get(2)?, f.get(3)?);
            if name.is_empty() || *value == "na" || *unit == "discrete" {
                return None;
            }
            Some(Sensor { name: name.chars().take(48).collect(), reading: format!("{value} {unit}"), status: status.chars().take(8).collect() })
        })
        .take(128)
        .collect()
}

/// `apt list --upgradable`: `paket/depo yeni mimari [upgradable from: eski]`.
pub fn parse_upgradable(text: &str) -> Vec<PackageUpdate> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once('/')?;
            let to = rest.split_whitespace().nth(1)?;
            let from = line.split("from: ").nth(1)?.trim_end_matches(']');
            Some(PackageUpdate { name: name.to_string(), from: from.to_string(), to: to.to_string() })
        })
        .take(500)
        .collect()
}

/// Çalıştırılacak araçlar (testler sahtelerini verir).
pub struct Tools {
    pub smartctl: String,
    pub ipmitool: String,
    pub apt: String,
    pub apt_get: String,
}

impl Default for Tools {
    fn default() -> Self {
        Tools { smartctl: "smartctl".into(), ipmitool: "ipmitool".into(), apt: "apt".into(), apt_get: "apt-get".into() }
    }
}

fn output(bin: &str, args: &[&str]) -> Result<(bool, String, String), String> {
    let out = Command::new(bin).args(args).env("LC_ALL", "C").env("DEBIAN_FRONTEND", "noninteractive").output().map_err(|e| format!("{bin} çalıştırılamadı: {e}"))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// Bütün kaynakları okur. Bir kaynağın okunamaması ötekileri engellemez; nedeni `notes` içinde yazılır.
/// `refresh`: önce paket listeleri yenilensin mi (kök ister).
pub fn collect(tools: &Tools, refresh: bool, now: u64) -> HostHealth {
    let mut health = HostHealth { collected: now, ..Default::default() };
    match output(&tools.smartctl, &["--json", "--scan"]) {
        Err(e) => health.notes.push(e),
        // smartctl, disk sorunlarında sıfırdan farklı döner; çıktı yine okunur.
        Ok((_, scan, _)) => {
            for device in parse_scan(&scan) {
                if let Ok((_, json, _)) = output(&tools.smartctl, &["--json", "-a", &device]) {
                    health.disks.extend(parse_smart(&device, &json));
                }
            }
        }
    }
    match output(&tools.ipmitool, &["sensor"]) {
        Ok((true, text, _)) => health.sensors = parse_sensors(&text),
        Ok((false, _, error)) => health.notes.push(format!("IPMI okunamadı: {}", error.lines().next().unwrap_or("bilinmeyen hata"))),
        Err(e) => health.notes.push(e),
    }
    if refresh {
        match output(&tools.apt_get, &["update"]) {
            Ok((true, ..)) => {}
            Ok((false, _, error)) => health.notes.push(format!("paket listeleri yenilenemedi: {}", error.lines().last().unwrap_or("bilinmeyen hata"))),
            Err(e) => health.notes.push(e),
        }
    }
    match output(&tools.apt, &["list", "--upgradable"]) {
        Ok((_, text, _)) => health.updates = parse_upgradable(&text),
        Err(e) => health.notes.push(e),
    }
    health
}

/// Bekleyen güncellemeleri kurar. Yeni paket kaldırmaz (`upgrade`); yapılandırma sorularında eski dosya korunur.
pub fn apply_updates(tools: &Tools, now: u64) -> UpdateRun {
    let result = output(&tools.apt_get, &["-y", "-o", "Dpkg::Options::=--force-confold", "upgrade"]);
    let (ok, text) = match result {
        Ok((ok, out, error)) => (ok, format!("{out}\n{error}")),
        Err(e) => (false, e),
    };
    let mut tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(20).collect();
    tail.reverse();
    UpdateRun { finished: now, ok, output: tail.join("\n") }
}

/// Dosyayı yarım okunmayacak biçimde yazar (geçici dosya + yeniden adlandırma).
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| format!("{} yazılamadı: {e}", tmp.display()))?;
    file.write_all(&text).map_err(|e| format!("{} yazılamadı: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("{} yazılamadı: {e}", path.display()))
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Örnekler smartctl 7 ve ipmitool çıktı biçimlerine göre elle yazılmıştır.
    #[test]
    fn smart_reports_of_ata_and_nvme_disks_are_read() {
        let ata = r#"{"model_name":"WDC WD40","smart_status":{"passed":false},"temperature":{"current":41},"power_on_time":{"hours":12000},"ata_smart_attributes":{"table":[{"id":5,"raw":{"value":24}},{"id":9,"raw":{"value":12000}}]}}"#;
        let d = parse_smart("/dev/sda", ata).unwrap();
        assert_eq!((d.passed, d.temperature_c, d.power_on_hours, d.bad_sectors, d.wear_pct), (Some(false), Some(41), Some(12000), Some(24), None));
        let nvme = r#"{"model_name":"Samsung 980","smart_status":{"passed":true},"temperature":{"current":36},"nvme_smart_health_information_log":{"percentage_used":3,"media_errors":0}}"#;
        let d = parse_smart("/dev/nvme0", nvme).unwrap();
        assert_eq!((d.passed, d.bad_sectors, d.wear_pct), (Some(true), Some(0), Some(3)));
        assert!(parse_smart("/dev/vda", "{}").is_none() && parse_smart("/dev/vda", "bozuk").is_none());
        let scan = r#"{"devices":[{"name":"/dev/sda"},{"name":"/dev/../etc/x"},{"name":"sda"},{"name":"/dev/nvme0"}]}"#;
        assert_eq!(parse_scan(scan), ["/dev/sda", "/dev/nvme0"]);
    }

    #[test]
    fn sensor_and_package_lists_are_read() {
        let sensors = parse_sensors("CPU Temp   | 42.000   | degrees C  | ok    | na\nFan2       | na       |            | na    |\nChassis    | 0x0      | discrete   | 0x0000|\nFan1       | 0.000    | RPM        | cr    | 300\n");
        assert_eq!(sensors.iter().map(|s| (s.name.as_str(), s.reading.as_str(), s.status.as_str())).collect::<Vec<_>>(), [("CPU Temp", "42.000 degrees C", "ok"), ("Fan1", "0.000 RPM", "cr")]);
        let updates = parse_upgradable("Listing...\nopenssl/stable-security 3.5.4-1 amd64 [upgradable from: 3.5.1-1]\nN: bir not\n");
        assert_eq!(updates, [PackageUpdate { name: "openssl".into(), from: "3.5.1-1".into(), to: "3.5.4-1".into() }]);
    }

    #[test]
    fn missing_tools_become_notes_not_failures() {
        let tools = Tools { smartctl: "/yok/smartctl".into(), ipmitool: "/yok/ipmitool".into(), apt: "/yok/apt".into(), apt_get: "/yok/apt-get".into() };
        let health = collect(&tools, true, 7);
        assert_eq!((health.collected, health.disks.len(), health.notes.len()), (7, 0, 4), "{:?}", health.notes);
        let run = apply_updates(&tools, 9);
        assert!(!run.ok && run.output.contains("çalıştırılamadı"));
    }

    #[test]
    fn results_are_written_atomically_and_oversized_files_are_ignored() {
        let dir = std::env::temp_dir().join(format!("redox-health-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("health.json");
        write_json(&file, &HostHealth { collected: 5, ..Default::default() }).unwrap();
        write_json(&file, &HostHealth { collected: 6, ..Default::default() }).unwrap();
        assert_eq!(read_json::<HostHealth>(&file).unwrap().collected, 6);
        std::fs::write(&file, vec![b' '; MAX_FILE as usize + 1]).unwrap();
        assert!(read_json::<HostHealth>(&file).is_none() && read_json::<HostHealth>(&dir.join("yok")).is_none() && read_json::<HostHealth>(&dir).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
