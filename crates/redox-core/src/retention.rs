//! Saklama kuralları: "son N yedek"e ek olarak haftalık, aylık ve yıllık yedekler (GFS) ve süreli kilit.
//!
//! GFS: son `weekly` haftanın, son `monthly` ayın ve son `yearly` yılın **en yeni** yedeği saklanır. "Son N hafta"
//! takvimdeki son N hafta değil, yedeği olan son N haftadır: sunucu bir hafta kapalı kalsa da N haftalık yedek durur.
//! Haftalar Pazartesi başlar; tarihler UTC'ye göre hesaplanır (gece yarısına yakın alınan yedek komşu güne düşebilir).

use serde::{Deserialize, Serialize};

pub const MAX_GFS: u32 = 520;
pub const MAX_LOCK_DAYS: u32 = 3650;
pub const DAY_SECS: u64 = 86_400;
/// Paneldeki "Kilitle" düğmesinin süresi.
pub const DEFAULT_LOCK_DAYS: u32 = 30;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gfs {
    pub weekly: u32,
    pub monthly: u32,
    pub yearly: u32,
}

impl Gfs {
    pub fn is_off(&self) -> bool {
        self.weekly == 0 && self.monthly == 0 && self.yearly == 0
    }

    pub fn valid(&self) -> bool {
        self.weekly <= MAX_GFS && self.monthly <= MAX_GFS && self.yearly <= MAX_GFS
    }

    /// Kısa gösterim: `4H 12A 1Y`; kapalıysa boş.
    pub fn label(&self) -> String {
        [(self.weekly, "H"), (self.monthly, "A"), (self.yearly, "Y")].iter().filter(|(n, _)| *n > 0).map(|(n, unit)| format!("{n}{unit}")).collect::<Vec<_>>().join(" ")
    }
}

/// Epoch gününden (1970-01-01 = 0) takvim tarihi: `(yıl, ay 1-12, gün 1-31)`.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Epoch saniyesinin okunur tarihi (UTC): `2026-11-25`.
pub fn date_label(ts: u64) -> String {
    let (y, m, d) = civil((ts / DAY_SECS) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn week_of(ts: u64) -> i64 {
    // 1970-01-01 Perşembe'dir: +3 ile haftalar Pazartesi başlar.
    ((ts / DAY_SECS) as i64 + 3).div_euclid(7)
}

fn month_of(ts: u64) -> i64 {
    let (y, m, _) = civil((ts / DAY_SECS) as i64);
    y * 12 + i64::from(m)
}

fn year_of(ts: u64) -> i64 {
    civil((ts / DAY_SECS) as i64).0
}

/// Saklanacak yedeklerin sıra numaraları. `created`: yedeklerin alınma anları, **en yeni başta**.
/// Önce en yeni `keep` yedek, sonra her dönem için o dönemin en yeni yedeği işaretlenir.
pub fn kept(created: &[u64], keep: u32, gfs: Gfs) -> Vec<bool> {
    let mut keep_flags: Vec<bool> = (0..created.len()).map(|i| i < keep.max(1) as usize).collect();
    for (count, bucket) in [(gfs.weekly, week_of as fn(u64) -> i64), (gfs.monthly, month_of), (gfs.yearly, year_of)] {
        let mut seen: Vec<i64> = Vec::new();
        for (i, ts) in created.iter().enumerate() {
            let period = bucket(*ts);
            if seen.contains(&period) {
                continue;
            }
            if seen.len() >= count as usize {
                break;
            }
            // Liste en yeni başta olduğu için dönemin ilk görülen yedeği o dönemin en yenisidir.
            seen.push(period);
            keep_flags[i] = true;
        }
    }
    keep_flags
}

/// Doluluk tahmininde bakılan süre (gün) ve tahmin için gereken en az yedek sayısı.
pub const FORECAST_WINDOW_DAYS: u64 = 14;
pub const FORECAST_MIN_BACKUPS: usize = 3;

/// Yedeklerin büyüme hızından, yedeklerin durduğu alanın kaç gün sonra dolacağı tahmini.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forecast {
    /// Son iki haftada günde ortalama eklenen yedek verisi (MB).
    pub mb_per_day: u64,
    /// Bu hızla alanın dolmasına kalan gün; büyüme yoksa `None`.
    pub days_left: Option<u64>,
}

/// `added`: son `FORECAST_WINDOW_DAYS` günde alınan yedeklerin diske eklediği veri (MB), `(an, MB)` çiftleri.
/// Silinen yedeklerin geri verdiği yer hesaba katılmaz: tahmin temkinlidir (gerçekte daha geç dolar).
pub fn forecast(added: &[(u64, u64)], free_mb: u64, now: u64) -> Option<Forecast> {
    let since = now.saturating_sub(FORECAST_WINDOW_DAYS * DAY_SECS);
    let recent: Vec<&(u64, u64)> = added.iter().filter(|(at, _)| *at >= since && *at <= now).collect();
    if recent.len() < FORECAST_MIN_BACKUPS {
        return None;
    }
    // Yedekler iki haftadan kısa bir süredir alınıyorsa o süreye bölünür (en az bir gün).
    let oldest = recent.iter().map(|(at, _)| *at).min().unwrap_or(now);
    let days = (now.saturating_sub(oldest)).div_ceil(DAY_SECS).clamp(1, FORECAST_WINDOW_DAYS);
    let mb_per_day = recent.iter().map(|(_, mb)| *mb).sum::<u64>() / days;
    Some(Forecast { mb_per_day, days_left: (mb_per_day > 0).then(|| free_mb / mb_per_day) })
}

/// Olağan dışı değişim için en az bu kadar önceki yedek gerekir.
pub const ANOMALY_MIN_SAMPLES: usize = 3;
/// Değişen veri, önceki yedeklerin ortancasının en az bu kadar katıysa şüphelidir.
pub const ANOMALY_FACTOR: u64 = 4;
/// Küçük değişimler (MB) ve diskin küçük bir bölümünü (yüzde) kapsayanlar uyarı üretmez.
pub const ANOMALY_MIN_MB: u64 = 512;
pub const ANOMALY_MIN_PCT: u64 = 25;

/// Yedekte değişen veri olağan dışı mı? `history`: konuğun önceki yedeklerinde değişen veri (MB).
/// Fidye yazılımı diskin büyük bölümünü yeniden yazar: artımlı yedek birden olağanın katlarına çıkar.
/// Büyük bir veri taşıma ya da yükseltme de aynı izi bırakır; bu bir kanıt değil, bakılması gereken bir işarettir.
pub fn change_anomaly(history: &[u64], new_mb: u64, size_mb: u64) -> Option<String> {
    if history.len() < ANOMALY_MIN_SAMPLES || new_mb < ANOMALY_MIN_MB || new_mb * 100 < size_mb * ANOMALY_MIN_PCT {
        return None;
    }
    let mut sorted = history.to_vec();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2].max(1);
    (new_mb >= median * ANOMALY_FACTOR).then(|| {
        format!("değişen veri {new_mb} MB; önceki yedeklerin ortancası {median} MB ({} kat). Konukta toplu şifreleme (fidye yazılımı) ya da büyük bir veri taşıma olabilir", new_mb / median)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forecast_estimates_days_until_the_backup_storage_is_full() {
        const NOW: u64 = 1_700_000_000;
        let day = |d: u64, mb: u64| (NOW - d * DAY_SECS, mb);
        assert_eq!(forecast(&[day(1, 500), day(2, 500)], 10_000, NOW), None, "yedek sayısı yetersiz");
        // On günde 10 GB: günde 1 GB; 30 GB boş yer 30 günde dolar.
        let ten: Vec<(u64, u64)> = (0..10).map(|d| day(d + 1, 1024)).collect();
        assert_eq!(forecast(&ten, 30 * 1024, NOW), Some(Forecast { mb_per_day: 1024, days_left: Some(30) }));
        // Pencerenin dışındaki eski yedekler sayılmaz.
        let mut old = ten.clone();
        old.push(day(40, 1_000_000));
        assert_eq!(forecast(&old, 30 * 1024, NOW).unwrap().mb_per_day, 1024);
        // Hiç veri eklenmiyorsa (tamamı artımlı ve değişiklik yok) dolma günü yoktur.
        assert_eq!(forecast(&[day(1, 0), day(2, 0), day(3, 0)], 1000, NOW), Some(Forecast { mb_per_day: 0, days_left: None }));
        // Yer kalmadıysa sıfır gün.
        assert_eq!(forecast(&ten, 100, NOW).unwrap().days_left, Some(0));
    }

    #[test]
    fn change_anomaly_needs_history_size_and_a_large_jump() {
        let usual = [120, 90, 150, 110, 4000];
        // 20 GB'lık yedekte 9 GB değişti; olağanı 120 MB.
        let note = change_anomaly(&usual, 9000, 20_000).unwrap();
        assert!(note.contains("9000 MB") && note.contains("ortancası 120 MB (75 kat)"), "{note}");
        assert_eq!(change_anomaly(&usual, 400, 20_000), None, "olağanın birkaç katı ama küçük");
        assert_eq!(change_anomaly(&usual, 2000, 20_000), None, "diskin %25'inden az");
        assert_eq!(change_anomaly(&[120, 90], 9000, 20_000), None, "geçmiş yetersiz");
        assert_eq!(change_anomaly(&[6000, 5000, 7000], 9000, 20_000), None, "hep çok değişen konuk");
        assert!(change_anomaly(&[0, 0, 0], 600, 1000).is_some(), "hiç değişmeyen konukta ani değişim");
    }

    /// 2026-11-25 Çarşamba 12:00 UTC.
    const WED: u64 = 1_795_608_000;

    #[test]
    fn calendar_arithmetic_is_correct() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29), "artık yıl");
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(date_label(WED), "2026-11-25");
        assert_eq!(date_label(0), "1970-01-01");
        // Pazartesi haftayı başlatır: Pazar ile ertesi Pazartesi farklı haftadadır, Pazartesi ile Pazar aynı.
        let monday = WED - 2 * DAY_SECS;
        assert_eq!(week_of(monday), week_of(monday + 6 * DAY_SECS));
        assert_ne!(week_of(monday - DAY_SECS), week_of(monday));
        assert_ne!(month_of(WED), month_of(WED + 6 * DAY_SECS), "25 Kasım + 6 gün Aralık'tır");
        assert_ne!(year_of(WED), year_of(WED + 40 * DAY_SECS));
    }

    #[test]
    fn gfs_keeps_the_newest_backup_of_each_period() {
        // 100 gün boyunca her gün bir yedek, en yeni başta.
        let daily: Vec<u64> = (0..100).map(|i| WED - i * DAY_SECS).collect();
        let count = |flags: &[bool]| flags.iter().filter(|k| **k).count();
        assert_eq!(count(&kept(&daily, 7, Gfs::default())), 7);
        assert_eq!(count(&kept(&daily, 0, Gfs::default())), 1, "en az bir yedek hep kalır");

        let flags = kept(&daily, 7, Gfs { weekly: 4, monthly: 0, yearly: 0 });
        // Son 7 gün + önceki 3 haftanın en yenileri (bu haftanınki zaten ilk 7'de).
        assert_eq!(count(&flags), 7 + 2);
        let weekly: Vec<usize> = flags.iter().enumerate().filter(|(i, k)| **k && *i >= 7).map(|(i, _)| i).collect();
        // Çarşamba'dan geriye haftaların en yenileri: bu hafta 0., önceki haftaların Pazar günleri 3., 10., 17. sırada.
        // İlk ikisi zaten son 7 yedeğin içinde.
        assert_eq!(weekly, [10, 17]);

        let flags = kept(&daily, 1, Gfs { weekly: 0, monthly: 3, yearly: 0 });
        let kept_dates: Vec<String> = flags.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| date_label(daily[i])).collect();
        assert_eq!(kept_dates, ["2026-11-25", "2026-10-31", "2026-09-30"], "her ayın en yeni yedeği");

        let years: Vec<u64> = (0..5).map(|i| WED - i * 200 * DAY_SECS).collect();
        let flags = kept(&years, 1, Gfs { weekly: 0, monthly: 0, yearly: 2 });
        assert_eq!(flags, [true, false, true, false, false], "2026 ve 2025'in en yenileri");
        // Arada boşluk olsa da yedeği olan son N dönem sayılır.
        let sparse = [WED, WED - 30 * DAY_SECS, WED - 400 * DAY_SECS];
        assert_eq!(kept(&sparse, 1, Gfs { weekly: 3, monthly: 0, yearly: 0 }), [true, true, true]);
        assert!(kept(&[], 3, Gfs { weekly: 1, monthly: 1, yearly: 1 }).is_empty());
    }

    #[test]
    fn labels_and_limits() {
        assert_eq!(Gfs { weekly: 4, monthly: 12, yearly: 1 }.label(), "4H 12A 1Y");
        assert_eq!(Gfs { weekly: 0, monthly: 6, yearly: 0 }.label(), "6A");
        assert!(Gfs::default().is_off() && Gfs::default().label().is_empty());
        assert!(Gfs { weekly: MAX_GFS, ..Default::default() }.valid() && !Gfs { yearly: MAX_GFS + 1, ..Default::default() }.valid());
    }
}
