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

    /// Adres kümesi ya da güvenlik grubu değişikliği: kaydeder, günlüğe yazar ve kuralları yeniden yükler.
    pub fn fw_definition(&self, what: String, user: &str, change: impl FnOnce(&mut Cluster) -> Result<(), Error>) -> Result<(), Error> {
        self.mutate(|c| {
            change(c)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, what, &Ok(()), now(), None);
            Ok::<_, Error>(())
        })?;
        self.sync_firewall().map_err(Error::Driver)
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
        // Gruplar ve adres kümeleri sürücüye verilmeden önce düz kurallara açılır.
        let guests: Vec<redox_core::Guest> = {
            let c = self.lock();
            c.guests.iter().map(|g| c.effective_firewall(g)).collect()
        };
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
