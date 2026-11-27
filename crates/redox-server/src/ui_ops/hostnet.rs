//! Düğüm sayfasındaki sunucu ağı formları.

use super::*;
use redox_core::hostnet::{BondMode, HostNet};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct HostNetForm {
    op: String,
    kind: String,
    name: String,
    mode: String,
    ports: String,
    parent: String,
    id: String,
    address: String,
    dhcp: String,
    nat: Option<String>,
    vlan_aware: Option<String>,
}

fn parse(f: &HostNetForm) -> Result<HostNet, String> {
    let ports = f.ports.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let name = f.name.trim().to_string();
    Ok(match f.kind.as_str() {
        "bridge" => HostNet::Bridge { name, ports, vlan_aware: f.vlan_aware.is_some() },
        "bond" => HostNet::Bond { name, mode: BondMode::parse(&f.mode).ok_or_else(|| tx("Bilinmeyen işlem.").to_string())?, ports },
        "vlan" => {
            let parent = f.parent.trim().to_string();
            let id: u16 = f.id.trim().parse().map_err(|_| tfmt(format!("{} bir tam sayı olmalı.", "VLAN")))?;
            HostNet::Vlan { name: format!("{parent}.{id}"), parent, id }
        }
        "guest" => {
            let (dhcp_start, dhcp_end) = match f.dhcp.trim() {
                "" => (None, None),
                range => {
                    let parsed = range.split_once('-').and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)));
                    let (a, b) = parsed.ok_or_else(|| tx("DHCP aralığını baş-son biçiminde yazın, örn. 10.10.0.50-10.10.0.200.").to_string())?;
                    (Some(a), Some(b))
                }
            };
            HostNet::Guest { name, address: f.address.trim().to_string(), dhcp_start, dhcp_end, nat: f.nat.is_some() }
        }
        _ => return Err(tx("Bilinmeyen işlem.").to_string()),
    })
}

pub async fn host_net_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<HostNetForm>) -> Redirect {
    let node = st.snapshot().nodes.first().map(|n| format!("/nodes/{}", n.id)).unwrap_or_else(|| "/".to_string());
    let (outcome, done) = match f.op.as_str() {
        "remove" => {
            let name = f.name.trim();
            (st.host_net_remove(name, &me.user), tfmt(format!("{name} ağı silindi.")))
        }
        "save" => match parse(&f) {
            Ok(net) => {
                let name = net.name().to_string();
                (st.host_net_put(net, &me.user), tfmt(format!("{name} ağı kaydedildi.")))
            }
            Err(message) => return flash(&node, "error", &capitalize(&message)),
        },
        _ => return flash(&node, "error", tx("Bilinmeyen işlem.")),
    };
    match outcome {
        Ok(()) => flash(&node, "notice", &done),
        Err(e) => flash(&node, "error", &(capitalize(&e.to_string()) + ".")),
    }
}
