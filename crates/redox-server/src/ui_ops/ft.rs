//! Küme sayfasındaki hata toleransı formu.

use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FtForm {
    op: String,
    vmid: String,
    peer: String,
    witness: String,
}

pub async fn ft_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<FtForm>) -> Redirect {
    let Ok(vmid) = f.vmid.trim().parse::<VmId>() else {
        return flash("/cluster", "error", tx("Geçersiz konuk kimliği."));
    };
    let error = |e: Error| flash("/cluster", "error", &(capitalize(&e.to_string()) + "."));
    match f.op.as_str() {
        "on" => match st.ft_enable(vmid, f.peer.trim(), f.witness.trim(), &me.user) {
            Ok(()) => flash("/cluster", "notice", &tfmt(format!("{vmid} için hata toleransı açıldı; konuğu başlatınca ikizi kurulur."))),
            Err(e) => error(e),
        },
        "off" => {
            let user = me.user.clone();
            match tokio::task::spawn_blocking(move || st.ft_disable(vmid, &user)).await {
                Ok(Ok(())) => flash("/cluster", "notice", &tfmt(format!("{vmid} için hata toleransı kapatıldı."))),
                Ok(Err(e)) => error(e),
                Err(_) => flash("/cluster", "error", tx("İşlem başlatılamadı.")),
            }
        }
        _ => flash("/cluster", "error", tx("Bilinmeyen işlem.")),
    }
}
