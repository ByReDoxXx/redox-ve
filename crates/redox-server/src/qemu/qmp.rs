//! QMP satırlarını ve /proc ölçümlerini ayrıştırma.

use super::*;

pub fn qmp_command(name: &str) -> String {
    format!("{}\n", serde_json::json!({ "execute": name }))
}

#[derive(Debug, Clone, PartialEq)]
pub enum QmpLine {
    Greeting,
    Return(Value),
    Error(String),
    /// Kendiliğinden gelen olay (SHUTDOWN, STOP …); yanıt beklerken atlanır.
    Event(String),
    Other,
}

pub fn parse_qmp_line(line: &str) -> QmpLine {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return QmpLine::Other;
    };
    if v.get("QMP").is_some() {
        QmpLine::Greeting
    } else if let Some(r) = v.get("return") {
        QmpLine::Return(r.clone())
    } else if let Some(e) = v.get("error") {
        QmpLine::Error(e["desc"].as_str().unwrap_or("bilinmeyen QMP hatası").to_string())
    } else if let Some(e) = v.get("event") {
        QmpLine::Event(e.as_str().unwrap_or_default().to_string())
    } else {
        QmpLine::Other
    }
}

/// `/proc/<pid>/stat` içinden toplam CPU tıkları (utime + stime).
pub fn parse_stat_ticks(stat: &str) -> Option<u64> {
    // Süreç adı parantez içinde boşluk içerebilir; son ')' sonrasından say.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

/// `/proc/<pid>/status` içinden bellek kullanımı (kB).
pub fn parse_vmrss_kb(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// İki örnek arasındaki tık farkından, konuğun çekirdeklerine göre CPU yüzdesi.
/// Linux'ta saniyede 100 tık (USER_HZ) varsayılır.
pub fn cpu_pct(delta_ticks: u64, elapsed: Duration, cores: u32) -> u8 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 || cores == 0 {
        return 0;
    }
    let used = delta_ticks as f64 / 100.0;
    (used / secs / cores as f64 * 100.0).round().clamp(0.0, 100.0) as u8
}
