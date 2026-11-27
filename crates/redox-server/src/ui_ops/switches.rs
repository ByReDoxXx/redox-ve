//! Küme sayfasındaki dağıtık anahtar formu.

use super::*;
use redox_core::VSwitch;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SwitchForm {
    op: String,
    name: String,
    vni: String,
    mtu: String,
    members: String,
    everywhere: Option<String>,
}

fn parse(f: &SwitchForm) -> Result<VSwitch, String> {
    let number = |raw: &str, label: &str| raw.trim().parse::<u32>().map_err(|_| tfmt(format!("{label} bir tam sayı olmalı.")));
    let members = f
        .members
        .split([',', ' '])
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| m.parse().map_err(|_| tfmt(format!("geçersiz adres \"{m}\""))))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(VSwitch { name: f.name.trim().to_string(), vni: number(&f.vni, "VNI")?, mtu: number(&f.mtu, "MTU")?, members })
}

/// Eş sonuçlarını tek iletiye indirir: hepsi başarılıysa bildirim, değilse hata.
fn report(done: String, results: &[(String, Result<(), String>)]) -> Redirect {
    let failed: Vec<String> = results.iter().filter_map(|(peer, r)| r.as_ref().err().map(|e| format!("{peer}: {}", tfmt(e.clone())))).collect();
    if failed.is_empty() {
        flash("/cluster", "notice", &done)
    } else {
        flash("/cluster", "error", &format!("{} {}", tx("Bazı eşlerde işlem yapılamadı:"), failed.join("; ")))
    }
}

pub async fn switch_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<SwitchForm>) -> Redirect {
    let name = f.name.trim().to_string();
    let error = |e: Error| flash("/cluster", "error", &(capitalize(&e.to_string()) + "."));
    match f.op.as_str() {
        "save" => match parse(&f) {
            Err(message) => flash("/cluster", "error", &message),
            Ok(switch) => match st.vswitch_put(switch, &me.user) {
                Ok(()) => flash("/cluster", "notice", &tfmt(format!("{name} anahtarı kaydedildi."))),
                Err(e) => error(e),
            },
        },
        "remove" if f.everywhere.is_none() => match st.vswitch_remove(&name, &me.user) {
            Ok(()) => flash("/cluster", "notice", &tfmt(format!("{name} anahtarı silindi."))),
            Err(e) => error(e),
        },
        // Eşlerle konuşan işler istek iş parçacığını tutmaz.
        op @ ("push" | "remove") => {
            let (push, st2, user, target) = (op == "push", st.clone(), me.user.clone(), name.clone());
            let work = tokio::task::spawn_blocking(move || if push { st2.vswitch_push(&target, &user) } else { st2.vswitch_remove_everywhere(&target, &user) });
            match work.await {
                Ok(Ok(results)) if push => report(tfmt(format!("{name} anahtarı eşlere gönderildi.")), &results),
                Ok(Ok(results)) => report(tfmt(format!("{name} anahtarı silindi.")), &results),
                Ok(Err(e)) => error(e),
                Err(_) => flash("/cluster", "error", tx("İşlem başlatılamadı.")),
            }
        }
        _ => flash("/cluster", "error", tx("Bilinmeyen işlem.")),
    }
}
