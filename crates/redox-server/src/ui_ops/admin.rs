//! Kaynak havuzları ve dört göz onayları.

use super::*;

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
            text => text.parse().map_err(|_| tfmt(format!("{label} bir tam sayı olmalı."))),
        }
    };
    let parsed = (|| {
        Ok::<_, String>(redox_core::Pool {
            name: f.name.trim().to_string(),
            cores: number(&f.cores, (tx("Çekirdek kotası")))?.min(u32::MAX as u64) as u32,
            memory_mb: number(&f.memory_mb, (tx("Bellek kotası")))?,
            disk_gb: number(&f.disk_gb, (tx("Disk kotası")))?,
            guests: number(&f.guests, (tx("Konuk sayısı")))?.min(u32::MAX as u64) as u32,
        })
    })();
    let pool = match parsed {
        Ok(pool) => pool,
        Err(message) => return flash("/pools", "error", &message),
    };
    let name = pool.name.clone();
    match st.pool_put(pool) {
        Ok(()) => flash("/pools", "notice", &tfmt(format!("{name} havuzu kaydedildi."))),
        Err(e) => failed("/pools", &e),
    }
}

pub async fn pool_delete(State(st): State<AppState>, Path(name): Path<String>) -> Redirect {
    match st.pool_remove(&name) {
        Ok(()) => flash("/pools", "notice", &tfmt(format!("{name} havuzu silindi."))),
        Err(e) => failed("/pools", &e),
    }
}

/// Bakım kipi (`op`: `on`, `drain`, `off`).
pub async fn maintenance_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(op): Path<String>) -> Redirect {
    match op.as_str() {
        "on" => {
            st.maintenance_on(false, &me.user);
            flash("/", "notice", (tx("Bakım kipi açıldı: konuk başlatılmaz, zamanlanmış işler durdu.")))
        }
        "drain" => {
            let n = st.maintenance_on(true, &me.user);
            flash("/", "notice", &tfmt(format!("Bakım kipi açıldı; {n} konuk sırayla kapatılıyor. Bakım bitince yeniden başlatılırlar.")))
        }
        "off" => {
            let n = st.maintenance_off(&me.user);
            flash("/", "notice", &tfmt(format!("Bakım bitti; {n} konuk yeniden başlatılıyor.")))
        }
        _ => flash("/", "error", &tfmt(format!("Bilinmeyen işlem: {op}"))),
    }
}

/// Kilit kipini açar ya da kaldırır (`op`: `on` / `off`).
pub async fn lockdown_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(op): Path<String>) -> Redirect {
    match op.as_str() {
        "on" => {
            st.lockdown_set(true, &me.user);
            flash("/", "notice", (tx("Kilit kipi açıldı: kilit kaldırılana kadar hiçbir değişiklik yapılamaz.")))
        }
        "off" => {
            st.lockdown_set(false, &me.user);
            flash("/", "notice", (tx("Kilit kaldırıldı.")))
        }
        _ => flash("/", "error", &tfmt(format!("Bilinmeyen işlem: {op}"))),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct EvacuateForm {
    peer: String,
}

/// Küme sayfasındaki "Konuklarımı Buraya Taşı ve Bakıma Al" düğmesi.
pub async fn maintenance_evacuate(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<EvacuateForm>) -> Redirect {
    let (st2, peer, user) = (st.clone(), f.peer.trim().to_string(), me.user.clone());
    let started = tokio::task::spawn_blocking(move || st2.maintenance_evacuate(&peer, None, false, &user)).await;
    match started {
        Ok(Ok(n)) => flash("/cluster", "notice", &tfmt(format!("Bakım kipi açıldı; {n} sanal makine kapatılmadan {} sunucusuna taşınıyor. Sonuç görevlerde görünecek.", f.peer.trim()))),
        Ok(Err(e)) => flash("/cluster", "error", &(capitalize(&e.to_string()) + ".")),
        Err(_) => flash("/cluster", "error", (tx("İşlem başlatılamadı."))),
    }
}

/// Dört göz ilkesi: bekleyen isteği onaylar ya da reddeder (`op`: `approve` / `reject`).
pub async fn approval_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((id, op)): Path<(u64, String)>) -> Redirect {
    let result = match op.as_str() {
        "approve" => st.approve(id, &me.user).map(|done| capitalize(&done)),
        "reject" => st.reject(id, &me.user).map(|()| tfmt(format!("İstek #{id} reddedildi."))),
        _ => return flash("/", "error", &tfmt(format!("Bilinmeyen işlem: {op}"))),
    };
    match result {
        Ok(text) => flash("/", "notice", &text),
        Err(e) => failed("/", &e),
    }
}
