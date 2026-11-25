//! Kaynak havuzları: konukları gruplar ve gruba toplam çekirdek, bellek, disk ve konuk sayısı kotası koyar.
//!
//! Kota **ayrılan** kaynağa uygulanır (konuğun çekirdek sayısı, belleği ve disklerinin toplamı), o anki
//! kullanıma değil. Sınır 0 ise o kaynak sınırsızdır. Kota; konuğu havuza alırken, havuzdaki konuğun
//! çekirdeğini ya da belleğini artırırken, disk eklerken ya da büyütürken ve havuzdaki konuğu klonlarken
//! denetlenir. Konuk oluşturma havuzsuzdur; havuz sonradan atanır.

use serde::{Deserialize, Serialize};

use crate::cluster::Cluster;
use crate::error::Error;
use crate::model::{valid_tag, VmId};

pub const MAX_POOLS: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pool {
    /// Küçük harf, rakam ve tire; havuzun kimliğidir.
    pub name: String,
    /// Toplam çekirdek kotası; 0 sınırsız.
    pub cores: u32,
    /// Toplam bellek kotası (MB); 0 sınırsız.
    pub memory_mb: u64,
    /// Toplam disk kotası (GB, ek diskler dahil); 0 sınırsız.
    pub disk_gb: u64,
    /// En fazla konuk sayısı; 0 sınırsız.
    pub guests: u32,
}

/// Havuzdaki konukların ayrılmış kaynak toplamı.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PoolUsage {
    pub guests: u32,
    pub cores: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
}

impl Cluster {
    pub fn pool(&self, name: &str) -> Result<&Pool, Error> {
        self.pools.iter().find(|p| p.name == name).ok_or_else(|| Error::InvalidSpec(format!("{name} adında bir havuz yok")))
    }

    /// Havuzun şimdiki kullanımı; `except` verilirse o konuk sayılmaz (kendi değişikliği denetlenirken).
    pub fn pool_usage(&self, name: &str, except: Option<VmId>) -> PoolUsage {
        self.guests.iter().filter(|g| g.pool.as_deref() == Some(name) && Some(g.vmid) != except).fold(PoolUsage::default(), |u, g| PoolUsage {
            guests: u.guests + 1,
            cores: u.cores + g.cores,
            memory_mb: u.memory_mb + g.mem_total_mb,
            disk_gb: u.disk_gb + g.total_disk_gb(),
        })
    }

    /// Havuzu ekler; aynı adda havuz varsa kotalarını günceller. Kota, şimdiki kullanımın altına indirilemez.
    pub fn put_pool(&mut self, pool: Pool) -> Result<(), Error> {
        if !valid_tag(&pool.name) {
            return Err(Error::InvalidSpec("havuz adı küçük harf, rakam ve tireden oluşmalı (en fazla 24 karakter)".to_string()));
        }
        let used = self.pool_usage(&pool.name, None);
        for (limit, now, what) in [(pool.cores as u64, used.cores as u64, "çekirdek"), (pool.memory_mb, used.memory_mb, "MB bellek"), (pool.disk_gb, used.disk_gb, "GB disk"), (pool.guests as u64, used.guests as u64, "konuk")] {
            if limit != 0 && limit < now {
                return Err(Error::InvalidSpec(format!("kota şimdiki kullanımın altına inemez: {} havuzunda {now} {what} ayrılmış, istenen kota {limit}", pool.name)));
            }
        }
        let full = self.pools.len() >= MAX_POOLS;
        match self.pools.iter_mut().find(|p| p.name == pool.name) {
            Some(old) => *old = pool,
            None if full => return Err(Error::InvalidSpec(format!("en fazla {MAX_POOLS} havuz tanımlanabilir"))),
            None => {
                self.pools.push(pool);
                self.pools.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
        Ok(())
    }

    /// Havuzu siler; içinde konuk varsa önce onları havuzdan çıkarmak gerekir.
    pub fn remove_pool(&mut self, name: &str) -> Result<(), Error> {
        self.pool(name)?;
        let inside = self.pool_usage(name, None).guests;
        if inside > 0 {
            return Err(Error::InvalidSpec(format!("{name} havuzunda {inside} konuk var; önce onları havuzdan çıkarın")));
        }
        self.pools.retain(|p| p.name != name);
        Ok(())
    }

    /// `vmid` konuğu (yeni bir konuksa `None`) verilen kaynaklarla bu havuza sığar mı?
    pub fn pool_allows(&self, pool: &str, vmid: Option<VmId>, cores: u32, memory_mb: u64, disk_gb: u64) -> Result<(), Error> {
        let limits = self.pool(pool)?;
        let used = self.pool_usage(pool, vmid);
        let checks = [
            (limits.cores as u64, used.cores as u64, cores as u64, "çekirdek"),
            (limits.memory_mb, used.memory_mb, memory_mb, "MB bellek"),
            (limits.disk_gb, used.disk_gb, disk_gb, "GB disk"),
            (limits.guests as u64, used.guests as u64, 1, "konuk"),
        ];
        for (limit, now, wanted, what) in checks {
            if limit != 0 && now + wanted > limit {
                return Err(Error::InvalidSpec(format!(
                    "{pool} havuzunun kotası aşılıyor: kota {limit} {what}, öbür konuklara {now} ayrılmış, bu konuk {wanted} istiyor"
                )));
            }
        }
        Ok(())
    }

    /// Havuzdaki konuğun diski `extra_gb` kadar büyürse kota aşılır mı? Havuzsuz konukta her zaman olur.
    pub fn pool_allows_disk(&self, vmid: VmId, extra_gb: u64) -> Result<(), Error> {
        let g = self.guest(vmid)?;
        match &g.pool {
            Some(pool) => self.pool_allows(pool, Some(vmid), g.cores, g.mem_total_mb, g.total_disk_gb() + extra_gb),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GuestPatch;

    const NOW: u64 = 1_800_000_000;

    fn cluster() -> Cluster {
        Cluster::demo(NOW)
    }

    fn pool(name: &str, cores: u32, memory_mb: u64, disk_gb: u64, guests: u32) -> Pool {
        Pool { name: name.into(), cores, memory_mb, disk_gb, guests }
    }

    fn join(c: &mut Cluster, vmid: VmId, name: &str) -> Result<(), Error> {
        c.update_guest(vmid, &GuestPatch { pool: Some(name.into()), ..Default::default() }, "u", NOW)
    }

    #[test]
    fn pools_are_validated_updated_and_removed() {
        let mut c = cluster();
        assert!(c.put_pool(pool("Büyük Harf", 0, 0, 0, 0)).is_err());
        c.put_pool(pool("web", 16, 0, 0, 0)).unwrap();
        c.put_pool(pool("db", 0, 0, 0, 0)).unwrap();
        assert_eq!(c.pools.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["db", "web"]);
        c.put_pool(pool("web", 32, 65536, 0, 0)).unwrap();
        assert_eq!((c.pools.len(), c.pool("web").unwrap().cores, c.pool("web").unwrap().memory_mb), (2, 32, 65536));
        // İçinde konuk olan havuz silinmez; kota kullanımın altına inmez.
        join(&mut c, 100, "web").unwrap();
        let g = c.guest(100).unwrap().clone();
        assert_eq!(c.pool_usage("web", None), PoolUsage { guests: 1, cores: g.cores, memory_mb: g.mem_total_mb, disk_gb: g.total_disk_gb() });
        assert!(c.remove_pool("web").unwrap_err().to_string().contains("1 konuk var"));
        assert!(c.put_pool(pool("web", g.cores - 1, 0, 0, 0)).unwrap_err().to_string().contains("kullanımın altına inemez"));
        c.put_pool(pool("web", g.cores, 0, 0, 0)).unwrap();
        join(&mut c, 100, "").unwrap();
        assert_eq!(c.guest(100).unwrap().pool, None);
        c.remove_pool("web").unwrap();
        assert!(c.remove_pool("web").is_err());
        for i in 0..MAX_POOLS - 1 {
            c.put_pool(pool(&format!("h{i}"), 0, 0, 0, 0)).unwrap();
        }
        assert!(c.put_pool(pool("fazla", 0, 0, 0, 0)).unwrap_err().to_string().contains("en fazla"));
    }

    #[test]
    fn quotas_apply_to_joining_growing_and_cloning() {
        let mut c = cluster();
        let (a, b) = (c.guest(100).unwrap().clone(), c.guest(101).unwrap().clone());
        assert!(join(&mut c, 100, "yok").unwrap_err().to_string().contains("adında bir havuz yok"));
        // Çekirdek kotası: ilk konuk sığar, ikincisi sığmaz.
        c.put_pool(pool("web", a.cores + b.cores - 1, 0, 0, 0)).unwrap();
        join(&mut c, 100, "web").unwrap();
        let err = join(&mut c, 101, "web").unwrap_err().to_string();
        assert!(err.contains("web havuzunun kotası aşılıyor") && err.contains("çekirdek"), "{err}");
        assert_eq!(c.guest(101).unwrap().pool, None, "reddedilen konuk havuza girmez");
        c.put_pool(pool("web", a.cores + b.cores, a.mem_total_mb + b.mem_total_mb, 0, 2)).unwrap();
        join(&mut c, 101, "web").unwrap();
        // Konuk sayısı kotası.
        assert!(join(&mut c, 103, "web").unwrap_err().to_string().contains("konuk"));
        // Havuzdaki konuğun belleği ya da çekirdeği kotayı aşacak biçimde artırılamaz; azaltılabilir.
        // (Donanım değişikliği kapalı konukta yapılır: 103 kapalıdır, onu ayrı bir havuza alalım.)
        let s = c.guest(103).unwrap().clone();
        c.put_pool(pool("tek", s.cores, s.mem_total_mb, s.total_disk_gb() + 5, 0)).unwrap();
        join(&mut c, 103, "tek").unwrap();
        let grow = |cores, mem| GuestPatch { cores: Some(cores), memory_mb: Some(mem), ..Default::default() };
        assert!(c.update_guest(103, &grow(s.cores + 1, s.mem_total_mb), "u", NOW).unwrap_err().to_string().contains("çekirdek"));
        assert!(c.update_guest(103, &grow(s.cores, s.mem_total_mb + 1024), "u", NOW).unwrap_err().to_string().contains("MB bellek"));
        c.update_guest(103, &grow(s.cores - 1, s.mem_total_mb), "u", NOW).unwrap();
        c.update_guest(103, &grow(s.cores, s.mem_total_mb), "u", NOW).unwrap();
        // Disk: ekleme ve büyütme kotaya takılır.
        assert_eq!(c.add_disk(103, 3).unwrap(), 1);
        assert!(c.add_disk(103, 3).unwrap_err().to_string().contains("GB disk"));
        assert!(c.check_grow(103, s.disk_gb + 3).unwrap_err().to_string().contains("GB disk"));
        c.check_grow(103, s.disk_gb + 2).unwrap();
        assert!(c.pool_allows_disk(100, 1_000_000).is_ok(), "disk kotası olmayan havuz sınırlamaz");
        // Klon havuzu devralır: kota doluysa klonlanamaz.
        assert!(c.clone_guest_as(103, "kopya", false).unwrap_err().to_string().contains("tek havuzunun kotası aşılıyor"));
        c.put_pool(pool("tek", 0, 0, 0, 0)).unwrap();
        let copy = c.clone_guest_as(103, "kopya", false).unwrap();
        assert_eq!(c.guest(copy).unwrap().pool.as_deref(), Some("tek"));
        assert_eq!(c.pool_usage("tek", None).guests, 2);
        // Konuk silinince havuzdaki payı da gider.
        assert_eq!(c.pool_usage("tek", Some(copy)).guests, 1);
    }

    #[test]
    fn old_state_files_load_without_pools() {
        let mut v = serde_json::to_value(cluster()).unwrap();
        v.as_object_mut().unwrap().remove("pools");
        for g in v["guests"].as_array_mut().unwrap() {
            g.as_object_mut().unwrap().remove("pool");
        }
        let c: Cluster = serde_json::from_value(v).unwrap();
        assert!(c.pools.is_empty() && c.guests.iter().all(|g| g.pool.is_none()));
        let p: Pool = serde_json::from_str(r#"{"name":"web"}"#).unwrap();
        assert_eq!(p, pool("web", 0, 0, 0, 0));
    }
}
