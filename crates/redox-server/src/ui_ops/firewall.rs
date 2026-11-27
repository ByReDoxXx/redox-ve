//! Küme sayfasındaki güvenlik grubu ve adres kümesi formu.

use super::*;
use redox_core::fwsets::{FwGroup, IpSet};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FirewallForm {
    op: String,
    name: String,
    entries: String,
    rules: String,
    out: String,
}

pub async fn firewall_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<FirewallForm>) -> Redirect {
    let name = f.name.trim().to_string();
    let error = |message: String| flash("/cluster?tab=ag", "error", &message);
    let outcome = match f.op.as_str() {
        "save-set" => {
            let set = IpSet { name: name.clone(), entries: f.entries.split([',', ' ', '\n']).map(str::trim).filter(|e| !e.is_empty()).map(str::to_string).collect() };
            st.fw_definition(format!("firewall set {name}"), &me.user, |c| c.put_ip_set(set))
        }
        "save-group" => {
            let parsed = redox_core::hw::parse_fw_rules(&f.rules).and_then(|rules| Ok(FwGroup { name: name.clone(), rules, out: redox_core::hw::parse_fw_out_rules(&f.out)? }));
            match parsed {
                Ok(group) => st.fw_definition(format!("firewall group {name}"), &me.user, |c| c.put_fw_group(group)),
                Err(message) => return error(capitalize(&tfmt(message))),
            }
        }
        "remove-set" => st.fw_definition(format!("firewall set remove {name}"), &me.user, |c| c.remove_ip_set(&name)),
        "remove-group" => st.fw_definition(format!("firewall group remove {name}"), &me.user, |c| c.remove_fw_group(&name)),
        _ => return error(tx("Bilinmeyen işlem.").to_string()),
    };
    match outcome {
        Ok(()) if f.op.starts_with("remove") => flash("/cluster?tab=ag", "notice", &tfmt(format!("{name} tanımı silindi."))),
        Ok(()) => flash("/cluster?tab=ag", "notice", &tfmt(format!("{name} tanımı kaydedildi."))),
        Err(e) => error(capitalize(&e.to_string()) + "."),
    }
}
