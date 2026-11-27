//! Kök yardımcısının sonuçlarını okuma ve panelden güncelleme isteme.

use super::*;
use redox_core::health::{HostHealth, UpdateRun};

/// Yardımcıyla konuşulan dosyalar.
#[derive(Debug, Clone)]
pub struct HealthFiles {
    /// Yardımcının yazdığı dizin (köke ait).
    pub results: std::path::PathBuf,
    /// Sunucunun güncelleme istediğini bildiren dosya (sunucunun veri dizininde).
    pub request: std::path::PathBuf,
}

impl AppState {
    pub fn set_health_files(&self, files: HealthFiles) {
        *self.0.health.lock().unwrap_or_else(|e| e.into_inner()) = Some(files);
        self.health_tick();
    }

    fn health_files(&self) -> Option<HealthFiles> {
        self.0.health.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Son toplanan durumu kayda alır; uyarılar buradan üretilir.
    pub fn health_tick(&self) {
        let Some(files) = self.health_files() else { return };
        let found: Option<HostHealth> = crate::health::read_json(&files.results.join("health.json"));
        if self.lock().host_health != found {
            self.mutate(|c| c.host_health = found);
        }
    }

    pub fn update_status(&self) -> (bool, Option<UpdateRun>) {
        match self.health_files() {
            None => (false, None),
            Some(files) => (files.request.exists(), crate::health::read_json(&files.results.join("update.json"))),
        }
    }

    /// Kök yardımcısından bekleyen güncellemeleri kurmasını ister. İsteğin içeriği yoktur: yardımcı yalnızca
    /// sunucuda tanımlı depolardan olağan yükseltmeyi çalıştırır.
    pub fn request_update(&self, user: &str) -> Result<(), Error> {
        let files = self.health_files().ok_or_else(|| Error::InvalidSpec("sunucu güncellemesi bu kurulumda kapalı".to_string()))?;
        if self.lock().host_health.as_ref().is_none_or(|h| h.updates.is_empty()) {
            return Err(Error::InvalidSpec("bekleyen güncelleme yok".to_string()));
        }
        let outcome = std::fs::write(&files.request, b"guncelle\n").map_err(|e| format!("güncelleme isteği yazılamadı: {e}"));
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, "host update".to_string(), &outcome, now(), None);
        });
        outcome.map_err(Error::Driver)
    }
}
