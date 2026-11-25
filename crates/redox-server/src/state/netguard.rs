//! Konuk güvenlik duvarı ve ağ kartı korumaları.

use super::*;

impl AppState {
    /// Gerçek sürücüde güvenlik duvarı ancak sunucu `--firewall` ile başlatıldıysa açılabilir.
    pub(super) fn check_firewall(&self, wanted: bool) -> Result<(), Error> {
        if wanted && self.driver().is_real() && !self.driver().firewall() {
            return Err(Error::InvalidSpec("konuk güvenlik duvarı kapalı: sunucuyu --firewall ile başlatın (nftables ve CAP_NET_ADMIN gerekir)".to_string()));
        }
        Ok(())
    }

    /// Ağ kartı korumalarını konukların o anki arayüzleriyle eşitler (konuk açılıp kapandıkça arayüz adları
    /// değişir). Düzenli aralıkla çağrılır; kurallar değişmediyse hiçbir şey yüklenmez.
    pub fn guards_tick(&self) -> Result<(), String> {
        if !self.driver().firewall() {
            return Ok(());
        }
        let guests = self.lock().guests.clone();
        self.driver().refresh_guards(&guests)
    }

    /// Bütün konukların güvenlik duvarı kurallarını yeniden yükler. Başarısızlık görev günlüğüne yazılır.
    pub fn sync_firewall(&self) -> Result<(), String> {
        if !self.driver().firewall() {
            return Ok(());
        }
        let guests = self.lock().guests.clone();
        let outcome = self.driver().apply_firewall(&guests);
        if outcome.is_err() {
            self.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, SCHEDULER, "firewall apply".to_string(), &outcome, now(), None);
            });
        }
        outcome
    }
}
