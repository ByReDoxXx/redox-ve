//! Bakım kipi: sunucuyu güncellemek ya da yeniden başlatmak için konukları düzenli kapatıp sonra geri açma.
//!
//! Açıkken hiçbir konuk başlatılmaz (elle, açılışta, gözcüyle ya da zamanlamayla) ve zamanlanmış yedekler
//! alınmaz. Durum dosyada saklanır: sunucu bakım sırasında yeniden başlasa da konuklar kendiliğinden açılmaz.

use super::*;

impl AppState {
    pub fn in_maintenance(&self) -> bool {
        self.lock().maintenance
    }

    /// Bakıma alır. `drain`: çalışan konuklar açılış sırasının tersine kapatılır ve bakım bitince yeniden
    /// başlatılmak üzere kaydedilir. Kapatılmaya başlanan konuk sayısını döndürür.
    pub fn maintenance_on(&self, drain: bool, user: &str) -> usize {
        let plan = self.mutate(|c| {
            c.maintenance = true;
            let plan = if drain { c.maintenance_drain_plan() } else { Vec::new() };
            for vmid in &plan {
                if !c.maintenance_resume.contains(vmid) {
                    c.maintenance_resume.push(*vmid);
                }
            }
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("maintenance on{}", if drain { " --drain" } else { "" }), &Ok(()), now(), None);
            plan
        });
        let mut stopped = 0;
        for vmid in plan {
            match self.run_action(vmid, Action::Stop, user) {
                Ok(_) => stopped += 1,
                Err(e) => {
                    // Kapatılamayan konuk bakım bitince "yeniden başlatılacaklar" arasında sayılmaz.
                    eprintln!("uyarı: bakım için {vmid} kapatılamadı: {e}");
                    self.mutate(|c| c.maintenance_resume.retain(|v| *v != vmid));
                }
            }
        }
        stopped
    }

    /// Bakımı bitirir ve bakıma girerken kapatılan konukları açılış sırasıyla yeniden başlatır; başlatılan
    /// konuk sayısını döndürür.
    pub fn maintenance_off(&self, user: &str) -> usize {
        let back = self.mutate(|c| {
            let back = c.maintenance_finish();
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, "maintenance off".to_string(), &Ok(()), now(), None);
            back
        });
        let mut started = 0;
        for vmid in back {
            match self.run_action(vmid, Action::Start, user) {
                Ok(_) => started += 1,
                Err(e) => eprintln!("uyarı: bakım sonrası {vmid} başlatılamadı: {e}"),
            }
        }
        started
    }
}
