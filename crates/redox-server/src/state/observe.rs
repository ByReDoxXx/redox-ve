//! Gözlem: sürücüden ölçüm okuma, geçmiş ve ölçüm dosyası.

use super::*;

impl AppState {
    pub(super) fn record(&self, key: String, value: f64) {
        let mut history = self.0.history.lock().unwrap_or_else(|e| e.into_inner());
        let series = history.entry(key).or_default();
        series.push_back(value);
        while series.len() > HISTORY_LEN {
            series.pop_front();
        }
    }

    /// Uzun süreli ölçümlerin durduğu dosya: durum dosyasının yanında.
    pub(super) fn metrics_path(&self) -> Option<PathBuf> {
        self.0.state_file.as_ref().map(|p| p.with_file_name("metrics.json"))
    }

    /// Kayıtlı uzun süreli ölçümleri yükler (başlangıçta bir kez).
    pub fn load_metrics(&self) {
        if let Some(path) = self.metrics_path() {
            *self.0.long.lock().unwrap_or_else(|e| e.into_inner()) = crate::history::Long::load(&path);
        }
    }

    pub fn save_metrics(&self) {
        if let Some(path) = self.metrics_path() {
            if let Err(e) = self.0.long.lock().unwrap_or_else(|e| e.into_inner()).save(&path) {
                eprintln!("uyarı: {e}");
            }
        }
    }

    /// Seçilen aralığın ölçümleri: son 10 dakika ham örneklerden, daha uzunu ortalamalardan.
    pub fn history_for(&self, range: crate::history::Range) -> History {
        match range {
            crate::history::Range::Recent => self.history(),
            other => self.0.long.lock().unwrap_or_else(|e| e.into_inner()).series(other),
        }
    }

    /// Ölçüm turu: `refresh` gerçek saatle bunu çağırır; testler zamanı kendileri verir.
    pub fn record_long(&self, samples: &[(String, f64)], at: u64) {
        let save = {
            let mut long = self.0.long.lock().unwrap_or_else(|e| e.into_inner());
            let rolled = long.roll(at);
            for (key, value) in samples {
                long.push(key, *value);
            }
            let live: Vec<String> = samples.iter().map(|(k, _)| k.clone()).collect();
            long.retain(&live);
            // On dakikada bir diske yaz: her dakika yazmak diski boşuna yıpratır.
            rolled && (at / 60).is_multiple_of(10)
        };
        if save {
            self.save_metrics();
        }
    }

    pub fn history(&self) -> History {
        let history = self.0.history.lock().unwrap_or_else(|e| e.into_inner());
        history.iter().map(|(k, v)| (k.clone(), v.iter().copied().collect())).collect()
    }

    /// Çalışan ve ajanı yanıt veren sanal makinelerin dosya sistemi doluluklarını okur (dakikada bir çağrılır).
    /// Bloklayan G/Ç yapar. Okunan konuk sayısını döndürür.
    pub fn guest_fs_tick(&self) -> usize {
        let driver = self.driver();
        let guests = self.snapshot().guests;
        let seen: Vec<_> = guests
            .iter()
            .map(|g| {
                let live = g.kind == redox_core::GuestKind::Vm && g.status == redox_core::GuestStatus::Running && g.agent.is_some();
                (g.vmid, if live { driver.guest_filesystems(g) } else { None })
            })
            .collect();
        let read = seen.iter().filter(|(_, list)| list.is_some()).count();
        self.mutate(|c| seen.into_iter().for_each(|(vmid, list)| c.observe_filesystems(vmid, list)));
        read
    }

    /// Gerçek sürücüden konukların, düğümün ve depolamanın güncel durumunu okur.
    /// Bloklayan G/Ç yapar; `spawn_blocking` içinden çağrılmalıdır.
    pub fn refresh(&self, host: Option<&HostSample>, disk_gb: Option<(u64, u64)>) {
        self.reconcile_pending_unplugs();
        let driver = self.driver();
        if !driver.is_real() {
            return;
        }
        let snapshot = self.snapshot();
        let probes: Vec<_> = snapshot.guests.iter().filter_map(|g| driver.probe(g).map(|p| (g.vmid, p))).collect();
        // Adres yalnızca çalışan konukta anlamlıdır; duran konuğun eski adresi silinir.
        let ips: Vec<_> = probes
            .iter()
            .zip(&snapshot.guests)
            .map(|((vmid, p), g)| (*vmid, if p.status == redox_core::GuestStatus::Stopped { None } else { driver.guest_ip(g) }))
            .collect();
        let agents: Vec<_> = probes
            .iter()
            .zip(&snapshot.guests)
            .map(|((vmid, p), g)| (*vmid, if p.status == redox_core::GuestStatus::Stopped { None } else { driver.guest_agent(g) }))
            .collect();
        let node = snapshot.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        let ios: Vec<_> = probes.iter().zip(&snapshot.guests).map(|((_, p), g)| if p.status == redox_core::GuestStatus::Stopped { None } else { driver.io_counters(g) }).collect();

        let nets: Vec<_> = probes.iter().zip(&snapshot.guests).map(|((_, p), g)| if p.status == redox_core::GuestStatus::Stopped { None } else { driver.net_counters(g) }).collect();

        let mut cluster = self.lock();
        let mut changed = false;
        for (((vmid, probe), io), net) in probes.iter().zip(ios).zip(nets) {
            changed |= cluster.observe_guest(*vmid, probe, now());
            cluster.record_usage(*vmid, io, now());
            if let Some((rx, tx)) = net {
                cluster.record_net_usage(*vmid, rx, tx);
            }
        }
        // Ölçüm sayaçları her turda değişir; dosyayı yıpratmamak için beş dakikada bir yazılır.
        static USAGE_SAVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if now().saturating_sub(USAGE_SAVED.load(std::sync::atomic::Ordering::Relaxed)) >= 300 {
            USAGE_SAVED.store(now(), std::sync::atomic::Ordering::Relaxed);
            changed = true;
        }
        for (vmid, ip) in ips {
            changed |= cluster.observe_ip(vmid, ip);
        }
        for (vmid, agent) in agents {
            changed |= cluster.observe_agent(vmid, agent);
        }
        if let Some(h) = host {
            cluster.observe_node(&node, h.cpu_pct, h.mem_used_mb, h.mem_total_mb, h.uptime_secs);
        }
        if let Some((used, total)) = disk_gb {
            cluster.observe_storage(crate::host::LOCAL_STORAGE, &node, used, total);
        }
        let samples: Vec<(String, f64)> = cluster
            .guests
            .iter()
            .flat_map(|g| {
                [
                    (format!("guest:{}:cpu", g.vmid), g.cpu_pct as f64),
                    (format!("guest:{}:mem", g.vmid), g.mem_pct() as f64),
                ]
            })
            .chain(cluster.nodes.first().into_iter().flat_map(|n| {
                [
                    ("node:cpu".to_string(), n.cpu_pct as f64),
                    ("node:mem".to_string(), redox_core::fmt::pct(n.mem_used_mb, n.mem_total_mb) as f64),
                ]
            }))
            .collect();
        // Yalnızca durum değiştiyse diske yaz; ölçümler her 5 saniyede dosyayı yıpratmasın.
        if changed {
            self.persist(&cluster);
        }
        drop(cluster);

        self.record_long(&samples, now());
        let live: Vec<String> = samples.iter().map(|(k, _)| k.clone()).collect();
        for (key, value) in samples {
            self.record(key, value);
        }
        // Silinen konukların geçmişi tutulmaz.
        self.0.history.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| live.contains(k));
    }

    /// Ek depolama alanlarının doluluğunu günceller (`(ad, dolu GB, toplam GB)`); durum dosyası yazılmaz.
    pub fn observe_storages(&self, usage: &[(String, u64, u64)]) {
        let mut cluster = self.lock();
        let node = cluster.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        for (id, used, total) in usage {
            cluster.observe_storage(id, &node, *used, *total);
        }
    }
}
