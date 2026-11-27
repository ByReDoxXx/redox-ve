//! Destek paketi: sorun bildirirken gönderilecek, tek dosyalık durum dökümü.
//!
//! Sürüm, sürücünün yetenekleri, ayarlar, düğümler, depolama, konukların donanımı, son görevler ve sunucunun
//! temel ölçümleri. **Gizli bilgi içermez:** kullanıcılar, parolalar, token'lar, eş sunucuların token'ları ve
//! denetim günlüğü pakete girmez; konukların yedek öncesi/sonrası komutları ve SSH anahtarları maskelenir.
//! Yine de konuk adları, ağ adresleri ve görev iletileri içinde yer alır: göndermeden önce göz atın.

use super::*;

/// Pakete giren en fazla görev ve yedek kaydı.
const MAX_TASKS: usize = 200;
const MAX_BACKUPS: usize = 100;
const MASK: &str = "(gizlendi)";

/// Gizli bilgi taşıyabilecek alanlar: yedek öncesi/sonrası ve sınama komutları (içlerinde parola geçebilir) ile
/// SSH anahtarı.
const SENSITIVE: [&str; 4] = ["backup_pre", "backup_post", "ci_ssh_key", "test_cmd"];

/// Konuk kayıtlarında gizli olabilecek alanları maskeler. Alan, kaydın hangi derinliğinde durursa dursun
/// adından tanınır: kaydın biçimi değişse de maskeleme atlanmaz.
pub fn scrub_guests(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => items.iter_mut().for_each(scrub_guests),
        serde_json::Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                if SENSITIVE.contains(&key.as_str()) {
                    if inner.as_str().is_some_and(|s| !s.is_empty()) {
                        *inner = MASK.into();
                    }
                } else {
                    scrub_guests(inner);
                }
            }
        }
        _ => {}
    }
}

/// Sunucunun temel ölçümleri (yalnızca Linux'ta doludur).
fn host_facts() -> serde_json::Value {
    let read = |path: &str| std::fs::read_to_string(path).ok().map(|text| text.trim().to_string());
    let addresses: Vec<String> = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", "scope", "global"])
        .output()
        .ok()
        .map(|out| crate::tty::parse_addresses(&String::from_utf8_lossy(&out.stdout)).into_iter().map(|(name, address)| format!("{address} ({name})")).collect())
        .unwrap_or_default();
    serde_json::json!({
        "hostname": crate::host::hostname(),
        "kernel": read("/proc/version"),
        "loadavg": read("/proc/loadavg"),
        "uptime_secs": read("/proc/uptime").and_then(|u| crate::host::parse_uptime(&u)),
        "meminfo_kb": read("/proc/meminfo").and_then(|m| crate::host::parse_meminfo(&m)).map(|(total, available)| serde_json::json!({ "total": total, "available": available })),
        "cgroup": read("/proc/self/cgroup"),
        "kvm": std::path::Path::new("/dev/kvm").exists(),
        "addresses": addresses,
    })
}

impl AppState {
    /// Destek paketi. Yalnızca yöneticiye verilir.
    pub fn support_bundle(&self) -> serde_json::Value {
        let c = self.snapshot();
        let driver = self.driver();
        let vm = redox_core::GuestKind::Vm;
        let mut guests = serde_json::to_value(&c.guests).unwrap_or_default();
        scrub_guests(&mut guests);
        let migrate = self.migrate_settings();
        let peers: Vec<serde_json::Value> = self.peers().views().iter().map(|v| serde_json::json!({ "name": v.name, "url": v.url, "online": v.online(), "error": v.error, "guests": v.guests.len() })).collect();
        serde_json::json!({
            "about": "RedoX VE destek paketi / support bundle. Parola, token ve denetim günlüğü içermez; konuk adları ve ağ adresleri içerir.",
            "generated": now(),
            "version": env!("CARGO_PKG_VERSION"),
            "platform": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            "driver": {
                "name": driver.name(),
                "real": driver.is_real(),
                "containers": driver.supports(redox_core::GuestKind::Ct),
                "uefi": driver.uefi(),
                "tpm": driver.tpm(),
                "hotplug": driver.hotplug(vm),
                "live_migration": driver.live_migration(),
                "live_backup": driver.live_backup(vm),
                "firewall": driver.firewall(),
                "shared_folders": driver.shared_folders(),
                "nested_virt": driver.nested_virt(),
                "backups_sealed": driver.backups_sealed(),
                "bridges": driver.bridges(),
            },
            "settings": {
                "maintenance": c.maintenance,
                "four_eyes": self.four_eyes(),
                "move_precopy": self.move_precopy(),
                "file_restore": self.guestfish().is_some(),
                "offsite": self.offsite().is_some(),
                "migrate": { "ports": [migrate.ports.0, migrate.ports.1], "host": migrate.host, "bind": migrate.bind },
                "peers": peers,
            },
            "summary": c.summary(),
            "nodes": c.nodes,
            "storages": c.storages,
            "pools": c.pools,
            "guests": guests,
            "replicas": c.replicas,
            "backups": { "count": c.backups.len(), "recent": c.backups.iter().take(MAX_BACKUPS).collect::<Vec<_>>() },
            "tasks": c.tasks.iter().take(MAX_TASKS).collect::<Vec<_>>(),
            "host": host_facts(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_fields_that_may_hold_secrets_are_masked() {
        let mut guests = serde_json::json!([
            { "vmid": 1, "hw": { "backup_pre": "/usr/bin/dump --password=gizli", "backup_post": "", "ci_ssh_key": "ssh-ed25519 AAAA x", "cpu_type": "host" } },
            { "vmid": 2, "hw": { "backup_pre": "", "ci_ssh_key": null, "test_cmd": "/bin/kontrol --token=x" } },
            { "vmid": 3 }
        ]);
        scrub_guests(&mut guests);
        let text = guests.to_string();
        assert!(!text.contains("gizli") && !text.contains("AAAA") && !text.contains("token=x"), "{text}");
        assert_eq!(guests[0]["hw"]["backup_pre"], MASK);
        assert_eq!((guests[0]["hw"]["backup_post"].as_str(), guests[0]["hw"]["cpu_type"].as_str()), (Some(""), Some("host")), "boş ve zararsız alanlara dokunulmaz");
        assert!(guests[1]["hw"]["ci_ssh_key"].is_null());
        // Alan kaydın en üst düzeyinde ya da daha derinde dursa da maskelenir.
        let mut flat = serde_json::json!([{ "vmid": 4, "backup_pre": "parola=1", "ic": { "derin": { "test_cmd": "parola=2" } } }]);
        scrub_guests(&mut flat);
        assert!(!flat.to_string().contains("parola="), "{flat}");
    }
}
