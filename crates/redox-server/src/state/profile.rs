//! Sunucu profili: bir sunucunun yönetim ayarlarını dışa alır, başka bir sunucununkiyle karşılaştırır ve
//! durumda tutulan kısmını uygular.
//!
//! Profil iki bölümdür. **Seçenekler**, hizmetin başlatma seçenekleri ve sunucunun yetenekleridir; çalışırken
//! değişmezler, bu yüzden yalnızca karşılaştırılır (fark varsa birim dosyasında elle düzeltilir). **Havuzlar**
//! ve **yedek işleri** durum dosyasındadır ve uygulanabilir. Profil gizli bilgi içermez: kullanıcılar, token'lar
//! ve eş sunucuların adresleri girmez.

use super::*;
use serde_json::{json, Value};

pub const PROFILE_KIND: &str = "redox-profile";

/// Yedek işinin tanımı (zamanlama ve seçim); o anki tura ait alanlar profile girmez.
fn job_definition(job: &redox_core::BackupJob) -> Value {
    json!({ "name": job.name, "time": job.time, "days": job.days, "tags": job.tags, "keep": job.keep, "gfs": job.gfs, "lock_days": job.lock_days, "until": job.until, "verify": job.verify })
}

/// Profildeki havuzları ve yedek işlerini bu sürümün yazdığı biçime getirir: yazılmamış alanlar varsayılan
/// değerini alır, tura ait alanlar atılır. Böylece aynı ayarı farklı biçimde yazan iki profil "farklı" görünmez.
/// Okunamayan kayıt olduğu gibi bırakılır (karşılaştırmada fark olarak görünür, uygulamada hata verir).
pub fn normalize(profile: &Value) -> Value {
    let mut out = profile.clone();
    if let Some(pools) = out["pools"].as_array_mut() {
        for pool in pools.iter_mut() {
            if let Some(clean) = serde_json::from_value::<redox_core::Pool>(pool.clone()).ok().and_then(|p| serde_json::to_value(p).ok()) {
                *pool = clean;
            }
        }
    }
    if let Some(jobs) = out["jobs"].as_array_mut() {
        for job in jobs.iter_mut() {
            if let Ok(parsed) = serde_json::from_value::<redox_core::BackupJob>(job.clone()) {
                *job = job_definition(&parsed);
            }
        }
    }
    out
}

/// İki profil arasındaki farklar, okunur satırlar halinde. `here` bu sunucu, `there` karşılaştırılan profildir.
pub fn profile_diff(here: &Value, there: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let show = |v: Option<&Value>| v.map(Value::to_string).unwrap_or_else(|| "yok".to_string());
    let (mine, theirs) = (here["options"].as_object(), there["options"].as_object());
    let mut keys: Vec<&String> = mine.into_iter().flatten().chain(theirs.into_iter().flatten()).map(|(k, _)| k).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let (a, b) = (mine.and_then(|m| m.get(key)), theirs.and_then(|m| m.get(key)));
        if a != b {
            out.push(format!("seçenek {key}: burada {}, profilde {}", show(a), show(b)));
        }
    }
    for (section, label) in [("pools", "havuz"), ("jobs", "yedek işi")] {
        let named = |v: &Value| -> Vec<(String, Value)> { v[section].as_array().into_iter().flatten().filter_map(|item| Some((item["name"].as_str()?.to_string(), item.clone()))).collect() };
        let (mine, theirs) = (named(here), named(there));
        for (name, item) in &theirs {
            match mine.iter().find(|(n, _)| n == name) {
                None => out.push(format!("{label} {name}: burada yok")),
                Some((_, own)) if own != item => out.push(format!("{label} {name}: farklı (burada {own}, profilde {item})")),
                Some(_) => {}
            }
        }
        for (name, _) in mine.iter().filter(|(n, _)| !theirs.iter().any(|(t, _)| t == n)) {
            out.push(format!("{label} {name}: profilde yok"));
        }
    }
    out
}

impl AppState {
    /// Bu sunucunun profili.
    pub fn profile(&self) -> Value {
        let c = self.snapshot();
        let driver = self.driver();
        let migrate = self.migrate_settings();
        let mut storages: Vec<&str> = c.storages.iter().map(|s| s.id.as_str()).collect();
        storages.sort_unstable();
        storages.dedup();
        json!({
            "kind": PROFILE_KIND,
            "version": env!("CARGO_PKG_VERSION"),
            "server": crate::host::hostname(),
            "options": {
                "driver": driver.name(),
                "containers": driver.supports(redox_core::GuestKind::Ct),
                "firewall": driver.firewall(),
                "uefi": driver.uefi(),
                "tpm": driver.tpm(),
                "backups_sealed": driver.backups_sealed(),
                "live_migration": driver.live_migration(),
                "file_restore": self.guestfish().is_some(),
                "offsite": self.offsite().is_some(),
                "four_eyes": self.four_eyes(),
                "move_precopy": self.move_precopy(),
                "backup_age_days": self.backup_age_days(),
                "migrate_ports": format!("{}-{}", migrate.ports.0, migrate.ports.1),
                "storages": storages,
            },
            "pools": c.pools,
            "jobs": c.jobs.iter().map(job_definition).collect::<Vec<_>>(),
        })
    }

    /// Profili bu sunucuyla karşılaştırır.
    pub fn profile_compare(&self, other: &Value) -> Result<Vec<String>, Error> {
        check_profile(other)?;
        Ok(profile_diff(&self.profile(), &normalize(other)))
    }

    /// Profilin uygulanabilir kısmını (havuzlar ve yedek işleri) bu sunucuya uygular: profildekiler eklenir ya da
    /// güncellenir, burada olup profilde olmayanlara dokunulmaz. `(uygulananlar, elle yapılacaklar)` döner.
    pub fn profile_apply(&self, other: &Value, user: &str) -> Result<(Vec<String>, Vec<String>), Error> {
        check_profile(other)?;
        // Önce hepsi ayrıştırılır: bozuk bir kayıt yüzünden profil yarım uygulanmasın.
        let parse = |section: &str, what: &str| -> Result<Vec<Value>, Error> { Ok(other[section].as_array().ok_or_else(|| Error::InvalidSpec(format!("profilde {what} bölümü yok")))?.clone()) };
        let pools: Vec<redox_core::Pool> = parse("pools", "havuzlar")?.into_iter().map(|p| serde_json::from_value(p).map_err(|e| Error::InvalidSpec(format!("profildeki havuz okunamadı: {e}")))).collect::<Result<_, _>>()?;
        let jobs: Vec<redox_core::BackupJob> = parse("jobs", "yedek işleri")?.into_iter().map(|j| serde_json::from_value(j).map_err(|e| Error::InvalidSpec(format!("profildeki yedek işi okunamadı: {e}")))).collect::<Result<_, _>>()?;
        let before = self.profile();
        let mut applied = Vec::new();
        for pool in pools {
            let name = pool.name.clone();
            if before["pools"].as_array().is_some_and(|all| all.iter().any(|p| serde_json::to_value(&pool).is_ok_and(|v| v == *p))) {
                continue;
            }
            self.pool_put(pool)?;
            applied.push(format!("havuz {name}"));
        }
        for job in jobs {
            let name = job.name.clone();
            if before["jobs"].as_array().is_some_and(|all| all.iter().any(|j| *j == job_definition(&job))) {
                continue;
            }
            self.job_put(job)?;
            applied.push(format!("yedek işi {name}"));
        }
        // Geriye kalan farklar çalışırken değiştirilemeyen seçenekler ve profilde olmayan kayıtlardır.
        let manual = profile_diff(&self.profile(), &normalize(other));
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("profile apply {} ({} değişiklik)", other["server"].as_str().unwrap_or("?"), applied.len()), &Ok(()), now(), None);
        });
        Ok((applied, manual))
    }
}

fn check_profile(profile: &Value) -> Result<(), Error> {
    if profile["kind"] != PROFILE_KIND || !profile["options"].is_object() {
        return Err(Error::InvalidSpec("bu bir RedoX VE sunucu profili değil (redox profile export ile alınmış bir dosya verin)".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differences_between_two_profiles_are_listed_by_name() {
        let here = json!({
            "options": { "four_eyes": true, "driver": "qemu", "storages": ["local"] },
            "pools": [{ "name": "web", "cores": 8 }, { "name": "yerel", "cores": 2 }],
            "jobs": [{ "name": "gece", "time": "02:00" }],
        });
        assert!(profile_diff(&here, &here).is_empty(), "aynı profilde fark yoktur");
        let there = json!({
            "options": { "four_eyes": false, "driver": "qemu", "storages": ["local"], "firewall": true },
            "pools": [{ "name": "web", "cores": 16 }, { "name": "veri", "cores": 4 }],
            "jobs": [{ "name": "gece", "time": "02:00" }, { "name": "hafta", "time": "04:00" }],
        });
        let lines = profile_diff(&here, &there);
        for part in ["seçenek firewall: burada yok, profilde true", "seçenek four_eyes: burada true, profilde false", "havuz web: farklı", "havuz veri: burada yok", "havuz yerel: profilde yok", "yedek işi hafta: burada yok"] {
            assert!(lines.iter().any(|l| l.contains(part)), "{part}\n{lines:#?}");
        }
        assert_eq!(lines.len(), 6, "{lines:#?}");

        // Varsayılanı yazılmamış kayıt, yazılmış olanla aynıdır.
        let written = json!({ "options": {}, "pools": [{ "name": "web", "cores": 8, "memory_mb": 0, "disk_gb": 0, "guests": 0 }], "jobs": [{ "name": "gece", "time": "02:00", "days": 127, "tags": [], "keep": 7, "gfs": { "weekly": 0, "monthly": 0, "yearly": 0 }, "lock_days": 0, "until": null, "verify": false }] });
        let terse = json!({ "options": {}, "pools": [{ "name": "web", "cores": 8, "memory_mb": 0, "disk_gb": 0, "guests": 0 }], "jobs": [{ "name": "gece", "time": "02:00", "keep": 7, "last_run": "2026-01-01", "queue": [100] }] });
        assert!(profile_diff(&normalize(&written), &normalize(&terse)).is_empty(), "{:#?}", profile_diff(&normalize(&written), &normalize(&terse)));
        assert!(!normalize(&terse).to_string().contains("last_run"), "tura ait alanlar atılır");
    }
}
