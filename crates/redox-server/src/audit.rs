//! Denetim günlüğü: kim, nereden, ne yaptı. Bellekte son kayıtlar tutulur ve
//! (dosya yolu verilmişse) her kayıt JSON satırı olarak dosyaya eklenir.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Bellekte tutulan ve sayfada aranabilen kayıt sayısı.
pub const KEEP: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts: u64,
    pub user: String,
    pub ip: String,
    /// Yapılan iş, örn. `POST /guests/103/action` ya da `giriş`.
    pub action: String,
    pub ok: bool,
}

pub struct Audit {
    path: Option<PathBuf>,
    entries: Mutex<VecDeque<AuditEntry>>,
    /// Kayıtların kopyasını dışarı gönderen uç (`--syslog`).
    forward: std::sync::OnceLock<crate::syslog::Forwarder>,
}

impl Audit {
    /// Dosya varsa son kayıtları yükler; bozuk satırlar atlanır.
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut entries = VecDeque::new();
        if let Some(text) = path.as_deref().and_then(|p| std::fs::read_to_string(p).ok()) {
            let lines: Vec<&str> = text.lines().collect();
            for line in &lines[lines.len().saturating_sub(KEEP)..] {
                if let Ok(entry) = serde_json::from_str(line) {
                    entries.push_back(entry);
                }
            }
        }
        Audit { path, entries: Mutex::new(entries), forward: std::sync::OnceLock::new() }
    }

    /// Bundan sonraki kayıtlar syslog sunucusuna da gönderilir.
    pub fn forward_to(&self, forwarder: crate::syslog::Forwarder) {
        let _ = self.forward.set(forwarder);
    }

    pub fn record(&self, ts: u64, user: &str, ip: &str, action: &str, ok: bool) {
        let entry = AuditEntry { ts, user: user.to_string(), ip: ip.to_string(), action: action.to_string(), ok };
        if let Some(path) = &self.path {
            if let Err(e) = append(path, &entry) {
                eprintln!("uyarı: denetim kaydı {} dosyasına yazılamadı: {e}", path.display());
            }
        }
        if let Some(forwarder) = self.forward.get() {
            forwarder.push(&entry);
        }
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.push_back(entry);
        while entries.len() > KEEP {
            entries.pop_front();
        }
    }

    /// En yeniden eskiye, `query` (kullanıcı, adres ya da eylemde geçen metin) ile süzülmüş kayıtlar.
    pub fn recent(&self, query: &str, limit: usize) -> Vec<AuditEntry> {
        let q = query.trim().to_lowercase();
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .rev()
            .filter(|e| q.is_empty() || e.user.to_lowercase().contains(&q) || e.ip.contains(&q) || e.action.to_lowercase().contains(&q))
            .take(limit)
            .cloned()
            .collect()
    }
}

fn append(path: &Path, entry: &AuditEntry) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    options.open(path)?.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_filter_and_stay_bounded() {
        let a = Audit::new(None);
        a.record(1, "admin", "10.0.0.1", "giriş", true);
        a.record(2, "ayse", "10.0.0.2", "POST /guests/103/action", true);
        a.record(3, "-", "10.0.0.9", "giriş: admin", false);
        assert_eq!(a.recent("", 10).iter().map(|e| e.ts).collect::<Vec<_>>(), [3, 2, 1], "en yeni başta");
        assert_eq!(a.recent("AYSE", 10).len(), 1);
        assert!(!a.recent("10.0.0.9", 10)[0].ok);
        assert_eq!(a.recent("giriş", 10).len(), 2);
        assert_eq!(a.recent("", 2).len(), 2);
        assert!(a.recent("yok", 10).is_empty());

        for i in 0..KEEP as u64 + 50 {
            a.record(100 + i, "u", "ip", "x", true);
        }
        assert_eq!(a.recent("", usize::MAX).len(), KEEP);
    }

    #[test]
    fn entries_survive_restart_and_bad_lines_are_skipped() {
        let dir = std::env::temp_dir().join(format!("redox-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.log");
        let a = Audit::new(Some(path.clone()));
        a.record(1, "admin", "ip", "giriş", true);
        a.record(2, "admin", "ip", "POST /users", false);
        drop(a);

        let mut text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        text.push_str("bozuk satır\n");
        std::fs::write(&path, text).unwrap();
        let b = Audit::new(Some(path.clone()));
        let loaded = b.recent("", 10);
        assert_eq!(loaded.len(), 2);
        assert_eq!((loaded[0].action.as_str(), loaded[0].ok), ("POST /users", false));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::remove_file(&path).unwrap();
            Audit::new(Some(path.clone())).record(1, "u", "ip", "x", true);
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
