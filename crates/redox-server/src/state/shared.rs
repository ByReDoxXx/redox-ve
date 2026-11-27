//! Ortak depo: paylaşımın izlenmesi, kapalı konuğun devri, susmuş sunucudan devralma ve diskleri
//! kopyalamayan canlı taşımanın hedef tarafı.

use std::collections::HashMap;

use super::*;
use crate::shared::{self, Owner};
use serde::{Deserialize, Serialize};

/// Bir sunucunun sayacı bu kadar saniye değişmezse susmuş sayılır ve konukları devralınabilir.
pub const QUIET_SECS: u64 = 30;
/// Paylaşımın izlenme aralığı.
pub const TICK_SECS: u64 = 5;

/// Eşin, ortak depodaki konuğu için söylediği: kim olduğu ve hangi paylaşımda durduğu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub server: String,
    pub share: String,
}

/// Paylaşımdaki bir konuğun bu sunucudan görünüşü.
#[derive(Debug, Clone, Serialize)]
pub struct SharedItem {
    pub storage: String,
    pub vmid: VmId,
    pub name: String,
    pub owner: String,
    pub mine: bool,
    /// Bu sunucuda devredilmemiş bir kaydı var mı?
    pub here: bool,
    /// Sahibi susmuş mu (devralınabilir mi)? Yeterince uzun izlenmediyse `false`.
    pub quiet: bool,
}

/// İzleme turlarının hatırladıkları.
#[derive(Debug, Default)]
pub struct SharedWatch {
    counter: u64,
    /// `(paylaşım, sunucu)` → son görülen sayaç ve onun ilk görüldüğü an.
    seen: HashMap<(String, String), (Option<String>, u64)>,
    /// Okunamayan paylaşımlar için son hata (aynı hata her turda günlüğe yazılmasın).
    errors: HashMap<String, String>,
}

impl SharedWatch {
    fn observe(&mut self, share: &str, server: &str, value: Option<String>, at: u64) {
        let entry = self.seen.entry((share.to_string(), server.to_string())).or_insert((value.clone(), at));
        if entry.0 != value {
            *entry = (value, at);
        }
    }

    /// Sayaç en az `QUIET_SECS` süredir değişmeden izlendi mi?
    fn quiet(&self, share: &str, server: &str, at: u64) -> bool {
        self.seen.get(&(share.to_string(), server.to_string())).is_some_and(|(_, since)| at.saturating_sub(*since) >= QUIET_SECS)
    }
}

impl AppState {
    fn shared_dir_of(&self, storage: &str) -> Option<std::path::PathBuf> {
        self.driver().shared_storages().into_iter().find(|(id, _)| id == storage).map(|(_, dir)| dir)
    }

    fn shared_watch(&self) -> std::sync::MutexGuard<'_, SharedWatch> {
        self.0.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Açılışta: ortak depodaki kayıtlı konuklardan sahibi olmayanlar (paylaşım yeni işaretlendiyse) üstlenilir.
    /// Başkasınınkine dokunulmaz. Üstlenilemeyenlerin iletilerini döndürür.
    pub fn shared_claim_existing(&self) -> Vec<String> {
        let me = self.driver().server_id();
        let mut problems = Vec::new();
        for g in self.snapshot().guests.iter().filter(|g| g.hw.handed_to.is_none()) {
            let Some(dir) = self.shared_dir_of(&g.storage) else { continue };
            match shared::read_owner(&dir, g.vmid) {
                Ok(None) => {
                    if let Err(e) = shared::claim(&dir, g.vmid, &Owner { server: me.clone(), node: g.node.clone() }) {
                        problems.push(e);
                    }
                }
                Ok(Some(_)) => {}
                Err(e) => problems.push(e),
            }
        }
        problems
    }

    /// Bir izleme turu: sayacını artırır, öbür sunucuların sayaçlarını izler, sahibi olduğu konukların kaydını
    /// yazar ve sahipliğini yitirdiği konuğu hemen kapatır (aynı diske iki sunucu yazmasın).
    pub fn shared_tick(&self, at: u64) {
        let storages = self.driver().shared_storages();
        if storages.is_empty() {
            return;
        }
        let me = self.driver().server_id();
        let counter = {
            let mut watch = self.shared_watch();
            watch.counter += 1;
            watch.counter
        };
        let cluster = self.snapshot();
        for (storage, dir) in &storages {
            let outcome = shared::beat(dir, &me, counter).and_then(|()| shared::scan(dir));
            let entries = match outcome {
                Ok(entries) => {
                    self.shared_watch().errors.remove(storage);
                    entries
                }
                Err(e) => {
                    let new = self.shared_watch().errors.insert(storage.clone(), e.clone()) != Some(e.clone());
                    if new {
                        self.shared_log(format!("shared {storage}"), SCHEDULER, &Err(e), None);
                    }
                    continue;
                }
            };
            for owner in entries.iter().map(|e| &e.owner.server).filter(|server| **server != me) {
                let value = shared::read_beat(dir, owner).ok().flatten();
                self.shared_watch().observe(storage, owner, value, at);
            }
            // Süren görevi olan konuğa dokunulmaz: taşıma sırasında sahiplik bilerek değişir.
            for g in cluster.guests.iter().filter(|g| &g.storage == storage && g.hw.handed_to.is_none() && g.lock.is_none()) {
                match entries.iter().find(|e| e.vmid == g.vmid) {
                    Some(entry) if entry.owner.server != me => self.shared_lost(g.vmid, &entry.owner.node),
                    Some(_) => {
                        if let Err(e) = shared::publish(dir, g, &me) {
                            eprintln!("uyarı: {} konuğunun paylaşım kaydı yazılamadı: {e}", g.vmid);
                        }
                    }
                    None => {}
                }
            }
        }
    }

    /// Konuğun sahipliği başka sunucuya geçmiş: kayıt işaretlenir ve burada çalışıyorsa konuk zorla kapatılır.
    fn shared_lost(&self, vmid: VmId, to: &str) {
        // Karar kaydın son haline göre verilir: bu arada bir görev başlamış olabilir.
        let guest = self.mutate(|c| {
            let g = c.guest(vmid).ok()?;
            if g.lock.is_some() || g.hw.handed_to.is_some() {
                return None;
            }
            let guest = g.clone();
            c.mark_handed_over(vmid, to);
            Some(guest)
        });
        let Some(guest) = guest else { return };
        let outcome = if guest.status == redox_core::GuestStatus::Stopped {
            Ok(())
        } else {
            self.driver().apply(&guest, redox_core::Action::Stop)
        };
        self.mutate(|c| {
            if outcome.is_ok() {
                if let Ok(g) = c.guest_mut_for_import(vmid) {
                    g.status = redox_core::GuestStatus::Stopped;
                    g.started_at = None;
                }
            }
        });
        let message = format!("{vmid} konuğunun diskleri {to} sunucusuna geçmiş; buradaki kopya kapatıldı");
        self.shared_log(format!("shared lost {vmid}"), SCHEDULER, &outcome.map_err(|e| format!("{message}; kapatılamadı: {e}")), Some(vmid));
    }

    fn shared_log(&self, command: String, user: &str, outcome: &Result<(), String>, vmid: Option<VmId>) {
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, command, outcome, now(), vmid);
        });
    }

    /// Paylaşımlardaki bütün konuklar.
    pub fn shared_list(&self, at: u64) -> Vec<SharedItem> {
        let me = self.driver().server_id();
        let c = self.snapshot();
        let mut items = Vec::new();
        for (storage, dir) in self.driver().shared_storages() {
            for entry in shared::scan(&dir).unwrap_or_default() {
                let mine = entry.owner.server == me;
                items.push(SharedItem {
                    storage: storage.clone(),
                    vmid: entry.vmid,
                    name: entry.guest.as_ref().map(|g| g.name.clone()).unwrap_or_default(),
                    quiet: !mine && self.shared_watch().quiet(&storage, &entry.owner.server, at),
                    owner: entry.owner.node,
                    mine,
                    here: c.guests.iter().any(|g| g.vmid == entry.vmid && g.storage == storage && g.hw.handed_to.is_none()),
                });
            }
        }
        items
    }

    /// Kaynak: konuğum ortak depodaysa kimliğim ve paylaşımın kimliği.
    pub fn shared_offer(&self, vmid: VmId) -> Result<Option<Offer>, Error> {
        let storage = self.lock().guest(vmid)?.storage.clone();
        let Some(dir) = self.shared_dir_of(&storage) else { return Ok(None) };
        let server = self.driver().server_id();
        shared::require(&dir, vmid, &server).map_err(Error::Driver)?;
        Ok(Some(Offer { server, share: shared::share_id(&dir).map_err(Error::Driver)? }))
    }

    /// Eşin söylediği paylaşım bu sunucuda hangi depolama alanı? Yalnızca konuğun sahiplik kaydı burada da
    /// görünüyor ve eşi gösteriyorsa eşleşir: aynı dizine bakıldığının kanıtı budur.
    pub(super) fn shared_match(&self, offer: &Offer, vmid: VmId) -> Option<(String, std::path::PathBuf)> {
        self.driver().shared_storages().into_iter().find(|(_, dir)| {
            shared::share_id(dir).is_ok_and(|id| id == offer.share) && shared::read_owner(dir, vmid).ok().flatten().is_some_and(|o| o.server == offer.server)
        })
    }

    /// Kaynak: kapalı konuğun disk sahipliğini `to` sunucusuna geçirir. Burada yalnızca kaydı kalır.
    pub fn shared_release(&self, vmid: VmId, to: Owner, user: &str) -> Result<(), Error> {
        let named = !to.node.is_empty() && to.node.len() <= 64 && to.node.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b));
        if !shared::valid_id(&to.server) || !named || to.server == self.driver().server_id() {
            return Err(Error::InvalidSpec("devralacak sunucunun bilgisi geçersiz".to_string()));
        }
        let me = self.driver().server_id();
        // Denetim, devir ve işaretleme tek kilit altında: arada konuk başlatılamaz.
        let outcome = self.mutate(|c| {
            let storage = c.check_release(vmid)?.storage.clone();
            let dir = self.shared_dir_of(&storage).ok_or_else(|| Error::InvalidSpec(format!("{vmid} ortak depoda değil")))?;
            let guest = c.guest(vmid)?.clone();
            shared::publish(&dir, &guest, &me).map_err(Error::Driver)?;
            shared::transfer(&dir, vmid, &me, &to).map_err(Error::Driver)?;
            c.mark_handed_over(vmid, &to.node);
            let node = guest.node.clone();
            c.log(&node, user, format!("shared release {vmid} {}", to.node), &Ok(()), now(), Some(vmid));
            Ok::<_, Error>(())
        });
        outcome
    }

    /// Bu sunucuda `vmid` kimliği paylaşımdan gelen konuk için boş mu? Devredilmiş eski kayıt yer açar.
    pub(super) fn shared_slot(&self, vmid: VmId) -> Result<Option<VmId>, Error> {
        match self.lock().guest(vmid) {
            Err(_) => Ok(None),
            Ok(g) if g.hw.handed_to.is_some() && g.lock.is_none() => Ok(Some(vmid)),
            Ok(_) => Err(Error::InvalidSpec(format!("bu sunucuda {vmid} kimliği dolu"))),
        }
    }

    /// Paylaşımda diskleri hazır duran konuğun kaydını, disk oluşturmadan kurar. `claim_task` verilirse konuk
    /// aynı adımda o görevle kilitlenir (izleme turu araya girmesin).
    pub(super) fn shared_record(&self, remote: &Guest, storage: &str, user: &str, claim_task: Option<&str>) -> Result<(Guest, Option<TaskId>), Error> {
        let stale = self.shared_slot(remote.vmid)?;
        let mut spec = self.local_spec(remote, Some(remote.vmid))?;
        spec.storage = storage.to_string();
        self.mutate(|c| {
            if let Some(vmid) = stale {
                c.delete_guest(vmid, user, now())?;
            }
            let id = c.create_guest(&spec, user, now())?;
            for disk in &remote.hw.disks {
                c.add_disk_exact(id, disk)?;
            }
            let g = c.guest_mut_for_import(id)?;
            (g.cores, g.mem_total_mb) = (remote.cores, remote.mem_total_mb);
            (g.hw.dimms, g.hw.max_cores, g.hw.max_mem_mb, g.hw.balloon_mb) = (remote.hw.dimms.clone(), remote.hw.max_cores, remote.hw.max_mem_mb, remote.hw.balloon_mb);
            g.hw.disk_seq = remote.hw.disk_seq;
            g.snapshots = remote.snapshots.clone();
            g.tags = remote.tags.iter().filter(|t| redox_core::valid_tag(t) && *t != redox_core::MIGRATED_TAG).take(redox_core::MAX_TAGS).cloned().collect();
            let task = match claim_task {
                Some(command) => Some(c.begin_op(id, command.to_string(), user, now(), None)?),
                None => None,
            };
            Ok::<_, Error>((c.guest(id)?.clone(), task))
        })
    }

    /// Eşteki kapalı konuğu, disklerini kopyalamadan bu sunucuya alır.
    pub fn shared_take(&self, peer_name: &str, vmid: VmId, user: &str) -> Result<(), Error> {
        let peer = self.peers().peer(peer_name).ok_or_else(|| Error::InvalidSpec(format!("\"{peer_name}\" adında bir eş sunucu tanımlı değil")))?;
        let remote = peer.guest(vmid).map_err(Error::Driver)?;
        let offer = peer.shared_offer(vmid).map_err(Error::Driver)?.ok_or_else(|| Error::InvalidSpec(format!("{vmid} eş sunucuda ortak depoda değil")))?;
        let (storage, _) = self.shared_match(&offer, vmid).ok_or_else(|| Error::InvalidSpec("eşin ortak deposu bu sunucuda görünmüyor: iki sunucu aynı paylaşımı --shared-storage ile tanımlamalı".to_string()))?;
        if let Some(why) = redox_core::handover_blocker(&remote) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        if remote.hw.encrypted && !self.driver().disk_encryption() {
            return Err(Error::InvalidSpec("konuğun diskleri şifreli: bu sunucu aynı anahtarla --disk-key-file ile başlatılmalı".to_string()));
        }
        // Burada kurulamayacak konuk için devir istenmez.
        self.shared_slot(vmid)?;
        let node = self.snapshot().nodes.first().map(|n| (n.id.clone(), n.cores, n.mem_total_mb)).ok_or_else(|| Error::InvalidSpec("bu sunucuda düğüm yok".to_string()))?;
        if remote.cores > node.1 || remote.mem_total_mb > node.2 {
            return Err(Error::InvalidSpec(format!("bu sunucu {} çekirdek ve {} MB bellekli konuğu barındıramaz", remote.cores, remote.mem_total_mb)));
        }
        peer.shared_release(vmid, &Owner { server: self.driver().server_id(), node: node.0 }).map_err(Error::Driver)?;
        let outcome = self.shared_record(&remote, &storage, user, None).map(|_| ());
        let logged = outcome.map_err(|e| format!("{e}. Diskler artık bu sunucunun: nedeni giderip \"redox shared adopt {storage} {vmid}\" ile kaydı kurun"));
        self.shared_log(format!("shared take {peer_name}/{vmid}"), user, &logged, Some(vmid));
        logged.map_err(Error::Driver)?;
        self.shared_tick(now());
        Ok(())
    }

    /// Paylaşımdaki bir konuğu devralır: sahibi bu sunucuysa yalnızca kaydı kurulur; başka sunucuysa o sunucu
    /// en az `QUIET_SECS` süredir susmuş olmalıdır. Sunucunun gerçekten kapalı olduğundan emin olmak
    /// yöneticinin işidir: ağı kopmuş ama paylaşıma yazabilen bir sunucu bu denetimden geçemez, paylaşıma
    /// erişimi kopmuş ama çalışan bir sunucu ise geçer.
    pub fn shared_adopt(&self, storage: &str, vmid: VmId, user: &str, at: u64) -> Result<(), Error> {
        let dir = self.shared_dir_of(storage).ok_or_else(|| Error::InvalidSpec(format!("{storage} bu sunucuda ortak depo değil")))?;
        let me = self.driver().server_id();
        let owner = shared::read_owner(&dir, vmid).map_err(Error::Driver)?.ok_or_else(|| Error::InvalidSpec(format!("{vmid} paylaşımda kayıtlı değil")))?;
        let remote = shared::manifest(&dir, vmid).map_err(Error::Driver)?.ok_or_else(|| Error::InvalidSpec(format!("{vmid} konuğunun paylaşımda kaydı yok; devralınamaz")))?;
        if let Some(why) = redox_core::handover_blocker(&remote) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        self.shared_slot(vmid)?;
        if owner.server != me {
            if !self.shared_watch().quiet(storage, &owner.server, at) {
                return Err(Error::InvalidSpec(format!("{} sunucusu hâlâ yaşıyor olabilir: en az {QUIET_SECS} saniye susmadan devralınamaz", owner.node)));
            }
            let node = self.snapshot().nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            shared::transfer(&dir, vmid, &owner.server, &Owner { server: me, node }).map_err(Error::Driver)?;
        }
        let outcome = self.shared_record(&remote, storage, user, None).map(|_| ());
        let logged: Result<(), String> = outcome.as_ref().err().map_or(Ok(()), |e| Err(e.to_string()));
        self.shared_log(format!("shared adopt {storage}/{vmid}"), user, &logged, Some(vmid));
        outcome
    }
}

impl AppState {
    /// Ortak depo olarak işaretli depolama alanlarının adları.
    pub fn shared_storage_ids(&self) -> Vec<String> {
        self.driver().shared_storages().into_iter().map(|(id, _)| id).collect()
    }
}
