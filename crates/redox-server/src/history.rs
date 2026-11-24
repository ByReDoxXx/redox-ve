//! Uzun süreli ölçüm geçmişi: dakikalık ortalamalar (24 saat) ve saatlik ortalamalar (30 gün).
//!
//! Beş saniyelik ham örnekler bellekte kalır (son 10 dakika); buradaki katmanlar onların
//! ortalamasıdır ve diske yazılır, böylece hizmet yeniden başlayınca grafikler kaybolmaz.
//! Sunucunun kapalı olduğu süreler doldurulmaz: grafikte yer almazlar.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MINUTE_POINTS: usize = 24 * 60;
pub const HOUR_POINTS: usize = 30 * 24;

/// Grafiklerde gösterilen zaman aralığı.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    /// Son 10 dakika (ham örnekler).
    #[default]
    Recent,
    Day,
    Month,
}

impl Range {
    pub fn parse(text: &str) -> Range {
        match text {
            "24s" => Range::Day,
            "30g" => Range::Month,
            _ => Range::Recent,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Range::Recent => "",
            Range::Day => "24s",
            Range::Month => "30g",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Range::Recent => "10 dakika",
            Range::Day => "24 saat",
            Range::Month => "30 gün",
        }
    }

    /// Grafik ekseninin sol, orta etiketleri.
    pub fn axis(self) -> (&'static str, &'static str) {
        match self {
            Range::Recent => ("-10 dk", "-5 dk"),
            Range::Day => ("-24 sa", "-12 sa"),
            Range::Month => ("-30 gün", "-15 gün"),
        }
    }
}

type Tier = HashMap<String, VecDeque<f32>>;
type Sums = HashMap<String, (f64, u32)>;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Long {
    minute: Tier,
    hour: Tier,
    /// İçinde bulunulan dakikanın ve saatin toplamları (diske yazılmaz).
    #[serde(skip)]
    minute_sums: Sums,
    #[serde(skip)]
    hour_sums: Sums,
    #[serde(skip)]
    current_minute: u64,
    #[serde(skip)]
    current_hour: u64,
}

fn flush(sums: &mut Sums, tier: &mut Tier, cap: usize, mut also: Option<&mut Sums>) {
    for (key, (sum, count)) in sums.drain() {
        if count == 0 {
            continue;
        }
        let average = sum / count as f64;
        let series = tier.entry(key.clone()).or_default();
        series.push_back(average as f32);
        while series.len() > cap {
            series.pop_front();
        }
        if let Some(next) = also.as_deref_mut() {
            let slot = next.entry(key).or_insert((0.0, 0));
            slot.0 += average;
            slot.1 += 1;
        }
    }
}

impl Long {
    /// Zamanı ilerletir: dakika ya da saat değiştiyse biriken ortalamaları katmanlara yazar.
    /// Dakika değiştiyse `true` döner.
    pub fn roll(&mut self, now: u64) -> bool {
        let (minute, hour) = (now / 60, now / 3600);
        if self.current_minute == 0 {
            (self.current_minute, self.current_hour) = (minute, hour);
            return false;
        }
        let rolled = minute != self.current_minute;
        if rolled {
            flush(&mut self.minute_sums, &mut self.minute, MINUTE_POINTS, Some(&mut self.hour_sums));
            self.current_minute = minute;
        }
        if hour != self.current_hour {
            flush(&mut self.hour_sums, &mut self.hour, HOUR_POINTS, None);
            self.current_hour = hour;
        }
        rolled
    }

    pub fn push(&mut self, key: &str, value: f64) {
        let slot = self.minute_sums.entry(key.to_string()).or_insert((0.0, 0));
        slot.0 += value;
        slot.1 += 1;
    }

    /// Artık var olmayan kaynakların (silinen konuklar) geçmişini atar.
    pub fn retain(&mut self, live: &[String]) {
        for map in [&mut self.minute, &mut self.hour] {
            map.retain(|k, _| live.contains(k));
        }
        for sums in [&mut self.minute_sums, &mut self.hour_sums] {
            sums.retain(|k, _| live.contains(k));
        }
    }

    pub fn series(&self, range: Range) -> HashMap<String, Vec<f64>> {
        let tier = match range {
            Range::Day => &self.minute,
            Range::Month => &self.hour,
            Range::Recent => return HashMap::new(),
        };
        tier.iter().map(|(k, v)| (k.clone(), v.iter().map(|x| *x as f64).collect())).collect()
    }

    pub fn load(path: &Path) -> Long {
        match std::fs::read(path) {
            // Bozuk ölçüm dosyası hizmeti durdurmamalı: geçmiş sıfırdan başlar.
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("uyarı: ölçüm geçmişi okunamadı ({}): {e}; sıfırdan başlanıyor", path.display());
                Long::default()
            }),
            Err(_) => Long::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        crate::auth::write_private(path, &bytes).map_err(|e| format!("ölçüm geçmişi {} dosyasına yazılamadı: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_800_000_000; // tam saat başı

    #[test]
    fn averages_roll_into_minute_and_hour_tiers() {
        let mut long = Long::default();
        assert!(!long.roll(T0), "ilk çağrı yalnızca saati kurar");
        // Bir dakika içinde 12 örnek (5 sn aralık): 0..=11 → ortalama 5.5
        for i in 0..12 {
            long.roll(T0 + i * 5);
            long.push("node:cpu", i as f64);
        }
        assert!(long.series(Range::Day).is_empty(), "dakika dolmadan katmana yazılmaz");
        assert!(long.roll(T0 + 60));
        assert_eq!(long.series(Range::Day)["node:cpu"], [5.5]);

        // Saatin kalan 59 dakikası sabit 20 → saatlik ortalama (5.5 + 59*20)/60
        for minute in 1..60 {
            long.roll(T0 + minute * 60);
            long.push("node:cpu", 20.0);
        }
        assert!(long.roll(T0 + 3600));
        let day = &long.series(Range::Day)["node:cpu"];
        assert_eq!((day.len(), day[1], day[59]), (60, 20.0, 20.0));
        let month = &long.series(Range::Month)["node:cpu"];
        assert_eq!(month.len(), 1);
        assert!((month[0] - (5.5 + 59.0 * 20.0) / 60.0).abs() < 1e-4, "{}", month[0]);
        assert!(long.series(Range::Recent).is_empty(), "ham örnekler burada tutulmaz");
    }

    #[test]
    fn tiers_are_bounded_and_pruned() {
        let mut long = Long::default();
        long.roll(T0);
        for minute in 0..(MINUTE_POINTS as u64 + 30) {
            long.push("guest:100:cpu", 1.0);
            long.push("guest:101:cpu", 2.0);
            long.roll(T0 + (minute + 1) * 60);
        }
        let day = long.series(Range::Day);
        assert_eq!(day["guest:100:cpu"].len(), MINUTE_POINTS, "24 saatten eskisi atılır");
        assert_eq!(long.series(Range::Month)["guest:101:cpu"].len(), (MINUTE_POINTS + 30) / 60);
        long.retain(&["guest:101:cpu".to_string()]);
        assert!(!long.series(Range::Day).contains_key("guest:100:cpu") && long.series(Range::Month).contains_key("guest:101:cpu"));
    }

    #[test]
    fn survives_a_restart_and_a_corrupt_file() {
        let path = std::env::temp_dir().join(format!("redox-metrics-{}.json", std::process::id()));
        let mut long = Long::default();
        long.roll(T0);
        long.push("node:mem", 40.0);
        long.roll(T0 + 60);
        long.save(&path).unwrap();
        let back = Long::load(&path);
        assert_eq!(back.series(Range::Day)["node:mem"], [40.0]);
        // Yeniden başlayan hizmet kaldığı yerden ekler.
        let mut back = back;
        back.roll(T0 + 600);
        back.push("node:mem", 60.0);
        back.roll(T0 + 660);
        assert_eq!(back.series(Range::Day)["node:mem"], [40.0, 60.0]);

        std::fs::write(&path, "{ bozuk").unwrap();
        assert!(Long::load(&path).series(Range::Day).is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(Long::load(&path).series(Range::Day).is_empty(), "dosya yoksa boş başlar");
    }

    #[test]
    fn range_names_round_trip() {
        for range in [Range::Recent, Range::Day, Range::Month] {
            assert_eq!(Range::parse(range.as_str()), range);
        }
        assert_eq!(Range::parse("saçma"), Range::Recent);
    }
}
