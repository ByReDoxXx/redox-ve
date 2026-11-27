//! Yük dengeleme önerisi: bu sunucu sıkıştığında, hangi sanal makinenin hangi eş sunucuya canlı taşınmasının
//! işe yarayacağını hesaplar.
//!
//! Olağan durumda yalnızca öneridir; sunucu `--auto-balance` ile başlatılırsa ilk öneri, koşul bir süre
//! sürdüyse kendiliğinden uygulanır (`AutoBalance`). Yerleştirme etiketleri (`redox_core::affinity`) gözetilir.
//! Öneri, eşlerin son okunan durumuna dayanır (birkaç
//! saniye eski olabilir) ve basit bir kuralla üretilir: bu sunucunun işlemcisi ya da belleği eşiğin üstündeyse,
//! darboğazı en çok rahatlatan ve bir eşe o eşi sıkıştırmadan sığan konuk seçilir. Konukların gelecekteki yükü
//! tahmin edilmez.

use super::*;
use crate::peers::PeerView;
use redox_core::affinity;
use serde::Serialize;

/// Kendiliğinden dengelemede koşul bu kadar saniye kesintisiz sürmelidir.
pub const AUTO_HOLD_SECS: u64 = 300;
/// İki kendiliğinden taşıma arasındaki en az süre.
pub const AUTO_COOLDOWN_SECS: u64 = 900;
/// Kendiliğinden taşınan konuk bu süre içinde yeniden taşınmaz.
pub const AUTO_GUEST_REST_SECS: u64 = 3600;

/// Kendiliğinden dengelemenin turlar arasında hatırladıkları. Anlık sıçramalar ve ileri geri taşımalar
/// olmasın diye bekler: koşul sürmeli, taşımalar seyrek olmalı, aynı konuk dinlenmeli.
#[derive(Debug, Default)]
pub struct AutoBalance {
    since: Option<u64>,
    last_move: Option<u64>,
    moved: Vec<(VmId, u64)>,
}

impl AutoBalance {
    /// Şimdi uygulanacak taşıma; yoksa `None`. Döndürülen taşıma yapılmış sayılır.
    pub fn decide(&mut self, balance: &Balance, now: u64) -> Option<Move> {
        self.moved.retain(|(_, at)| now.saturating_sub(*at) < AUTO_GUEST_REST_SECS);
        if balance.moves.is_empty() {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now);
        if now.saturating_sub(since) < AUTO_HOLD_SECS || self.last_move.is_some_and(|at| now.saturating_sub(at) < AUTO_COOLDOWN_SECS) {
            return None;
        }
        let chosen = balance.moves.iter().find(|m| !self.moved.iter().any(|(vmid, _)| *vmid == m.vmid))?.clone();
        self.last_move = Some(now);
        self.since = None;
        self.moved.push((chosen.vmid, now));
        Some(chosen)
    }
}

/// Bu yüzdenin üstündeki sunucu "sıkışmış" sayılır.
pub const BUSY_PCT: u64 = 80;
/// Taşınan konuk, hedef eşi bu yüzdenin üstüne çıkarmamalıdır.
pub const SAFE_PCT: u64 = 70;
/// Bir seferde en fazla bu kadar öneri üretilir.
pub const MAX_MOVES: usize = 3;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Move {
    pub vmid: VmId,
    pub name: String,
    pub peer: String,
    /// Öneri uygulanırsa bu sunucunun ve eşin tahmini bellek doluluğu (yüzde).
    pub here_mem_pct: u64,
    pub peer_mem_pct: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Balance {
    pub cpu_pct: u64,
    pub mem_pct: u64,
    /// Sunucu eşiğin üstünde mi?
    pub busy: bool,
    pub moves: Vec<Move>,
    /// Sıkışıkken öneri üretilemediyse nedeni.
    pub note: Option<String>,
    /// Şu anda bozulmuş yerleştirme kuralları.
    pub violations: Vec<String>,
}

fn pct(used: u64, total: u64) -> u64 {
    (used * 100).checked_div(total).unwrap_or(0)
}

/// Öneriyi hesaplar. Saf işlevdir: `local` bu sunucunun kaydı, `peers` eşlerin son okunan durumudur.
pub fn recommend(local: &Cluster, peers: &[PeerView]) -> Balance {
    let Some(node) = local.nodes.first() else {
        return Balance { cpu_pct: 0, mem_pct: 0, busy: false, moves: Vec::new(), note: None, violations: Vec::new() };
    };
    let (mut mem_used, mem_total, cpu) = (node.mem_used_mb, node.mem_total_mb, u64::from(node.cpu_pct));
    let mem = pct(mem_used, mem_total);
    let busy = cpu >= BUSY_PCT || mem >= BUSY_PCT;
    let mut balance = Balance { cpu_pct: cpu, mem_pct: mem, busy, moves: Vec::new(), note: None, violations: Vec::new() };
    let apart = affinity::apart_violations(&local.guests);
    for (tag, members) in &apart {
        balance.violations.push(format!("{tag}: {} aynı sunucuda çalışıyor", members.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
    }
    for (tag, peer) in affinity::together_violations(&local.guests, peers.iter().filter(|v| v.online()).map(|v| (v.name.as_str(), v.guests.as_slice()))) {
        balance.violations.push(format!("{tag}: üyeleri bu sunucuda ve {peer} sunucusunda ayrı çalışıyor"));
    }
    if !busy && apart.is_empty() {
        return balance;
    }
    // Eşlerin boş yeri: (ad, kullanılan bellek, toplam bellek, çekirdek, işlemci yüzdesi, dolu kimlikler).
    let mut targets: Vec<(String, u64, u64, u32, u64, Vec<VmId>)> = peers
        .iter()
        .filter(|v| v.online())
        .filter_map(|v| {
            let n = v.nodes.first()?;
            // Buradan taşınmış konuğun eşte kalan kapalı kopyası kimliği tutmaz: konuk geri gidebilir.
            let taken = v.guests.iter().filter(|g| !(g.status == redox_core::GuestStatus::Stopped && g.tags.iter().any(|t| t == redox_core::MIGRATED_TAG))).map(|g| g.vmid).collect();
            Some((v.name.clone(), n.mem_used_mb, n.mem_total_mb, n.cores, u64::from(n.cpu_pct), taken))
        })
        .collect();
    if targets.is_empty() {
        balance.note = Some("erişilebilen eş sunucu yok".to_string());
        return balance;
    }
    // Taşınabilecek konuklar: çalışan, kilitsiz, canlı taşınabilen sanal makineler. Darboğaz bellekse en çok
    // bellek kullanan, işlemciyse en çok işlemci kullanan önce denenir.
    let memory_bound = mem >= cpu;
    let mut guests: Vec<&Guest> = local.guests.iter().filter(|g| g.status == redox_core::GuestStatus::Running && g.lock.is_none() && redox_core::migrate_blocker(g).is_none() && local.mirror(g.vmid).is_none()).collect();
    guests.sort_by_key(|g| std::cmp::Reverse(if memory_bound { g.mem_used_mb } else { u64::from(g.cpu_pct) * u64::from(g.cores) }));
    let mut blocked = 0;
    guests.retain(|g| !affinity::pinned(g) && affinity::bound_here(g, &local.guests).is_none());
    // Ayrı tutulması gerekenler: grubun ilk üyesi kalır, ötekiler başka sunucuya önerilir.
    let must_leave: Vec<(VmId, String)> = apart.iter().flat_map(|(tag, members)| members.iter().skip(1).map(move |vmid| (*vmid, tag.clone()))).collect();
    guests.sort_by_key(|g| !must_leave.iter().any(|(vmid, _)| *vmid == g.vmid));
    // Bu turda hedefe önerilen konukların etiketleri de hesaba katılır.
    let mut planned: Vec<(String, Guest)> = Vec::new();
    for g in guests {
        let leaving = must_leave.iter().find(|(vmid, _)| *vmid == g.vmid).map(|(_, tag)| tag.clone());
        let relieved = !busy || (pct(mem_used, mem_total) < BUSY_PCT && memory_bound);
        if balance.moves.len() >= MAX_MOVES || (relieved && leaving.is_none()) {
            if balance.moves.len() >= MAX_MOVES {
                break;
            }
            continue;
        }
        // En boş eş: konuk sığmalı (çekirdek ve bellek), kimliği orada boş olmalı ve eş güvenli sınırı aşmamalı.
        let fit = targets
            .iter_mut()
            .filter(|(_, used, total, cores, peer_cpu, taken)| g.cores <= *cores && !taken.contains(&g.vmid) && *peer_cpu < SAFE_PCT && pct(used + g.mem_total_mb, *total) <= SAFE_PCT)
            .filter(|(name, ..)| {
                let there = peers.iter().filter(|v| &v.name == name).flat_map(|v| &v.guests);
                let coming = planned.iter().filter(|(to, _)| to == name).map(|(_, g)| g);
                affinity::apart_conflict(g, there.chain(coming)).is_none()
            })
            .min_by_key(|(_, used, total, ..)| pct(*used, *total));
        let Some((peer, used, total, _, _, taken)) = fit else {
            blocked += 1;
            continue;
        };
        *used += g.mem_total_mb;
        taken.push(g.vmid);
        planned.push((peer.clone(), g.clone()));
        mem_used = mem_used.saturating_sub(g.mem_used_mb);
        balance.moves.push(Move {
            vmid: g.vmid,
            name: g.name.clone(),
            peer: peer.clone(),
            here_mem_pct: pct(mem_used, mem_total),
            peer_mem_pct: pct(*used, *total),
            reason: match leaving {
                Some(tag) => format!("{tag} kuralı: aynı gruptan başka bir konukla aynı sunucuda"),
                None if memory_bound => format!("{} MB bellek kullanıyor", g.mem_used_mb),
                None => format!("{} çekirdekte %{} işlemci kullanıyor", g.cores, g.cpu_pct),
            },
        });
    }
    if balance.moves.is_empty() {
        balance.note = Some(if blocked > 0 { "taşınabilecek konuklar hiçbir eşe güvenli biçimde sığmıyor (eşler dolu ya da kimlikler orada kullanılıyor)".to_string() } else { "canlı taşınabilecek çalışan sanal makine yok".to_string() });
    }
    balance
}

impl AppState {
    /// Bu sunucu için yük dengeleme önerisi.
    pub fn balance(&self) -> Balance {
        recommend(&self.snapshot(), &self.peers().views())
    }

    /// Kendiliğinden dengelemenin bir turu. Bakımda ve kilit kipinde hiçbir şey taşınmaz.
    pub fn auto_balance_tick(&self, memory: &mut AutoBalance, as_name: Option<&str>, at: u64) {
        let frozen = {
            let c = self.lock();
            c.maintenance || c.lockdown
        };
        if frozen || !self.driver().live_migration() {
            *memory = AutoBalance::default();
            return;
        }
        let balance = self.balance();
        let Some(chosen) = memory.decide(&balance, at) else { return };
        if let Err(e) = self.migrate_to_peer(&chosen.peer, chosen.vmid, as_name, SCHEDULER) {
            let outcome: Result<(), String> = Err(e.to_string());
            self.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, SCHEDULER, format!("vm send {} {}", chosen.vmid, chosen.peer), &outcome, now(), Some(chosen.vmid));
            });
        }
    }

    /// Bu sunucudaki çalışan sanal makineyi eşe canlı taşıtır: eşe "bu konuğu al" denir ve iş eşte yürür.
    /// Sonuç iki sunucunun görev günlüğüne yazılır.
    pub fn migrate_to_peer(&self, peer_name: &str, vmid: VmId, as_name: Option<&str>, user: &str) -> Result<(), Error> {
        let peer = self.peers().peer(peer_name).ok_or_else(|| Error::InvalidSpec(format!("\"{peer_name}\" adında bir eş sunucu tanımlı değil")))?;
        self.lock().check_migrate_out(vmid)?;
        if !self.driver().live_migration() {
            return Err(Error::InvalidSpec("bu sürücü canlı taşımayı desteklemiyor (gerçek sürücü gerekir)".to_string()));
        }
        let me_name = self.name_on_peer(&peer, as_name).map_err(Error::InvalidSpec)?;
        peer.migrate_pull(&me_name, vmid).map_err(Error::Driver)?;
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("vm send {vmid} {peer_name}"), &Ok(()), now(), Some(vmid));
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::{GuestStatus, Node, NodeStatus};

    fn peer(name: &str, mem_used: u64, mem_total: u64, cpu: u8, guests: Vec<Guest>) -> PeerView {
        PeerView {
            vswitches: None,
            name: name.into(),
            url: format!("https://{name}"),
            checked: Some(1),
            error: None,
            cluster: name.into(),
            nodes: vec![Node { id: name.into(), status: NodeStatus::Online, cores: 32, cpu_pct: cpu, mem_used_mb: mem_used, mem_total_mb: mem_total, uptime_secs: 1, kernel: String::new(), ip: String::new() }],
            guests,
        }
    }

    /// Tek düğümlü yerel küme: 64 GB bellek, çalışan üç sanal makine.
    fn local(mem_used: u64, cpu: u8) -> Cluster {
        let mut c = Cluster::demo(1_700_000_000);
        let node = c.nodes[0].id.clone();
        c.nodes.truncate(1);
        (c.nodes[0].mem_total_mb, c.nodes[0].mem_used_mb, c.nodes[0].cpu_pct) = (65_536, mem_used, cpu);
        c.guests.retain(|g| g.node == node && g.kind == redox_core::GuestKind::Vm);
        for (i, g) in c.guests.iter_mut().enumerate() {
            (g.status, g.lock, g.mem_total_mb, g.mem_used_mb, g.cores, g.cpu_pct) = (GuestStatus::Running, None, 8192 * (i as u64 + 1), 6000 * (i as u64 + 1), 2, 10 * (i as u8 + 1));
            g.iso = None;
        }
        assert!(c.guests.len() >= 2, "örnek veride bu düğümde en az iki sanal makine olmalı");
        c
    }

    #[test]
    fn placement_rules_steer_the_recommendation() {
        let mut c = local(62_000, 30);
        let plain = recommend(&c, &[peer("ankara", 1000, 262_144, 5, Vec::new())]);
        assert!(!plain.moves.is_empty() && plain.violations.is_empty());
        let first = plain.moves[0].vmid;
        // Sabit konuk ve "birlikte" grubundaki konuk önerilmez.
        c.guests.iter_mut().find(|g| g.vmid == first).unwrap().tags = vec!["sabit".into()];
        assert!(recommend(&c, &[peer("ankara", 1000, 262_144, 5, Vec::new())]).moves.iter().all(|m| m.vmid != first));
        // Hedefte aynı "ayrı" grubundan çalışan konuk varsa o eşe önerilmez.
        for g in &mut c.guests {
            g.tags = vec!["ayri-db".into()];
        }
        let mut there = c.guests[0].clone();
        there.vmid = 900;
        let blocked = recommend(&c, &[peer("ankara", 1000, 262_144, 5, vec![there])]);
        assert!(blocked.moves.is_empty() && !blocked.violations.is_empty(), "{blocked:?}");
        // Sunucu boşken de aynı sunucudaki "ayrı" üyeler için taşıma önerilir; her eşe en çok biri.
        let mut calm = local(10_000, 5);
        let mut third = calm.guests[1].clone();
        third.vmid = 950;
        calm.guests.push(third);
        for g in &mut calm.guests {
            g.tags = vec!["ayri-db".into()];
        }
        let spread = recommend(&calm, &[peer("ankara", 1000, 262_144, 5, Vec::new()), peer("izmir", 1000, 262_144, 5, Vec::new())]);
        assert!(!spread.busy && spread.moves.len() == 2 && spread.moves[0].peer != spread.moves[1].peer, "{spread:?}");
        assert!(spread.moves.iter().all(|m| m.reason.contains("ayri-db")) && spread.moves.iter().all(|m| m.vmid != calm.guests[0].vmid), "ilk üye yerinde kalır");
    }

    #[test]
    fn automatic_moves_wait_for_the_condition_and_rest_between_moves() {
        let move_of = |vmid| Move { vmid, name: String::new(), peer: "ankara".into(), here_mem_pct: 0, peer_mem_pct: 0, reason: String::new() };
        let wanting = |vmids: &[u32]| Balance { cpu_pct: 0, mem_pct: 90, busy: true, moves: vmids.iter().map(|v| move_of(*v)).collect(), note: None, violations: Vec::new() };
        let calm = wanting(&[]);
        let mut auto = AutoBalance::default();
        let t = 10_000;
        assert!(auto.decide(&wanting(&[1, 2]), t).is_none(), "hemen taşımaz");
        assert!(auto.decide(&wanting(&[1, 2]), t + AUTO_HOLD_SECS - 1).is_none());
        assert!(auto.decide(&calm, t + 10).is_none());
        assert!(auto.decide(&wanting(&[1]), t + AUTO_HOLD_SECS).is_none(), "koşul kesilince süre baştan sayılır");
        let t = t + 2 * AUTO_HOLD_SECS;
        assert_eq!(auto.decide(&wanting(&[1, 2]), t).map(|m| m.vmid), Some(1));
        assert!(auto.decide(&wanting(&[1, 2]), t + AUTO_HOLD_SECS + 1).is_none(), "taşımalar arasında bekler");
        let later = t + AUTO_COOLDOWN_SECS + 1;
        assert_eq!(auto.decide(&wanting(&[1, 2]), later).map(|m| m.vmid), Some(2), "az önce taşınan konuk dinlenir");
        auto.decide(&wanting(&[1]), later + AUTO_COOLDOWN_SECS + 1);
        assert!(auto.decide(&wanting(&[1]), later + AUTO_COOLDOWN_SECS + AUTO_HOLD_SECS + 2).is_none());
        assert_eq!(auto.decide(&wanting(&[1]), t + AUTO_GUEST_REST_SECS + AUTO_COOLDOWN_SECS).map(|m| m.vmid), Some(1), "dinlenme bitince yeniden taşınabilir");
    }

    #[test]
    fn nothing_is_recommended_while_the_server_is_not_busy() {
        let b = recommend(&local(20_000, 30), &[peer("ankara", 1000, 65_536, 5, Vec::new())]);
        assert_eq!((b.busy, b.moves.len(), b.note), (false, 0, None));
        assert_eq!(b.mem_pct, 30);
    }

    #[test]
    fn a_busy_server_is_told_which_guest_to_move_where() {
        let c = local(58_000, 40);
        let biggest = c.guests.iter().max_by_key(|g| g.mem_used_mb).unwrap().clone();
        // İki eş: biri dolu, biri boş. En çok bellek kullanan konuk boş olana önerilir.
        let peers = [peer("dolu", 60_000, 65_536, 20, Vec::new()), peer("bos", 4000, 65_536, 10, Vec::new())];
        let b = recommend(&c, &peers);
        assert!(b.busy && b.mem_pct == 88, "{b:?}");
        let first = &b.moves[0];
        assert_eq!((first.vmid, first.peer.as_str()), (biggest.vmid, "bos"));
        assert!(first.here_mem_pct < 88 && first.peer_mem_pct <= SAFE_PCT && first.reason.contains("MB bellek"), "{first:?}");
        // Öneriler birbirini hesaba katar: aynı konuk iki kez önerilmez, eşik altına inince durulur.
        let ids: Vec<VmId> = b.moves.iter().map(|m| m.vmid).collect();
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(ids, unique);
        assert!(b.moves.len() <= MAX_MOVES && b.moves.last().unwrap().here_mem_pct < 88);

        // İşlemci darboğazında gerekçe işlemcidir ve en çok işlemci kullanan konuk seçilir.
        let c = local(20_000, 92);
        let hungry = c.guests.iter().max_by_key(|g| u64::from(g.cpu_pct) * u64::from(g.cores)).unwrap().vmid;
        let b = recommend(&c, &peers);
        assert!(b.busy && b.moves[0].vmid == hungry && b.moves[0].reason.contains("işlemci"), "{b:?}");
    }

    #[test]
    fn guests_that_cannot_move_or_do_not_fit_are_not_recommended() {
        let mut c = local(58_000, 40);
        // Erişilebilen eş yoksa bunu söyler.
        let mut offline = peer("kapali", 0, 65_536, 0, Vec::new());
        offline.error = Some("erişilemiyor".into());
        assert_eq!(recommend(&c, &[offline]).note.as_deref(), Some("erişilebilen eş sunucu yok"));
        assert_eq!(recommend(&c, &[]).note.as_deref(), Some("erişilebilen eş sunucu yok"));
        // Eş doluysa ya da kimlik orada kullanılıyorsa konuk önerilmez.
        let full = recommend(&c, &[peer("dolu", 60_000, 65_536, 20, Vec::new())]);
        assert!(full.moves.is_empty() && full.note.as_deref().is_some_and(|n| n.contains("sığmıyor")), "{full:?}");
        let same_ids: Vec<Guest> = c.guests.clone();
        let taken = recommend(&c, &[peer("bos", 4000, 262_144, 10, same_ids.clone())]);
        assert!(taken.moves.is_empty(), "{taken:?}");
        // Buradan taşınmış konuğun eşte kalan kapalı kopyası engel değildir.
        let leftovers: Vec<Guest> = same_ids.into_iter().map(|mut g| { g.status = GuestStatus::Stopped; g.tags = vec![redox_core::MIGRATED_TAG.to_string()]; g }).collect();
        assert!(!recommend(&c, &[peer("bos", 4000, 262_144, 10, leftovers)]).moves.is_empty());
        // Canlı taşınamayan (UEFI) ve kilitli konuklar önerilmez.
        for g in c.guests.iter_mut() {
            g.hw.firmware = redox_core::hw::Firmware::Uefi;
        }
        let none = recommend(&c, &[peer("bos", 4000, 65_536, 10, Vec::new())]);
        assert!(none.moves.is_empty() && none.note.as_deref().is_some_and(|n| n.contains("canlı taşınabilecek")), "{none:?}");
    }
}
