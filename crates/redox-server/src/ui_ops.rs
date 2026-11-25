//! Konuk işlemlerinin istek işleyicileri (düzenleme, anlık görüntü, yedek, klon, konsol, toplu işlem).

use axum::extract::{Path, Query, RawForm, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use maud::html;
use redox_core::{parse_forwards, parse_tags, Action, Error, GuestPatch, VmId};
use serde::Deserialize;

use crate::auth::Identity;
use crate::console;
use crate::state::{now, AppState};
use crate::ui::{capitalize, flash, Ctx, PageQuery};
use crate::view::Current;
use crate::view_ops::{self, Confirm, EditForm};

fn vmid_of(raw: &str) -> Result<VmId, Redirect> {
    raw.parse().map_err(|_| flash("/", "error", &format!("Geçersiz konuk kimliği: {raw}")))
}

fn failed(path: &str, e: &Error) -> Redirect {
    match e {
        Error::GuestNotFound(_) => flash("/", "error", &capitalize(&e.to_string())),
        _ => flash(path, "error", &capitalize(&e.to_string())),
    }
}

/// Eski düzenleme adresi: ayarlar artık konuk sayfasının Donanım ve Seçenekler sekmelerinde.
pub async fn edit_form(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Path(raw): Path<String>) -> Response {
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    match raw.parse::<VmId>().ok().and_then(|id| ctx.cluster.guest(id).ok()) {
        Some(g) => Redirect::to(&format!("/guests/{}?tab=donanim", g.vmid)).into_response(),
        None => ctx.not_found(&format!("“{raw}” kimlikli bir konuk yok.")),
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
            t => t.parse().map(Some).map_err(|_| format!("{label} bir tam sayı olmalı.")),
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
            cores: number(&f.cores, "Çekirdek")?.map(|v| v as u32).filter(|_| hardware),
            memory_mb: number(&f.memory_mb, "Bellek")?.filter(|_| hardware),
            iso: hardware_sent.then(|| f.iso.trim().to_string()).filter(|_| guest.kind == redox_core::GuestKind::Vm),
            forwards: if hardware_sent { Some(parse_forwards(&f.forwards)?) } else { None },
            bridge: hardware_sent.then(|| f.bridge.trim().to_string()).filter(|b| !b.is_empty()),
            backup_time: general.then(|| f.backup_time.trim().to_string()),
            backup_keep: number(&f.backup_keep, "Saklanacak yedek sayısı")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            rpo_hours: number(&f.rpo_hours, "Yedek yaşı hedefi")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            backup_weekly: number(&f.backup_weekly, "Haftalık yedek sayısı")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            backup_monthly: number(&f.backup_monthly, "Aylık yedek sayısı")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            backup_yearly: number(&f.backup_yearly, "Yıllık yedek sayısı")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            firewall: general.then(|| f.firewall.is_some()),
            nic_guard: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.nic_guard.is_some()),
            fw_rules: if general { Some(redox_core::hw::parse_fw_rules(&f.fw_rules)?) } else { None },
            backup_pre: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.backup_pre.clone()),
            test_port: number(&f.test_port, "Sınama kapısı")?.map(|v| v.min(u16::MAX as u64) as u16).filter(|_| general && guest.kind == redox_core::GuestKind::Vm),
            test_cmd: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.test_cmd.clone()),
            backup_post: (general && guest.kind == redox_core::GuestKind::Vm).then(|| f.backup_post.clone()),
            firmware: machine.map(|(firmware, _)| firmware),
            secure_boot: machine.map(|(_, secure)| secure),
            tpm: machine.map(|_| f.tpm.is_some()),
            // Köprü adları büyük/küçük harfe duyarlıdır: yalnızca `nat` küçültülür.
            nics: hardware_sent.then(|| f.nics.split([',', ' ', '\n']).filter(|n| !n.trim().is_empty()).map(redox_core::hw::normalize_network).collect()),
            usb: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| list(&f.usb)),
            pci: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| list(&f.pci)),
            boot_order: number(&f.boot_order, "Başlatma sırası")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            boot_delay: number(&f.boot_delay, "Başlatma gecikmesi")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| general),
            schedules: None,
            restart: general.then(|| f.restart.is_some()),
            disk_mbps: number(&f.disk_mbps, "Disk hız sınırı")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            disk_iops: number(&f.disk_iops, "Disk işlem sınırı")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            shares: if hardware_sent && guest.kind == redox_core::GuestKind::Vm { Some(redox_core::hw::parse_shares(&f.shares)?) } else { None },
            vlan: number(&f.vlan, "VLAN etiketi")?.map(|v| v.min(u16::MAX as u64) as u16).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            net_mbps: number(&f.net_mbps, "Ağ hız sınırı")?.map(|v| v.min(u32::MAX as u64) as u32).filter(|_| hardware_sent && guest.kind == redox_core::GuestKind::Vm),
            cpu_type: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.cpu_type.trim().to_string()),
            balloon: (hardware_sent && guest.kind == redox_core::GuestKind::Vm).then(|| f.balloon.is_some()),
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
        return flash(&back, "error", "Bir işlem seçin: başlat, kapat ya da yeniden başlat.");
    };
    let days = pairs.iter().filter(|(k, _)| k == "day").filter_map(|(_, v)| v.parse::<u8>().ok()).filter(|d| (1..=7).contains(d)).fold(0u8, |bits, d| bits | 1 << (d - 1));
    if days == 0 {
        return flash(&back, "error", "En az bir gün seçin.");
    }
    let Ok(guest) = st.snapshot().guest(vmid).cloned() else {
        return failed("/guests", &Error::GuestNotFound(vmid));
    };
    let mut schedules = guest.hw.schedules.clone();
    schedules.push(redox_core::hw::Schedule { action, time: value("time").to_string(), days, last_run: None });
    match st.update_guest(vmid, &GuestPatch { schedules: Some(schedules), ..Default::default() }, &me.user) {
        Ok(()) => flash(&back, "notice", "Zamanlanmış işlem eklendi."),
        Err(e) => failed(&back, &e),
    }
}

/// Anında kurtarılmış konuğu yedekten ayırır.
pub async fn guest_detach(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=cogalt");
    match st.guest_detach(vmid, &me.user) {
        Ok(_) => flash(&back, "notice", "Diskler yedekten ayrılıyor; bitince yedek serbest kalır. Sonuç görevlerde görünür."),
        Err(e) => failed(&back, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MoveForm {
    storage: String,
}

/// Çoğalt ve Taşı sekmesindeki depolama alanı formu.
pub async fn storage_move(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<MoveForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=cogalt");
    let target = f.storage.trim();
    if target.is_empty() {
        return flash(&back, "error", "Bir depolama alanı seçin.");
    }
    match st.storage_move(vmid, target, &me.user) {
        Ok(_) => flash(&back, "notice", &format!("Diskler {target} alanına taşınıyor; sonuç görevlerde görünür.")),
        Err(e) => failed(&back, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PoolForm {
    name: String,
    cores: String,
    memory_mb: String,
    disk_gb: String,
    guests: String,
}

/// Havuzlar sayfasındaki form: ekler ya da aynı adlı havuzun kotalarını günceller. Boş alan sınırsızdır.
pub async fn pool_save(State(st): State<AppState>, Form(f): Form<PoolForm>) -> Redirect {
    let number = |raw: &str, label: &str| -> Result<u64, String> {
        match raw.trim() {
            "" => Ok(0),
            text => text.parse().map_err(|_| format!("{label} bir tam sayı olmalı.")),
        }
    };
    let parsed = (|| {
        Ok::<_, String>(redox_core::Pool {
            name: f.name.trim().to_string(),
            cores: number(&f.cores, "Çekirdek kotası")?.min(u32::MAX as u64) as u32,
            memory_mb: number(&f.memory_mb, "Bellek kotası")?,
            disk_gb: number(&f.disk_gb, "Disk kotası")?,
            guests: number(&f.guests, "Konuk sayısı")?.min(u32::MAX as u64) as u32,
        })
    })();
    let pool = match parsed {
        Ok(pool) => pool,
        Err(message) => return flash("/pools", "error", &message),
    };
    let name = pool.name.clone();
    match st.pool_put(pool) {
        Ok(()) => flash("/pools", "notice", &format!("{name} havuzu kaydedildi.")),
        Err(e) => failed("/pools", &e),
    }
}

pub async fn pool_delete(State(st): State<AppState>, Path(name): Path<String>) -> Redirect {
    match st.pool_remove(&name) {
        Ok(()) => flash("/pools", "notice", &format!("{name} havuzu silindi.")),
        Err(e) => failed("/pools", &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct BalloonForm {
    mb: String,
    /// Dinamik bellek formunda: alt sınır (MB); 0 kapatır.
    min: Option<String>,
}

/// Dört göz ilkesi: bekleyen isteği onaylar ya da reddeder (`op`: `approve` / `reject`).
pub async fn approval_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((id, op)): Path<(u64, String)>) -> Redirect {
    let result = match op.as_str() {
        "approve" => st.approve(id, &me.user).map(|done| capitalize(&done)),
        "reject" => st.reject(id, &me.user).map(|()| format!("İstek #{id} reddedildi.")),
        _ => return flash("/", "error", &format!("Bilinmeyen işlem: {op}")),
    };
    match result {
        Ok(text) => flash("/", "notice", &text),
        Err(e) => failed("/", &e),
    }
}

/// Donanım sekmesindeki bellek hedefi formu.
pub async fn balloon_set(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<BalloonForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=donanim");
    if let Some(min) = &f.min {
        let Ok(min) = min.trim().parse::<u64>() else {
            return flash(&back, "error", "En az bellek MB olarak bir tam sayı olmalı.");
        };
        return match st.dynamic_memory_set(vmid, min, &me.user) {
            Ok(()) if min == 0 => flash(&back, "notice", "Dinamik bellek kapatıldı: konuk belleğinin tamamını kullanır."),
            Ok(()) => flash(&back, "notice", &format!("Dinamik bellek açıldı: hedef {min} MB ile belleğin tamamı arasında ayarlanır.")),
            Err(e) => failed(&back, &e),
        };
    }
    let Ok(mb) = f.mb.trim().parse::<u64>() else {
        return flash(&back, "error", "Bellek hedefi MB olarak bir tam sayı olmalı.");
    };
    match st.balloon_set(vmid, mb, &me.user) {
        Ok(()) if mb == 0 => flash(&back, "notice", "Balon boşaltıldı: konuk belleğinin tamamını kullanır."),
        Ok(()) => flash(&back, "notice", &format!("Bellek hedefi {mb} MB olarak ayarlandı.")),
        Err(e) => failed(&back, &e),
    }
}

/// Yedekler sayfasındaki iş formu: ekler ya da aynı adlı işi günceller.
pub async fn job_save(State(st): State<AppState>, RawForm(body): RawForm) -> Redirect {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.trim()).unwrap_or("");
    let days: String = pairs.iter().filter(|(k, _)| k == "day").filter_map(|(_, v)| v.parse::<u8>().ok()).filter(|d| (1..=7).contains(d)).map(|d| d.to_string()).collect();
    if days.is_empty() {
        return flash("/backups", "error", "En az bir gün seçin.");
    }
    let keep = match value("keep") {
        "" => None,
        text => match text.parse::<u32>() {
            Ok(n) => Some(n),
            Err(_) => return flash("/backups", "error", "Saklanacak yedek sayısı bir sayı olmalı."),
        },
    };
    let count = |key: &str, label: &str| -> Result<u32, String> {
        match value(key) {
            "" => Ok(0),
            text => text.parse().map_err(|_| format!("{label} bir sayı olmalı.")),
        }
    };
    let extras = (|| Ok::<_, String>((redox_core::Gfs { weekly: count("weekly", "Haftalık yedek sayısı")?, monthly: count("monthly", "Aylık yedek sayısı")?, yearly: count("yearly", "Yıllık yedek sayısı")? }, count("lock", "Kilit süresi")?)))();
    let (gfs, lock) = match extras {
        Ok(extras) => extras,
        Err(message) => return flash("/backups", "error", &message),
    };
    let job = match redox_core::BackupJob::from_text(value("name"), value("time"), &days, value("tags"), keep).and_then(|job| job.with_retention(gfs, lock)).and_then(|job| job.with_window(value("until"), !value("verify").is_empty())) {
        Ok(job) => job,
        Err(e) => return failed("/backups", &e),
    };
    let name = job.name.clone();
    match st.job_put(job) {
        Ok(()) => flash("/backups", "notice", &format!("{name} işi kaydedildi.")),
        Err(e) => failed("/backups", &e),
    }
}

/// `run`: işi şimdi başlatır; `delete`: siler (aldığı yedekler kalır).
pub async fn job_action(State(st): State<AppState>, Path((name, op)): Path<(String, String)>) -> Redirect {
    match op.as_str() {
        "run" => match st.job_run(&name) {
            Ok(0) => flash("/backups", "error", &format!("{name} işinin kapsamında konuk yok.")),
            Ok(n) => flash("/backups", "notice", &format!("{name} işi başlatıldı: {n} konuk sırayla yedeklenecek; sonuç görevlerde görünür.")),
            Err(e) => failed("/backups", &e),
        },
        "delete" => match st.job_remove(&name) {
            Ok(()) => flash("/backups", "notice", &format!("{name} işi silindi; aldığı yedekler duruyor.")),
            Err(e) => failed("/backups", &e),
        },
        _ => flash("/backups", "error", "Bilinmeyen işlem."),
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
        None => return flash(&back, "error", "Böyle bir zamanlanmış işlem yok."),
    }
    match st.update_guest(vmid, &GuestPatch { schedules: Some(schedules), ..Default::default() }, &me.user) {
        Ok(()) => flash(&back, "notice", "Zamanlanmış işlem silindi."),
        Err(e) => failed(&back, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ResizeForm {
    disk_gb: String,
}

pub async fn disk_add(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<ResizeForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=donanim");
    let Ok(size_gb) = f.disk_gb.trim().parse::<u64>() else {
        return flash(&back, "error", "Disk boyutu bir tam sayı olmalı.");
    };
    match st.disk_add(vmid, size_gb, &me.user) {
        Ok(id) => flash(&back, "notice", &format!("{size_gb} GB'lık {id} numaralı disk eklendi.")),
        Err(e) => failed(&back, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NicForm {
    network: String,
    /// İsteğe bağlı VLAN etiketi; boş ya da 0 etiketsiz.
    vlan: String,
}

pub async fn nic_add(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<NicForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=donanim");
    let mut network = redox_core::hw::normalize_network(&f.network);
    if network.is_empty() {
        return flash(&back, "error", "Kartın bağlanacağı ağı yazın: nat ya da bir köprü adı.");
    }
    let vlan = f.vlan.trim();
    if !vlan.is_empty() && vlan != "0" {
        network = format!("{network}:{vlan}");
    }
    match st.nic_add(vmid, &network, &me.user) {
        Ok(index) => flash(&back, "notice", &format!("{index} numaralı ek ağ kartı eklendi ({network}).")),
        Err(e) => failed(&back, &e),
    }
}

pub async fn disk_remove(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((raw, id)): Path<(String, String)>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=donanim");
    let Ok(id) = id.parse::<u8>() else {
        return flash(&back, "error", "Geçersiz disk numarası.");
    };
    match st.disk_remove(vmid, id, &me.user) {
        Ok(()) => flash(&back, "notice", &format!("{id} numaralı disk silindi.")),
        Err(e) => failed(&back, &e),
    }
}

pub async fn resize(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<ResizeForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=donanim");
    let Ok(new_gb) = f.disk_gb.trim().parse::<u64>() else {
        return flash(&back, "error", "Disk boyutu bir tam sayı olmalı.");
    };
    match st.resize_disk(vmid, new_gb, &me.user) {
        Ok(_) => flash(&back, "notice", &format!("Disk {new_gb} GB boyutuna büyütülüyor.")),
        Err(e) => failed(&back, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NameForm {
    name: String,
    /// Klon formunda: şablondan bağlı klon.
    linked: Option<String>,
    /// Şablon formunda: `1` şablona çevirir, `0` çıkarır.
    on: Option<String>,
    /// Anlık görüntü formunda: belleksiz (üretim) denetim noktası.
    production: Option<String>,
}

pub async fn suspend(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}");
    match st.suspend(vmid, &me.user) {
        Ok(_) => flash(&back, "notice", "Konuk askıya alınıyor: bellek durumu diske yazılıp konuk kapatılacak."),
        Err(e) => failed(&back, &e),
    }
}

pub async fn discard_suspend(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}");
    match st.discard_suspend(vmid, &me.user) {
        Ok(()) => flash(&back, "notice", "Askı durumu atıldı; konuk bir sonraki başlatmada sıfırdan açılır."),
        Err(e) => failed(&back, &e),
    }
}

pub async fn template(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<NameForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let on = f.on.as_deref() == Some("1");
    let back = format!("/guests/{vmid}?tab=cogalt");
    match st.set_template(vmid, on, &me.user) {
        Ok(()) => flash(&back, "notice", if on { "Konuk şablona çevrildi." } else { "Konuk şablonluktan çıkarıldı." }),
        Err(e) => failed(&back, &e),
    }
}

pub async fn clone(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<NameForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match st.clone_guest_as(vmid, f.name.trim(), f.linked.is_some(), &me.user) {
        Ok(new_id) => flash(&format!("/guests/{new_id}"), "notice", &format!("{vmid} klonlanıyor: yeni konuk {new_id}.")),
        Err(e) => failed(&format!("/guests/{vmid}?tab=cogalt"), &e),
    }
}

pub async fn snapshot_create(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<NameForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=goruntuler");
    let started = if f.production.is_some() { st.snapshot_create_production(vmid, f.name.trim(), &me.user) } else { st.snapshot_create(vmid, f.name.trim(), &me.user) };
    match started {
        Ok(_) => flash(&back, "notice", &format!("Anlık görüntü alınıyor: {}", f.name.trim())),
        Err(e) => failed(&back, &e),
    }
}

pub async fn snapshot_action(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path((raw, name, op)): Path<(String, String, String)>,
) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=goruntuler");
    let result = match op.as_str() {
        "rollback" => st.snapshot_rollback(vmid, &name, &me.user).map(|_| format!("{name} anlık görüntüsüne dönülüyor.")),
        "delete" => st.snapshot_delete(vmid, &name, &me.user).map(|_| format!("{name} siliniyor.")),
        _ => return flash(&back, "error", &format!("Bilinmeyen işlem: {op}")),
    };
    match result {
        Ok(notice) => flash(&back, "notice", &notice),
        Err(e) => failed(&back, &e),
    }
}

pub async fn backup_create(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=yedekler");
    match st.backup_create(vmid, &me.user) {
        Ok(id) => flash(&back, "notice", &format!("Yedek alınıyor: {id}")),
        Err(e) => failed(&back, &e),
    }
}

pub async fn backup_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((id, op)): Path<(String, String)>) -> Redirect {
    // Yedekten yeni konuk: özgün konuk silinmiş de olabilir; başarılıysa yeni konuğun sayfasına gidilir.
    if op == "restore-new" || op == "instant" {
        let name = match st.snapshot().backup(&id) {
            Ok(b) => redox_core::restored_name(&b.guest_name),
            Err(e) => return flash("/backups", "error", &capitalize(&e.to_string())),
        };
        let made = if op == "instant" { st.backup_instant(&id, &name, &me.user) } else { st.backup_restore_new(&id, &name, &me.user) };
        return match made {
            Ok(vmid) if op == "instant" => flash(&format!("/guests/{vmid}"), "notice", &format!("{name}, {id} yedeğinin üzerinde açılmaya hazır. Kalıcı olması için Çoğalt ve Taşı sekmesinden yedekten ayırın.")),
            Ok(vmid) => flash(&format!("/guests/{vmid}"), "notice", &format!("{id} yedeğinden {name} oluşturuluyor; sonuç görevlerde görünür.")),
            Err(e) => failed("/backups", &e),
        };
    }
    let owner = st.snapshot().backup(&id).map(|b| b.vmid);
    let back = match owner {
        Ok(vmid) => format!("/guests/{vmid}?tab=yedekler"),
        Err(e) => return flash("/", "error", &capitalize(&e.to_string())),
    };
    let result = match op.as_str() {
        "restore" => st.backup_restore(&id, &me.user).map(|_| format!("{id} geri yükleniyor.")),
        "delete" => st.request_backup_delete(&id, &me.user).map(|queued| match queued {
            Some(n) => format!("{id} silinmedi: ikinci bir yöneticinin onayı bekleniyor (istek #{n})."),
            None => format!("{id} silindi."),
        }),
        "push" => st.backup_push(&id, &me.user).map(|()| format!("{id} dış hedefe gönderiliyor.")),
        "verify" => st.backup_verify(&id, &me.user).map(|()| format!("{id} doğrulanıyor; sonuç görev günlüğüne yazılır.")),
        "test" => st.backup_test(&id, &me.user).map(|()| format!("{id} geçici bir konuk olarak açılıp sınanıyor; sonuç görev günlüğüne ve yedeğin satırına yazılır.")),
        "lock" => st.backup_lock(&id, redox_core::retention::DEFAULT_LOCK_DAYS, &me.user).map(|until| format!("{id}, {} tarihine kadar silinemez.", redox_core::retention::date_label(until))),
        _ => return flash(&back, "error", &format!("Bilinmeyen işlem: {op}")),
    };
    match result {
        Ok(notice) => flash(&back, "notice", &notice),
        Err(e) => failed(&back, &e),
    }
}

/// Yedek dosyasını indirir (dışa aktarma). Yalnızca yöneticiler; dosya parça parça akıtılır.
/// Kapalı sanal makineyi OVA arşivi olarak indirir. Arşiv diske yazılmaz: tanım ve diskler sırayla akıtılır.
pub async fn guest_export(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Response {
    use std::io::Read;
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, "Dışa aktarmak için yönetici yetkisi gerekir.").into_response();
    }
    let Ok(vmid) = raw.parse::<redox_core::VmId>() else {
        return (StatusCode::NOT_FOUND, "Konuk bulunamadı.").into_response();
    };
    // Disk dönüştürme uzun sürebilir: istek iş parçacığını tutmamalı.
    let exporter = st.clone();
    let prepared = tokio::task::spawn_blocking(move || -> Result<(String, u64, Box<dyn Read + Send>), redox_core::Error> {
        let (guest, dir, disks) = exporter.export_ova(vmid)?;
        let built = (|| -> Result<(u64, Box<dyn Read + Send>), String> {
            let mut sized = Vec::with_capacity(disks.len());
            let mut files = Vec::with_capacity(disks.len());
            for (name, path, capacity) in &disks {
                let file = std::fs::File::open(path).map_err(|e| format!("dışa aktarılan disk açılamadı: {e}"))?;
                let size = file.metadata().map_err(|e| e.to_string())?.len();
                sized.push((name.clone(), size, *capacity));
                files.push((name.clone(), size, file));
            }
            let ovf = crate::ova::ovf_xml(&guest.name, guest.cores, guest.mem_total_mb, &sized).into_bytes();
            let pad = |size: u64| std::io::Cursor::new(vec![0u8; crate::ova::padding(size) as usize]);
            let mut total = 512 + ovf.len() as u64 + crate::ova::padding(ovf.len() as u64) + 1024;
            let ovf_len = ovf.len() as u64;
            let mut stream: Box<dyn Read + Send> = Box::new(std::io::Cursor::new(crate::ova::tar_header(&format!("{}.ovf", guest.name), ovf_len).to_vec()).chain(std::io::Cursor::new(ovf)).chain(pad(ovf_len)));
            for (name, size, file) in files {
                total += 512 + size + crate::ova::padding(size);
                stream = Box::new(stream.chain(std::io::Cursor::new(crate::ova::tar_header(&name, size).to_vec())).chain(file.take(size)).chain(pad(size)));
            }
            Ok((total, Box::new(stream.chain(std::io::Cursor::new(vec![0u8; 1024])))))
        })();
        // Açık dosyalar silinse de okunmaya devam eder; geçici dizin diskte kalmaz.
        let _ = std::fs::remove_dir_all(&dir);
        let (total, stream) = built.map_err(redox_core::Error::Driver)?;
        Ok((guest.name, total, stream))
    })
    .await;
    let (name, total, reader) = match prepared {
        Ok(Ok(ready)) => ready,
        Ok(Err(e)) => {
            let status = match e.kind() {
                redox_core::ErrorKind::NotFound => StatusCode::NOT_FOUND,
                redox_core::ErrorKind::Conflict => StatusCode::CONFLICT,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            return (status, capitalize(&e.to_string())).into_response();
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Dışa aktarma hazırlanamadı.").into_response(),
    };
    let stream = futures_util::stream::unfold(reader, |mut reader| async move {
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; 256 * 1024];
            let n = reader.read(&mut buf);
            (
                n.map(|n| {
                    buf.truncate(n);
                    buf
                }),
                reader,
            )
        })
        .await;
        match read {
            Ok((Ok(buf), _)) if buf.is_empty() => None,
            Ok((result, reader)) => Some((result, reader)),
            Err(_) => None,
        }
    });
    (
        [
            (CONTENT_TYPE, "application/x-tar".to_string()),
            (axum::http::header::CONTENT_LENGTH, total.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}.ova\"")),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// Tarayıcıdan kalıp ya da imaj yükleme (çok parçalı form). Dosya parça parça diske akar; belleğe alınmaz.
pub async fn media_upload(State(st): State<AppState>, Extension(me): Extension<Identity>, mut form: axum::extract::Multipart) -> Redirect {
    let back = "/storage";
    let stored: Result<String, String> = async {
        while let Some(mut field) = form.next_field().await.map_err(|e| format!("yükleme okunamadı: {e}"))? {
            if field.name() != Some("file") {
                continue;
            }
            // Tarayıcı tam yol gönderebilir: yalnızca dosya adı alınır, o da ad kuralından geçer.
            let name = field.file_name().unwrap_or_default().rsplit(['/', '\\']).next().unwrap_or_default().to_string();
            if name.is_empty() {
                return Err("yüklenecek dosyayı seçin".to_string());
            }
            let (dest, _) = crate::media::target(&st, &name).map_err(|e| e.to_string())?;
            let mut upload = crate::media::Upload::begin(dest)?;
            while let Some(chunk) = field.chunk().await.map_err(|e| format!("yükleme yarıda kesildi: {e}"))? {
                upload.write(chunk).await?;
            }
            upload.finish(None).await?;
            return Ok(name);
        }
        Err("yüklenecek dosyayı seçin".to_string())
    }
    .await;
    match stored {
        Ok(name) => {
            st.media_uploaded(&name, &Ok(()), &me.user);
            flash(back, "notice", &format!("{name} yüklendi."))
        }
        Err(e) => flash(back, "error", &capitalize(&e)),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FetchForm {
    url: String,
    name: String,
    sha256: String,
}

pub async fn media_fetch(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<FetchForm>) -> Redirect {
    match st.media_fetch(&f.url, &f.name, &f.sha256, &me.user) {
        Ok(name) => flash("/storage", "notice", &format!("{name} indiriliyor; bitince görev günlüğünde görünür.")),
        Err(e) => failed("/storage", &e),
    }
}

pub async fn media_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>) -> Redirect {
    match st.media_delete(&name, &me.user) {
        Ok(()) => flash("/storage", "notice", &format!("{name} silindi.")),
        Err(e) => failed("/storage", &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FilesQuery {
    /// 0: asıl disk; diğerleri yedekteki ek disk numarası.
    pub disk: u8,
    path: String,
    /// Dizin, tar arşivi olarak indirilsin mi?
    tar: Option<String>,
}

impl FilesQuery {
    /// Verilmemiş yol köktür.
    pub fn path(&self) -> String {
        if self.path.is_empty() { "/".to_string() } else { self.path.clone() }
    }
}

/// Yedeğin içindeki dizini listeler. Okuma birkaç saniye sürer (ve artımlı yedekte önce imaj dökülür): istek iş
/// parçacığını tutmaz. Her erişim denetim günlüğüne yazılır: yedeğin içeriği konuğun bütün dosyalarıdır.
pub async fn list_backup_files(st: &AppState, user: &str, ip: &str, id: &str, disk: u8, path: &str) -> Result<Vec<crate::guestfs::Entry>, Error> {
    if !crate::guestfs::valid_path(path) {
        return Err(Error::InvalidSpec(format!("geçersiz yol: {path}")));
    }
    st.auth().audit.record(now(), user, ip, &format!("backup ls {id} {}", crate::guestfs::normalize(path)), true);
    let (state, id, path) = (st.clone(), id.to_string(), path.to_string());
    tokio::task::spawn_blocking(move || {
        let (bin, image, temporary) = state.backup_image(&id, disk)?;
        let listed = crate::guestfs::list(&bin, &image, &path).map_err(Error::Driver);
        if temporary {
            crate::state::discard_export(&image);
        }
        listed
    })
    .await
    .unwrap_or_else(|_| Err(Error::Driver("dosya listesi okunamadı".to_string())))
}

/// Yedeğin içindeki dosyaları gösteren sayfa (yalnızca yönetici).
pub async fn backup_files(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<FilesQuery>,
) -> Response {
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    if !me.can_write() {
        return ctx.not_found("Yedeğin içindeki dosyaları yalnızca yönetici görebilir.");
    }
    let Ok(backup) = st.snapshot().backup(&id).cloned() else {
        return ctx.not_found("Yedek bulunamadı.");
    };
    let path = query.path();
    let listed = list_backup_files(&st, &me.user, &ip, &id, query.disk, &path).await.map_err(|e| capitalize(&e.to_string()));
    Html(crate::view::backup_files(&ctx.page(Current::Backups, "/backups"), &backup, query.disk, &crate::guestfs::normalize(&path), &listed)).into_response()
}

/// Yedeğin içinden tek bir dosyayı ya da (`tar=1` ile) bir dizinin tar arşivini indirir. Yalnızca yönetici.
pub async fn backup_file(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(id): Path<String>,
    Query(query): Query<FilesQuery>,
) -> Response {
    use std::io::Read;
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, "Yedekten dosya indirmek için yönetici yetkisi gerekir.").into_response();
    }
    let (path, tar) = (query.path(), query.tar.is_some());
    if !crate::guestfs::valid_path(&path) || (!tar && crate::guestfs::normalize(&path) == "/") {
        return (StatusCode::UNPROCESSABLE_ENTITY, "Geçersiz yol.").into_response();
    }
    st.auth().audit.record(now(), &me.user, &ip, &format!("backup {} {id} {}", if tar { "tar" } else { "get" }, crate::guestfs::normalize(&path)), true);
    let (state, wanted, disk, guest_path) = (st.clone(), id.clone(), query.disk, path.clone());
    let opened = tokio::task::spawn_blocking(move || {
        let (bin, image, temporary) = state.backup_image(&wanted, disk)?;
        let cleanup: Option<Box<dyn FnOnce() + Send>> = if temporary {
            let image = image.clone();
            Some(Box::new(move || crate::state::discard_export(&image)))
        } else {
            None
        };
        let mut stream = crate::guestfs::open(&bin, &image, &guest_path, tar, cleanup).map_err(Error::Driver)?;
        // İlk parça burada okunur: dosya yoksa ya da okunamıyorsa boş bir indirme yerine açık bir hata döner.
        let mut first = vec![0u8; 64 * 1024];
        let n = stream.read(&mut first).map_err(|e| Error::Driver(format!("yedekteki dosya okunamadı: {e}")))?;
        first.truncate(n);
        Ok::<_, Error>((first, stream))
    })
    .await;
    let (first, reader) = match opened {
        Ok(Ok(ready)) if !ready.0.is_empty() => ready,
        Ok(Ok(_)) => return (StatusCode::NOT_FOUND, "Dosya yedekte yok, boş ya da okunamıyor.").into_response(),
        Ok(Err(e)) => return (StatusCode::UNPROCESSABLE_ENTITY, capitalize(&e.to_string())).into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Dosya okunamadı.").into_response(),
    };
    let stream = futures_util::stream::unfold((Some(first), reader), |(first, mut reader)| async move {
        if let Some(first) = first {
            return Some((Ok::<_, std::io::Error>(first), (None, reader)));
        }
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; 256 * 1024];
            let n = reader.read(&mut buf);
            (
                n.map(|n| {
                    buf.truncate(n);
                    buf
                }),
                reader,
            )
        })
        .await;
        match read {
            Ok((Ok(buf), _)) if buf.is_empty() => None,
            Ok((result, reader)) => Some((result, (None, reader))),
            Err(_) => None,
        }
    });
    // Dosya adı başlığa güvenle konabilsin diye sadeleştirilir.
    let leaf = crate::guestfs::normalize(&path).rsplit('/').next().unwrap_or("").to_string();
    let safe: String = leaf.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    let name = match (safe.is_empty(), tar) {
        (true, _) => format!("{id}.tar"),
        (false, true) => format!("{safe}.tar"),
        (false, false) => safe,
    };
    (
        [
            (CONTENT_TYPE, if tar { "application/x-tar" } else { "application/octet-stream" }.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
            (HeaderName::from_static("x-content-type-options"), "nosniff".to_string()),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DownloadQuery {
    /// 0: asıl disk; diğerleri yedekteki ek disk numarası.
    disk: u8,
}

pub async fn backup_download(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>, Query(query): Query<DownloadQuery>) -> Response {
    use std::io::Read;
    let disk = query.disk;
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, "Yedek indirmek için yönetici yetkisi gerekir.").into_response();
    }
    match st.snapshot().backup(&id) {
        Err(_) => return (StatusCode::NOT_FOUND, "Yedek bulunamadı.").into_response(),
        Ok(backup) if disk != 0 && !backup.disks.iter().any(|d| d.id == disk) => {
            return (StatusCode::NOT_FOUND, "Bu yedekte o numarada ek disk yok.").into_response();
        }
        Ok(_) => {}
    }
    // Artımlı yedek önce tek dosyaya dökülür; bu uzun sürebilir, istek iş parçacığını tutmamalı.
    let (exporter, wanted) = (st.clone(), id.clone());
    let opened = tokio::task::spawn_blocking(move || -> Result<Option<(std::fs::File, u64, String)>, String> {
        let Some((path, temporary)) = exporter.driver().export_backup(&wanted, disk)? else {
            return Ok(None);
        };
        let opened = std::fs::File::open(&path).ok().and_then(|file| {
            let len = file.metadata().ok()?.len();
            Some((file, len, path.file_name()?.to_str()?.to_string()))
        });
        // Açık dosya silinse de okunmaya devam eder; geçici kopya diskte kalmaz.
        if temporary {
            crate::state::discard_export(&path);
        }
        Ok(opened)
    })
    .await;
    let (file, len, name) = match opened {
        Ok(Ok(Some(found))) => found,
        Ok(Err(e)) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Yedek hazırlanamadı: {e}")).into_response(),
        _ => return (StatusCode::NOT_FOUND, "Yedek dosyası bu sunucuda yok (yalnızca dış kopyası olabilir).").into_response(),
    };
    let stream = futures_util::stream::unfold(file, |mut file| async move {
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; 256 * 1024];
            let n = file.read(&mut buf);
            (n.map(|n| {
                buf.truncate(n);
                buf
            }), file)
        })
        .await;
        match read {
            Ok((Ok(buf), _)) if buf.is_empty() => None,
            Ok((result, file)) => Some((result, file)),
            Err(_) => None,
        }
    });
    (
        [
            (CONTENT_TYPE, "application/octet-stream".to_string()),
            (axum::http::header::CONTENT_LENGTH, len.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// `/backups/{id}/{op}` adresinin GET karşılığı: yalnızca `download`.
pub async fn backup_get(state: State<AppState>, me: Extension<Identity>, Path((id, op)): Path<(String, String)>, query: Query<DownloadQuery>) -> Response {
    if op == "download" { backup_download(state, me, Path(id), query).await } else { StatusCode::NOT_FOUND.into_response() }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfirmQuery {
    op: String,
    name: String,
}

/// Geri alınamayan işlemlerden önce gösterilen onay sayfası (GET hiçbir şeyi değiştirmez).
pub async fn confirm(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(q): Query<ConfirmQuery>,
) -> Response {
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Some(g) = raw.parse().ok().and_then(|id| ctx.cluster.guest(id).ok()) else {
        return ctx.not_found(&format!("“{raw}” kimlikli bir konuk yok."));
    };
    let base = format!("/guests/{}", g.vmid);
    let name = q.name.as_str();
    let has_snapshot = g.snapshots.iter().any(|s| s.name == name);
    let has_backup = ctx.cluster.backups_of(g.vmid).any(|b| b.id == name);
    let c = match q.op.as_str() {
        "rollback" if has_snapshot => Confirm {
            title: "Anlık görüntüye dönülsün mü?",
            message: html! { b { (g.vmid) " (" (g.name) ")" } " diski " b { (name) } " anındaki haline dönecek. O andan sonra diske yazılan her şey kaybolur." },
            action: format!("{base}/snapshots/{name}/rollback"),
            button: "Geri Dön",
            back: format!("{base}?tab=goruntuler"),
            cli: format!("{} rollback {} {name}", g.kind, g.vmid),
        },
        "delsnapshot" if has_snapshot => Confirm {
            title: "Anlık görüntü silinsin mi?",
            message: html! { b { (name) } " anlık görüntüsü silinecek. Diskin şimdiki hali etkilenmez, ama o ana bir daha dönemezsiniz." },
            action: format!("{base}/snapshots/{name}/delete"),
            button: "Anlık Görüntüyü Sil",
            back: format!("{base}?tab=goruntuler"),
            cli: format!("{} delsnapshot {} {name}", g.kind, g.vmid),
        },
        "restore" if has_backup => Confirm {
            title: "Yedek geri yüklensin mi?",
            message: html! { b { (g.vmid) " (" (g.name) ")" } " diskinin yerine " b { (name) } " yedeği konacak. Diskin şimdiki içeriği ve bütün anlık görüntüler kaybolur."
                @if let Ok(disks) = st.snapshot().restore_disks(name) {
                    @if !disks.is_empty() { " Yedekteki ek diskler de (" (disks.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ")) ") geri yüklenir; şimdiki içerikleri kaybolur." }
                }
            },
            action: format!("/backups/{name}/restore"),
            button: "Geri Yükle",
            back: format!("{base}?tab=yedekler"),
            cli: format!("{} restore {} {name}", g.kind, g.vmid),
        },
        "delbackup" if has_backup => Confirm {
            title: "Yedek silinsin mi?",
            message: html! { b { (name) } " yedeği kalıcı olarak silinecek. Bu işlem geri alınamaz." },
            action: format!("/backups/{name}/delete"),
            button: "Yedeği Sil",
            back: format!("{base}?tab=yedekler"),
            cli: format!("backup delete {name}"),
        },
        "deldisk" if g.hw.disks.iter().any(|d| d.id.to_string() == name) => Confirm {
            title: "Ek disk silinsin mi?",
            message: html! { b { (g.vmid) " (" (g.name) ")" } " konuğunun " b { (name) } " numaralı ek diski ve içindeki bütün veriler kalıcı olarak silinecek. Bu işlem geri alınamaz." },
            action: format!("{base}/disks/{name}/delete"),
            button: "Diski Sil",
            back: format!("{base}?tab=donanim"),
            cli: format!("vm disk remove {} {name}", g.vmid),
        },
        _ => return ctx.not_found("Onaylanacak işlem bulunamadı; anlık görüntü ya da yedek silinmiş olabilir."),
    };
    let page = ctx.page(Current::Guest(g.vmid), format!("{base}/confirm"));
    Html(view_ops::confirm_page(&page, g, &c)).into_response()
}

/// Konsol: yöneticiler ve konuk kapsamındaki operatörler.
fn console_allowed(st: &AppState, me: &Identity, raw: &str) -> Result<(), Box<Response>> {
    let tags = raw.parse::<VmId>().ok().and_then(|vmid| st.guest_tags(vmid)).unwrap_or_default();
    if me.can_operate(&tags) {
        Ok(())
    } else {
        // Ekranda parola gibi gizli bilgiler görünebilir; salt okunur hesaplar konsolu göremez.
        Err(Box::new((StatusCode::FORBIDDEN, "Konsol için yönetici ya da bu konukta operatör yetkisi gerekir.").into_response()))
    }
}

/// Canlı konsol sayfasının içerik ilkesi: betik yalnızca bu sunucudan (konsol istemcisi), bağlantı yalnızca
/// bu sunucuya. Satır içi betik ve başka kaynak yine yasaktır. Uygulamanın geri kalanı betiksizdir.
pub const CONSOLE_CSP: &str = "default-src 'self'; script-src 'self'; connect-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

/// Canlı konsol sayfası: konuğun ekranı tuvalde, giriş WebSocket üzerinden.
pub async fn live_console(State(st): State<AppState>, Extension(me): Extension<Identity>, headers: HeaderMap, Path(raw): Path<String>) -> Response {
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Some(g) = raw.parse::<VmId>().ok().and_then(|id| ctx.cluster.guest(id).ok()) else {
        return ctx.not_found(&format!("“{raw}” kimlikli bir konuk yok."));
    };
    let available = st.driver().vnc_socket(g).is_some();
    let page = ctx.page(Current::Guest(g.vmid), format!("/guests/{}/console", g.vmid));
    ([(HeaderName::from_static("content-security-policy"), CONSOLE_CSP)], Html(view_ops::live_console_page(&page, g, available))).into_response()
}

/// Canlı konsolun WebSocket ucu: tarayıcı ile konuğun yerel VNC soketi arasında baytları taşır.
pub async fn vnc(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(raw): Path<String>,
    ws: Result<axum::extract::ws::WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    // Yetki, bağlantı yükseltilmeden önce denetlenir.
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let Some(guest) = raw.parse::<VmId>().ok().and_then(|id| st.snapshot().guest(id).ok().cloned()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(socket) = st.driver().vnc_socket(&guest) else {
        return (StatusCode::CONFLICT, "Konuk çalışmıyor ya da canlı konsolu yok (bir kez kapatıp açın).").into_response();
    };
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    st.auth().audit.record(now(), &me.user, &ip, &format!("console {}", guest.vmid), true);
    ws.on_upgrade(move |client| bridge(client, socket))
}

/// Bir konsol oturumunun en uzun süresi; dolunca bağlantı kapanır (sayfa yenilenerek yeniden açılır).
const CONSOLE_SESSION_LIMIT: std::time::Duration = std::time::Duration::from_secs(12 * 3600);

#[cfg(unix)]
async fn bridge(client: axum::extract::ws::WebSocket, socket: std::path::PathBuf) {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(stream) = tokio::net::UnixStream::connect(&socket).await else { return };
    let (mut from_guest, mut to_guest) = stream.into_split();
    let (mut to_browser, mut from_browser) = client.split();
    let up = async {
        while let Some(Ok(message)) = from_browser.next().await {
            match message {
                Message::Binary(bytes) => {
                    if to_guest.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let down = async {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match from_guest.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if to_browser.send(Message::Binary(buffer[..n].to_vec().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    // Bir yön kapanınca öbürü de bırakılır: iki uç da düşer.
    let _ = tokio::time::timeout(CONSOLE_SESSION_LIMIT, async {
        tokio::select! {
            _ = up => {}
            _ = down => {}
        }
    })
    .await;
}

#[cfg(not(unix))]
async fn bridge(_client: axum::extract::ws::WebSocket, _socket: std::path::PathBuf) {
    let _ = CONSOLE_SESSION_LIMIT;
}

/// Konsol çerçevesi: yalnızca aynı siteden çerçevelenebilir.
pub async fn screen(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Response {
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let Ok(vmid) = raw.parse::<VmId>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (HeaderName::from_static("x-frame-options"), "SAMEORIGIN"),
            (
                HeaderName::from_static("content-security-policy"),
                "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'self'",
            ),
        ],
        Html(view_ops::console_frame(vmid, now())),
    )
        .into_response()
}

pub async fn screen_png(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Response {
    if let Err(denied) = console_allowed(&st, &me, &raw) {
        return *denied;
    }
    let Ok(vmid) = raw.parse::<VmId>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Ekran dökümü bloklayan G/Ç yapar (QMP + dosya).
    match tokio::task::spawn_blocking(move || st.screenshot(vmid)).await {
        Ok(Ok(png)) => ([(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")], png).into_response(),
        Ok(Err(Error::GuestNotFound(_))) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => (StatusCode::SERVICE_UNAVAILABLE, capitalize(&e.to_string())).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct KeysForm {
    text: String,
    enter: Option<String>,
    key: String,
}

pub async fn keys(State(st): State<AppState>, Extension(_me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<KeysForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=konsol");
    let chords = if !f.key.is_empty() {
        match console::special_key(&f.key) {
            Some(chord) => vec![chord],
            None => return flash(&back, "error", &format!("Bilinmeyen tuş: {}", f.key)),
        }
    } else {
        let mut chords = match console::text_to_chords(&f.text) {
            Ok(c) => c,
            Err(c) => return flash(&back, "error", &format!("“{c}” karakteri gönderilemiyor; yalnızca ABD klavyesindeki karakterler desteklenir.")),
        };
        if f.enter.is_some() {
            chords.push(vec!["ret".to_string()]);
        }
        chords
    };
    if chords.is_empty() {
        return Redirect::to(&back);
    }
    if chords.len() > console::MAX_CHORDS {
        return flash(&back, "error", &format!("Tek seferde en fazla {} karakter gönderilebilir.", console::MAX_CHORDS));
    }
    match tokio::task::spawn_blocking(move || st.send_keys(vmid, &chords)).await {
        Ok(Ok(())) => Redirect::to(&back),
        Ok(Err(e)) => failed(&back, &e),
        Err(_) => flash(&back, "error", "Tuşlar gönderilemedi."),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExecForm {
    command: String,
}

/// Konteynerde komut çalıştırır ve çıktısını gösterir.
pub async fn exec(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Form(f): Form<ExecForm>,
) -> Response {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r.into_response(),
    };
    let command = f.command.trim().to_string();
    if command.is_empty() {
        return flash(&format!("/guests/{vmid}?tab=konsol"), "error", "Bir komut yazın.").into_response();
    }
    let (state, user, cmd) = (st.clone(), me.user.clone(), command.clone());
    // Komut 20 saniyeye kadar sürebilir: bloklayan iş parçacığında çalışır.
    let result = match tokio::task::spawn_blocking(move || state.exec(vmid, &cmd, &user)).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(Error::GuestNotFound(_))) => return failed("/", &Error::GuestNotFound(vmid)).into_response(),
        Ok(Err(Error::Driver(message))) => Err(message),
        Ok(Err(e)) => return failed(&format!("/guests/{vmid}?tab=konsol"), &e).into_response(),
        Err(_) => Err("komut çalıştırılamadı".to_string()),
    };
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Ok(g) = ctx.cluster.guest(vmid) else {
        return ctx.not_found("Konuk bulunamadı.");
    };
    let page = ctx.page(Current::Guest(vmid), format!("/guests/{vmid}?tab=konsol"));
    Html(view_ops::exec_page(&page, g, &command, &result)).into_response()
}

/// Seçili konuklara aynı eylemi uygular. Form aynı adlı birden çok `id` alanı taşır.
pub async fn bulk(State(st): State<AppState>, Extension(me): Extension<Identity>, RawForm(body): RawForm) -> Redirect {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("");
    let back = if value("back").starts_with("/nodes/") { value("back").to_string() } else { "/guests".to_string() };
    let Ok(action) = value("action").parse::<Action>() else {
        return flash(&back, "error", "Toplu işlem için bir eylem seçin.");
    };
    let ids: Vec<VmId> = pairs.iter().filter(|(k, _)| k == "id").filter_map(|(_, v)| v.parse().ok()).collect();
    if ids.is_empty() {
        return flash(&back, "error", "Önce en az bir konuk seçin.");
    }
    let (mut done, mut skipped) = (0, Vec::new());
    for id in &ids {
        match st.run_action(*id, action, &me.user) {
            Ok(_) => done += 1,
            Err(_) => skipped.push(id.to_string()),
        }
    }
    let mut message = format!("{} isteği {done} konuk için alındı.", action.label());
    if !skipped.is_empty() {
        message.push_str(&format!(" Uygun durumda olmadığı için atlananlar: {}.", skipped.join(", ")));
    }
    flash(&back, if done > 0 { "notice" } else { "error" }, &message)
}
