//! Konuk ayarlarını değiştirme: doğrulama ve uygulama.

use super::*;

impl Cluster {
    /// Konuğun ayarlarını değiştirir. Donanımı etkileyen alanlar (çekirdek, bellek, kalıp,
    /// port yönlendirme) yalnızca konuk durmuşken; ad, etiket, açılış ayarı her zaman değişir.
    pub fn update_guest(&mut self, vmid: VmId, patch: &GuestPatch, user: &str, now: u64) -> Result<(), Error> {
        let guest = self.guest(vmid)?;
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        let node = self.node(&guest.node)?;
        let hardware = patch.cores.is_some_and(|c| c != guest.cores)
            || patch.memory_mb.is_some_and(|m| m != guest.mem_total_mb)
            || patch.iso.as_ref().is_some_and(|i| i.as_str() != guest.iso.as_deref().unwrap_or(""))
            || patch.forwards.as_ref().is_some_and(|f| *f != guest.forwards)
            || patch.bridge.as_ref().is_some_and(|b| *b != guest.bridge)
            || patch.firmware.is_some_and(|f| f != guest.hw.firmware)
            || patch.secure_boot.is_some_and(|s| s != guest.hw.secure_boot)
            || patch.tpm.is_some_and(|t| t != guest.hw.tpm)
            || patch.nics.as_ref().is_some_and(|n| *n != guest.hw.nics)
            || patch.usb.as_ref().is_some_and(|u| *u != guest.hw.usb)
            || patch.pci.as_ref().is_some_and(|p| *p != guest.hw.pci)
            || patch.disk_mbps.is_some_and(|v| v != guest.hw.disk_mbps)
            || patch.disk_iops.is_some_and(|v| v != guest.hw.disk_iops)
            || patch.net_mbps.is_some_and(|v| v != guest.hw.net_mbps)
            || patch.vlan.is_some_and(|v| v != guest.hw.vlan)
            || patch.shares.as_ref().is_some_and(|s| *s != guest.hw.shares)
            || patch.apply_cloud_init(&mut guest.hw.clone())
            || patch.cpu_type.as_ref().is_some_and(|c| *c != guest.hw.cpu_type)
            || patch.balloon.is_some_and(|b| b != guest.hw.balloon)
            || patch.nested.is_some_and(|n| n != guest.hw.nested);
        if hardware && guest.status != GuestStatus::Stopped {
            return Err(Error::RequiresStopped { vmid, what: "donanım ayarlarını değiştirmek".to_string() });
        }
        if hardware {
            // Askıdaki konuğun bellek durumu şimdiki donanımı bekler.
            self.not_suspended(vmid)?;
        }
        if let Some(name) = &patch.name {
            if !valid_name(name) {
                return Err(Error::InvalidName(name.clone()));
            }
        }
        if let Some(cores) = patch.cores {
            if cores == 0 || cores > node.cores {
                return Err(Error::InvalidSpec(format!("çekirdek sayısı 1 ile {} arasında olmalı", node.cores)));
            }
        }
        if let Some(mem) = patch.memory_mb {
            let min = guest.kind.min_memory_mb();
            if mem < min || mem > node.mem_total_mb {
                return Err(Error::InvalidSpec(format!("bellek {min} MB ile {} MB arasında olmalı", node.mem_total_mb)));
            }
        }
        if let Some(iso) = patch.iso.as_deref().filter(|i| !i.is_empty()) {
            if guest.kind != GuestKind::Vm {
                return Err(Error::InvalidSpec("ISO yalnızca sanal makinelere takılabilir".to_string()));
            }
            if !valid_iso_name(iso) {
                return Err(Error::InvalidSpec(format!("geçersiz ISO adı: {iso}")));
            }
        }
        if let Some(bridge) = patch.bridge.as_deref().filter(|b| !valid_bridge(b)) {
            return Err(Error::InvalidSpec(format!("geçersiz ağ ayarı: {bridge}")));
        }
        if let Some(time) = patch.backup_time.as_deref().filter(|t| !t.is_empty() && parse_time(t).is_none()) {
            return Err(Error::InvalidSpec(format!("geçersiz yedek saati \"{time}\"; SS:DD biçiminde yazın, örn. 03:30")));
        }
        if patch.rpo_hours.is_some_and(|h| h > crate::hw::MAX_RPO_HOURS) {
            return Err(Error::InvalidSpec(format!("yedek yaşı hedefi en fazla {} saat olabilir (0: genel ayar)", crate::hw::MAX_RPO_HOURS)));
        }
        if patch.backup_keep.is_some_and(|k| k == 0 || k > MAX_KEEP) {
            return Err(Error::InvalidSpec(format!("saklanacak yedek sayısı 1 ile {MAX_KEEP} arasında olmalı")));
        }
        if [patch.backup_weekly, patch.backup_monthly, patch.backup_yearly].iter().flatten().any(|n| *n > crate::retention::MAX_GFS) {
            return Err(Error::InvalidSpec(format!("haftalık, aylık ve yıllık yedek sayıları en fazla {} olabilir", crate::retention::MAX_GFS)));
        }
        if patch.test_port.is_some() || patch.test_cmd.is_some() || patch.nested.is_some() || patch.nic_guard.is_some() || patch.firewall.is_some() || patch.fw_rules.is_some() || patch.firmware.is_some() || patch.secure_boot.is_some() || patch.tpm.is_some() || patch.nics.is_some() || patch.usb.is_some() || patch.pci.is_some() || patch.boot_order.is_some() || patch.boot_delay.is_some() || patch.schedules.is_some() || patch.disk_mbps.is_some() || patch.disk_iops.is_some() || patch.net_mbps.is_some() || patch.vlan.is_some() || patch.shares.is_some() || patch.bridge.is_some() || patch.apply_cloud_init(&mut guest.hw.clone()) || patch.cpu_type.is_some() || patch.balloon.is_some() || patch.backup_pre.is_some() || patch.backup_post.is_some() {
            let mut hw = guest.hw.clone();
            hw.backup_pre = patch.backup_pre.clone().map(|c| c.trim().to_string()).unwrap_or(hw.backup_pre);
            hw.backup_post = patch.backup_post.clone().map(|c| c.trim().to_string()).unwrap_or(hw.backup_post);
            hw.cpu_type = patch.cpu_type.clone().unwrap_or(hw.cpu_type);
            hw.balloon = patch.balloon.unwrap_or(hw.balloon);
            hw.nic_guard = patch.nic_guard.unwrap_or(hw.nic_guard);
            hw.nested = patch.nested.unwrap_or(hw.nested);
            hw.test_port = patch.test_port.unwrap_or(hw.test_port);
            hw.test_cmd = patch.test_cmd.clone().map(|c| c.trim().to_string()).unwrap_or(hw.test_cmd);
            patch.apply_cloud_init(&mut hw);
            hw.nics = patch.nics.clone().unwrap_or(hw.nics);
            hw.usb = patch.usb.clone().unwrap_or(hw.usb);
            hw.pci = patch.pci.clone().unwrap_or(hw.pci);
            hw.firmware = patch.firmware.unwrap_or(hw.firmware);
            hw.secure_boot = patch.secure_boot.unwrap_or(hw.secure_boot);
            hw.tpm = patch.tpm.unwrap_or(hw.tpm);
            hw.firewall = patch.firewall.unwrap_or(hw.firewall);
            hw.fw_rules = patch.fw_rules.clone().unwrap_or(hw.fw_rules);
            hw.boot_order = patch.boot_order.unwrap_or(hw.boot_order);
            hw.boot_delay = patch.boot_delay.unwrap_or(hw.boot_delay);
            hw.schedules = patch.schedules.clone().unwrap_or(hw.schedules);
            hw.disk_mbps = patch.disk_mbps.unwrap_or(hw.disk_mbps);
            hw.disk_iops = patch.disk_iops.unwrap_or(hw.disk_iops);
            hw.net_mbps = patch.net_mbps.unwrap_or(hw.net_mbps);
            hw.vlan = patch.vlan.unwrap_or(hw.vlan);
            hw.shares = patch.shares.clone().unwrap_or(hw.shares);
            hw.validate(guest.kind).map_err(Error::InvalidSpec)?;
            if hw.vlan > 0 && patch.bridge.as_deref().unwrap_or(&guest.bridge) == NAT {
                return Err(Error::InvalidSpec("VLAN etiketi yalnızca köprüye bağlı kartta kullanılır; birinci kart NAT'ta. Etiketi 0 yapın ya da kartı köprüye bağlayın".to_string()));
            }
            if !hw.net_limit_applies(patch.bridge.as_deref().unwrap_or(&guest.bridge)) {
                return Err(Error::InvalidSpec("ağ hız sınırı yalnızca köprüye bağlı kartlarda uygulanır; bu konuğun bütün kartları NAT'ta. Sınırı 0 yapın ya da bir kartı köprüye bağlayın".to_string()));
            }
        }
        // Havuz kotası: konuk havuza giriyorsa ya da havuzdayken çekirdeği/belleği değişiyorsa denetlenir.
        let pool_after = match &patch.pool {
            Some(name) => Some(name.as_str()).filter(|n| !n.is_empty()),
            None => guest.pool.as_deref(),
        };
        if let Some(pool) = pool_after {
            if patch.pool.is_some() || patch.cores.is_some() || patch.memory_mb.is_some() {
                self.pool_allows(pool, Some(vmid), patch.cores.unwrap_or(guest.cores), patch.memory_mb.unwrap_or(guest.mem_total_mb), guest.total_disk_gb())?;
            }
        }
        if let Some(tags) = &patch.tags {
            if tags.len() > MAX_TAGS {
                return Err(Error::InvalidSpec(format!("en fazla {MAX_TAGS} etiket eklenebilir")));
            }
            if let Some(bad) = tags.iter().find(|t| !valid_tag(t)) {
                return Err(Error::InvalidSpec(format!(
                    "geçersiz etiket \"{bad}\": 1-24 karakter; küçük harf, rakam ve tire kullanın"
                )));
            }
        }
        if let Some(forwards) = &patch.forwards {
            if forwards.len() > MAX_FORWARDS {
                return Err(Error::InvalidSpec(format!("en fazla {MAX_FORWARDS} port yönlendirmesi eklenebilir")));
            }
            for (i, f) in forwards.iter().enumerate() {
                if f.host_port < 1024 || f.guest_port == 0 {
                    return Err(Error::InvalidSpec(format!(
                        "geçersiz yönlendirme {}:{}; sunucu portu 1024-65535, konuk portu 1-65535 olmalı",
                        f.host_port, f.guest_port
                    )));
                }
                let elsewhere = self.guests.iter().any(|g| g.vmid != vmid && g.forwards.iter().any(|o| o.host_port == f.host_port));
                if elsewhere || forwards[..i].iter().any(|o| o.host_port == f.host_port) {
                    return Err(Error::PortTaken(f.host_port));
                }
            }
        }

        let (kind, node_id) = (guest.kind, guest.node.clone());
        let g = self.guest_mut(vmid)?;
        if let Some(name) = &patch.name {
            g.name = name.clone();
        }
        if let Some(cores) = patch.cores {
            g.cores = cores;
        }
        if let Some(mem) = patch.memory_mb {
            g.mem_total_mb = mem;
            g.mem_used_mb = g.mem_used_mb.min(mem);
        }
        if let Some(os) = &patch.os {
            g.os = os.clone();
        }
        if let Some(iso) = &patch.iso {
            g.iso = Some(iso.clone()).filter(|i| !i.is_empty());
        }
        if let Some(onboot) = patch.onboot {
            g.onboot = onboot;
        }
        if let Some(tags) = &patch.tags {
            let mut tags = tags.clone();
            tags.sort();
            tags.dedup();
            g.tags = tags;
        }
        if let Some(forwards) = &patch.forwards {
            g.forwards = forwards.clone();
        }
        if let Some(bridge) = &patch.bridge {
            g.bridge = bridge.clone();
        }
        if let Some(time) = &patch.backup_time {
            g.backup_time = Some(time.clone()).filter(|t| !t.is_empty());
        }
        if let Some(keep) = patch.backup_keep {
            g.backup_keep = keep;
        }
        if let Some(hours) = patch.rpo_hours {
            g.hw.rpo_hours = hours;
        }
        if let Some(port) = patch.test_port {
            g.hw.test_port = port;
        }
        if let Some(command) = &patch.test_cmd {
            g.hw.test_cmd = command.trim().to_string();
        }
        if let Some(command) = &patch.backup_pre {
            g.hw.backup_pre = command.trim().to_string();
        }
        if let Some(command) = &patch.backup_post {
            g.hw.backup_post = command.trim().to_string();
        }
        g.backup_gfs.weekly = patch.backup_weekly.unwrap_or(g.backup_gfs.weekly);
        g.backup_gfs.monthly = patch.backup_monthly.unwrap_or(g.backup_gfs.monthly);
        g.backup_gfs.yearly = patch.backup_yearly.unwrap_or(g.backup_gfs.yearly);
        if let Some(firewall) = patch.firewall {
            g.hw.firewall = firewall;
        }
        if let Some(rules) = &patch.fw_rules {
            g.hw.fw_rules = rules.clone();
        }
        if let Some(firmware) = patch.firmware {
            g.hw.firmware = firmware;
        }
        if let Some(secure) = patch.secure_boot {
            g.hw.secure_boot = secure;
        }
        if let Some(tpm) = patch.tpm {
            g.hw.tpm = tpm;
        }
        if let Some(nics) = &patch.nics {
            g.hw.nics = nics.clone();
        }
        if let Some(usb) = &patch.usb {
            g.hw.usb = usb.clone();
        }
        if let Some(pci) = &patch.pci {
            g.hw.pci = pci.clone();
        }
        if let Some(v) = patch.disk_mbps {
            g.hw.disk_mbps = v;
        }
        if let Some(v) = patch.disk_iops {
            g.hw.disk_iops = v;
        }
        if let Some(v) = patch.net_mbps {
            g.hw.net_mbps = v;
        }
        if let Some(v) = patch.vlan {
            g.hw.vlan = v;
        }
        if let Some(v) = patch.restart {
            g.hw.restart = v;
        }
        patch.apply_cloud_init(&mut g.hw);
        if let Some(pool) = &patch.pool {
            g.pool = Some(pool.clone()).filter(|p| !p.is_empty());
        }
        if let Some(cpu) = &patch.cpu_type {
            g.hw.cpu_type = cpu.clone();
        }
        if let Some(balloon) = patch.balloon {
            g.hw.balloon = balloon;
        }
        if let Some(guard) = patch.nic_guard {
            g.hw.nic_guard = guard;
        }
        if let Some(nested) = patch.nested {
            g.hw.nested = nested;
        }
        // Balon kaldırıldıysa ya da bellek hedefin altına indiyse hedef sıfırlanır (konuk belleğin tamamını kullanır).
        if !g.hw.balloon || g.hw.balloon_mb >= g.mem_total_mb {
            g.hw.balloon_mb = 0;
        }
        // Dinamik bellek balonsuz çalışmaz; bellek alt sınırın altına indirildiyse de kapanır.
        if !g.hw.balloon || g.hw.mem_min_mb > g.mem_total_mb {
            g.hw.mem_min_mb = 0;
        }
        if let Some(shares) = &patch.shares {
            g.hw.shares = shares.clone();
        }
        if let Some(order) = patch.boot_order {
            g.hw.boot_order = order;
        }
        if let Some(delay) = patch.boot_delay {
            g.hw.boot_delay = delay;
        }
        if let Some(schedules) = &patch.schedules {
            // Değişmeyen satır bugün çalıştıysa kaydı korunur: liste kaydedilince aynı gün yeniden çalışmasın.
            let old = std::mem::take(&mut g.hw.schedules);
            g.hw.schedules = schedules
                .iter()
                .map(|s| {
                    let last_run = old.iter().find(|o| (o.action, &o.time, o.days) == (s.action, &s.time, s.days)).and_then(|o| o.last_run.clone());
                    crate::hw::Schedule { last_run, ..s.clone() }
                })
                .collect();
        }
        self.push_task(&node_id, user, format!("{kind} set {vmid}"), TaskStatus::Ok, now, Some(vmid), None);
        Ok(())
    }
}
