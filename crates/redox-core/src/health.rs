//! Sunucunun donanım ve güncelleme durumu. Bunları okumak kök yetkisi istediği için ayrı bir yardımcı toplar
//! (`redox host health`); yönetim sunucusu yalnızca sonucu okur.

use serde::{Deserialize, Serialize};

/// Bu sıcaklığın üstündeki disk için uyarı verilir.
pub const DISK_HOT_C: i64 = 60;
/// NVMe diskin kullanılmış ömrü bu yüzdeyi geçince uyarı verilir.
pub const DISK_WORN_PCT: u64 = 90;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskHealth {
    pub device: String,
    pub model: String,
    /// SMART genel değerlendirmesi; `None`: disk bildirmiyor.
    pub passed: Option<bool>,
    pub temperature_c: Option<i64>,
    pub power_on_hours: Option<u64>,
    /// Yeniden eşlenen (ATA) ya da onarılamayan (NVMe) hata sayısı.
    pub bad_sectors: Option<u64>,
    /// NVMe: kullanılmış ömür yüzdesi.
    pub wear_pct: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sensor {
    pub name: String,
    /// Okunan değer, birimiyle.
    pub reading: String,
    /// IPMI durumu: `ok`, `nc` (uyarı), `cr` / `nr` (kritik).
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PackageUpdate {
    pub name: String,
    pub from: String,
    pub to: String,
}

/// Panelden istenen son güncellemenin sonucu.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateRun {
    pub finished: u64,
    pub ok: bool,
    /// Çıktının son satırları.
    pub output: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostHealth {
    /// Toplandığı an (epoch saniyesi).
    pub collected: u64,
    pub disks: Vec<DiskHealth>,
    pub sensors: Vec<Sensor>,
    pub updates: Vec<PackageUpdate>,
    /// Okunamayan kaynaklar ve nedenleri.
    pub notes: Vec<String>,
}

/// `(anahtar, kritik mi, ileti)` biçiminde dikkat isteyen durumlar.
pub fn problems(health: &HostHealth) -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    for d in &health.disks {
        let key = |what: &str| format!("disk:{}:{what}", d.device);
        if d.passed == Some(false) {
            out.push((key("smart"), true, format!("{} diski SMART sınamasını geçemiyor: verileri yedekleyin ve diski değiştirin", d.device)));
        }
        if let Some(n) = d.bad_sectors.filter(|n| *n > 0) {
            out.push((key("sector"), false, format!("{} diskinde {n} bozuk ya da yeniden eşlenmiş sektör var", d.device)));
        }
        if let Some(t) = d.temperature_c.filter(|t| *t >= DISK_HOT_C) {
            out.push((key("temp"), false, format!("{} diski {t} °C", d.device)));
        }
        if let Some(w) = d.wear_pct.filter(|w| *w >= DISK_WORN_PCT) {
            out.push((key("wear"), false, format!("{} diskinin ömrünün %{w} kadarı tükenmiş", d.device)));
        }
    }
    for s in health.sensors.iter().filter(|s| matches!(s.status.as_str(), "nc" | "cr" | "nr")) {
        out.push((format!("sensor:{}", s.name), s.status != "nc", format!("donanım algılayıcısı sınır dışında: {} = {}", s.name, s.reading)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_problems_are_reported() {
        let disk = |passed, bad, temp, wear| DiskHealth { device: "/dev/sda".into(), passed, bad_sectors: bad, temperature_c: temp, wear_pct: wear, ..Default::default() };
        let fine = HostHealth { disks: vec![disk(Some(true), Some(0), Some(40), Some(10)), disk(None, None, None, None)], sensors: vec![Sensor { name: "CPU".into(), reading: "40 C".into(), status: "ok".into() }, Sensor { name: "PSU2".into(), reading: "na".into(), status: "na".into() }], ..Default::default() };
        assert!(problems(&fine).is_empty());
        let bad = HostHealth { disks: vec![disk(Some(false), Some(8), Some(61), Some(95))], sensors: vec![Sensor { name: "Fan1".into(), reading: "0 RPM".into(), status: "cr".into() }, Sensor { name: "Temp".into(), reading: "80 C".into(), status: "nc".into() }], ..Default::default() };
        let found = problems(&bad);
        assert_eq!(found.len(), 6, "{found:?}");
        assert_eq!(found.iter().filter(|(_, critical, _)| *critical).count(), 2, "SMART hatası ve kritik algılayıcı");
    }
}
