//! Günlük yedek raporu: son 24 saatte alınan ve alınamayan yedekler, korumasız konuklar ve yedeklerin genel durumu.
//!
//! Rapor düz metindir; e-postayla gönderilir (`--backup-report SS:DD`) ve istenince API'den ya da komut satırından okunur.

use redox_core::retention::{date_label, DAY_SECS};
use redox_core::{Cluster, TaskStatus};

pub struct Report {
    pub subject: String,
    pub body: String,
    pub taken: usize,
    pub failed: usize,
    pub unprotected: usize,
}

/// Görev bir yedek alma görevi mi (`vm backup 103`, `ct backup 200 (iş: gece)`)?
fn is_backup_task(command: &str) -> bool {
    let mut words = command.split(' ');
    matches!((words.next(), words.next()), (Some("vm" | "ct"), Some("backup")))
}

/// `age_days`: bu kadar gündür yedeği olmayan konuk "korumasız" sayılır (0: bu bölüm rapora girmez).
pub fn compose(c: &Cluster, now: u64, age_days: u32) -> Report {
    let since = now.saturating_sub(DAY_SECS);
    let taken: Vec<_> = c.backups.iter().filter(|b| b.created >= since).collect();
    let failed: Vec<_> = c.tasks.iter().filter(|t| t.status == TaskStatus::Failed && t.started >= since && is_backup_task(&t.command)).collect();
    let unprotected = c.unprotected(now, age_days);
    let suspicious: Vec<_> = c.backups.iter().filter(|b| b.suspicious.is_some() && b.created >= now.saturating_sub(7 * DAY_SECS)).collect();
    let bad_tests: Vec<_> = c.backups.iter().filter(|b| b.tested.as_ref().is_some_and(|t| !t.ok)).collect();

    let mut body = format!("{} — yedek raporu, {}\n\n", c.name, date_label(now));
    let total_mb: u64 = taken.iter().map(|b| b.new_mb.unwrap_or(b.size_mb)).sum();
    body.push_str(&format!("Son 24 saat: {} yedek alındı ({} MB), {} yedek alınamadı.\n", taken.len(), total_mb, failed.len()));
    for b in &taken {
        let mut notes = Vec::new();
        if b.auto {
            notes.push(b.job.as_ref().map(|j| format!("iş: {j}")).unwrap_or_else(|| "zamanlanmış".to_string()));
        }
        if b.offsite {
            notes.push("dış kopya var".to_string());
        }
        if b.new_mb.is_some() {
            notes.push("artımlı".to_string());
        }
        let notes = if notes.is_empty() { String::new() } else { format!("  [{}]", notes.join(", ")) };
        body.push_str(&format!("  + {} {}  {} MB{notes}\n", b.vmid, b.guest_name, b.new_mb.unwrap_or(b.size_mb)));
    }
    if !failed.is_empty() {
        body.push_str("\nAlınamayan yedekler:\n");
        for t in &failed {
            body.push_str(&format!("  ! redox {}: {}\n", t.command, t.message.as_deref().unwrap_or("neden bilinmiyor")));
        }
    }
    if !unprotected.is_empty() {
        body.push_str(&match age_days {
            0 => "\nKorumasız konuklar (yedek yaşı hedefini kaçıranlar):\n".to_string(),
            days => format!("\nKorumasız konuklar ({days} gündür ya da kendi hedefinden uzun süredir yedeği yok):\n"),
        });
        for (vmid, name, last) in &unprotected {
            match last {
                Some(ts) => body.push_str(&format!("  - {vmid} {name}: son yedek {} ({} gün önce)\n", date_label(*ts), now.saturating_sub(*ts) / DAY_SECS)),
                None => body.push_str(&format!("  - {vmid} {name}: hiç yedeği yok\n")),
            }
        }
    }
    if !suspicious.is_empty() {
        body.push_str("\nOlağan dışı değişim görülen yedekler (son 7 gün; fidye yazılımı belirtisi olabilir):\n");
        for b in &suspicious {
            body.push_str(&format!("  ! {} ({} {}): {}\n", b.id, b.vmid, b.guest_name, b.suspicious.as_deref().unwrap_or_default()));
        }
    }
    if !bad_tests.is_empty() {
        body.push_str("\nAçılarak sınanıp başarısız olan yedekler:\n");
        for b in &bad_tests {
            body.push_str(&format!("  ! {}: {}\n", b.id, b.tested.as_ref().map(|t| t.note.as_str()).unwrap_or_default()));
        }
    }
    let offsite = c.backups.iter().filter(|b| b.offsite).count();
    let locked = c.backups.iter().filter(|b| b.is_locked(now)).count();
    let tested = c.backups.iter().filter(|b| b.tested.as_ref().is_some_and(|t| t.ok)).count();
    body.push_str(&format!("\nToplam: {} yedek; {} tanesinin dış kopyası var, {} tanesi kilitli, {} tanesi açılarak sınandı.\n", c.backups.len(), offsite, locked, tested));

    let mut parts = vec![format!("{} başarılı", taken.len())];
    if !failed.is_empty() {
        parts.push(format!("{} başarısız", failed.len()));
    }
    if !unprotected.is_empty() {
        parts.push(format!("{} korumasız konuk", unprotected.len()));
    }
    if !suspicious.is_empty() {
        parts.push(format!("{} şüpheli yedek", suspicious.len()));
    }
    // Rapor, istek içinde isteğin, e-postada sunucunun varsayılan dilindedir.
    let parts: Vec<String> = parts.into_iter().map(crate::i18n::auto).collect();
    let body = crate::i18n::auto(body);
    Report { subject: crate::i18n::auto(format!("[{}] yedek raporu: {}", c.name, parts.join(", "))), body, taken: taken.len(), failed: failed.len(), unprotected: unprotected.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::{Backup, BackupTest};

    const NOW: u64 = 1_795_608_000;

    fn backup(id: &str, vmid: u32, age_hours: u64) -> Backup {
        Backup {
            id: id.into(), vmid, guest_name: format!("konuk-{vmid}"), created: NOW - age_hours * 3600, size_mb: 100, auto: false, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 10,
            quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0,
        }
    }

    #[test]
    fn report_summarizes_the_last_day_and_names_unprotected_guests() {
        let mut c = Cluster::demo(NOW);
        let guests = c.guests.len();
        let empty = compose(&c, NOW, 7);
        assert_eq!((empty.taken, empty.failed, empty.unprotected), (0, 0, guests));
        assert!(empty.subject.ends_with(&format!("yedek raporu: 0 başarılı, {guests} korumasız konuk")), "{}", empty.subject);
        assert!(empty.body.contains("hiç yedeği yok") && empty.body.contains("Toplam: 0 yedek"));
        assert_eq!(compose(&c, NOW, 0).unprotected, 0, "0 gün: korumasız konuk bölümü kapalı");

        c.add_backup(Backup { auto: true, job: Some("gece".into()), offsite: true, ..backup("vm-100-1", 100, 2) });
        c.add_backup(Backup { new_mb: Some(7), suspicious: None, locked_until: Some(NOW + 1000), ..backup("vm-101-1", 101, 5) });
        c.add_backup(backup("vm-102-eski", 102, 24 * 10));
        c.add_backup(Backup { suspicious: Some("değişen veri 9000 MB".into()), ..backup("vm-103-supheli", 103, 24 * 9) });
        c.add_backup(Backup { suspicious: Some("değişen veri 9000 MB".into()), ..backup("vm-103-yeni-supheli", 103, 24 * 4) });
        c.add_backup(Backup { tested: Some(BackupTest { at: NOW, ok: false, note: "konuk kendiliğinden kapandı".into() }), ..backup("vm-103-eski", 103, 24 * 3) });
        let node = c.nodes[0].id.clone();
        c.log(&node, "zamanlayıcı", "ct backup 200 (iş: gece)".to_string(), &Err("konuk çalışıyor".to_string()), NOW - 600, Some(200));
        c.log(&node, "admin", "vm start 103".to_string(), &Err("başka bir hata".to_string()), NOW - 600, Some(103));
        c.log(&node, "admin", "backup verify vm-100-1".to_string(), &Err("bozuk".to_string()), NOW - 600, None);
        let r = compose(&c, NOW, 7);
        assert_eq!((r.taken, r.failed), (2, 1), "yalnızca yedek alma görevleri sayılır");
        assert!(r.body.contains("Son 24 saat: 2 yedek alındı (107 MB), 1 yedek alınamadı."), "{}", r.body);
        assert!(r.body.contains("+ 100 konuk-100  100 MB  [iş: gece, dış kopya var]") && r.body.contains("+ 101 konuk-101  7 MB  [artımlı]"), "{}", r.body);
        assert!(r.body.contains("! redox ct backup 200 (iş: gece): konuk çalışıyor") && !r.body.contains("vm start 103"));
        // 102'nin son yedeği 10 gün önce: korumasız. 103'ünki 3 gün önce: korunuyor ama sınaması başarısız.
        assert!(r.body.contains("- 102 ") && r.body.contains("(10 gün önce)") && !r.body.contains("- 103 "), "{}", r.body);
        assert!(r.body.contains("! vm-103-eski: konuk kendiliğinden kapandı"));
        assert!(r.body.contains("! vm-103-yeni-supheli (103 konuk-103): değişen veri 9000 MB") && !r.body.contains("! vm-103-supheli"), "{}", r.body);
        assert!(r.subject.ends_with("1 şüpheli yedek"), "{}", r.subject);
        assert!(r.body.contains("Toplam: 6 yedek; 1 tanesinin dış kopyası var, 1 tanesi kilitli, 0 tanesi açılarak sınandı."));
        assert!(r.subject.contains("2 başarılı, 1 başarısız") && r.subject.contains("korumasız konuk"));
        assert!(is_backup_task("vm backup 103") && is_backup_task("ct backup 200 (iş: x)") && !is_backup_task("backup push vm-1") && !is_backup_task("vm restore 103 x"));
    }
}
