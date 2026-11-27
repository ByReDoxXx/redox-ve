//! Küme sayfasındaki ortak depo formu.

use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SharedForm {
    op: String,
    storage: String,
    peer: String,
    vmid: String,
}

pub async fn shared_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<SharedForm>) -> Redirect {
    let Ok(vmid) = f.vmid.trim().parse::<VmId>() else {
        return flash("/cluster", "error", tx("Konuk kimliği bir sayı olmalı."));
    };
    let take = f.op == "take";
    if !take && f.op != "adopt" {
        return flash("/cluster", "error", tx("Bilinmeyen işlem."));
    }
    // Eşle konuşan iş, istek iş parçacığını tutmaz.
    let work = tokio::task::spawn_blocking(move || if take { st.shared_take(&f.peer, vmid, &me.user) } else { st.shared_adopt(&f.storage, vmid, &me.user, crate::state::now()) });
    match work.await {
        Ok(Ok(())) if take => flash("/cluster", "notice", &tfmt(format!("{vmid} disk kopyalanmadan alındı."))),
        Ok(Ok(())) => flash("/cluster", "notice", &tfmt(format!("{vmid} devralındı."))),
        Ok(Err(e)) => flash("/cluster", "error", &(capitalize(&e.to_string()) + ".")),
        Err(_) => flash("/cluster", "error", tx("İşlem başlatılamadı.")),
    }
}
