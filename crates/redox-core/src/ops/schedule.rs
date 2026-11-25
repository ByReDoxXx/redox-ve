//! Zamanlama: otomatik yedekler, güç zamanlamaları ve açılış sırası.

use super::*;

impl Cluster {
    /// Otomatik yedeği bugün henüz alınmamış ve saati gelmiş konuklar.
    /// Üzerinde işlem süren konuk bir sonraki denetimde yeniden değerlendirilir.
    pub fn auto_backups_due(&self, today: &str, minute_of_day: u16) -> Vec<VmId> {
        self.guests
            .iter()
            .filter(|g| g.lock.is_none() && g.last_auto_backup.as_deref() != Some(today))
            .filter(|g| g.backup_time.as_deref().and_then(parse_time).is_some_and(|at| at <= minute_of_day))
            .map(|g| g.vmid)
            .collect()
    }

    /// Saati gelen zamanlanmış güç işlemleri: `(konuk, satır, eylem)`. `weekday`: 0 Pazartesi … 6 Pazar.
    /// Saatinden en çok `SCHEDULE_GRACE_MIN` dakika sonra çalışır; sunucu o sırada kapalıysa o gün atlanır
    /// (kaçan bir "kapat" ile "başlat" saatler sonra art arda uygulanmasın). Kilitli konuk sonraki turu bekler.
    pub fn schedules_due(&self, today: &str, weekday: u8, minute_of_day: u16) -> Vec<(VmId, usize, Action)> {
        self.guests
            .iter()
            .filter(|g| g.lock.is_none() && !g.hw.template)
            .flat_map(|g| g.hw.schedules.iter().enumerate().map(move |(i, s)| (g.vmid, i, s)))
            .filter(|(_, _, s)| s.on_day(weekday) && s.last_run.as_deref() != Some(today))
            .filter(|(_, _, s)| parse_time(&s.time).is_some_and(|at| at <= minute_of_day && minute_of_day - at <= crate::hw::SCHEDULE_GRACE_MIN))
            .map(|(vmid, i, s)| (vmid, i, s.action))
            .collect()
    }

    /// Zamanlanmış işlemi bugün çalıştı olarak işaretler (başarısız olsa da gün içinde yinelenmez).
    pub fn mark_schedule(&mut self, vmid: VmId, index: usize, today: &str) {
        if let Some(s) = self.guest_mut(vmid).ok().and_then(|g| g.hw.schedules.get_mut(index)) {
            s.last_run = Some(today.to_string());
        }
    }

    /// Sunucu açılışında başlatılacak konuklar, sırasıyla: `(konuk, sonrakinden önce beklenecek saniye)`.
    /// Sıra numarası olanlar küçükten büyüğe önce, sırasızlar (0) kimlik sırasıyla sonra gelir.
    /// Zaten çalışan, kilitli ya da şablon olan konuklar listede yer almaz.
    pub fn boot_plan(&self) -> Vec<(VmId, u32)> {
        let mut due: Vec<&Guest> = self.guests.iter().filter(|g| g.onboot && !g.hw.template && g.lock.is_none() && g.status == GuestStatus::Stopped).collect();
        due.sort_by_key(|g| (g.hw.boot_order == 0, g.hw.boot_order, g.vmid));
        due.iter().map(|g| (g.vmid, g.hw.boot_delay)).collect()
    }

    /// Bugünün otomatik yedeği denendi olarak işaretler (başarısız olsa da gün içinde yinelenmez).
    pub fn mark_auto_backup(&mut self, vmid: VmId, today: &str) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.last_auto_backup = Some(today.to_string());
        }
    }
}
