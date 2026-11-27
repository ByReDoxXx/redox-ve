//! Küme sayfasındaki disk yansıtma formları.

use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MirrorForm {
    op: String,
    vmid: String,
    peer: String,
    origin: String,
    delete_copy: Option<String>,
}

pub async fn mirror_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<MirrorForm>) -> Redirect {
    let Ok(vmid) = f.vmid.trim().parse::<VmId>() else {
        return flash("/cluster?tab=koruma", "error", tx("Geçersiz konuk kimliği."));
    };
    let error = |e: Error| flash("/cluster?tab=koruma", "error", &(capitalize(&e.to_string()) + "."));
    if f.op == "add" {
        return match st.mirror_enable(vmid, f.peer.trim(), &me.user) {
            Ok(()) => flash("/cluster?tab=koruma", "notice", &tfmt(format!("{vmid} diskleri {} sunucusuna yansıtılacak; ilk eşitleme birazdan başlar.", f.peer.trim()))),
            Err(e) => error(e),
        };
    }
    if !matches!(f.op.as_str(), "off" | "promote" | "delete") {
        return flash("/cluster?tab=koruma", "error", tx("Bilinmeyen işlem."));
    }
    // Eşle ya da sürücüyle konuşan işler istek iş parçacığını tutmaz.
    let (op, origin, delete_copy, user) = (f.op.clone(), f.origin.trim().to_string(), f.delete_copy.is_some(), me.user.clone());
    let work = tokio::task::spawn_blocking(move || match op.as_str() {
        "off" => st.mirror_disable(vmid, delete_copy, &user).map(|()| tfmt(format!("{vmid} yansıtması kapatıldı."))),
        "promote" => st.mirror_promote(&origin, vmid, &user).map(|new| tfmt(format!("Kopyadan {new} numaralı konuk oluşturuldu; kapalı duruyor."))),
        _ => st.mirror_store_delete(&origin, vmid, &user).map(|()| tfmt(format!("{origin}/{vmid} kopyası silindi."))),
    });
    match work.await {
        Ok(Ok(done)) => flash("/cluster?tab=koruma", "notice", &done),
        Ok(Err(e)) => error(e),
        Err(_) => flash("/cluster?tab=koruma", "error", tx("İşlem başlatılamadı.")),
    }
}
