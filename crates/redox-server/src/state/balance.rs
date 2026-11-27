//! Yük dengeleme önerisi: bu sunucu sıkıştığında, hangi sanal makinenin hangi eş sunucuya canlı taşınmasının
//! işe yarayacağını hesaplar.
//!
//! Yalnızca öneridir: kendiliğinden hiçbir konuk taşınmaz. Öneri, eşlerin son okunan durumuna dayanır (birkaç
//! saniye eski olabilir) ve basit bir kuralla üretilir: bu sunucunun işlemcisi ya da belleği eşiğin üstündeyse,
//! darboğazı en çok rahatlatan ve bir eşe o eşi sıkıştırmadan sığan konuk seçilir. Konukların gelecekteki yükü
//! tahmin edilmez.

use super::*;
use crate::peers::PeerView;
use serde::Serialize;

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
}

fn pct(used: u64, total: u64) -> u64 {
    (used * 100).checked_div(total).unwrap_or(0)
}

/// Öneriyi hesaplar. Saf işlevdir: `local` bu sunucunun kaydı, `peers` eşlerin son okunan durumudur.
pub fn recommend(local: &Cluster, peers: &[PeerView]) -> Balance {
    let Some(node) = local.nodes.first() else {
        return Balance { cpu_pct: 0, mem_pct: 0, busy: false, moves: Vec::new(), note: None };
    };
    let (mut mem_used, mem_total, cpu) = (node.mem_used_mb, node.mem_total_mb, u64::from(node.cpu_pct));
    let mem = pct(mem_used, mem_total);
    let busy = cpu >= BUSY_PCT || mem >= BUSY_PCT;
    let mut balance = Balance { cpu_pct: cpu, mem_pct: mem, busy, moves: Vec::new(), note: None };
    if !busy {
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
    let mut guests: Vec<&Guest> = local.guests.iter().filter(|g| g.status == redox_core::GuestStatus::Running && g.lock.is_none() && redox_core::migrate_blocker(g).is_none()).collect();
    guests.sort_by_key(|g| std::cmp::Reverse(if memory_bound { g.mem_used_mb } else { u64::from(g.cpu_pct) * u64::from(g.cores) }));
    let mut blocked = 0;
    for g in guests {
        if balance.moves.len() >= MAX_MOVES || (pct(mem_used, mem_total) < BUSY_PCT && memory_bound) {
            break;
        }
        // En boş eş: konuk sığmalı (çekirdek ve bellek), kimliği orada boş olmalı ve eş güvenli sınırı aşmamalı.
        let fit = targets
            .iter_mut()
            .filter(|(_, used, total, cores, peer_cpu, taken)| g.cores <= *cores && !taken.contains(&g.vmid) && *peer_cpu < SAFE_PCT && pct(used + g.mem_total_mb, *total) <= SAFE_PCT)
            .min_by_key(|(_, used, total, ..)| pct(*used, *total));
        let Some((peer, used, total, _, _, taken)) = fit else {
            blocked += 1;
            continue;
        };
        *used += g.mem_total_mb;
        taken.push(g.vmid);
        mem_used = mem_used.saturating_sub(g.mem_used_mb);
        balance.moves.push(Move {
            vmid: g.vmid,
            name: g.name.clone(),
            peer: peer.clone(),
            here_mem_pct: pct(mem_used, mem_total),
            peer_mem_pct: pct(*used, *total),
            reason: if memory_bound { format!("{} MB bellek kullanıyor", g.mem_used_mb) } else { format!("{} çekirdekte %{} işlemci kullanıyor", g.cores, g.cpu_pct) },
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
