//! API'nin OpenAPI 3 tarifi (`GET /api/v1/openapi.json`).
//!
//! Uçların tam listesini, yöntemlerini, yol değişkenlerini ve birer satırlık açıklamalarını verir; istek ve yanıt
//! gövdelerinin şemalarını **vermez**. Liste, `api.rs` içindeki yönlendirmelerle bir testte karşılaştırılır:
//! yeni bir uç eklenip buraya yazılmazsa test kırılır. Açıklamalar İngilizcedir (araçlar için).

use serde_json::{json, Map, Value};

/// (yöntem, yol, açıklama)
pub const OPERATIONS: &[(&str, &str, &str)] = &[
    ("get", "/openapi.json", "This document"),
    ("get", "/cluster", "Whole cluster state: nodes, guests, storages, backups, tasks"),
    ("get", "/nodes", "List nodes"),
    ("get", "/nodes/{id}", "One node"),
    ("get", "/guests", "List guests"),
    ("post", "/guests", "Create a guest (virtual machine or container)"),
    ("get", "/guests/{vmid}", "One guest"),
    ("delete", "/guests/{vmid}", "Delete a stopped guest"),
    ("patch", "/guests/{vmid}", "Change guest settings and hardware"),
    ("post", "/guests/{vmid}/snapshots", "Create a snapshot"),
    ("delete", "/guests/{vmid}/snapshots/{name}", "Delete a snapshot"),
    ("post", "/guests/{vmid}/snapshots/{name}/rollback", "Roll back to a snapshot"),
    ("post", "/guests/{vmid}/backups", "Start a backup"),
    ("post", "/guests/{vmid}/clone", "Clone a guest"),
    ("post", "/guests/{vmid}/template", "Turn a stopped guest into a template"),
    ("post", "/guests/{vmid}/suspend", "Suspend a running guest to disk"),
    ("post", "/guests/{vmid}/discard-suspend", "Discard the saved memory state"),
    ("get", "/guests/{vmid}/state", "Download the saved memory state (used when moving a guest)"),
    ("get", "/guests/{vmid}/export", "Export a guest as an OVA file"),
    ("post", "/guests/{vmid}/resize", "Grow a disk"),
    ("post", "/guests/{vmid}/disks", "Add a disk"),
    ("delete", "/guests/{vmid}/disks/{id}", "Remove an extra disk"),
    ("post", "/guests/{vmid}/nics", "Add a network card"),
    ("post", "/guests/{vmid}/balloon", "Set the memory balloon target or dynamic memory"),
    ("post", "/guests/{vmid}/cores", "Add CPU cores to a running virtual machine"),
    ("post", "/guests/{vmid}/memory", "Add memory to a running virtual machine"),
    ("post", "/backups/{id}/file", "Restore a file or folder from a backup into the running guest (needs the guest agent)"),
    ("get", "/guests/{vmid}/usage", "Resource usage counters of a guest"),
    ("delete", "/guests/{vmid}/usage", "Reset the usage counters of a guest"),
    ("get", "/guests/{vmid}/filesystems", "Disk usage inside the guest (needs the guest agent)"),
    ("post", "/guests/{vmid}/move", "Move a guest's disks to another storage"),
    ("post", "/guests/{vmid}/exec", "Run a command inside a container"),
    ("post", "/guests/{vmid}/detach", "Make an instantly restored guest independent of its backup"),
    ("post", "/guests/{vmid}/{action}", "Power action: start, shutdown, stop, reboot, pause or resume"),
    ("get", "/usage", "Resource usage counters of all guests"),
    ("get", "/usage.csv", "Resource usage as CSV"),
    ("get", "/backups", "List backups"),
    ("get", "/backups/report", "Backup report: protected and unprotected guests"),
    ("get", "/backups/archive", "Archived backups of deleted guests"),
    ("get", "/backups/forecast", "When the backup storage is expected to fill up"),
    ("delete", "/backups/{id}", "Delete a backup"),
    ("post", "/backups/{id}/restore", "Restore a backup over its guest"),
    ("post", "/backups/{id}/restore-new", "Restore a backup as a new guest"),
    ("post", "/backups/{id}/instant", "Start a guest directly from a backup"),
    ("post", "/backups/{id}/lock", "Protect a backup from deletion until a date"),
    ("post", "/backups/{id}/test", "Boot-test a backup in an isolated guest"),
    ("post", "/backups/{id}/verify", "Verify a backup's integrity"),
    ("post", "/backups/{id}/push", "Copy a backup to the off-site target"),
    ("post", "/backups/{id}/recover", "Recover an archived backup as a new guest"),
    ("post", "/backups/{id}/ack", "Dismiss the unusual-change warning of a backup"),
    ("get", "/backups/{id}/download", "Download a backup disk"),
    ("get", "/backups/{id}/blocks", "Block hashes of a backup disk (incremental replication)"),
    ("get", "/backups/{id}/block", "One block of a backup disk (incremental replication)"),
    ("get", "/backups/{id}/machine", "Download the machine state of a backup (UEFI variables, TPM)"),
    ("get", "/backups/{id}/files", "List files inside a backup"),
    ("get", "/backups/{id}/file", "Download a file or directory from inside a backup"),
    ("get", "/approvals", "Pending four-eyes approvals"),
    ("post", "/approvals/{id}/approve", "Approve a pending request"),
    ("delete", "/approvals/{id}", "Reject a pending request"),
    ("get", "/maintenance", "Maintenance mode state"),
    ("post", "/maintenance", "Turn maintenance mode on or off"),
    ("get", "/replicas", "List replications"),
    ("post", "/replicas", "Add a replication or change its interval"),
    ("delete", "/replicas/{peer}/{vmid}", "Remove a replication"),
    ("post", "/replicas/{peer}/{vmid}/run", "Run a replication now"),
    ("post", "/replicas/{peer}/{vmid}/failover", "Set or turn off automatic failover"),
    ("get", "/storage", "List storages"),
    ("get", "/media", "List installation media and disk images"),
    ("post", "/media/fetch", "Download media from a URL"),
    ("get", "/media/catalog", "Catalog of well-known images"),
    ("post", "/media/catalog/{key}", "Download an image from the catalog"),
    ("put", "/media/{name}", "Upload media"),
    ("delete", "/media/{name}", "Delete media"),
    ("get", "/peers", "Peer servers and their last known state"),
    ("get", "/support", "Support bundle: a state dump without secrets, for bug reports (administrators only)"),
    ("get", "/balance", "Load balancing advice: which VM to live-migrate to which peer when this server is busy"),
    ("get", "/mirrors", "Synchronous disk mirrors of guests on this server"),
    ("post", "/mirrors", "Start mirroring a VM's disks synchronously to a peer (vmid, peer)"),
    ("delete", "/mirrors/{vmid}", "Stop mirroring; ?delete_copy=true also deletes the copy on the peer"),
    ("get", "/mirror-stores", "Disk copies this server holds for its peers"),
    ("post", "/mirror-stores", "Used by the source server: open a new copy generation and serve its disks (encrypted NBD)"),
    ("post", "/mirror-stores/{origin}/{vmid}/report", "Used by the source server: tell whether a generation is in sync"),
    ("post", "/mirror-stores/{origin}/{vmid}/promote", "Take over: create a stopped guest from the last complete copy"),
    ("delete", "/mirror-stores/{origin}/{vmid}", "Delete a held copy"),
    ("get", "/vswitches", "Distributed switches: definition, local network status, guests using it and comparison with the peers"),
    ("post", "/vswitches", "Create or update a distributed switch (name vds*, vni, mtu, members: tunnel addresses of every server)"),
    ("delete", "/vswitches/{name}", "Remove a distributed switch; ?everywhere=true removes it on the peers too"),
    ("post", "/vswitches/{name}/push", "Send the switch definition to every peer"),
    ("post", "/peers/{name}/send/{vmid}", "Live-migrate a running virtual machine from this server to a peer"),
    ("get", "/profile", "This server's profile: start options, pools and backup jobs"),
    ("post", "/profile/diff", "Compare a profile with this server"),
    ("post", "/profile/apply", "Apply the pools and backup jobs of a profile to this server"),
    ("get", "/lockdown", "Whether lockdown (change freeze) is on"),
    ("post", "/lockdown", "Turn lockdown on or off; while on, every other change is refused"),
    ("post", "/peers/{name}/import/{vmid}", "Copy or move a guest from a peer server to this one"),
    ("post", "/peers/{name}/migrate/{vmid}", "Live-migrate a running virtual machine from a peer server to this one"),
    ("post", "/guests/{vmid}/migrate/prepare", "Live migration source: check the guest and report its hardware layout (called by the target server)"),
    ("post", "/guests/{vmid}/migrate", "Live migration source: send the guest to a waiting target (called by the target server)"),
    ("get", "/pools", "List resource pools"),
    ("post", "/pools", "Create or change a resource pool"),
    ("delete", "/pools/{name}", "Delete a resource pool"),
    ("get", "/jobs", "List backup jobs"),
    ("post", "/jobs", "Create or change a backup job"),
    ("delete", "/jobs/{name}", "Delete a backup job"),
    ("post", "/jobs/{name}/run", "Run a backup job now"),
    ("get", "/tasks", "List tasks"),
    ("get", "/tasks/{id}", "One task"),
];

pub fn document() -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for (method, path, summary) in OPERATIONS {
        let parameters: Vec<Value> = path
            .split('/')
            .filter_map(|part| part.strip_prefix('{').and_then(|p| p.strip_suffix('}')))
            .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": if name == "vmid" { "integer" } else { "string" } } }))
            .collect();
        let operation = json!({
            "summary": summary,
            "operationId": format!("{method}{}", path.replace(['/', '{', '}', '.', '-'], "_")),
            "parameters": parameters,
            "responses": {
                "2XX": { "description": "Success" },
                "401": { "description": "Missing or invalid token" },
                "403": { "description": "The token's role may not do this" },
                "4XX": { "description": "Rejected; the body is {\"error\": \"...\"}" },
            },
        });
        paths.entry(path.to_string()).or_insert_with(|| json!({})).as_object_mut().expect("nesne").insert(method.to_string(), operation);
    }
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "RedoX VE API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Paths, methods and path parameters of the RedoX VE API. Request and response bodies are not described here; see the API page of the wiki. Send Accept-Language: en for English error messages.",
        },
        "servers": [{ "url": "/api/v1" }],
        "components": { "securitySchemes": { "token": { "type": "http", "scheme": "bearer" } } },
        "security": [{ "token": [] }],
        "paths": paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `api.rs` içindeki yönlendirmeler: (yöntem, yol).
    fn routed() -> Vec<(String, String)> {
        let mut found = Vec::new();
        for line in include_str!("api.rs").lines() {
            let Some(rest) = line.trim().strip_prefix(".route(\"") else { continue };
            let Some((path, handlers)) = rest.split_once("\", ") else { continue };
            for (needle, method) in [("get(", "get"), ("post(", "post"), ("patch(", "patch"), ("put(", "put"), ("delete(", "delete"), ("delete_route(", "delete")] {
                // `delete_route(` içinde `delete(` geçmez; her yöntem bir kez sayılır.
                if handlers.contains(needle) && !found.contains(&(method.to_string(), path.to_string())) {
                    found.push((method.to_string(), path.to_string()));
                }
            }
        }
        found
    }

    #[test]
    fn every_api_route_is_described_and_nothing_else() {
        let mut routed = routed();
        let mut described: Vec<(String, String)> = OPERATIONS.iter().map(|(m, p, _)| (m.to_string(), p.to_string())).collect();
        routed.sort();
        described.sort();
        assert!(routed.len() > 80, "yönlendirmeler okunamadı: {}", routed.len());
        assert_eq!(described, routed);
        assert!(OPERATIONS.iter().all(|(_, _, summary)| !summary.is_empty()));
    }

    #[test]
    fn document_is_valid_openapi_shape() {
        let doc = document();
        assert_eq!(doc["openapi"], "3.0.3");
        let guest = &doc["paths"]["/guests/{vmid}"];
        assert_eq!(guest.as_object().unwrap().keys().cloned().collect::<Vec<_>>(), ["delete", "get", "patch"]);
        assert_eq!(guest["get"]["parameters"], json!([{ "name": "vmid", "in": "path", "required": true, "schema": { "type": "integer" } }]));
        assert_eq!(doc["paths"]["/replicas/{peer}/{vmid}/failover"]["post"]["parameters"].as_array().unwrap().len(), 2);
        // İşlem kimlikleri benzersizdir.
        let mut ids: Vec<String> = doc["paths"].as_object().unwrap().values().flat_map(|p| p.as_object().unwrap().values().map(|o| o["operationId"].as_str().unwrap().to_string()).collect::<Vec<_>>()).collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!((ids.len(), total), (OPERATIONS.len(), OPERATIONS.len()));
    }
}
