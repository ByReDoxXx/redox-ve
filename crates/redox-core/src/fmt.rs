//! Arayüz ve komut satırı için ortak biçimlendirme yardımcıları.

/// `part / total` oranını 0-100 arası tam yüzdeye çevirir.
pub fn pct(part: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((part as f64 / total as f64) * 100.0).round().clamp(0.0, 100.0) as u8
}

/// GB değerini okunur biçime çevirir: `612 GB`, `1.4 TB`.
pub fn gb(value: u64) -> String {
    if value >= 1000 {
        format!("{:.1} TB", value as f64 / 1000.0)
    } else {
        format!("{value} GB")
    }
}

/// MB değerini GB olarak yazar; tam sayıysa ondalık göstermez: `8`, `5.2`.
pub fn mb_as_gb(mb: u64) -> String {
    let gb = mb as f64 / 1024.0;
    if (gb - gb.round()).abs() < 0.05 {
        format!("{}", gb.round() as u64)
    } else {
        format!("{gb:.1}")
    }
}

/// Çalışma süresi: `12g 4sa`, `3sa 20dk`, `7dk`.
pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d}g {h}sa")
    } else if h > 0 {
        format!("{h}sa {m}dk")
    } else {
        format!("{m}dk")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_rounds_and_clamps() {
        assert_eq!(pct(0, 0), 0);
        assert_eq!(pct(1, 3), 33);
        assert_eq!(pct(2, 3), 67);
        assert_eq!(pct(5, 5), 100);
        assert_eq!(pct(9, 5), 100);
    }

    #[test]
    fn gb_switches_to_tb() {
        assert_eq!(gb(612), "612 GB");
        assert_eq!(gb(999), "999 GB");
        assert_eq!(gb(1000), "1.0 TB");
        assert_eq!(gb(12_500), "12.5 TB");
    }

    #[test]
    fn mb_as_gb_drops_trailing_zero() {
        assert_eq!(mb_as_gb(8192), "8");
        assert_eq!(mb_as_gb(5325), "5.2");
        assert_eq!(mb_as_gb(512), "0.5");
        assert_eq!(mb_as_gb(0), "0");
    }

    #[test]
    fn uptime_units() {
        assert_eq!(uptime(0), "0dk");
        assert_eq!(uptime(7 * 60), "7dk");
        assert_eq!(uptime(3 * 3600 + 20 * 60), "3sa 20dk");
        assert_eq!(uptime(12 * 86_400 + 4 * 3600 + 59), "12g 4sa");
    }
}
