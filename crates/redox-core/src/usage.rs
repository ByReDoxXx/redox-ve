//! Kaynak ölçümü: konuk başına birikimli tüketim (faturalama ve kapasite planı için).
//!
//! Anlık yüzde değerlerinden farklı olarak bu sayaçlar yalnızca artar: konuk kapanıp açılsa da, sunucu
//! yeniden başlasa da kaldığı yerden sürer. Yönetici sıfırlayana ya da konuk silinene kadar tutulur.

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;
use crate::model::{GuestStatus, VmId};

/// İki ölçüm arası bundan uzunsa (sunucu kapalıydı, ölçüm durdu) aradaki süre sayılmaz.
pub const MAX_GAP_SECS: u64 = 120;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub vmid: VmId,
    /// Sayaçların başladığı (ya da son sıfırlandığı) an.
    pub since: u64,
    /// Konuğun açık kaldığı toplam süre.
    pub uptime_secs: u64,
    /// İşlemci tüketimi: çekirdek-milisaniye (bir çekirdeği bir saniye tam kullanmak 1000'dir).
    pub cpu_core_ms: u64,
    /// Bellek tüketimi: MB-saniye (kullanılan bellek × süre).
    pub mem_mb_secs: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    /// Son ölçüm anı; konuk kapalıyken 0.
    pub last_seen: u64,
    /// Sürücünün son bildirdiği disk sayaçları (QEMU her açılışta sıfırdan başlatır).
    pub io_seen: (u64, u64),
}

impl Usage {
    pub fn cpu_core_hours(&self) -> f64 {
        self.cpu_core_ms as f64 / 3_600_000.0
    }

    pub fn mem_gb_hours(&self) -> f64 {
        self.mem_mb_secs as f64 / (1024.0 * 3600.0)
    }

    /// Sayaçlara bir ölçüm ekler. `io`: sürücünün birikimli (okunan, yazılan) bayt sayaçları.
    fn add(&mut self, status: GuestStatus, cores: u32, cpu_pct: u8, mem_used_mb: u64, io: Option<(u64, u64)>, now: u64) {
        if status == GuestStatus::Stopped {
            self.last_seen = 0;
            self.io_seen = (0, 0);
            return;
        }
        let dt = match now.checked_sub(self.last_seen) {
            Some(dt) if self.last_seen > 0 && dt <= MAX_GAP_SECS => dt,
            _ => 0,
        };
        self.last_seen = now;
        self.uptime_secs += dt;
        self.cpu_core_ms += u64::from(cpu_pct) * u64::from(cores) * dt * 10;
        self.mem_mb_secs += mem_used_mb * dt;
        if let Some((read, written)) = io {
            // Sayaç geriye gittiyse QEMU yeniden başlamıştır: yeni değer baştan sayılır.
            let delta = |new: u64, old: u64| if new >= old { new - old } else { new };
            self.disk_read_bytes += delta(read, self.io_seen.0);
            self.disk_write_bytes += delta(written, self.io_seen.1);
            self.io_seen = (read, written);
        }
    }
}

impl Cluster {
    /// Konuğun o anki durumunu (gözlenen işlemci ve bellek kullanımı) ölçüm sayaçlarına işler.
    pub fn record_usage(&mut self, vmid: VmId, io: Option<(u64, u64)>, now: u64) {
        let Ok(g) = self.guest(vmid) else { return };
        let (status, cores, cpu, mem) = (g.status, g.cores, g.cpu_pct, g.mem_used_mb);
        let entry = match self.usage.iter().position(|u| u.vmid == vmid) {
            Some(i) => &mut self.usage[i],
            None => {
                self.usage.push(Usage { vmid, since: now, ..Usage::default() });
                self.usage.sort_by_key(|u| u.vmid);
                let i = self.usage.iter().position(|u| u.vmid == vmid).unwrap_or(0);
                &mut self.usage[i]
            }
        };
        entry.add(status, cores, cpu, mem, io, now);
    }

    /// Konuğun sayaçları; hiç ölçülmediyse sıfırlar.
    pub fn usage_of(&self, vmid: VmId) -> Usage {
        self.usage.iter().find(|u| u.vmid == vmid).cloned().unwrap_or(Usage { vmid, ..Usage::default() })
    }

    /// Sayaçları sıfırlar (fatura dönemi kapanınca).
    pub fn reset_usage(&mut self, vmid: VmId, now: u64) -> Result<(), Error> {
        self.guest(vmid)?;
        self.usage.retain(|u| u.vmid != vmid);
        self.usage.push(Usage { vmid, since: now, ..Usage::default() });
        self.usage.sort_by_key(|u| u.vmid);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;

    #[test]
    fn counters_accumulate_while_running_and_survive_restarts() {
        let mut c = Cluster::demo(NOW);
        let set = |c: &mut Cluster, status, cpu, mem| {
            let g = c.guest_mut(100).unwrap();
            (g.status, g.cores, g.cpu_pct, g.mem_used_mb) = (status, 4, cpu, mem);
        };
        // İlk ölçüm yalnızca başlangıcı işaretler.
        set(&mut c, GuestStatus::Running, 50, 2048);
        c.record_usage(100, Some((1000, 500)), NOW);
        let u = c.usage_of(100);
        assert_eq!((u.since, u.uptime_secs, u.cpu_core_ms, u.disk_read_bytes, u.disk_write_bytes), (NOW, 0, 0, 1000, 500));
        // 10 sn, 4 çekirdeğin %50'si = 20 çekirdek-saniye; 2048 MB × 10 sn.
        c.record_usage(100, Some((1500, 700)), NOW + 10);
        let u = c.usage_of(100);
        assert_eq!((u.uptime_secs, u.cpu_core_ms, u.mem_mb_secs, u.disk_read_bytes, u.disk_write_bytes), (10, 20_000, 20_480, 1500, 700));
        // Uzun boşluk (sunucu kapalıydı) sayılmaz.
        c.record_usage(100, None, NOW + 10 + MAX_GAP_SECS + 1);
        assert_eq!(c.usage_of(100).uptime_secs, 10);
        // Kapanınca sayaçlar durur; yeniden açılınca QEMU'nun sıfırlanan disk sayacı baştan eklenir.
        set(&mut c, GuestStatus::Stopped, 0, 0);
        c.record_usage(100, None, NOW + 200);
        c.record_usage(100, None, NOW + 300);
        set(&mut c, GuestStatus::Running, 100, 1024);
        c.record_usage(100, Some((40, 10)), NOW + 400);
        c.record_usage(100, Some((60, 10)), NOW + 405);
        let u = c.usage_of(100);
        assert_eq!((u.uptime_secs, u.cpu_core_ms, u.disk_read_bytes, u.disk_write_bytes), (15, 40_000, 1560, 710));
        assert!((u.cpu_core_hours() - 40.0 / 3600.0).abs() < 1e-9 && u.mem_gb_hours() > 0.0);

        // Sıfırlama ve silme.
        c.reset_usage(100, NOW + 500).unwrap();
        assert_eq!(c.usage_of(100), Usage { vmid: 100, since: NOW + 500, ..Usage::default() });
        assert!(c.reset_usage(999, NOW).is_err());
        c.record_usage(999, None, NOW);
        assert_eq!(c.usage.len(), 1, "olmayan konuk için kayıt açılmaz");
        assert_eq!(c.usage_of(101), Usage { vmid: 101, ..Usage::default() });
    }
}
