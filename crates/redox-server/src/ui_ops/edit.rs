//! Ayar formları: donanım ve seçenekler, güç zamanlamaları.

use super::*;

/// Eski düzenleme adresi: ayarlar artık konuk sayfasının Donanım ve Seçenekler sekmelerinde.
pub async fn edit_form(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Path(raw): Path<String>) -> Response {
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    match raw.parse::<VmId>().ok().and_then(|id| ctx.cluster.guest(id).ok()) {
        Some(g) => Redirect::to(&format!("/guests/{}?tab=donanim", g.vmid)).into_response(),
        None => ctx.not_found(&tfmt(format!("“{raw}” kimlikli bir konuk yok."))),
    }
}

pub async fn edit(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(raw): Path<String>,
    Form(f): Form<EditForm>,
) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // Form bir sekmeden geliyorsa yalnızca o sekmenin alanları uygulanır; öbür sekmenin ayarlarına dokunulmaz.
    let part = match f.part.as_str() {
        "donanim" | "secenekler" => f.part.as_str(),
        _ => "",
    };
    let (general, hardware) = (part != "donanim", part != "secenekler");
    let tab = if part.is_empty() { "donanim" } else { part };
    let back = format!("/guests/{vmid}?tab={tab}");
    let Ok(guest) = st.snapshot().guest(vmid).cloned() else {
        return failed("/", &Error::GuestNotFound(vmid));
    };
    // Kapalı alan grubundaki (disabled) değerler tarayıcıdan gelmez; boş alan "değiştirme" demektir.
    let number = |raw: &str, label: &str| -> Result<Option<u64>, String> {
        match raw.trim() {
            "" => Ok(None),
            t => t.parse().map(Some).map_err(|_| tfmt(format!("{label} bir tam sayı olmalı."))),
        }
    };
    let list = |raw: &str| -> Vec<String> { raw.split([',', ' ', '\n']).map(str::trim).filter(|n| !n.is_empty()).map(str::to_lowercase).collect() };
    let parsed = (|| {
        let hardware_sent = hardware && (!f.cores.trim().is_empty() || !f.memory_mb.trim().is_empty());
        let machine = (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| redox_core::hw::parse_firmware_choice(f.firmware.trim())).flatten();
        Ok::<_, String>(GuestPatch {
            name: general.then(|| f.name.trim().to_string()),
            os: general.then(|| f.os.trim().to_string()),
            tags: general.then(|| parse_tags(&f.tags)),
            pool: general.then(|| f.pool.trim().to_string()),
            onboot: general.then(|| f.onboot.is_some()),
            cores: number(&f.cores, (tx("Çekirdek")))?.map(|v| v as u32).filter(|_| hardware),
            memory_mb: number(&f.memory_mb, (tx("Bellek")))?.filter(|_| hardware),
            iso: hardware_sent.then(|| f.iso.trim().to_string()).filter(|_| guest.kind == redox_core::GuestKind::Vm),
            iso2: hardware_sent.then(|| f.iso2.trim().to_string()).filter(|_| guest.kind == redox_core::GuestKind::Vm),
            cpu_affinity: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.cpu_affinity.trim().to_string()),
            numa_node: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.numa_node.trim().to_string()),
            hugepages: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.hugepages.is_some()),
            disk_bus: redox_core::hw::DiskBus::parse(f.disk_bus.trim()).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            nic_model: redox_core::hw::NicModel::parse(f.nic_model.trim()).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            forwards: if hardware_sent { Some(parse_forwards(&f.forwards)?) } else { None },
            bridge: hardware_sent.then(|| f.bridge.trim().to_string()).filter(|b| !b.is_empty()),
            backup_time: general.then(|| f.backup_time.trim().to_string()),
            backup_keep: number(&f.backup_keep, (tx("Saklanacak yedek sayısı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            rpo_hours: number(&f.rpo_hours, (tx("Yedek yaşı hedefi")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            backup_weekly: number(&f.backup_weekly, (tx("Haftalık yedek sayısı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            backup_monthly: number(&f.backup_monthly, (tx("Aylık yedek sayısı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            backup_yearly: number(&f.backup_yearly, (tx("Yıllık yedek sayısı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            firewall: general.then(|| f.firewall.is_some()),
            nic_guard: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.nic_guard.is_some()),
            fw_rules: if general { Some(redox_core::hw::parse_fw_rules(&f.fw_rules)?) } else { None },
            fw_out_restrict: general.then(|| f.fw_out_restrict.is_some()),
            fw_out: if general { Some(redox_core::hw::parse_fw_out_rules(&f.fw_out)?) } else { None },
            fw_groups: general.then(|| list(&f.fw_groups)),
            backup_pre: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.backup_pre.clone()),
            test_port: number(&f.test_port, (tx("Sınama kapısı")))?.map(|v| v.min(u16::MAX as u64) as u16).filter(|_| general && guest.kind == redox_core::GuestKind::Vm),
            test_cmd: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.test_cmd.clone()),
            backup_post: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.backup_post.clone()),
            firmware: machine.map(|(firmware, _)| firmware),
            secure_boot: machine.map(|(_, secure)| secure),
            tpm: machine.map(|_| f.tpm.is_some()),
            // Köprü adları büyük/küçük harfe duyarlıdır: yalnızca `nat` küçültülür.
            nics: hardware_sent.then(|| f.nics.split([',', ' ', '\n']).filter(|n| !n.trim().is_empty()).map(redox_core::hw::normalize_network).collect()),
            usb: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| list(&f.usb)),
            pci: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| list(&f.pci)),
            boot_order: number(&f.boot_order, (tx("Başlatma sırası")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            boot_delay: number(&f.boot_delay, (tx("Başlatma gecikmesi")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            schedules: None,
            restart: general.then(|| f.restart.is_some()),
            cpu_shares: number(&f.cpu_shares, (tx("İşlemci payı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general && guest.kind == redox_core::GuestKind::Vm),
            mem_reserve_mb: number(&f.mem_reserve_mb, (tx("Bellek garantisi")))?.filter(|_| general && guest.kind == redox_core::GuestKind::Vm),
            disk_mbps: number(&f.disk_mbps, (tx("Disk hız sınırı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            disk_iops: number(&f.disk_iops, (tx("Disk işlem sınırı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            shares: if hardware_sent && guest.kind == redox_core::GuestKind::Vm { Some(redox_core::hw::parse_shares(&f.shares)?) } else { None },
            vlan: number(&f.vlan, (tx("VLAN etiketi")))?.map(|v| v.min(u16::MAX as u64) as u16).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            net_mbps: number(&f.net_mbps, (tx("Ağ hız sınırı")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            cpu_type: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.cpu_type.trim().to_string()),
            balloon: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.balloon.is_some()),
            sound: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.sound.is_some()),
            max_cores: number(&f.max_cores, (tx("En fazla çekirdek")))?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            max_memory_mb: number(&f.max_memory_mb, (tx("En fazla bellek")))?.filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            nested: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.nested.is_some()),
            ci_user: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.ci_user.clone()),
            ci_ssh_key: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.ci_ssh_key.clone()),
            ci_ip: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.ci_ip.clone()),
            ci_gw: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.ci_gw.clone()),
            ci_dns: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.ci_dns.clone()),
        })
    })();
    let patch = match parsed {
        Ok(p) => p,
        Err(message) => return flash(&back, "error", &capitalize(&message)),
    };
    match st.update_guest(vmid, &patch, &me.user) {
        Ok(()) => flash(&format!("/guests/{vmid}?tab={}", if part.is_empty() { "secenekler" } else { part }), "notice", "Ayarlar kaydedildi."),
        Err(e) => failed(&back, &e),
    }
}

/// Zamanlanmış güç işlemi ekler: `action`, `time` ve işaretli günler (`day=1` … `day=7`).
pub async fn schedule_add(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, RawForm(body): RawForm) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=secenekler");
    let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.trim()).unwrap_or("");
    let Ok(action) = value("action").parse::<Action>() else {
        return flash(&back, "error", (tx("Bir işlem seçin: başlat, kapat ya da yeniden başlat.")));
    };
    let days = pairs.iter().filter(|(k, _)| k == "day").filter_map(|(_, v)| v.parse::<u8>().ok()).filter(|d| (1..=7).contains(d)).fold(0u8, |bits, d| bits | 1 << (d - 1));
    if days == 0 {
        return flash(&back, "error", (tx("En az bir gün seçin.")));
    }
    let Ok(guest) = st.snapshot().guest(vmid).cloned() else {
        return failed("/guests", &Error::GuestNotFound(vmid));
    };
    let mut schedules = guest.hw.schedules.clone();
    schedules.push(redox_core::hw::Schedule { action, time: value("time").to_string(), days, last_run: None });
    match st.update_guest(vmid, &GuestPatch { schedules: Some(schedules), ..Default::default() }, &me.user) {
        Ok(()) => flash(&back, "notice", (tx("Zamanlanmış işlem eklendi."))),
        Err(e) => failed(&back, &e),
    }
}

pub async fn schedule_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((raw, index)): Path<(String, String)>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=secenekler");
    let Ok(guest) = st.snapshot().guest(vmid).cloned() else {
        return failed("/guests", &Error::GuestNotFound(vmid));
    };
    let mut schedules = guest.hw.schedules.clone();
    match index.parse::<usize>().ok().filter(|i| *i < schedules.len()) {
        Some(i) => drop(schedules.remove(i)),
        None => return flash(&back, "error", (tx("Böyle bir zamanlanmış işlem yok."))),
    }
    match st.update_guest(vmid, &GuestPatch { schedules: Some(schedules), ..Default::default() }, &me.user) {
        Ok(()) => flash(&back, "notice", (tx("Zamanlanmış işlem silindi."))),
        Err(e) => failed(&back, &e),
    }
}
