//! Çalışırken donanım: depolama taşıma, bellek hedefi, disk ve ağ kartı.

use super::*;

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
pub struct BalloonForm {
    mb: String,
    /// Dinamik bellek formunda: alt sınır (MB); 0 kapatır.
    min: Option<String>,
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
    match st.request_disk_remove(vmid, id, &me.user) {
        Ok(Some(n)) => flash(&back, "notice", &format!("Disk silinmedi: ikinci bir yöneticinin onayı bekleniyor (istek #{n}).")),
        Ok(None) => flash(&back, "notice", &format!("{id} numaralı disk silindi.")),
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
