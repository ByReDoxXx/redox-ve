//! Hata toleransının yönetimi.
//!
//! Güvenlik tek bir kurala dayanır: **birincil, konuğu bilerek durdurmadan ya da bir ölümden sonra yeniden
//! başlatmadan önce hakemden oyu almak zorundadır.** Oyu birincil aldıysa ikiz o kurulum için artık asla
//! devralamaz; ikiz aldıysa birincil o konuğu başlatamaz. Bağlantı koptuğunda da aynı hakem karar verir:
//! kazanan sürer, kaybeden kendi QEMU'sunu sonlandırır. Hakeme ulaşamayan birincil konuğu duraklatır (iki
//! yerde birden çalışmasındansa beklemesi iyidir), hakeme ulaşamayan ikiz ise beklemeyi sürdürür.
//!
//! `ft_tick` saniyede bir çalışır; yalnızca yerel hipervizöre bakar. Eşle ve hakemle konuşan işler arka
//! plandadır.

use super::*;
use redox_core::{ft_key, FtLink, FtSide, FtState, GuestStatus, Protection, Standby};
use std::collections::HashMap;

/// Karşı taraftan bu kadar saniye haber alınamazsa hakeme gidilir.
pub const DEAD_SECS: u64 = 5;
/// Korumasız kalan konuk bu kadar saniye sonra yeniden korunmaya çalışılır.
pub const RETRY_SECS: u64 = 120;
/// Kurulumu tamamlanmayan ikiz bu kadar saniye sonra kaldırılır.
pub const PREPARE_SECS: u64 = 900;

const HANDED_OVER_NOTE: &str = "konuk yedek sunucuda devralındı; burada başlatmak için önce orada kapalı olduğundan emin olup hata toleransını kapatın";

/// Devralınan ve devredilen konuklara konan etiket.
const HANDED_OVER_TAG: &str = "ft-devredildi";

#[derive(Debug, Default, Clone)]
pub struct FtRuntime {
    /// Karşı taraftan en son haber alınan an (ikizde: eşzamanlı çalışmanın ilk görüldüğü an).
    heard_at: u64,
    sent_at: u64,
    retry_at: u64,
    /// Süren bir kurulum, bildirim ya da hakem sorusu.
    busy: bool,
    beating: bool,
    /// Karşı taraf konuğunun öldüğünü bildirdi.
    lost: bool,
}

#[derive(Default)]
pub struct FtWatch(Mutex<HashMap<(bool, VmId), FtRuntime>>);

impl FtWatch {
    fn with<R>(&self, standby: bool, vmid: VmId, work: impl FnOnce(&mut FtRuntime) -> R) -> R {
        work(self.0.lock().unwrap_or_else(|e| e.into_inner()).entry((standby, vmid)).or_default())
    }

    fn forget(&self, standby: bool, vmid: VmId) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&(standby, vmid));
    }
}

/// Bir korumanın bu durumlarında ikiz devralabilir; birincil, hakemin oyu olmadan konuğu elleyemez.
fn twin_may_exist(state: FtState) -> bool {
    matches!(state, FtState::Preparing | FtState::Protected | FtState::Suspended)
}

/// Yedek sunucunun kurulum isteği.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FtPrepare {
    pub origin: String,
    pub guest: Box<Guest>,
    pub witness: String,
    pub generation: u32,
    pub key: String,
    /// Birincilin adresi; verilmezse isteğin geldiği adres kullanılır.
    #[serde(default)]
    pub primary_host: Option<String>,
    pub mirror_port: u16,
    pub compare_port: u16,
}

impl AppState {
    fn ft_log(&self, vmid: VmId, command: String, outcome: Result<(), String>, user: &str) {
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, command, &outcome, now(), Some(vmid));
        });
    }

    pub fn ft_enable(&self, vmid: VmId, peer: &str, witness: &str, user: &str) -> Result<(), Error> {
        for name in [peer, witness] {
            if self.peers().peer(name).is_none() {
                return Err(Error::InvalidSpec(format!("\"{name}\" adında bir eş sunucu tanımlı değil")));
            }
        }
        if !self.driver().fault_tolerance() {
            return Err(Error::InvalidSpec("bu sürücü hata toleransını desteklemiyor (gerçek sürücü gerekir)".to_string()));
        }
        self.mutate(|c| c.ft_enable(vmid, peer, witness, now()))?;
        self.0.ft.forget(false, vmid);
        self.ft_log(vmid, format!("vm ft on {vmid} {peer} {witness}"), Ok(()), user);
        Ok(())
    }

    /// Korumayı kapatır. Konuk çalışıyorsa çalışmayı sürdürür. Eşle konuşur; istek iş parçacığında çağrılmaz.
    pub fn ft_disable(&self, vmid: VmId, user: &str) -> Result<(), Error> {
        let record = self.snapshot().protection(vmid).cloned().ok_or_else(|| Error::InvalidSpec(format!("{vmid} korunmuyor")))?;
        self.ft_release(vmid).map_err(Error::Driver)?;
        self.mutate(|c| c.ft_disable(vmid))?;
        self.0.ft.forget(false, vmid);
        self.ft_log(vmid, format!("vm ft off {} {vmid}", record.peer), Ok(()), user);
        Ok(())
    }

    fn ft_arbitrate_at(&self, record: &Protection) -> Result<FtSide, String> {
        let origin = super::mirrors::origin_name(&self.snapshot());
        let witness = self.peers().peer(&record.witness).ok_or_else(|| format!("\"{}\" hakem sunucusu tanımlı değil", record.witness))?;
        witness.ft_arbitrate(&ft_key(&origin, record.vmid, record.generation), FtSide::Primary).map_err(|e| format!("hakeme ulaşılamıyor: {e}"))
    }

    /// Birincil konuğa dokunmadan önce: ikizin bir daha devralamayacağından emin olur ve onu bırakır.
    fn ft_release(&self, vmid: VmId) -> Result<(), String> {
        let cluster = self.snapshot();
        let Some(record) = cluster.protection(vmid).cloned() else { return Ok(()) };
        if !twin_may_exist(record.state) {
            return Ok(());
        }
        let origin = super::mirrors::origin_name(&cluster);
        let peer = self.peers().peer(&record.peer);
        let discard = || peer.as_ref().ok_or_else(|| format!("\"{}\" adında bir eş sunucu tanımlı değil", record.peer)).and_then(|p| p.ft_discard(&origin, vmid, record.generation));
        match self.ft_arbitrate_at(&record) {
            Ok(FtSide::Primary) => {
                // Oy bizde: ikiz devralamaz. Silinemese de zararsızdır, kendisi kaldırır.
                let _ = discard();
            }
            Ok(FtSide::Secondary) => {
                self.ft_handed_over(vmid, &record);
                return Err(HANDED_OVER_NOTE.to_string());
            }
            // Hakem yoksa ancak ikizin kaldırıldığı doğrulanırsa devam edilir.
            Err(e) => discard().map_err(|d| format!("{e}; ikiz de kaldırılamadı ({d}). Aynı konuk iki yerde çalışmasın diye işlem yapılmadı"))?,
        }
        if let Ok(guest) = self.snapshot().guest(vmid).cloned() {
            self.driver().ft_drop_secondary(&guest)?;
        }
        self.mutate(|c| c.ft_set(vmid, FtState::Waiting, None, now()));
        self.0.ft.forget(false, vmid);
        Ok(())
    }

    /// `run_action` çağırmadan önce: korunan konukta eylem güvenli mi? Gerekirse hakemden oy alır.
    pub(super) fn ft_before_action(&self, vmid: VmId, action: Action) -> Result<(), Error> {
        let Some(record) = self.snapshot().protection(vmid).cloned() else { return Ok(()) };
        let refuse = |why: &str| Err(Error::InvalidSpec(why.to_string()));
        if record.state == FtState::HandedOver {
            return refuse(HANDED_OVER_NOTE);
        }
        match action {
            Action::Pause | Action::Resume if twin_may_exist(record.state) => refuse("korunan konuk duraklatılamaz; önce hata toleransını kapatın"),
            Action::Pause | Action::Resume => Ok(()),
            // Başlatma: konuk en son ikizliyken öldüyse ikiz devralmış olabilir.
            Action::Start => self.ft_release(vmid).map_err(Error::Driver),
            Action::Stop | Action::Reboot => {
                self.ft_release(vmid).map_err(Error::Driver)?;
                // Görev kilidi konana kadar bekçi korumayı yeniden kurmaya kalkmasın.
                self.0.ft.with(false, vmid, |r| r.retry_at = now() + 30);
                Ok(())
            }
        }
    }

    /// İkiz devraldı: buradaki kayıt kapalı kalır, koruma kalkar ve konuk kendiliğinden açılmaz.
    fn ft_handed_over(&self, vmid: VmId, record: &Protection) {
        let guest = self.snapshot().guest(vmid).ok().cloned();
        if let Some(guest) = guest {
            let _ = self.driver().ft_fence(&guest);
        }
        self.mutate(|c| {
            c.ft_set(vmid, FtState::HandedOver, Some(HANDED_OVER_NOTE.to_string()), now());
            c.after_migrated_out(vmid);
            if let Ok(g) = c.guest_mut_for_import(vmid) {
                if !g.tags.iter().any(|t| t == HANDED_OVER_TAG) && g.tags.len() < redox_core::MAX_TAGS {
                    g.tags.push(HANDED_OVER_TAG.to_string());
                }
            }
        });
        self.0.ft.forget(false, vmid);
        self.ft_log(vmid, format!("vm ft handover {vmid} {}", record.peer), Ok(()), SCHEDULER);
    }

    pub fn ft_tick(&self) {
        self.ft_tick_at(now());
    }

    pub fn ft_tick_at(&self, at: u64) {
        let cluster = self.snapshot();
        for record in cluster.protections.clone() {
            self.ft_tick_primary(&cluster, record, at);
        }
        for standby in cluster.standbys.clone() {
            self.ft_tick_standby(&cluster, standby, at);
        }
    }

    fn ft_tick_primary(&self, cluster: &Cluster, record: Protection, at: u64) {
        let vmid = record.vmid;
        let Ok(guest) = cluster.guest(vmid).cloned() else {
            self.mutate(|c| c.ft_disable(vmid).map(drop)).ok();
            return;
        };
        let runtime = self.0.ft.with(false, vmid, |r| r.clone());
        if runtime.busy {
            return;
        }
        let running = matches!(guest.status, GuestStatus::Running | GuestStatus::Paused);
        match record.state {
            FtState::HandedOver => {}
            FtState::Waiting | FtState::Unprotected => {
                if running && guest.lock.is_none() && at >= runtime.retry_at {
                    self.ft_begin(guest, record, at);
                }
            }
            FtState::Preparing => {
                // Kurulumu yürüten iş yok (hizmet yeniden başlamış): yarım kurulum bırakılır.
                self.ft_resolve(guest, record, "koruma kurulumu yarıda kaldı", at);
            }
            FtState::Suspended => self.ft_resolve(guest, record, "hakem bekleniyordu", at),
            FtState::Protected => {
                let status = self.driver().ft_status(&guest);
                if status.is_none() {
                    // Buradaki QEMU öldü: ikize haber verilir; oy birincil tarafından istenmez.
                    self.ft_report_death(&record, at);
                    return;
                }
                if status.as_ref().is_some_and(|s| !s.unresponsive && !s.engaged) {
                    self.ft_resolve(guest, record, "eşzamanlı çalışma bozuldu", at);
                    return;
                }
                if runtime.lost {
                    self.ft_resolve(guest, record, "ikiz kayboldu", at);
                } else if at >= runtime.heard_at.saturating_add(DEAD_SECS) {
                    self.ft_resolve(guest, record, "yedek sunucudan haber alınamıyor", at);
                } else if !runtime.beating && at > runtime.sent_at {
                    self.ft_beat_later(record, true, at);
                }
            }
        }
    }

    fn ft_beat_later(&self, record: Protection, alive: bool, at: u64) {
        let vmid = record.vmid;
        let Some(peer) = self.peers().peer(&record.peer) else { return };
        let origin = super::mirrors::origin_name(&self.snapshot());
        self.0.ft.with(false, vmid, |r| (r.beating, r.sent_at) = (true, at));
        let me = self.clone();
        self.dispatch(move || {
            let reply = peer.ft_beat(&origin, vmid, record.generation, alive);
            me.0.ft.with(false, vmid, |r| {
                r.beating = false;
                match reply {
                    Ok(true) => r.heard_at = r.heard_at.max(at),
                    // Eş yanıt veriyor ama ikizi yok.
                    Ok(false) => r.lost = true,
                    Err(_) => {}
                }
            });
        });
    }

    /// Korunan konuğun buradaki QEMU'su kendiliğinden öldü.
    fn ft_report_death(&self, record: &Protection, at: u64) {
        let vmid = record.vmid;
        if self.0.ft.with(false, vmid, |r| r.beating || at <= r.sent_at) {
            return;
        }
        self.ft_beat_later(record.clone(), false, at);
        let origin = super::mirrors::origin_name(&self.snapshot());
        let Some(witness) = self.peers().peer(&record.witness) else { return };
        let (me, record) = (self.clone(), record.clone());
        self.dispatch(move || {
            if let Ok(Some(FtSide::Secondary)) = witness.ft_winner(&ft_key(&origin, vmid, record.generation)) {
                me.ft_handed_over(vmid, &record);
            }
        });
    }

    /// Birincil ikizi kaybettiğini düşünüyor: hakem karar verir.
    fn ft_resolve(&self, guest: Guest, record: Protection, why: &str, at: u64) {
        let vmid = guest.vmid;
        self.0.ft.with(false, vmid, |r| r.busy = true);
        let (me, why) = (self.clone(), why.to_string());
        self.dispatch(move || {
            match me.ft_arbitrate_at(&record) {
                Ok(FtSide::Primary) => {
                    let outcome = me.driver().ft_drop_secondary(&guest);
                    let reason = match &outcome {
                        Ok(()) => why.clone(),
                        Err(e) => format!("{why}. İkiz bırakılamadı: {e}"),
                    };
                    me.mutate(|c| c.ft_set(vmid, FtState::Unprotected, Some(reason.clone()), at));
                    me.0.ft.with(false, vmid, |r| *r = FtRuntime { retry_at: at + RETRY_SECS, ..Default::default() });
                    me.ft_log(vmid, format!("vm ft unprotected {vmid} {}", record.peer), Err(reason), SCHEDULER);
                    if let Some(peer) = me.peers().peer(&record.peer) {
                        let origin = super::mirrors::origin_name(&me.snapshot());
                        let _ = peer.ft_discard(&origin, vmid, record.generation);
                    }
                }
                Ok(FtSide::Secondary) => me.ft_handed_over(vmid, &record),
                Err(e) => {
                    // İki yerde birden çalışmasındansa beklemesi iyidir.
                    let held = me.driver().ft_hold(&guest, true);
                    let reason = match held {
                        Ok(()) => format!("{why}; {e}"),
                        Err(h) => format!("{why}; {e}; konuk duraklatılamadı: {h}"),
                    };
                    if me.mutate(|c| c.ft_set(vmid, FtState::Suspended, Some(reason.clone()), at)) {
                        me.ft_log(vmid, format!("vm ft suspended {vmid} {}", record.peer), Err(reason), SCHEDULER);
                    }
                    me.0.ft.with(false, vmid, |r| r.busy = false);
                }
            }
        });
    }

    /// Korumayı arka planda kurar.
    fn ft_begin(&self, guest: Guest, record: Protection, at: u64) {
        let vmid = guest.vmid;
        self.0.ft.with(false, vmid, |r| r.busy = true);
        let generation = match self.mutate(|c| {
            let generation = c.ft_next_generation(vmid)?;
            c.ft_set(vmid, FtState::Preparing, None, at);
            Ok::<_, Error>(generation)
        }) {
            Ok(generation) => generation,
            Err(_) => return,
        };
        let me = self.clone();
        self.dispatch(move || {
            let outcome = me.ft_establish(&guest, &record, generation);
            // Kurulum sürerken kapatıldıysa ya da konuk durdurulduysa sonuç yazılmaz.
            let current = me.snapshot().protection(vmid).cloned();
            let Some(current) = current.filter(|p| p.generation == generation && p.state == FtState::Preparing) else {
                me.0.ft.with(false, vmid, |r| r.busy = false);
                return;
            };
            match outcome {
                Ok(()) => {
                    me.mutate(|c| c.ft_set(vmid, FtState::Protected, None, at));
                    me.0.ft.with(false, vmid, |r| *r = FtRuntime { heard_at: at, ..Default::default() });
                    me.ft_log(vmid, format!("vm ft protected {vmid} {}", current.peer), Ok(()), SCHEDULER);
                    me.ft_beat_later(current, true, at);
                }
                Err(e) => {
                    // İkiz devreye girmiş olabilir: hakem oyu alınmadan "korumasız" sayılmaz.
                    me.0.ft.with(false, vmid, |r| r.busy = false);
                    me.ft_resolve(guest, current, &e, at);
                }
            }
        });
    }

    fn ft_establish(&self, guest: &Guest, record: &Protection, generation: u32) -> Result<(), String> {
        if let Some(why) = redox_core::ft_blocker(guest) {
            return Err(why.to_string());
        }
        let peer = self.peers().peer(&record.peer).ok_or_else(|| format!("\"{}\" adında bir eş sunucu tanımlı değil", record.peer))?;
        // Hakemsiz koruma kurulmaz: ilk arızada konuk duraklardı.
        let witness = self.peers().peer(&record.witness).ok_or_else(|| format!("\"{}\" hakem sunucusu tanımlı değil", record.witness))?;
        let origin = super::mirrors::origin_name(&self.snapshot());
        witness.ft_winner(&ft_key(&origin, guest.vmid, generation)).map_err(|e| format!("hakeme ulaşılamıyor: {e}"))?;
        let settings = self.migrate_settings();
        let key = super::migrate::new_key()?;
        let ports = {
            let _pick = super::migrate::PICK.lock().unwrap_or_else(|e| e.into_inner());
            super::migrate::free_port_set(settings.ports, &settings.bind, 4)?
        };
        let request = FtPrepare {
            origin,
            guest: Box::new(guest.clone()),
            witness: record.witness.clone(),
            generation,
            key: key.clone(),
            primary_host: settings.host.clone(),
            mirror_port: ports[0],
            compare_port: ports[1],
        };
        let opened = peer.ft_prepare(&request)?;
        let secondary_host = opened.host.or_else(|| peer.host_name()).ok_or("eş sunucunun adresi anlaşılamadı")?;
        let link = FtLink {
            bind: settings.bind,
            primary_host: String::new(),
            secondary_host,
            mirror_port: ports[0],
            compare_port: ports[1],
            nbd_port: opened.nbd_port,
            migrate_port: opened.migrate_port,
            local_ports: (ports[2], ports[3]),
        };
        self.driver().ft_protect(guest, &link, &key)
    }

    // ---------- Yedek taraf ----------

    /// Eş sunucunun konuğu için ikiz açar: `(disk kapısı, bellek kapısı)`.
    pub fn ft_standby_prepare(&self, request: &FtPrepare, primary_host: &str, user: &str) -> Result<(u16, u16), Error> {
        let invalid = |why: String| Err(Error::InvalidSpec(why));
        let remote = &request.guest;
        let vmid = remote.vmid;
        let driver = self.driver();
        if !driver.fault_tolerance() {
            return invalid("bu sürücü hata toleransını desteklemiyor (gerçek sürücü gerekir)".to_string());
        }
        if !redox_core::mirrors::valid_origin(&request.origin) || !redox_core::valid_migrate_key(&request.key) || !redox_core::valid_migrate_host(primary_host) || request.mirror_port < 1024 || request.compare_port < 1024 {
            return invalid("hata toleransı isteği eksik ya da geçersiz".to_string());
        }
        if let Some(why) = redox_core::ft_blocker(remote) {
            return invalid(why.to_string());
        }
        if self.peers().peer(&request.witness).is_none() {
            return invalid(format!("hakem \"{}\" bu sunucuda tanımlı değil; iki sunucuda da aynı adla tanımlı olmalı", request.witness));
        }
        if !driver.bridges().contains(&remote.bridge) {
            return invalid(format!("bu sunucuda {} köprüsü yok; ikiz aynı ağda olmalı (dağıtık anahtar kullanın)", remote.bridge));
        }
        // Aynı korumanın eski ikizi varsa önce o kaldırılır.
        if let Some(old) = self.snapshot().standby(&request.origin, vmid).cloned() {
            self.ft_drop_standby(&old);
        }
        let local = self.snapshot();
        self.lock().check_not_in_maintenance()?;
        if local.nodes.first().is_none_or(|n| remote.cores > n.cores || remote.mem_total_mb > n.mem_total_mb) {
            return invalid("bu sunucunun işlemcisi ya da belleği ikize yetmiyor".to_string());
        }
        if let Ok(here) = local.guest(vmid) {
            // Tek istisna: bu konuğun daha önce buradan devredilmiş kapalı kaydı.
            if here.status != GuestStatus::Stopped || here.lock.is_some() || !here.tags.iter().any(|t| t == HANDED_OVER_TAG) {
                return invalid(format!("bu sunucuda {vmid} kimliği dolu; ikiz aynı kimlikle açılır (ağ kartının adresi kimlikten türetilir)"));
            }
            self.delete_guest(vmid, user)?;
        }
        let settings = self.migrate_settings();
        let spec = self.local_spec(remote, Some(vmid))?;
        let id = self.create_guest(&spec, user)?;
        let built = (|| -> Result<(u16, u16), String> {
            let mut guest = self.restore_hardware(id, remote)?;
            guest.hw.ft = true;
            self.mutate(|c| {
                let g = c.guest_mut_for_import(id)?;
                g.hw.ft = true;
                // Kilit, devralmaya ya da kaldırmaya kadar bütün işlemleri engeller.
                c.begin_op(id, format!("vm ft-standby {id}"), user, now(), None).map(drop)
            })
            .map_err(|e| e.to_string())?;
            let _pick = super::migrate::PICK.lock().unwrap_or_else(|e| e.into_inner());
            let ports = super::migrate::free_port_set(settings.ports, &settings.bind, 2)?;
            let link = FtLink {
                bind: settings.bind.clone(),
                primary_host: primary_host.to_string(),
                secondary_host: String::new(),
                mirror_port: request.mirror_port,
                compare_port: request.compare_port,
                nbd_port: ports[0],
                migrate_port: ports[1],
                local_ports: (0, 0),
            };
            driver.ft_standby_start(&guest, remote.disk_gb << 30, &link, &request.key)?;
            Ok((ports[0], ports[1]))
        })();
        match built {
            Ok(ports) => {
                self.mutate(|c| {
                    c.standbys.push(Standby { origin: request.origin.clone(), source: vmid, vmid: id, witness: request.witness.clone(), generation: request.generation, last_beat: now() });
                });
                self.0.ft.forget(true, id);
                self.ft_log(id, format!("ft standby {}/{vmid}", request.origin), Ok(()), user);
                Ok(ports)
            }
            Err(e) => {
                self.ft_unlock(id, Err(e.clone()));
                let _ = self.delete_guest(id, user);
                Err(Error::Driver(e))
            }
        }
    }

    fn ft_unlock(&self, vmid: VmId, outcome: Result<(), String>) {
        self.mutate(|c| {
            if let Some(task) = c.guest(vmid).ok().and_then(|g| g.lock) {
                let _ = c.complete_task(task, outcome, now());
            }
        });
    }

    /// İkizi kaldırır: QEMU kapatılır, kayıt ve diskler silinir.
    fn ft_drop_standby(&self, standby: &Standby) {
        let vmid = standby.vmid;
        if let Ok(guest) = self.snapshot().guest(vmid).cloned() {
            self.driver().ft_standby_stop(&guest);
        }
        self.mutate(|c| c.standbys.retain(|s| s.vmid != vmid));
        self.ft_unlock(vmid, Ok(()));
        if let Err(e) = self.delete_guest(vmid, SCHEDULER) {
            self.ft_log(vmid, format!("ft standby remove {vmid}"), Err(e.to_string()), SCHEDULER);
        }
        self.0.ft.forget(true, vmid);
    }

    /// Birincilin yaşam bildirimi. Dönen değer: ikiz duruyor mu? `alive` yanlışsa birincilin konuğu ölmüştür.
    pub fn ft_standby_beat(&self, origin: &str, source: VmId, generation: u32, alive: bool) -> bool {
        let found = self.mutate(|c| {
            let standby = c.standbys.iter_mut().find(|s| s.origin == origin && s.source == source && s.generation == generation)?;
            if alive {
                standby.last_beat = now();
            }
            Some(standby.vmid)
        });
        let Some(vmid) = found else { return false };
        let exists = self.snapshot().guest(vmid).ok().is_some_and(|g| self.driver().ft_status(g).is_some_and(|s| !s.unresponsive));
        self.0.ft.with(true, vmid, |r| {
            if alive {
                r.heard_at = r.sent_at;
            } else {
                r.lost = true;
            }
        });
        exists
    }

    /// Birincil ikizi artık istemiyor.
    pub fn ft_standby_discard(&self, origin: &str, source: VmId, generation: u32) -> Result<(), Error> {
        let standby = self.snapshot().standby(origin, source).cloned();
        match standby {
            Some(standby) if standby.generation == generation => {
                self.ft_drop_standby(&standby);
                Ok(())
            }
            // Zaten yoksa istenen olmuştur; devralınmış bir konuk bu yoldan silinemez.
            _ => Ok(()),
        }
    }

    fn ft_tick_standby(&self, cluster: &Cluster, standby: Standby, at: u64) {
        let vmid = standby.vmid;
        let Ok(guest) = cluster.guest(vmid).cloned() else {
            self.mutate(|c| c.standbys.retain(|s| s.vmid != vmid));
            return;
        };
        if self.0.ft.with(true, vmid, |r| r.busy) {
            return;
        }
        let Some(status) = self.driver().ft_status(&guest) else {
            self.ft_drop_standby(&standby);
            return;
        };
        if status.unresponsive {
            return;
        }
        // Hizmet yeniden başladıysa görev kilidi düşmüş olabilir.
        if guest.lock.is_none() {
            let _ = self.mutate(|c| c.begin_op(vmid, format!("vm ft-standby {vmid}"), SCHEDULER, at, None));
        }
        // `sent_at` ikizde "bu turun anı"dır; yaşam bildirimi geldiğinde o an duyulmuş sayılır.
        let (silent, lost) = self.0.ft.with(true, vmid, |r| {
            r.sent_at = at;
            if status.engaged && r.heard_at == 0 {
                r.heard_at = at;
            }
            (r.heard_at != 0 && at >= r.heard_at.saturating_add(DEAD_SECS), r.lost)
        });
        if !status.engaged {
            if at >= standby.last_beat.saturating_add(PREPARE_SECS) {
                self.ft_drop_standby(&standby);
            }
            return;
        }
        if silent || lost {
            self.ft_claim(guest, standby);
        }
    }

    /// İkiz birincili kaybettiğini düşünüyor: hakem onaylarsa devralır.
    fn ft_claim(&self, guest: Guest, standby: Standby) {
        let vmid = guest.vmid;
        let Some(witness) = self.peers().peer(&standby.witness) else { return };
        self.0.ft.with(true, vmid, |r| r.busy = true);
        let me = self.clone();
        self.dispatch(move || {
            match witness.ft_arbitrate(&ft_key(&standby.origin, standby.source, standby.generation), FtSide::Secondary) {
                Ok(FtSide::Secondary) => {
                    let outcome = me.driver().ft_take_over(&guest);
                    me.ft_unlock(vmid, outcome.clone());
                    me.mutate(|c| {
                        c.standbys.retain(|s| s.vmid != vmid);
                        if let Ok(g) = c.guest_mut_for_import(vmid) {
                            if outcome.is_ok() {
                                (g.status, g.started_at) = (GuestStatus::Running, Some(now()));
                            }
                            // Sonraki açılış olağan düzendedir; yeniden korumak için korumayı burada açmak gerekir.
                            g.hw.ft = false;
                            g.tags.push(format!("devralindi-{}", standby.origin));
                            g.tags.retain(|t| redox_core::valid_tag(t));
                            g.tags.truncate(redox_core::MAX_TAGS);
                        }
                    });
                    me.0.ft.forget(true, vmid);
                    me.ft_log(vmid, format!("ft takeover {}/{}", standby.origin, standby.source), outcome, SCHEDULER);
                }
                Ok(FtSide::Primary) => me.ft_drop_standby(&standby),
                // Hakeme ulaşılamıyor: kopan bu sunucu olabilir. Beklenir.
                Err(_) => me.0.ft.with(true, vmid, |r| r.busy = false),
            }
        });
    }

    // ---------- Hakem ----------

    pub fn ft_vote(&self, key: &str, side: FtSide) -> Result<FtSide, Error> {
        self.mutate(|c| c.ft_arbitrate(key, side, now()))
    }

    pub fn ft_winner(&self, key: &str) -> Option<FtSide> {
        self.snapshot().ft_votes.iter().find(|v| v.key == key).map(|v| v.winner)
    }
}
