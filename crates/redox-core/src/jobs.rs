//! Yedek işleri: birden çok konuğu tek bir zamanlamayla, sırayla yedekler.
//!
//! Konuk başına zamanlamadan (`backup_time`) farkı: iş, etiketle seçilen bütün konukları kapsar, haftanın
//! günlerine göre çalışır ve konukları **tek tek** yedekler (aynı anda en çok bir yedek sürer; diskler
//! birbiriyle yarışmaz). Saklama sayısı işe aittir ve yalnızca o işin aldığı yedeklere uygulanır.

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;
use crate::hw::{days_label, EVERY_DAY};
use crate::model::{parse_time, valid_tag, VmId, DEFAULT_KEEP, MAX_KEEP, MAX_TAGS};

pub const MAX_JOBS: usize = 16;

fn every_day() -> u8 {
    EVERY_DAY
}

fn default_keep() -> u32 {
    DEFAULT_KEEP
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupJob {
    /// Küçük harf, rakam ve tire; işin kimliğidir.
    pub name: String,
    /// `SS:DD`, sunucu saatiyle.
    pub time: String,
    /// Günler: bit 0 Pazartesi … bit 6 Pazar.
    #[serde(default = "every_day")]
    pub days: u8,
    /// Bu etiketlerden en az birini taşıyan konuklar; boşsa bütün konuklar. Şablonlar hiçbir zaman dahil değildir.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Konuk başına saklanacak, bu işin aldığı yedek sayısı.
    #[serde(default = "default_keep")]
    pub keep: u32,
    /// "Son N"e ek olarak saklanacak haftalık, aylık ve yıllık yedekler.
    #[serde(default)]
    pub gfs: crate::retention::Gfs,
    /// Bu işin aldığı yedekler kaç gün silinemez olsun; 0 kilitsiz.
    #[serde(default)]
    pub lock_days: u32,
    /// Yedek penceresinin sonu (`SS:DD`): bu saatten sonra sıradaki konukların yedeğine başlanmaz. Boşsa sınırsız.
    #[serde(default)]
    pub until: Option<String>,
    /// Her yedek alındıktan sonra bozulma denetimi (doğrulama) yapılsın mı?
    #[serde(default)]
    pub verify: bool,
    /// Bu tur elle başlatıldı: yedek penceresi uygulanmaz.
    #[serde(default)]
    pub manual: bool,
    /// Doğrulanmayı bekleyen, bu turda alınmış yedekler.
    #[serde(default)]
    pub verify_pending: Vec<String>,
    /// Son başlatıldığı gün (`YYYY-AA-GG`); aynı gün kendiliğinden ikinci kez başlamaz.
    #[serde(default)]
    pub last_run: Option<String>,
    /// Bu turda sırası gelmemiş konuklar.
    #[serde(default)]
    pub queue: Vec<VmId>,
    /// Yedeği şu an süren konuk; bitince sıradaki başlar.
    #[serde(default)]
    pub running: Option<VmId>,
}

impl BackupJob {
    pub fn new(name: &str, time: &str, days: u8, tags: Vec<String>, keep: u32) -> Self {
        BackupJob { name: name.to_string(), time: time.to_string(), days, tags, keep, gfs: Default::default(), lock_days: 0, until: None, verify: false, manual: false, verify_pending: Vec::new(), last_run: None, queue: Vec::new(), running: None }
    }

    /// Form, API ve komut satırının ortak girişi: günler `1-5` biçiminde (boşsa her gün), etiketler virgülle.
    pub fn from_text(name: &str, time: &str, days: &str, tags: &str, keep: Option<u32>) -> Result<Self, Error> {
        let days = match days.trim() {
            "" => EVERY_DAY,
            text => crate::hw::parse_days(text).ok_or_else(|| Error::InvalidSpec("günler 1-7 arasında olmalı (1 Pazartesi; örn. 1-5)".to_string()))?,
        };
        let tags = tags.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect();
        let job = BackupJob::new(name.trim(), time.trim(), days, tags, keep.unwrap_or(DEFAULT_KEEP));
        job.check()?;
        Ok(job)
    }

    /// Haftalık/aylık/yıllık saklama ve kilit süresi ekler.
    pub fn with_retention(mut self, gfs: crate::retention::Gfs, lock_days: u32) -> Result<Self, Error> {
        (self.gfs, self.lock_days) = (gfs, lock_days);
        self.check()?;
        Ok(self)
    }

    /// Yedek penceresinin sonunu ve yedek sonrası doğrulamayı ayarlar. `until` boşsa pencere yoktur.
    pub fn with_window(mut self, until: &str, verify: bool) -> Result<Self, Error> {
        self.until = Some(until.trim().to_string()).filter(|u| !u.is_empty());
        self.verify = verify;
        self.check()?;
        Ok(self)
    }

    /// Yedek penceresi şu an kapalı mı? Pencere, işin saatinde açılır ve `until` saatinde kapanır;
    /// kapanış saati başlangıçtan küçükse pencere gece yarısını aşar (22:00 → 06:00).
    pub fn window_closed(&self, minute_of_day: u16) -> bool {
        let (Some(open), Some(close)) = (parse_time(&self.time), self.until.as_deref().and_then(parse_time)) else { return false };
        if self.manual {
            return false;
        }
        if open <= close { minute_of_day < open || minute_of_day >= close } else { minute_of_day >= close && minute_of_day < open }
    }

    /// Saklamanın okunur hali: `son 7`, `son 7 · 4H 12A`, `son 7 · kilit 14 gün`.
    pub fn retention_label(&self) -> String {
        let mut parts = vec![format!("son {}", self.keep)];
        if !self.gfs.is_off() {
            parts.push(self.gfs.label());
        }
        if self.lock_days > 0 {
            parts.push(format!("kilit {} gün", self.lock_days));
        }
        parts.join(" · ")
    }

    pub fn days_label(&self) -> String {
        days_label(self.days)
    }

    /// Seçimin okunur hali: `bütün konuklar` ya da `etiket: web, db`.
    pub fn selection_label(&self) -> String {
        match self.tags.is_empty() {
            true => "bütün konuklar".to_string(),
            false => format!("etiket: {}", self.tags.join(", ")),
        }
    }

    /// Tur sürüyor mu (sırada ya da çalışan konuk var mı)?
    pub fn active(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }

    fn check(&self) -> Result<(), Error> {
        let bad = |why: &str| Err(Error::InvalidSpec(why.to_string()));
        if !valid_tag(&self.name) {
            return bad("iş adı küçük harf, rakam ve tireden oluşmalı (en fazla 24 karakter)");
        }
        if parse_time(&self.time).is_none() {
            return bad("saat SS:DD biçiminde olmalı (örn. 03:30)");
        }
        if self.days == 0 || self.days > EVERY_DAY {
            return bad("günler 1-7 arasında olmalı (1 Pazartesi; örn. 1-5)");
        }
        if self.tags.len() > MAX_TAGS || !self.tags.iter().all(|t| valid_tag(t)) {
            return bad("etiketler küçük harf, rakam ve tireden oluşmalı (en fazla 8 etiket)");
        }
        if self.keep == 0 || self.keep > MAX_KEEP {
            return Err(Error::InvalidSpec(format!("saklanacak yedek sayısı 1 ile {MAX_KEEP} arasında olmalı")));
        }
        if !self.gfs.valid() {
            return Err(Error::InvalidSpec(format!("haftalık, aylık ve yıllık yedek sayıları en fazla {} olabilir", crate::retention::MAX_GFS)));
        }
        if let Some(until) = &self.until {
            if parse_time(until).is_none() {
                return Err(Error::InvalidSpec("yedek penceresinin sonu SS:DD biçiminde olmalı (örn. 06:00)".to_string()));
            }
            if parse_time(until) == parse_time(&self.time) {
                return Err(Error::InvalidSpec("yedek penceresinin sonu başlangıç saatinden farklı olmalı".to_string()));
            }
        }
        if self.lock_days > crate::retention::MAX_LOCK_DAYS {
            return Err(Error::InvalidSpec(format!("kilit süresi en fazla {} gün olabilir", crate::retention::MAX_LOCK_DAYS)));
        }
        Ok(())
    }
}

/// `step_jobs` çıktısı: zamanlayıcının yapması gereken tek adım.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStep {
    pub job: String,
    pub vmid: VmId,
}

impl Cluster {
    pub fn job(&self, name: &str) -> Result<&BackupJob, Error> {
        self.jobs.iter().find(|j| j.name == name).ok_or_else(|| Error::InvalidSpec(format!("{name} adında bir yedek işi yok")))
    }

    /// İşi ekler; aynı adda iş varsa zamanlamasını ve seçimini günceller (süren tur etkilenmez).
    pub fn put_job(&mut self, job: BackupJob) -> Result<(), Error> {
        job.check()?;
        let full = self.jobs.len() >= MAX_JOBS;
        match self.jobs.iter_mut().find(|j| j.name == job.name) {
            Some(old) => {
                // Saat ya da gün değişince bugünkü tur yeniden değerlendirilsin diye işaret korunur:
                // bugün zaten çalıştıysa yeni saatte ikinci kez çalışmaz.
                (old.time, old.days, old.tags, old.keep, old.gfs, old.lock_days) = (job.time, job.days, job.tags, job.keep, job.gfs, job.lock_days);
                (old.until, old.verify) = (job.until, job.verify);
            }
            None if full => return Err(Error::InvalidSpec(format!("en fazla {MAX_JOBS} yedek işi tanımlanabilir"))),
            None => {
                self.jobs.push(BackupJob { last_run: None, queue: Vec::new(), running: None, manual: false, verify_pending: Vec::new(), ..job });
                self.jobs.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
        Ok(())
    }

    /// İşi siler. Aldığı yedekler kalır; artık hiçbir işe ait olmadıkları için kendiliğinden silinmezler.
    pub fn remove_job(&mut self, name: &str) -> Result<(), Error> {
        self.job(name)?;
        self.jobs.retain(|j| j.name != name);
        Ok(())
    }

    /// İşin şu an kapsadığı konuklar (kimlik sırasıyla).
    pub fn job_guests(&self, job: &BackupJob) -> Vec<VmId> {
        self.guests
            .iter()
            .filter(|g| !g.hw.template && (job.tags.is_empty() || g.tags.iter().any(|t| job.tags.contains(t))))
            .map(|g| g.vmid)
            .collect()
    }

    /// Turu başlatır: kapsamdaki konukları sıraya koyar. Tur zaten sürüyorsa hata verir.
    /// `today` verilirse iş bugün çalıştı olarak işaretlenir (zamanlayıcı); elle başlatmada `None`.
    pub fn start_job(&mut self, name: &str, today: Option<&str>) -> Result<usize, Error> {
        // Son konuğun yedeği bitmiş olabilir: zamanlayıcının bir sonraki turunu beklemeden düşülür.
        self.settle_jobs();
        let job = self.job(name)?;
        if job.active() {
            return Err(Error::InvalidSpec(format!("{name} işinin önceki turu henüz bitmedi")));
        }
        let queue = self.job_guests(job);
        let job = self.jobs.iter_mut().find(|j| j.name == name).expect("az önce bulundu");
        if let Some(today) = today {
            job.last_run = Some(today.to_string());
        }
        // Elle başlatılan tur yedek penceresine bakmaz.
        job.manual = today.is_none();
        job.queue = queue;
        Ok(job.queue.len())
    }

    /// Saati gelmiş, bugün henüz çalışmamış ve turu sürmeyen işleri başlatır; başlatılanların adlarını döndürür.
    /// Sunucu o saatte kapalıysa iş gün bitmeden, açılınca çalışır. `weekday`: 0 Pazartesi … 6 Pazar.
    pub fn start_due_jobs(&mut self, today: &str, weekday: u8, minute_of_day: u16) -> Vec<String> {
        self.settle_jobs();
        let due: Vec<String> = self
            .jobs
            .iter()
            .filter(|j| !j.active() && j.last_run.as_deref() != Some(today))
            .filter(|j| weekday < 7 && j.days & (1 << weekday) != 0)
            .filter(|j| parse_time(&j.time).is_some_and(|at| at <= minute_of_day))
            .map(|j| j.name.clone())
            .collect();
        for name in &due {
            let _ = self.start_job(name, Some(today));
        }
        due
    }

    /// Süren turları bir adım ilerletir: yedeği başlatılacak konukları döndürür (iş başına en çok bir tane).
    /// Bir işin konuğu hâlâ yedekleniyorsa (kilitliyse) o iş bekler. Sıradaki konuk silinmişse atlanır;
    /// başka bir işlemle kilitliyse sırası gelene kadar beklenir. Bütün işler arasında aynı anda en çok
    /// bir yedek başlatılır.
    pub fn step_jobs(&mut self) -> Option<JobStep> {
        self.settle_jobs();
        let locked: Vec<VmId> = self.guests.iter().filter(|g| g.lock.is_some()).map(|g| g.vmid).collect();
        let known: Vec<VmId> = self.guests.iter().map(|g| g.vmid).collect();
        if self.jobs.iter().any(|j| j.running.is_some()) {
            return None;
        }
        for job in &mut self.jobs {
            while let Some(&vmid) = job.queue.first() {
                if !known.contains(&vmid) {
                    job.queue.remove(0);
                    continue;
                }
                if locked.contains(&vmid) {
                    break;
                }
                job.queue.remove(0);
                job.running = Some(vmid);
                return Some(JobStep { job: job.name.clone(), vmid });
            }
        }
        None
    }

    /// Yedek penceresi kapanmış işlerin sırasını boşaltır; `(iş, yedeklenmeden kalan konuk sayısı)` döndürür.
    /// Süren yedek yarıda kesilmez; yalnızca sıradakilere başlanmaz.
    pub fn close_job_windows(&mut self, minute_of_day: u16) -> Vec<(String, usize)> {
        let mut closed = Vec::new();
        for job in &mut self.jobs {
            if !job.queue.is_empty() && job.window_closed(minute_of_day) {
                closed.push((job.name.clone(), job.queue.len()));
                job.queue.clear();
            }
        }
        closed
    }

    /// İşin bu turda aldığı bir yedeği doğrulama sırasına koyar (iş doğrulama istiyorsa).
    pub fn job_backup_taken(&mut self, name: &str, backup: &str) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.name == name && j.verify) {
            job.verify_pending.push(backup.to_string());
        }
    }

    /// Doğrulanmaya hazır yedekler: alınması bitmiş (kaydı oluşmuş) olanlar. Kaydı oluşmayan (alınamayan)
    /// yedekler, konuğunun işi bittiyse sıradan düşer.
    pub fn take_verifiable(&mut self) -> Vec<String> {
        let known: Vec<String> = self.backups.iter().map(|b| b.id.clone()).collect();
        let mut ready = Vec::new();
        for job in &mut self.jobs {
            let busy = job.running.is_some();
            let (done, rest): (Vec<String>, Vec<String>) = std::mem::take(&mut job.verify_pending).into_iter().partition(|id| known.contains(id));
            ready.extend(done);
            // Kaydı henüz oluşmamış yedek, yalnızca işin bir yedeği sürerken beklenir.
            job.verify_pending = if busy { rest } else { Vec::new() };
        }
        ready
    }

    /// Yedeği bitmiş (kilidi kalkmış ya da silinmiş) konukları işlerin "çalışan" kaydından düşürür.
    fn settle_jobs(&mut self) {
        let locked: Vec<VmId> = self.guests.iter().filter(|g| g.lock.is_some()).map(|g| g.vmid).collect();
        for job in &mut self.jobs {
            if job.running.is_some_and(|v| !locked.contains(&v)) {
                job.running = None;
            }
            // Tur bitti: elle başlatma işareti bir sonraki zamanlanmış tura taşınmaz.
            if job.running.is_none() && job.queue.is_empty() {
                job.manual = false;
            }
        }
    }

    /// Yedeği başlatılamayan konuğu işin "çalışan" kaydından düşürür (sıradaki beklemesin).
    pub fn job_step_failed(&mut self, name: &str) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.name == name) {
            job.running = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: &str = "2026-11-25";

    fn cluster() -> Cluster {
        Cluster::demo(1_800_000_000)
    }

    fn job(name: &str, time: &str, tags: &[&str]) -> BackupJob {
        BackupJob::new(name, time, EVERY_DAY, tags.iter().map(|t| t.to_string()).collect(), 3)
    }

    #[test]
    fn jobs_are_validated_added_updated_and_removed() {
        let mut c = cluster();
        for bad in [job("Gece", "03:00", &[]), job("gece", "3:00", &[]), job("gece", "03:00", &["Büyük"]), BackupJob { days: 0, ..job("gece", "03:00", &[]) }, BackupJob { keep: 0, ..job("gece", "03:00", &[]) }] {
            assert!(c.put_job(bad.clone()).is_err(), "{bad:?}");
        }
        c.put_job(job("gece", "03:00", &[])).unwrap();
        c.put_job(job("ara", "12:00", &["web"])).unwrap();
        assert_eq!(c.jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["ara", "gece"], "ada göre sıralı");
        // Aynı adla yazmak günceller; çalışma kaydı korunur.
        c.start_due_jobs(DAY, 2, 13 * 60);
        c.put_job(BackupJob { keep: 9, ..job("ara", "14:00", &["db"]) }).unwrap();
        let ara = c.job("ara").unwrap();
        assert_eq!((ara.time.as_str(), ara.keep, ara.tags.as_slice(), ara.last_run.as_deref()), ("14:00", 9, &["db".to_string()][..], Some(DAY)));
        assert_eq!(c.jobs.len(), 2);
        c.remove_job("ara").unwrap();
        assert!(c.remove_job("ara").is_err() && c.jobs.len() == 1);
        for i in 0..MAX_JOBS - 1 {
            c.put_job(job(&format!("is-{i}"), "01:00", &[])).unwrap();
        }
        assert!(c.put_job(job("fazla", "01:00", &[])).unwrap_err().to_string().contains("en fazla"));
        assert_eq!((job("a", "01:00", &[]).selection_label(), job("a", "01:00", &["web", "db"]).selection_label()), ("bütün konuklar".to_string(), "etiket: web, db".to_string()));
    }

    #[test]
    fn jobs_are_read_from_text() {
        let j = BackupJob::from_text(" gece ", "03:30", "1-5", "web, db,", Some(4)).unwrap();
        assert_eq!((j.name.as_str(), j.days, j.tags.len(), j.keep, j.days_label().as_str()), ("gece", 0b001_1111, 2, 4, "hafta içi"));
        let all = BackupJob::from_text("hepsi", "23:59", "", "", None).unwrap();
        assert_eq!((all.days, all.tags.is_empty(), all.keep), (EVERY_DAY, true, DEFAULT_KEEP));
        for (days, tags) in [("8", ""), ("5-1", ""), ("", "Büyük Harf")] {
            assert!(BackupJob::from_text("x", "03:30", days, tags, None).is_err(), "{days} {tags}");
        }
    }

    #[test]
    fn retention_extras_are_validated_and_described() {
        use crate::retention::Gfs;
        let plain = job("gece", "03:00", &[]);
        assert_eq!(plain.retention_label(), "son 3");
        let rich = plain.clone().with_retention(Gfs { weekly: 4, monthly: 12, yearly: 0 }, 14).unwrap();
        assert_eq!(rich.retention_label(), "son 3 · 4H 12A · kilit 14 gün");
        assert!(plain.clone().with_retention(Gfs { weekly: 9999, ..Default::default() }, 0).is_err());
        assert!(plain.clone().with_retention(Gfs::default(), 99_999).is_err());
        // Güncelleme saklama kurallarını da değiştirir.
        let mut c = cluster();
        c.put_job(rich.clone()).unwrap();
        c.put_job(plain).unwrap();
        assert_eq!((c.job("gece").unwrap().gfs, c.job("gece").unwrap().lock_days), (Gfs::default(), 0));
    }

    #[test]
    fn the_backup_window_stops_new_backups_but_not_manual_runs() {
        let day = job("gunduz", "09:00", &[]).with_window("17:00", false).unwrap();
        assert!(day.window_closed(8 * 60) && !day.window_closed(9 * 60) && !day.window_closed(16 * 60 + 59) && day.window_closed(17 * 60) && day.window_closed(23 * 60));
        let night = job("gece", "22:00", &[]).with_window("06:00", true).unwrap();
        assert!(!night.window_closed(22 * 60) && !night.window_closed(23 * 60 + 59) && !night.window_closed(0) && !night.window_closed(5 * 60 + 59), "pencere gece yarısını aşar");
        assert!(night.window_closed(6 * 60) && night.window_closed(12 * 60) && night.window_closed(21 * 60 + 59));
        assert!(!job("sinirsiz", "03:00", &[]).window_closed(12 * 60), "pencere tanımlı değilse hep açıktır");
        assert!(job("x", "03:00", &[]).with_window("25:00", false).is_err() && job("x", "03:00", &[]).with_window("03:00", false).is_err());
        assert_eq!(job("x", "03:00", &[]).with_window("", true).unwrap().until, None);

        let mut c = cluster();
        c.put_job(night).unwrap();
        c.start_due_jobs(DAY, 2, 22 * 60);
        let n = c.guests.len();
        assert!(c.close_job_windows(23 * 60).is_empty(), "pencere açıkken sıra durur");
        assert_eq!(c.close_job_windows(6 * 60), [("gece".to_string(), n)], "pencere kapanınca kalan konuklar yedeklenmez");
        assert!(c.job("gece").unwrap().queue.is_empty() && c.close_job_windows(7 * 60).is_empty());
        // Elle başlatılan tur pencereye bakmaz; tur bitince işaret kalkar.
        c.start_job("gece", None).unwrap();
        assert!(c.close_job_windows(12 * 60).is_empty() && c.job("gece").unwrap().manual);
        c.jobs[0].queue.clear();
        c.step_jobs();
        assert!(!c.job("gece").unwrap().manual);
    }

    #[test]
    fn job_backups_wait_for_verification_until_they_exist() {
        let mut c = cluster();
        c.put_job(job("dogrulu", "03:00", &[]).with_window("", true).unwrap()).unwrap();
        c.put_job(job("duz", "03:00", &[])).unwrap();
        c.job_backup_taken("duz", "vm-1-1");
        assert!(c.job("duz").unwrap().verify_pending.is_empty(), "doğrulama istemeyen iş sıraya koymaz");
        c.job_backup_taken("dogrulu", "vm-100-1");
        c.job_backup_taken("dogrulu", "vm-101-1");
        c.jobs.iter_mut().find(|j| j.name == "dogrulu").unwrap().running = Some(101);
        assert!(c.take_verifiable().is_empty(), "kaydı oluşmamış yedek beklenir");
        c.add_backup(crate::model::Backup { id: "vm-100-1".into(), vmid: 100, guest_name: "x".into(), created: 1, size_mb: 1, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: Some("dogrulu".into()), machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
        assert_eq!(c.take_verifiable(), ["vm-100-1"]);
        assert_eq!(c.job("dogrulu").unwrap().verify_pending, ["vm-101-1"]);
        // İşin süren yedeği kalmayınca kaydı oluşmayan (alınamayan) yedek sıradan düşer.
        c.jobs.iter_mut().find(|j| j.name == "dogrulu").unwrap().running = None;
        assert!(c.take_verifiable().is_empty() && c.job("dogrulu").unwrap().verify_pending.is_empty());
    }

    #[test]
    fn selection_follows_tags_and_skips_templates() {
        let mut c = cluster();
        let all: Vec<VmId> = c.guests.iter().map(|g| g.vmid).collect();
        assert_eq!(c.job_guests(&job("hepsi", "03:00", &[])), all);
        c.guests.iter_mut().find(|g| g.vmid == 100).unwrap().tags = vec!["web".into()];
        c.guests.iter_mut().find(|g| g.vmid == 102).unwrap().tags = vec!["db".into(), "prod".into()];
        c.guests.iter_mut().find(|g| g.vmid == 103).unwrap().tags = vec!["web".into()];
        assert_eq!(c.job_guests(&job("web", "03:00", &["web"])), [100, 103]);
        assert_eq!(c.job_guests(&job("iki", "03:00", &["web", "prod"])), [100, 102, 103]);
        c.guests.iter_mut().find(|g| g.vmid == 103).unwrap().hw.template = true;
        assert_eq!(c.job_guests(&job("web", "03:00", &["web"])), [100], "şablon yedeklenmez");
        assert!(c.job_guests(&job("yok", "03:00", &["olmayan"])).is_empty());
    }

    #[test]
    fn due_jobs_start_once_a_day_on_their_days() {
        let mut c = cluster();
        c.put_job(BackupJob { days: 0b001_1111, ..job("hafta-ici", "03:30", &[]) }).unwrap();
        assert!(c.start_due_jobs(DAY, 2, 209).is_empty(), "saati gelmeden başlamaz");
        assert!(c.start_due_jobs(DAY, 5, 300).is_empty(), "cumartesi çalışmaz");
        assert_eq!(c.start_due_jobs(DAY, 2, 210), ["hafta-ici"]);
        let n = c.guests.len();
        assert_eq!(c.job("hafta-ici").unwrap().queue.len(), n);
        assert!(c.start_due_jobs(DAY, 2, 400).is_empty(), "tur sürerken yeniden başlamaz");
        c.jobs[0].queue.clear();
        assert!(c.start_due_jobs(DAY, 2, 400).is_empty(), "aynı gün ikinci kez başlamaz");
        assert_eq!(c.start_due_jobs("2026-11-26", 3, 1439), ["hafta-ici"], "ertesi gün; kaçan saat gün içinde telafi edilir");
        // Elle başlatma günü işaretlemez ve süren turun üstüne başlamaz.
        c.jobs[0].queue.clear();
        c.jobs[0].last_run = None;
        assert_eq!(c.start_job("hafta-ici", None).unwrap(), n);
        assert_eq!(c.job("hafta-ici").unwrap().last_run, None);
        assert!(c.start_job("hafta-ici", None).unwrap_err().to_string().contains("bitmedi"));
        assert!(c.start_job("yok", None).is_err());
    }

    #[test]
    fn guests_are_backed_up_one_at_a_time() {
        let mut c = cluster();
        c.put_job(job("a", "03:00", &[])).unwrap();
        c.put_job(job("b", "03:00", &[])).unwrap();
        c.jobs[0].queue = vec![100, 999, 101];
        c.jobs[1].queue = vec![102];
        let lock = |c: &mut Cluster, vmid: VmId, on: bool| c.guests.iter_mut().find(|g| g.vmid == vmid).unwrap().lock = on.then_some(1);

        assert_eq!(c.step_jobs(), Some(JobStep { job: "a".into(), vmid: 100 }));
        lock(&mut c, 100, true);
        assert_eq!(c.step_jobs(), None, "100 sürerken hiçbir işte yenisi başlamaz");
        lock(&mut c, 100, false);
        // 999 silinmiş bir konuk: atlanır, 101'e geçilir.
        assert_eq!(c.step_jobs(), Some(JobStep { job: "a".into(), vmid: 101 }));
        assert_eq!(c.job("a").unwrap().queue, Vec::<VmId>::new());
        // 101'in yedeği başlatılamadı (kilitlenmedi): kayıt düşer, sıra öbür işe geçer.
        c.job_step_failed("a");
        lock(&mut c, 102, true);
        assert_eq!(c.step_jobs(), None, "başka bir işlemle kilitli konuk beklenir");
        assert_eq!(c.job("b").unwrap().queue, [102]);
        lock(&mut c, 102, false);
        assert_eq!(c.step_jobs(), Some(JobStep { job: "b".into(), vmid: 102 }));
        assert!(c.job("b").unwrap().active());
        assert_eq!(c.step_jobs(), None);
        assert!(!c.job("b").unwrap().active(), "kilitlenmeyen (hemen biten) yedek sonraki turda düşer");
        // Son konuğun yedeği bitince iş, zamanlayıcı turu beklenmeden yeniden başlatılabilir.
        c.jobs[1].queue = vec![102];
        assert_eq!(c.step_jobs(), Some(JobStep { job: "b".into(), vmid: 102 }));
        lock(&mut c, 102, true);
        assert!(c.start_job("b", None).unwrap_err().to_string().contains("bitmedi"));
        lock(&mut c, 102, false);
        assert!(c.start_job("b", None).unwrap() > 0);
    }

    #[test]
    fn old_state_files_load_without_jobs() {
        let mut v = serde_json::to_value(cluster()).unwrap();
        v.as_object_mut().unwrap().remove("jobs");
        let c: Cluster = serde_json::from_value(v).unwrap();
        assert!(c.jobs.is_empty());
        let j: BackupJob = serde_json::from_str(r#"{"name":"gece","time":"03:00"}"#).unwrap();
        assert_eq!((j.days, j.keep, j.active()), (EVERY_DAY, DEFAULT_KEEP, false));
    }
}
