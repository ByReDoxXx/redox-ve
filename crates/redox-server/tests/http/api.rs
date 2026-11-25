//! JSON API.

use super::*;

#[tokio::test]
pub(super) async fn api_reads() {
    let (app, _) = setup();
    let (status, v) = api(&app, get("/api/v1/cluster")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((v["guests_total"].as_u64(), v["quorate"].as_bool(), v["cpu_pct"].as_u64()), (Some(8), Some(true), Some(40)));

    assert_eq!(api(&app, get("/api/v1/nodes")).await.1.as_array().unwrap().len(), 3);
    assert_eq!(api(&app, get("/api/v1/guests")).await.1.as_array().unwrap().len(), 8);
    assert_eq!(api(&app, get("/api/v1/storage")).await.1.as_array().unwrap().len(), 4);
    assert_eq!(api(&app, get("/api/v1/guests/102")).await.1["name"], "db-postgres");
    assert_eq!(api(&app, get("/api/v1/nodes/pve-ank-01")).await.1["cores"], 16);

    for (uri, code) in [("/api/v1/guests/999", "guest_not_found"), ("/api/v1/nodes/yok", "node_not_found"), ("/api/v1/tasks/42", "task_not_found")] {
        let (status, v) = api(&app, get(uri)).await;
        assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some(code)), "{uri}");
        assert!(v["error"].as_str().is_some_and(|m| !m.is_empty()));
    }
}

#[tokio::test]
pub(super) async fn api_actions_and_conflicts() {
    let (app, state) = setup();
    let start = || request(Method::POST, "/api/v1/guests/103/start").body(Body::empty()).unwrap();

    let (status, task) = api(&app, start()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!((task["command"].as_str(), task["status"].as_str()), (Some("vm start 103"), Some("ok")));
    assert_eq!(api(&app, get("/api/v1/guests/103")).await.1["status"], "running");
    let id = task["id"].as_u64().unwrap();
    assert_eq!(api(&app, get(&format!("/api/v1/tasks/{id}"))).await.1["vmid"], 103);

    let (status, v) = api(&app, start()).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("invalid_transition")));

    let explode = request(Method::POST, "/api/v1/guests/103/explode").body(Body::empty()).unwrap();
    let (status, v) = api(&app, explode).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("unknown_action")));

    assert_eq!(state.snapshot().tasks.iter().filter(|t| t.status == TaskStatus::Ok).count(), 1);
}

#[tokio::test]
pub(super) async fn api_create_and_delete() {
    let (app, state) = setup();
    let spec = json!({"name": "api-vm", "kind": "ct", "node": "pve-ank-01", "cores": 1, "memory_mb": 256, "disk_gb": 8, "storage": "local-lvm"});
    let (status, g) = api(&app, post_json("/api/v1/guests", &spec)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!((g["vmid"].as_u64(), g["status"].as_str(), g["bridge"].as_str()), (Some(105), Some("stopped"), Some("nat")), "ağ verilmezse NAT");

    let mut bad = spec.clone();
    bad["name"] = json!("Kötü Ad");
    let (status, v) = api(&app, post_json("/api/v1/guests", &bad)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid_name")));

    let mut huge = spec.clone();
    huge["name"] = json!("buyuk");
    huge["disk_gb"] = json!(99_999);
    let (status, v) = api(&app, post_json("/api/v1/guests", &huge)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("insufficient_storage")));

    let del = |id: u32| request(Method::DELETE, &format!("/api/v1/guests/{id}")).body(Body::empty()).unwrap();
    let (status, v) = api(&app, del(100)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::CONFLICT, Some("not_stopped")));
    assert_eq!(send(&app, del(105)).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, del(105)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(state.snapshot().guests.len(), 8);
}

// ---------- Kimlik doğrulama ve yetki ----------

#[tokio::test]
pub(super) async fn openapi_document_lists_the_api() {
    let (app, _) = setup();
    let (status, doc) = api(&app, get("/api/v1/openapi.json")).await;
    assert_eq!((status, doc["openapi"].as_str(), doc["servers"][0]["url"].as_str()), (StatusCode::OK, Some("3.0.3"), Some("/api/v1")));
    assert_eq!(doc["paths"]["/guests"]["post"]["summary"], "Create a guest (virtual machine or container)");
    assert!(doc["paths"]["/openapi.json"]["get"].is_object());
    // Oturumsuz okunamaz.
    assert_eq!(send(&app, anon(Method::GET, "/api/v1/openapi.json").body(Body::empty()).unwrap()).await.0, StatusCode::UNAUTHORIZED);
}
