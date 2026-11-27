//! Operatörün yetki türleri. Operatöre bunların yalnızca bir kısmı verilebilir ("yalnızca yedek alsın").
//! Kısıt yalnızca daraltır: yönetici ve izleyici etkilenmez, operatör bu dördünün dışına çıkamaz.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permit {
    /// Başlatma, kapatma, yeniden başlatma, duraklatma ve askıya alma.
    Power,
    /// Ekran, canlı konsol, terminal, tuş gönderme ve konukta komut çalıştırma.
    Console,
    Snapshot,
    Backup,
}

impl Permit {
    pub const ALL: [Permit; 4] = [Permit::Power, Permit::Console, Permit::Snapshot, Permit::Backup];

    pub fn name(self) -> &'static str {
        match self {
            Permit::Power => "power",
            Permit::Console => "console",
            Permit::Snapshot => "snapshot",
            Permit::Backup => "backup",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Permit::Power => "güç",
            Permit::Console => "konsol",
            Permit::Snapshot => "anlık görüntü",
            Permit::Backup => "yedek",
        }
    }

    pub fn parse(text: &str) -> Result<Permit, String> {
        Permit::ALL.into_iter().find(|p| p.name() == text).ok_or_else(|| format!("bilinmeyen yetki: {text} (power, console, snapshot, backup)"))
    }
}

impl UserFile {
    /// Operatörün yetkilerini belirler. Dördü birden "kısıt yok" demektir; hiçbiri geçersizdir (o izleyicidir).
    pub fn set_permits(&mut self, name: &str, mut permits: Vec<Permit>) -> Result<(), String> {
        permits.sort();
        permits.dedup();
        if permits.is_empty() {
            return Err("en az bir yetki seçin; hiç yetkisi olmayacaksa rolü izleyici yapın".to_string());
        }
        self.find_mut(name)?.permits = Some(permits).filter(|p| p.len() < Permit::ALL.len());
        Ok(())
    }
}

impl Identity {
    /// Bu etiketleri taşıyan konukta bu tür işlemi yapabilir mi?
    pub fn allows(&self, permit: Permit, tags: &[String]) -> bool {
        self.can_operate(tags) && (self.role != Role::Operator || self.permits.as_ref().is_none_or(|p| p.contains(&permit)))
    }
}
