//! Ölçümler ve balon: disk sayaçları, bellek ölçümü, bellek hedefi.

use super::*;

/// Konuğun bütün blok aygıtlarından açılıştan beri okunan ve yazılan bayt.
pub(in crate::qemu) fn io_counters(d: &QemuDriver, g: &Guest) -> Option<(u64, u64)> {
    let replies = qmp(&d.cfg.qmp_path(g.vmid), &["query-blockstats"]).ok()?;
    let devices = replies.first()?.as_array()?;
    let sum = |key: &str| devices.iter().filter_map(|b| b["stats"][key].as_u64()).sum::<u64>();
    Some((sum("rd_bytes"), sum("wr_bytes")))
}

/// Balon sürücüsünün bellek ölçümü. QEMU ölçümleri yalnızca yoklama aralığı ayarlıysa toplar: ayarlı
/// değilse burada açılır ve ilk ölçüm bir sonraki turda gelir. Sürücüsüz konukta değerler hiç dolmaz.
pub(in crate::qemu) fn balloon_stats(d: &QemuDriver, g: &Guest) -> Option<redox_core::BalloonStats> {
    const PATH: &str = "/machine/peripheral/balloon0";
    const POLL_SECS: u64 = 10;
    if !g.hw.balloon {
        return None;
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let get = |property: &str| serde_json::json!({ "execute": "qom-get", "arguments": { "path": PATH, "property": property } });
    let replies = qmp_values(&sock, &[get("guest-stats-polling-interval"), get("guest-stats")]).ok()?;
    if replies.first().and_then(Value::as_u64) != Some(POLL_SECS) {
        let set = serde_json::json!({ "execute": "qom-set", "arguments": { "path": PATH, "property": "guest-stats-polling-interval", "value": POLL_SECS } });
        let _ = qmp_values(&sock, &[set]);
        return None;
    }
    let report = replies.get(1)?;
    if report["last-update"].as_u64().unwrap_or(0) == 0 {
        return None;
    }
    // Sürücünün bildirmediği değerleri QEMU -1 olarak, yani işaretsiz okununca 2^64-1 olarak verir
    // (eski çekirdekler ve bazı Windows sürücüleri "kullanılabilir bellek"i bildirmez). Böyle bir değer
    // ölçüm sayılırsa konuk boşta görünür ve belleği haksız yere kısılır.
    let mb = |key: &str| report["stats"][key].as_u64().filter(|bytes| *bytes < u64::MAX / 2).map(|bytes| bytes / (1024 * 1024));
    Some(redox_core::BalloonStats { total_mb: mb("stat-total-memory")?, available_mb: mb("stat-available-memory")? })
}

/// Balonu hedefe ayarlar; hedef 0 ise konuk belleğinin tamamını geri alır.
pub(in crate::qemu) fn set_balloon(d: &QemuDriver, g: &Guest, target_mb: u64) -> Result<(), String> {
    if !g.hw.balloon {
        return Err("konukta bellek balonu aygıtı yok".to_string());
    }
    let mb = if target_mb == 0 { g.mem_total_mb } else { target_mb.min(g.mem_total_mb) };
    let command = serde_json::json!({ "execute": "balloon", "arguments": { "value": mb * 1024 * 1024 } });
    qmp_values(&d.cfg.qmp_path(g.vmid), &[command]).map(|_| ()).map_err(|e| format!("bellek hedefi ayarlanamadı: {e}"))
}
