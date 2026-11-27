//! Eşzamanlı disk yansıtmanın yönetimi.
//!
//! **Kaynakta** `mirror_tick` saniyede bir çalışır ve bekçidir: yalnızca yerel hipervizöre bakar. İlerlemesi
//! takılan yansıtmayı hemen durdurur (karşı taraf donduğunda konuğun disk yazmaları bekler). Eşle konuşan işler
//! (yansıtmayı başlatma, durum bildirme) arka planda yürür; bekçi ağ yüzünden gecikmez.
//!
//! **Hedefte** kopyalar kuşak kuşak tutulur ve eş sustuktan sonra kopyadan konuk açılabilir (devralma). Devralma
//! bilerek elledir: hedef, "kaynak öldü" ile "aramız koptu" durumlarını tek başına ayıramaz.

use super::*;
use redox_core::{MirrorProbe, MirrorState, MirrorStore};
use std::collections::HashMap;

/// İlerleme bu kadar saniye takılı kalırsa yansıtma durdurulur.
pub const STALL_SECS: u64 = 8;
/// Eşit yansıtma hedefe bu aralıkla bildirilir (devralmanın sessizlik süresinden çok kısa olmalı).
pub const HEARTBEAT_SECS: u64 = 5;
/// Kopan yansıtma bu kadar saniye sonra yeniden denenir.
pub const RETRY_SECS: u64 = 60;
/// Tek kopya diskinin üst sınırı.
const MAX_DISK_BYTES: u64 = 64 << 40;

#[derive(Debug, Default, Clone)]
pub struct MirrorRuntime {
    done: u64,
    moved_at: u64,
    reported_at: u64,
    retry_at: u64,
    starting: bool,
}

#[derive(Default)]
pub struct MirrorWatch(Mutex<HashMap<VmId, MirrorRuntime>>);

impl MirrorWatch {
    fn with<R>(&self, vmid: VmId, work: impl FnOnce(&mut MirrorRuntime) -> R) -> R {
        work(self.0.lock().unwrap_or_else(|e| e.into_inner()).entry(vmid).or_default())
    }

    fn forget(&self, vmid: VmId) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&vmid);
    }
}

/// Sürücünün yansıtmayı bilerek duraklattığı bir işlem, yansıtmanın son durum değişikliğinden beri çalıştı mı?
/// (Kısa süren bir işlem iki bekçi turu arasında başlayıp bitebilir; bu yüzden yalnızca kilide bakılmaz.)
fn paused_by_task(c: &Cluster, vmid: VmId, since: u64) -> bool {
    c.tasks.iter().any(|t| {
        t.vmid == Some(vmid) && t.ended.is_none_or(|ended| ended >= since) && matches!(t.command.split(' ').nth(1), Some("backup" | "snapshot" | "suspend" | "rollback"))
    })
}

/// Bu sunucunun hedefe kendini tanıttığı ad: ilk düğümün adı, dosya adına uygun biçimde.
pub fn origin_name(c: &Cluster) -> String {
    let raw = c.nodes.first().map(|n| n.id.to_ascii_lowercase()).unwrap_or_default();
    let clean: String = raw.chars().map(|ch| if ch.is_ascii_lowercase() || ch.is_ascii_digit() { ch } else { '-' }).take(32).collect();
    if clean.is_empty() {
        "sunucu".to_string()
    } else {
        clean
    }
}

impl AppState {
    fn mirror_log(&self, vmid: VmId, command: String, outcome: Result<(), String>, user: &str) {
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, command, &outcome, now(), Some(vmid));
        });
    }

    pub fn mirror_enable(&self, vmid: VmId, peer: &str, user: &str) -> Result<(), Error> {
        if self.peers().peer(peer).is_none() {
            return Err(Error::InvalidSpec(format!("\"{peer}\" adında bir eş sunucu tanımlı değil")));
        }
        if !self.driver().disk_mirroring() {
            return Err(Error::InvalidSpec("bu sürücü disk yansıtmayı desteklemiyor (gerçek sürücü gerekir)".to_string()));
        }
        self.mutate(|c| c.mirror_add(vmid, peer, now()))?;
        self.0.mirrors.forget(vmid);
        self.mirror_log(vmid, format!("vm mirror {vmid} {peer}"), Ok(()), user);
        Ok(())
    }

    /// Yansıtmayı kapatır. `delete_copy` doğruysa hedefteki kopya da silinir; değilse hedefe "artık eşit değil"
    /// denir ve kopya orada kalır. Eşle konuştuğu için istek iş parçacığında çağrılmaz.
    pub fn mirror_disable(&self, vmid: VmId, delete_copy: bool, user: &str) -> Result<(), Error> {
        let record = self.mutate(|c| c.mirror_remove(vmid))?;
        self.0.mirrors.forget(vmid);
        let guest = self.snapshot().guest(vmid).ok().cloned();
        let stopped = guest.map_or(Ok(()), |g| self.driver().mirror_stop(&g));
        let origin = origin_name(&self.snapshot());
        let told = match self.peers().peer(&record.peer) {
            Some(peer) if delete_copy => peer.mirror_delete(&origin, vmid).map_err(|e| format!("yansıtma kapatıldı ama {} sunucusundaki kopya silinemedi: {e}", record.peer)),
            Some(peer) => {
                // Kopya duracaksa haber verilemese de sorun yoktur: sessizlik süresi dolunca zaten kullanılabilir.
                if record.generation > 0 {
                    let _ = peer.mirror_report(&origin, vmid, record.generation, false);
                }
                Ok(())
            }
            None if delete_copy => Err(format!("yansıtma kapatıldı ama \"{}\" eş sunucusu artık tanımlı değil; kopya orada duruyor", record.peer)),
            None => Ok(()),
        };
        let outcome = stopped.and(told);
        self.mirror_log(vmid, format!("vm mirror off {vmid}"), outcome.clone(), user);
        outcome.map_err(Error::Driver)
    }

    pub fn mirror_tick(&self) {
        self.mirror_tick_at(now());
    }

    /// Bekçinin bir turu.
    pub fn mirror_tick_at(&self, at: u64) {
        let cluster = self.snapshot();
        for record in cluster.mirrors.clone() {
            let vmid = record.vmid;
            let Ok(guest) = cluster.guest(vmid).cloned() else {
                // Konuk silinmiş: yansıtması da kalkar (hedefteki kopya elle silinene kadar durur).
                self.mutate(|c| c.mirror_remove(vmid).map(drop)).ok();
                self.0.mirrors.forget(vmid);
                continue;
            };
            let runtime = self.0.mirrors.with(vmid, |r| r.clone());
            if runtime.starting {
                continue;
            }
            let active = matches!(record.state, MirrorState::Syncing | MirrorState::InSync);
            if !matches!(guest.status, redox_core::GuestStatus::Running | redox_core::GuestStatus::Paused) {
                if record.state != MirrorState::Waiting {
                    self.mutate(|c| c.mirror_set(vmid, MirrorState::Waiting, None, at));
                    self.0.mirrors.forget(vmid);
                    self.mirror_report_later(&record.peer, vmid, record.generation, false);
                }
                continue;
            }
            let Some(probe) = self.driver().mirror_probe(&guest) else { continue };
            if active && probe.jobs < probe.disks && paused_by_task(&cluster, vmid, record.since) {
                self.mirror_pause(&guest, &record, at);
            } else if probe.jobs == 0 {
                if active {
                    self.mirror_break(&guest, &record.peer, record.generation, "yansıtma koptu (kopyaya yazılamıyor ya da bağlantı kesildi)", at);
                } else if (record.state == MirrorState::Waiting || at >= runtime.retry_at) && guest.lock.is_none() {
                    self.mirror_begin(guest, record.peer.clone(), at);
                }
            } else if probe.jobs < probe.disks {
                self.mirror_break(&guest, &record.peer, record.generation, "disklerden birinin yansıtması koptu", at);
            } else {
                self.mirror_follow(&guest, &record, probe, runtime, at);
            }
        }
    }

    fn mirror_follow(&self, guest: &Guest, record: &redox_core::Mirror, probe: MirrorProbe, runtime: MirrorRuntime, at: u64) {
        let vmid = guest.vmid;
        let caught_up = probe.done >= probe.total;
        let moved = probe.done != runtime.done;
        if !caught_up && !moved && at >= runtime.moved_at.saturating_add(STALL_SECS) {
            self.mirror_break(guest, &record.peer, record.generation, "kopya sunucusu yanıt vermiyor; konuğun disk yazmaları beklemesin diye yansıtma durduruldu", at);
            return;
        }
        let state = if probe.synced { MirrorState::InSync } else { MirrorState::Syncing };
        let changed = self.mutate(|c| {
            c.mirror_progress(vmid, record.generation, probe.done, probe.total);
            // Yetişmemiş bir "eşit" tur son eşit anı ilerletmez: karşı taraf donmuş olabilir.
            if state == MirrorState::InSync && !caught_up && record.state == MirrorState::InSync {
                false
            } else {
                c.mirror_set(vmid, state, None, at)
            }
        });
        if changed && state == MirrorState::InSync {
            self.mirror_log(vmid, format!("vm mirror synced {vmid} {}", record.peer), Ok(()), SCHEDULER);
        }
        let report = self.0.mirrors.with(vmid, |r| {
            if moved || caught_up {
                (r.done, r.moved_at) = (probe.done, at);
            }
            let due = probe.synced && caught_up && (changed || at >= r.reported_at.saturating_add(HEARTBEAT_SECS));
            if due {
                r.reported_at = at;
            }
            due
        });
        if report {
            self.mirror_report_later(&record.peer, vmid, record.generation, true);
        }
    }

    /// Yedek ya da anlık görüntü yansıtmayı durdurdu: bekleme durumuna geçilir; iş bitince baştan eşitlenir.
    fn mirror_pause(&self, guest: &Guest, record: &redox_core::Mirror, at: u64) {
        let vmid = guest.vmid;
        let note = "yedek ya da anlık görüntü için yansıtma duraklatıldı; iş bitince baştan eşitlenir";
        self.mutate(|c| c.mirror_set(vmid, MirrorState::Waiting, Some(note.to_string()), at));
        self.0.mirrors.forget(vmid);
        self.mirror_report_later(&record.peer, vmid, record.generation, false);
    }

    fn mirror_break(&self, guest: &Guest, peer: &str, generation: u32, why: &str, at: u64) {
        let vmid = guest.vmid;
        // Önce konuk kurtarılır; durum ve bildirim sonra gelir.
        let reason = match self.driver().mirror_stop(guest) {
            Ok(()) => why.to_string(),
            Err(e) => format!("{why}. Durdurulamadı: {e}"),
        };
        let changed = self.mutate(|c| c.mirror_set(vmid, MirrorState::Broken, Some(reason.clone()), at));
        self.0.mirrors.with(vmid, |r| *r = MirrorRuntime { retry_at: at + RETRY_SECS, ..Default::default() });
        if changed {
            self.mirror_log(vmid, format!("vm mirror broken {vmid} {peer}"), Err(reason), SCHEDULER);
        }
        self.mirror_report_later(peer, vmid, generation, false);
    }

    fn mirror_report_later(&self, peer: &str, vmid: VmId, generation: u32, in_sync: bool) {
        let Some(peer) = self.peers().peer(peer) else { return };
        if generation == 0 {
            return;
        }
        let origin = origin_name(&self.snapshot());
        // Bildirim ulaşmazsa bir sonraki turda yenisi gider.
        self.dispatch(move || {
            let _ = peer.mirror_report(&origin, vmid, generation, in_sync);
        });
    }

    /// Yansıtmayı arka planda başlatır: hedefte yeni kuşak açtırır, sonra yerel işleri kurar.
    fn mirror_begin(&self, guest: Guest, peer_name: String, at: u64) {
        let vmid = guest.vmid;
        self.0.mirrors.with(vmid, |r| r.starting = true);
        let me = self.clone();
        self.dispatch(move || {
            let outcome = me.mirror_open(&guest, &peer_name);
            // Başlarken kapatıldıysa kurulan işler geri alınır.
            if me.snapshot().mirror(vmid).is_none_or(|m| m.peer != peer_name) {
                let _ = me.driver().mirror_stop(&guest);
                me.0.mirrors.forget(vmid);
                return;
            }
            match outcome {
                Ok(generation) => {
                    me.mutate(|c| {
                        c.mirror_progress(vmid, generation, 0, 0);
                        c.mirror_set(vmid, MirrorState::Syncing, None, at);
                    });
                    me.0.mirrors.with(vmid, |r| *r = MirrorRuntime { moved_at: at, ..Default::default() });
                }
                Err(e) => {
                    let changed = me.mutate(|c| c.mirror_set(vmid, MirrorState::Broken, Some(e.clone()), at));
                    me.0.mirrors.with(vmid, |r| *r = MirrorRuntime { retry_at: at + RETRY_SECS, ..Default::default() });
                    if changed {
                        me.mirror_log(vmid, format!("vm mirror broken {vmid} {peer_name}"), Err(e), SCHEDULER);
                    }
                }
            }
        });
    }

    fn mirror_open(&self, guest: &Guest, peer_name: &str) -> Result<u32, String> {
        let peer = self.peers().peer(peer_name).ok_or_else(|| format!("\"{peer_name}\" adında bir eş sunucu tanımlı değil"))?;
        let sizes = self.driver().mirror_disk_sizes(guest)?;
        let key = super::migrate::new_key()?;
        let opened = peer.mirror_prepare(&origin_name(&self.snapshot()), guest, &sizes, &key)?;
        let host = opened.host.or_else(|| peer.host_name()).ok_or("eş sunucunun adresi anlaşılamadı")?;
        self.driver().mirror_start(guest, &host, opened.port, &key)?;
        Ok(opened.generation)
    }

    // ---------- Hedef taraf ----------

    /// Eş sunucu için yeni bir kopya kuşağı açar: `(kuşak, disk kapısı)`.
    pub fn mirror_store_prepare(&self, origin: &str, guest: Guest, sizes: &[u64], key: &str, user: &str) -> Result<(u32, u16), Error> {
        let driver = self.driver();
        if !driver.disk_mirroring() {
            return Err(Error::InvalidSpec("bu sürücü disk kopyası tutamaz (gerçek sürücü gerekir)".to_string()));
        }
        if sizes.len() != 1 + guest.hw.disks.len() || sizes.iter().any(|s| *s == 0 || *s > MAX_DISK_BYTES) || !redox_core::valid_migrate_key(key) {
            return Err(Error::InvalidSpec("yansıtma isteği eksik ya da geçersiz (disk boyutları ve anahtar gerekir)".to_string()));
        }
        let vmid = guest.vmid;
        let settings = self.migrate_settings();
        let _pick = super::migrate::PICK.lock().unwrap_or_else(|e| e.into_inner());
        // Önceki kuşağın tutucusu kapatılır ki kapısı boşalsın; eksiksiz kopyanın dosyaları kalır.
        let previous = self.snapshot().mirror_store(origin, vmid).cloned();
        if let Some(old) = &previous {
            driver.replica_release(&old.replica_id(old.generation)).map_err(Error::Driver)?;
        }
        let port = super::migrate::free_port(settings.ports, &settings.bind).map_err(Error::Driver)?;
        let (generation, stale) = self.mutate(|c| c.store_prepare(origin, guest, port))?;
        let store = self.stored(origin, vmid)?;
        if let Some(stale) = stale {
            driver.replica_delete(&store.replica_id(stale)).map_err(Error::Driver)?;
        }
        let served = driver.replica_serve(&store.replica_id(generation), sizes, &settings.bind, port, key);
        self.mirror_log(vmid, format!("mirror store {origin}/{vmid} {generation}"), served.clone(), user);
        served.map_err(Error::Driver)?;
        Ok((generation, port))
    }

    fn stored(&self, origin: &str, vmid: VmId) -> Result<MirrorStore, Error> {
        self.snapshot().mirror_store(origin, vmid).cloned().ok_or_else(|| Error::InvalidSpec(format!("{origin}/{vmid} için tutulan bir kopya yok")))
    }

    pub fn mirror_store_report(&self, origin: &str, vmid: VmId, generation: u32, in_sync: bool) -> Result<(), Error> {
        let obsolete = self.mutate(|c| c.store_report(origin, vmid, generation, in_sync, now()))?;
        let store = self.stored(origin, vmid)?;
        if let Some(old) = obsolete {
            self.driver().replica_delete(&store.replica_id(old)).map_err(Error::Driver)?;
        }
        if !in_sync {
            self.driver().replica_release(&store.replica_id(generation)).map_err(Error::Driver)?;
        }
        Ok(())
    }

    pub fn mirror_store_delete(&self, origin: &str, vmid: VmId, user: &str) -> Result<(), Error> {
        let store = self.stored(origin, vmid)?;
        for generation in [Some(store.generation), store.complete].into_iter().flatten() {
            self.driver().replica_delete(&store.replica_id(generation)).map_err(Error::Driver)?;
        }
        self.mutate(|c| c.store_remove(origin, vmid))?;
        self.mirror_log(vmid, format!("mirror delete {origin}/{vmid}"), Ok(()), user);
        Ok(())
    }

    /// Devralma: eksiksiz kopyadan, kaynaktaki donanımla kapalı bir konuk oluşturur ve kimliğini döndürür.
    /// Konuğu başlatmak kullanıcıya kalır.
    pub fn mirror_promote(&self, origin: &str, vmid: VmId, user: &str) -> Result<VmId, Error> {
        let complete = self.snapshot().store_promotable(origin, vmid, now())?;
        let store = self.stored(origin, vmid)?;
        let driver = self.driver();
        // Tutucular kapatılır: bundan sonra kaynak bu kopyaya yazamaz.
        for generation in [store.generation, complete] {
            driver.replica_release(&store.replica_id(generation)).map_err(Error::Driver)?;
        }
        let remote = &store.guest;
        let keep_id = self.snapshot().guest(vmid).is_err();
        let spec = self.local_spec(remote, keep_id.then_some(vmid))?;
        let id = self.create_guest(&spec, user)?;
        let built = self.restore_hardware(id, remote).and_then(|guest| driver.replica_adopt(&store.replica_id(complete), &guest));
        if let Err(e) = built {
            let _ = self.delete_guest(id, user);
            self.mirror_log(vmid, format!("mirror promote {origin}/{vmid}"), Err(e.clone()), user);
            return Err(Error::Driver(e));
        }
        if store.generation != complete {
            driver.replica_delete(&store.replica_id(store.generation)).map_err(Error::Driver)?;
        }
        self.mutate(|c| {
            if let Ok(g) = c.guest_mut_for_import(id) {
                // Bellek parçaları sıfırdan açılışta açılış belleğine katılır.
                g.hw.dimms.clear();
                g.tags = remote.tags.clone();
                g.tags.push(format!("devralindi-{origin}"));
                g.tags.retain(|t| redox_core::valid_tag(t));
                g.tags.truncate(redox_core::MAX_TAGS);
            }
            c.store_remove(origin, vmid).map(drop)
        })?;
        self.mirror_log(id, format!("mirror promote {origin}/{vmid}"), Ok(()), user);
        let _ = self.sync_firewall();
        Ok(id)
    }
}
