//! Zamanlayıcılar: yedek zamanları ve işleri, güç zamanlamaları, açılışta başlatma, gözcü, uyarılar, havuzlar.

use super::*;

impl AppState {
    /// Uyarı turu: son turdan beri değişen koşulları ve olayları döndürür.
    pub fn alerts_tick(&self) -> Vec<crate::alerts::Event> {
        let snapshot = self.snapshot();
        self.0.alerts.update(&snapshot, now())
    }

    /// Zamanlayıcının bir turu: saati gelen otomatik yedekleri başlatır ve saklama sayısını
    /// aşan eski otomatik yedekleri siler. Başlatılan yedek sayısını döndürür.
    pub fn scheduler_tick(&self, today: &str, minute_of_day: u16) -> usize {
        let due = self.lock().auto_backups_due(today, minute_of_day);
        for vmid in &due {
            // Önce işaretle: başarısız olsa da aynı gün her turda yeniden denenmesin.
            self.mutate(|c| c.mark_auto_backup(*vmid, today));
            if let Err(e) = self.backup_create_as(*vmid, SCHEDULER, true, None) {
                self.mutate(|c| {
                    if let Ok(g) = c.guest(*vmid).cloned() {
                        c.log(&g.node, SCHEDULER, format!("{} backup {vmid}", g.kind), &Err(e.to_string()), now(), Some(*vmid));
                    }
                });
            }
        }
        // Kilit döngüden önce bırakılmalı: silme işlemi kümeyi yeniden kilitler.
        let stale = self.lock().prunable_backups(now());
        for id in stale {
            let _ = self.backup_delete(&id, SCHEDULER);
        }
        due.len()
    }

    /// Yedek işlerinin bir turu: saati gelen işleri başlatır ve süren turlarda sıradaki konuğun yedeğini
    /// başlatır (bütün işler arasında aynı anda en çok bir yedek). Başlatılan yedek sayısını döndürür.
    pub fn jobs_tick(&self, today: &str, weekday: u8, minute_of_day: u16) -> usize {
        self.mutate(|c| c.start_due_jobs(today, weekday, minute_of_day));
        // Yedek penceresi kapandıysa sıradaki konuklara başlanmaz; kaç konuğun kaldığı günlüğe yazılır.
        self.mutate(|c| {
            for (job, left) in c.close_job_windows(minute_of_day) {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, SCHEDULER, format!("job {job}"), &Err(format!("yedek penceresi kapandı: {left} konuk bu turda yedeklenmedi")), now(), None);
            }
        });
        // İşin aldığı ve kaydı oluşmuş yedekler doğrulanır (iş bunu istiyorsa).
        for id in self.mutate(|c| c.take_verifiable()) {
            let _ = self.backup_verify(&id, SCHEDULER);
        }
        let Some(step) = self.mutate(|c| c.step_jobs()) else { return 0 };
        match self.backup_create_as(step.vmid, SCHEDULER, true, Some(step.job.clone())) {
            Ok(id) => {
                self.mutate(|c| c.job_backup_taken(&step.job, &id));
                1
            }
            Err(e) => {
                // Yedeği alınamayan konuk (ör. çalışan konteyner) günlüğe yazılır; tur sıradakiyle sürer.
                self.mutate(|c| {
                    c.job_step_failed(&step.job);
                    if let Ok(g) = c.guest(step.vmid).cloned() {
                        c.log(&g.node, SCHEDULER, format!("{} backup {} (iş: {})", g.kind, step.vmid, step.job), &Err(e.to_string()), now(), Some(step.vmid));
                    }
                });
                0
            }
        }
    }

    /// Kaynak havuzunu ekler ya da kotalarını günceller.
    pub fn pool_put(&self, pool: redox_core::Pool) -> Result<(), Error> {
        self.mutate(|c| c.put_pool(pool))
    }

    pub fn pool_remove(&self, name: &str) -> Result<(), Error> {
        self.mutate(|c| c.remove_pool(name))
    }

    /// Yedek işini ekler ya da günceller.
    pub fn job_put(&self, job: redox_core::BackupJob) -> Result<(), Error> {
        self.mutate(|c| c.put_job(job))
    }

    pub fn job_remove(&self, name: &str) -> Result<(), Error> {
        self.mutate(|c| c.remove_job(name))
    }

    /// İşi şimdi başlatır (günlük işareti değişmez); sıraya giren konuk sayısını döndürür.
    pub fn job_run(&self, name: &str) -> Result<usize, Error> {
        self.mutate(|c| c.start_job(name, None))
    }

    /// Yeniden başlatma turu: "yeniden başlat" işaretli, beklenmedik biçimde kapanmış konukları açar.
    /// Başlatılan konuk sayısını döndürür. Vazgeçilen konuk görev günlüğüne hata olarak yazılır.
    pub fn ha_tick(&self) -> usize {
        let snapshot = self.snapshot();
        let decisions = self.0.ha.lock().unwrap_or_else(|e| e.into_inner()).update(&snapshot, now());
        let mut started = 0;
        for decision in decisions {
            let (vmid, outcome) = match decision {
                crate::ha::Decision::Restart(vmid) => match self.run_action(vmid, Action::Start, HA) {
                    Ok(_) => {
                        started += 1;
                        continue;
                    }
                    Err(e) => (vmid, Err(e.to_string())),
                },
                crate::ha::Decision::GiveUp(vmid) => (
                    vmid,
                    Err(format!("konuk {} dakika içinde {} kez kendiliğinden kapandı; yeniden başlatmaktan vazgeçildi", crate::ha::RESTART_WINDOW_SECS / 60, crate::ha::MAX_RESTARTS + 1)),
                ),
            };
            self.mutate(|c| {
                if let Ok(g) = c.guest(vmid).cloned() {
                    c.log(&g.node, HA, format!("{} start {vmid}", g.kind), &outcome, now(), Some(vmid));
                }
            });
        }
        started
    }

    /// Zamanlayıcının güç turu: saati gelen başlat, kapat ve yeniden başlat işlemlerini çalıştırır.
    /// Konuk zaten istenen durumdaysa işlem sessizce atlanır. Başlatılan görev sayısını döndürür.
    pub fn power_tick(&self, today: &str, weekday: u8, minute_of_day: u16) -> usize {
        let due = self.lock().schedules_due(today, weekday, minute_of_day);
        let mut started = 0;
        for (vmid, index, action) in due {
            // Önce işaretle: başarısız olsa da aynı gün her turda yeniden denenmesin.
            self.mutate(|c| c.mark_schedule(vmid, index, today));
            let Ok(guest) = self.lock().guest(vmid).cloned() else { continue };
            if action.target(guest.status).is_none() {
                continue;
            }
            match self.run_action(vmid, action, SCHEDULER) {
                Ok(_) => started += 1,
                Err(e) => self.mutate(|c| c.log(&guest.node, SCHEDULER, format!("{} {action} {vmid}", guest.kind), &Err(e.to_string()), now(), Some(vmid))),
            }
        }
        started
    }

    /// Sunucu açılışı: "açılışta başlat" işaretli, kapalı konukları sırasıyla başlatır. Her konuğun başlatma görevi
    /// bitince o konuğun gecikmesi kadar beklenir. `pause` beklemeyi yapar (testlerde kaydedilir).
    /// Başlatılan konuk sayısını döndürür; başlatılamayan konuk görev günlüğüne yazılır ve sıra sürer.
    pub fn autostart(&self, pause: &dyn Fn(Duration)) -> usize {
        let plan = self.lock().boot_plan();
        let mut started = 0;
        for (i, (vmid, delay)) in plan.iter().enumerate() {
            if let Err(e) = self.run_action(*vmid, Action::Start, BOOT) {
                self.mutate(|c| {
                    if let Ok(g) = c.guest(*vmid).cloned() {
                        c.log(&g.node, BOOT, format!("{} start {vmid}", g.kind), &Err(e.to_string()), now(), Some(*vmid));
                    }
                });
                continue;
            }
            // Başlatma görevi bitene kadar (en çok iki dakika) beklenir; gecikme ondan sonra sayılır.
            for _ in 0..600 {
                if self.lock().guest(*vmid).map(|g| g.lock.is_none()).unwrap_or(true) {
                    break;
                }
                pause(Duration::from_millis(200));
            }
            // Açılamayan konuk için beklenmez: görevi başarısız olarak günlükte durur.
            if !self.lock().guest(*vmid).is_ok_and(|g| g.status == redox_core::GuestStatus::Running) {
                continue;
            }
            started += 1;
            if *delay > 0 && i + 1 < plan.len() {
                pause(Duration::from_secs(u64::from(*delay)));
            }
        }
        started
    }
}
