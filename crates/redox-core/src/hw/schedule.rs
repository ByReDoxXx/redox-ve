//! Önyükleme sırası ve zamanlanmış güç işlemleri.

use super::*;

pub const MAX_BOOT_ORDER: u32 = 999;
pub const MAX_BOOT_DELAY: u32 = 600;
pub const MAX_SCHEDULES: usize = 8;
/// Saati kaçan zamanlanmış işlem bu kadar dakika içinde yine çalıştırılır; sonrası o gün atlanır.
pub const SCHEDULE_GRACE_MIN: u16 = 15;
pub const EVERY_DAY: u8 = 0b111_1111;
pub(super) const DAY_NAMES: [&str; 7] = ["Pzt", "Sal", "Çar", "Per", "Cum", "Cmt", "Paz"];

pub(super) fn every_day() -> u8 {
    EVERY_DAY
}

/// Zamanlanmış güç işlemi: belirli günlerde, belirli saatte konuğu başlatır, kapatır ya da yeniden başlatır.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub action: crate::model::Action,
    /// `SS:DD`, sunucu saatiyle.
    pub time: String,
    /// Günler: bit 0 Pazartesi … bit 6 Pazar.
    #[serde(default = "every_day")]
    pub days: u8,
    /// Son çalıştırıldığı gün (`YYYY-AA-GG`); aynı gün ikinci kez çalışmaz.
    #[serde(default)]
    pub last_run: Option<String>,
}

impl Schedule {
    pub fn valid(&self) -> bool {
        use crate::model::Action;
        matches!(self.action, Action::Start | Action::Stop | Action::Reboot)
            && crate::model::parse_time(&self.time).is_some()
            && self.days != 0
            && self.days <= EVERY_DAY
    }

    /// Bu gün (0 Pazartesi … 6 Pazar) çalışır mı?
    pub fn on_day(&self, weekday: u8) -> bool {
        weekday < 7 && self.days & (1 << weekday) != 0
    }

    /// Günlerin okunur hali: `her gün`, `hafta içi`, `Pzt, Çar`.
    pub fn days_label(&self) -> String {
        days_label(self.days)
    }

    /// Günlerin komut satırı biçimi: `1-5`, `67`, `135`.
    fn days_text(&self) -> String {
        (0..7u8).filter(|d| self.days & (1 << d) != 0).map(|d| (d + 1).to_string()).collect()
    }
}

/// Komut satırı ve form biçimi: `start@08:00` ya da günlerle `stop@20:00@12345`.
impl std::fmt::Display for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.action, self.time)?;
        if self.days != EVERY_DAY {
            write!(f, "@{}", self.days_text())?;
        }
        Ok(())
    }
}

/// Gün maskesinin okunur hali: `her gün`, `hafta içi`, `Pzt, Çar`.
pub fn days_label(days: u8) -> String {
    match days {
        EVERY_DAY => "her gün".to_string(),
        0b001_1111 => "hafta içi".to_string(),
        0b110_0000 => "hafta sonu".to_string(),
        days => (0..7).filter(|d| days & (1 << d) != 0).map(|d| DAY_NAMES[d as usize]).collect::<Vec<_>>().join(", "),
    }
}

/// Gün listesi: `1`-`7` rakamları (1 Pazartesi) ve `1-5` gibi aralıklar; `135`, `1-5`, `1-57`.
pub fn parse_days(text: &str) -> Option<u8> {
    let bytes = text.as_bytes();
    let digit = |b: u8| (b'1'..=b'7').contains(&b).then(|| b - b'1');
    let (mut days, mut i) = (0u8, 0);
    while i < bytes.len() {
        let from = digit(bytes[i])?;
        if bytes.get(i + 1) == Some(&b'-') {
            let to = digit(*bytes.get(i + 2)?)?;
            if to < from {
                return None;
            }
            (from..=to).for_each(|d| days |= 1 << d);
            i += 3;
        } else {
            days |= 1 << from;
            i += 1;
        }
    }
    (days != 0).then_some(days)
}

/// Virgülle ya da satırla ayrılmış zamanlama listesi: `start@08:00@1-5, stop@20:00`.
pub fn parse_schedules(text: &str) -> Result<Vec<Schedule>, String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let bad = || format!("geçersiz zamanlama \"{item}\"; eylem@SS:DD ya da eylem@SS:DD@günler biçiminde yazın, örn. start@08:00@1-5");
            let mut parts = item.split('@');
            let action: crate::model::Action = parts.next().unwrap_or("").trim().parse().map_err(|_| bad())?;
            let time = parts.next().ok_or_else(bad)?.trim().to_string();
            let days = match parts.next() {
                Some(d) => parse_days(d.trim()).ok_or_else(bad)?,
                None => EVERY_DAY,
            };
            let schedule = Schedule { action, time, days, last_run: None };
            if parts.next().is_some() || !schedule.valid() {
                return Err(bad());
            }
            Ok(schedule)
        })
        .collect()
}
