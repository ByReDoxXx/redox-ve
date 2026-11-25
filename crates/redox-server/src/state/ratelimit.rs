//! API hız sınırı: kullanıcı başına dakikada en çok N istek (sabit pencere).
//!
//! Amaç, döngüye girmiş bir betiğin ya da çalınmış bir token'ın sunucuyu boğmasını önlemektir. Sayaçlar
//! bellektedir; hizmet yeniden başlayınca sıfırlanır.

use super::*;

impl AppState {
    /// Dakikadaki istek sınırını ayarlar; 0 sınırı kaldırır.
    pub fn set_api_rate(&self, per_minute: u32) {
        let mut rate = self.0.api_rate.lock().unwrap_or_else(|e| e.into_inner());
        *rate = (per_minute, HashMap::new());
    }

    /// Bu kullanıcının bir API isteği daha yapmasına izin var mı? Yoksa kaç saniye sonra yeniden
    /// deneyebileceği döner.
    pub fn api_allowed(&self, user: &str, now: u64) -> Result<(), u64> {
        let mut rate = self.0.api_rate.lock().unwrap_or_else(|e| e.into_inner());
        let (limit, seen) = &mut *rate;
        if *limit == 0 {
            return Ok(());
        }
        let minute = now / 60;
        // Eski dakikaların kayıtları birikmesin.
        seen.retain(|_, (at, _)| *at == minute);
        let entry = seen.entry(user.to_string()).or_insert((minute, 0));
        if entry.1 >= *limit {
            return Err(60 - now % 60);
        }
        entry.1 += 1;
        Ok(())
    }
}
