//! JSON REST API (`/api/v1`). Arayüz ve `redox` komut satırı aynı kuralları kullanır.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete as delete_route, get, post};
use axum::{Extension, Json, Router};
use redox_core::{
    Action, Backup, ClusterSummary, Error, ErrorKind, Guest, GuestPatch, GuestSpec, Node, Storage, Task, TaskId, VmId,
};
use serde_json::json;

use crate::auth::Identity;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/openapi.json", get(openapi))
        .route("/cluster", get(cluster))
        .route("/nodes", get(nodes))
        .route("/nodes/{id}", get(node))
        .route("/guests", get(guests).post(create))
        .route("/guests/{vmid}", get(guest).delete(delete).patch(update))
        .route("/guests/{vmid}/snapshots", post(snapshot_create))
        .route("/guests/{vmid}/snapshots/{name}", delete_route(snapshot_delete))
        .route("/guests/{vmid}/snapshots/{name}/rollback", post(snapshot_rollback))
        .route("/guests/{vmid}/backups", post(backup_create))
        .route("/guests/{vmid}/clone", post(clone))
        .route("/guests/{vmid}/template", post(template))
        .route("/guests/{vmid}/suspend", post(suspend))
        .route("/guests/{vmid}/discard-suspend", post(discard_suspend))
        .route("/guests/{vmid}/state", get(suspend_state))
        .route("/guests/{vmid}/export", get(crate::ui_ops::guest_export))
        .route("/guests/{vmid}/resize", post(resize))
        .route("/guests/{vmid}/disks", post(disk_add))
        .route("/guests/{vmid}/nics", post(nic_add))
        .route("/guests/{vmid}/balloon", post(balloon))
        .route("/guests/{vmid}/media", post(media_change))
        .route("/guests/{vmid}/cores", post(cores_add))
        .route("/guests/{vmid}/memory", post(memory_add))
        .route("/guests/{vmid}/usage", get(usage).delete(usage_reset))
        .route("/guests/{vmid}/filesystems", get(filesystems))
        .route("/usage", get(usage_all))
        .route("/usage.csv", get(usage_csv))
        .route("/guests/{vmid}/move", post(storage_move))
        .route("/guests/{vmid}/disks/{id}", delete_route(disk_remove))
        .route("/guests/{vmid}/exec", post(exec))
        .route("/backups", get(backups))
        .route("/backups/report", get(backup_report))
        .route("/backups/{id}", delete_route(backup_delete))
        .route("/backups/{id}/restore", post(backup_restore))
        .route("/backups/{id}/restore-new", post(backup_restore_new))
        .route("/backups/{id}/lock", post(backup_lock))
        .route("/backups/{id}/instant", post(backup_instant))
        .route("/guests/{vmid}/detach", post(guest_detach))
        .route("/backups/{id}/test", post(backup_test))
        .route("/backups/archive", get(backup_archive))
        .route("/backups/forecast", get(backup_forecast))
        .route("/approvals", get(approvals))
        .route("/maintenance", get(maintenance).post(maintenance_set))
        .route("/lockdown", get(lockdown).post(lockdown_set))
        .route("/replicas", get(replicas).post(replica_put))
        .route("/replicas/{peer}/{vmid}", delete_route(replica_delete))
        .route("/replicas/{peer}/{vmid}/run", post(replica_run))
        .route("/replicas/{peer}/{vmid}/failover", post(replica_failover))
        .route("/approvals/{id}/approve", post(approval_approve))
        .route("/approvals/{id}", delete_route(approval_reject))
        .route("/backups/{id}/recover", post(backup_recover))
        .route("/backups/{id}/ack", post(backup_ack))
        .route("/backups/{id}/push", post(backup_push))
        .route("/backups/{id}/verify", post(backup_verify))
        .route("/backups/{id}/download", get(crate::ui_ops::backup_download))
        .route("/backups/{id}/blocks", get(crate::ui_ops::backup_blocks))
        .route("/backups/{id}/block", get(crate::ui_ops::backup_block))
        .route("/backups/{id}/machine", get(backup_machine))
        .route("/backups/{id}/files", get(backup_files))
        .route("/backups/{id}/file", get(crate::ui_ops::backup_file).post(backup_put))
        .route("/guests/{vmid}/{action}", post(act))
        .route("/storage", get(storage))
        .route("/media", get(media_list))
        .route("/media/fetch", post(media_fetch))
        .route("/media/catalog", get(media_catalog))
        .route("/media/catalog/{key}", post(media_catalog_fetch))
        .route("/media/{name}", axum::routing::put(media_put).delete(media_delete))
        .route("/peers", get(peers))
        .route("/support", get(support_bundle))
        .route("/profile", get(profile))
        .route("/profile/diff", post(profile_diff))
        .route("/profile/apply", post(profile_apply))
        .route("/peers/{name}/import/{vmid}", post(peer_import))
        .route("/peers/{name}/migrate/{vmid}", post(peer_migrate))
        .route("/peers/{name}/send/{vmid}", post(peer_send))
        .route("/balance", get(balance))
        .route("/ft/protections", get(ft_protections).post(ft_enable))
        .route("/ft/protections/{vmid}", delete_route(ft_disable))
        .route("/ft/standby", get(ft_standbys).post(ft_prepare))
        .route("/ft/standby/{origin}/{vmid}", delete_route(ft_discard))
        .route("/ft/standby/{origin}/{vmid}/beat", post(ft_beat))
        .route("/ft/arbitrate", post(ft_arbitrate))
        .route("/ft/arbitrate/{key}", get(ft_winner))
        .route("/mirrors", get(mirrors).post(mirror_enable))
        .route("/mirrors/{vmid}", delete_route(mirror_disable))
        .route("/mirror-stores", get(mirror_stores).post(mirror_prepare))
        .route("/mirror-stores/{origin}/{vmid}", delete_route(mirror_store_delete))
        .route("/mirror-stores/{origin}/{vmid}/report", post(mirror_report))
        .route("/mirror-stores/{origin}/{vmid}/promote", post(mirror_promote))
        .route("/vswitches", get(vswitches).post(vswitch_put))
        .route("/firewall/sets", get(ip_sets).post(ip_set_put))
        .route("/firewall/sets/{name}", delete_route(ip_set_delete))
        .route("/firewall/groups", get(fw_groups).post(fw_group_put))
        .route("/firewall/groups/{name}", delete_route(fw_group_delete))
        .route("/vswitches/{name}", delete_route(vswitch_delete))
        .route("/vswitches/{name}/push", post(vswitch_push))
        .route("/guests/{vmid}/migrate/prepare", post(migrate_prepare))
        .route("/guests/{vmid}/migrate", post(migrate_out))
        .route("/pools", get(pools).post(pool_put))
        .route("/pools/{name}", delete_route(pool_delete))
        .route("/jobs", get(jobs).post(job_put))
        .route("/jobs/{name}", delete_route(job_delete))
        .route("/jobs/{name}/run", post(job_run))
        .route("/tasks", get(tasks))
        .route("/tasks/{id}", get(task))
}

pub enum ApiError {
    Core(Error),
    UnknownAction(String),
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError::Core(e)
    }
}

pub fn status_for(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::Driver => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::Core(e) => (status_for(e.kind()), e.code(), e.to_string()),
            ApiError::UnknownAction(a) => (
                StatusCode::NOT_FOUND,
                "unknown_action",
                format!("bilinmeyen eylem \"{a}\"; geçerli eylemler: start, stop, reboot, pause, resume"),
            ),
        };
        // İleti isteğin diline çevrilir (`Accept-Language: en` ya da `lang` çerezi); `code` dilden bağımsızdır.
        (status, Json(json!({ "error": crate::i18n::tfmt(message), "code": code }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

async fn cluster(State(st): State<AppState>) -> Json<ClusterSummary> {
    Json(st.snapshot().summary())
}

/// Eş sunucudaki kapalı sanal makineyi bu sunucuya kopyalar; sonuç görev günlüğünde görünür.
async fn peer_import(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((name, vmid)): Path<(String, VmId)>) -> Result<StatusCode, ApiError> {
    st.import_from_peer(&name, vmid, &me.user)?;
    Ok(StatusCode::ACCEPTED)
}

/// Bu sunucunun profili: başlatma seçenekleri, havuzlar ve yedek işleri (gizli bilgi içermez).
async fn profile(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(st.profile())
}

/// Verilen profili bu sunucuyla karşılaştırır. Bir şey değiştirmez; yine de yalnızca yöneticiye açıktır.
async fn profile_diff(State(st): State<AppState>, Json(other): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    Ok(Json(json!({ "differences": st.profile_compare(&other)? })))
}

/// Profildeki havuzları ve yedek işlerini bu sunucuya uygular; çalışırken değişmeyen farkları `manual` olarak döndürür.
async fn profile_apply(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(other): Json<serde_json::Value>) -> ApiResult<serde_json::Value> {
    let (applied, manual) = st.profile_apply(&other, &me.user)?;
    Ok(Json(json!({ "applied": applied, "manual": manual })))
}

/// Destek paketi: sorun bildirimi için durum dökümü (parola, token ve denetim günlüğü içermez). Yalnızca yönetici.
async fn support_bundle(State(st): State<AppState>, Extension(me): Extension<Identity>) -> Response {
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "destek paketini yalnızca yönetici alabilir", "code": "forbidden" }))).into_response();
    }
    Json(st.support_bundle()).into_response()
}

/// Eş sunucudaki çalışan sanal makineyi kapatmadan bu sunucuya taşır; sonuç görev günlüğünde görünür.
async fn peer_migrate(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((name, vmid)): Path<(String, VmId)>) -> Result<StatusCode, ApiError> {
    // Eşle konuşan iş, istek iş parçacığını tutmaz.
    let user = me.user.clone();
    tokio::task::spawn_blocking(move || st.migrate_from_peer(&name, vmid, &user)).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??;
    Ok(StatusCode::ACCEPTED)
}

/// Yük dengeleme önerisi: bu sunucu sıkışmışsa hangi sanal makinenin hangi eşe taşınmasının işe yarayacağı.
/// Yalnızca öneridir; bir şey taşınmaz.
async fn balance(State(st): State<AppState>) -> Json<crate::state::balance::Balance> {
    Json(st.balance())
}

/// Dağıtık anahtarlar: tanım, bu sunucudaki durum, kullanan konuklar ve eşlerdeki tanımla karşılaştırma.
async fn ip_sets(State(st): State<AppState>) -> Json<Vec<redox_core::fwsets::IpSet>> {
    Json(st.snapshot().ip_sets)
}

async fn ip_set_put(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(set): Json<redox_core::fwsets::IpSet>) -> Result<StatusCode, ApiError> {
    st.fw_definition(format!("firewall set {}", set.name), &me.user, |c| c.put_ip_set(set))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ip_set_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    st.fw_definition(format!("firewall set remove {name}"), &me.user, |c| c.remove_ip_set(&name))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn fw_groups(State(st): State<AppState>) -> Json<Vec<redox_core::fwsets::FwGroup>> {
    Json(st.snapshot().fw_groups)
}

async fn fw_group_put(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(group): Json<redox_core::fwsets::FwGroup>) -> Result<StatusCode, ApiError> {
    st.fw_definition(format!("firewall group {}", group.name), &me.user, |c| c.put_fw_group(group))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn fw_group_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    st.fw_definition(format!("firewall group remove {name}"), &me.user, |c| c.remove_fw_group(&name))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn vswitches(State(st): State<AppState>) -> Json<Vec<crate::state::vswitch::SwitchReport>> {
    Json(st.vswitch_report())
}

async fn vswitch_put(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(switch): Json<redox_core::VSwitch>) -> Result<StatusCode, ApiError> {
    st.vswitch_put(switch, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- Hata toleransı ----------

async fn ft_protections(State(st): State<AppState>) -> Json<Vec<redox_core::Protection>> {
    Json(st.snapshot().protections)
}

#[derive(serde::Deserialize)]
struct FtBody {
    vmid: VmId,
    peer: String,
    witness: String,
}

async fn ft_enable(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(body): Json<FtBody>) -> Result<StatusCode, ApiError> {
    st.ft_enable(body.vmid, &body.peer, &body.witness, &me.user)?;
    Ok(StatusCode::ACCEPTED)
}

async fn ft_disable(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>) -> Result<StatusCode, ApiError> {
    let user = me.user.clone();
    blocking(move || st.ft_disable(vmid, &user)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Burada duran ikizler.
async fn ft_standbys(State(st): State<AppState>) -> Json<Vec<redox_core::Standby>> {
    Json(st.snapshot().standbys)
}

/// Birincil sunucu çağırır: ikiz açılır ve diskleri ile belleği beklenir.
async fn ft_prepare(State(st): State<AppState>, Extension(me): Extension<Identity>, Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>, Json(request): Json<crate::state::ft::FtPrepare>) -> ApiResult<serde_json::Value> {
    let primary = request.primary_host.clone().unwrap_or(ip);
    let (user, host) = (me.user.clone(), st.migrate_settings().host);
    let (nbd_port, migrate_port) = blocking(move || st.ft_standby_prepare(&request, &primary, &user)).await?;
    Ok(Json(json!({ "nbd_port": nbd_port, "migrate_port": migrate_port, "host": host })))
}

#[derive(serde::Deserialize)]
struct BeatBody {
    generation: u32,
    alive: bool,
}

async fn ft_beat(State(st): State<AppState>, Path((origin, vmid)): Path<(String, VmId)>, Json(body): Json<BeatBody>) -> ApiResult<serde_json::Value> {
    let standby = blocking(move || Ok(st.ft_standby_beat(&origin, vmid, body.generation, body.alive))).await?;
    Ok(Json(json!({ "standby": standby })))
}

#[derive(serde::Deserialize)]
struct GenerationQuery {
    generation: u32,
}

async fn ft_discard(State(st): State<AppState>, Path((origin, vmid)): Path<(String, VmId)>, axum::extract::Query(q): axum::extract::Query<GenerationQuery>) -> Result<StatusCode, ApiError> {
    blocking(move || st.ft_standby_discard(&origin, vmid, q.generation)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct ArbitrateBody {
    key: String,
    side: redox_core::FtSide,
}

/// Hakem: bir koruma için ilk oy isteyen taraf kazanır.
async fn ft_arbitrate(State(st): State<AppState>, Json(body): Json<ArbitrateBody>) -> ApiResult<serde_json::Value> {
    Ok(Json(json!({ "winner": st.ft_vote(&body.key, body.side)? })))
}

async fn ft_winner(State(st): State<AppState>, Path(key): Path<String>) -> Json<serde_json::Value> {
    Json(json!({ "winner": st.ft_winner(&key) }))
}

// ---------- Eşzamanlı disk yansıtma ----------

/// Sürücü ya da eş sunucu bekleten işi istek iş parçacığının dışında çalıştırır.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, redox_core::Error> + Send + 'static) -> Result<T, ApiError> {
    Ok(tokio::task::spawn_blocking(work).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??)
}

/// Bu sunucudaki konukların yansıtmaları.
async fn mirrors(State(st): State<AppState>) -> Json<Vec<redox_core::Mirror>> {
    Json(st.snapshot().mirrors)
}

#[derive(serde::Deserialize)]
struct MirrorBody {
    vmid: VmId,
    peer: String,
}

async fn mirror_enable(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(body): Json<MirrorBody>) -> Result<StatusCode, ApiError> {
    st.mirror_enable(body.vmid, &body.peer, &me.user)?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(serde::Deserialize, Default)]
struct MirrorOffQuery {
    #[serde(default)]
    delete_copy: bool,
}

async fn mirror_disable(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, axum::extract::Query(q): axum::extract::Query<MirrorOffQuery>) -> Result<StatusCode, ApiError> {
    let user = me.user.clone();
    blocking(move || st.mirror_disable(vmid, q.delete_copy, &user)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Burada tutulan kopyalar (konuğun kaydı yalnızca özetlenir).
async fn mirror_stores(State(st): State<AppState>) -> Json<Vec<serde_json::Value>> {
    Json(st.snapshot().mirror_stores.iter().map(|s| json!({ "origin": s.origin, "vmid": s.vmid, "name": s.guest.name, "disks": 1 + s.guest.hw.disks.len(), "generation": s.generation, "complete": s.complete, "in_sync": s.in_sync, "last_in_sync": s.last_in_sync })).collect())
}

#[derive(serde::Deserialize)]
struct PrepareBody {
    origin: String,
    guest: Box<Guest>,
    sizes: Vec<u64>,
    key: String,
}

/// Kaynak sunucu çağırır: yeni kopya kuşağı açılır ve diskler yazılabilir sunulur.
async fn mirror_prepare(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(body): Json<PrepareBody>) -> ApiResult<serde_json::Value> {
    let user = me.user.clone();
    let host = st.migrate_settings().host;
    let (generation, port) = blocking(move || st.mirror_store_prepare(&body.origin, *body.guest, &body.sizes, &body.key, &user)).await?;
    Ok(Json(json!({ "generation": generation, "port": port, "host": host })))
}

#[derive(serde::Deserialize)]
struct ReportBody {
    generation: u32,
    in_sync: bool,
}

async fn mirror_report(State(st): State<AppState>, Path((origin, vmid)): Path<(String, VmId)>, Json(body): Json<ReportBody>) -> Result<StatusCode, ApiError> {
    blocking(move || st.mirror_store_report(&origin, vmid, body.generation, body.in_sync)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mirror_store_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((origin, vmid)): Path<(String, VmId)>) -> Result<StatusCode, ApiError> {
    let user = me.user.clone();
    blocking(move || st.mirror_store_delete(&origin, vmid, &user)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Devralma: kopyadan kapalı bir konuk oluşturur.
async fn mirror_promote(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((origin, vmid)): Path<(String, VmId)>) -> ApiResult<serde_json::Value> {
    let user = me.user.clone();
    let id = blocking(move || st.mirror_promote(&origin, vmid, &user)).await?;
    Ok(Json(json!({ "vmid": id })))
}

#[derive(serde::Deserialize, Default)]
struct EverywhereQuery {
    #[serde(default)]
    everywhere: bool,
}

fn peer_results(results: Vec<(String, Result<(), String>)>) -> serde_json::Value {
    json!(results.into_iter().map(|(peer, r)| json!({ "peer": peer, "ok": r.is_ok(), "error": r.err().map(crate::i18n::tfmt) })).collect::<Vec<_>>())
}

async fn vswitch_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>, axum::extract::Query(q): axum::extract::Query<EverywhereQuery>) -> Result<Response, ApiError> {
    if !q.everywhere {
        st.vswitch_remove(&name, &me.user)?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let user = me.user.clone();
    let results = tokio::task::spawn_blocking(move || st.vswitch_remove_everywhere(&name, &user)).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??;
    Ok(Json(json!({ "peers": peer_results(results) })).into_response())
}

/// Anahtarın tanımını bütün eş sunuculara gönderir.
async fn vswitch_push(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>) -> ApiResult<serde_json::Value> {
    let user = me.user.clone();
    let results = tokio::task::spawn_blocking(move || st.vswitch_push(&name, &user)).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??;
    Ok(Json(json!({ "peers": peer_results(results) })))
}

#[derive(serde::Deserialize, Default)]
struct SendBody {
    /// Bu sunucunun o eşteki adı; verilmezse eşin eş listesinden bulunur.
    #[serde(default, rename = "as")]
    as_name: Option<String>,
}

/// Bu sunucudaki çalışan sanal makineyi eş sunucuya canlı taşıtır (iş eşte yürür); sonuç görev günlüklerindedir.
async fn peer_send(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((name, vmid)): Path<(String, VmId)>, body: Option<Json<SendBody>>) -> Result<StatusCode, ApiError> {
    let (as_name, user) = (body.and_then(|Json(b)| b.as_name), me.user.clone());
    tokio::task::spawn_blocking(move || st.migrate_to_peer(&name, vmid, as_name.as_deref(), &user)).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??;
    Ok(StatusCode::ACCEPTED)
}

/// Canlı taşımanın kaynağı (hedef sunucu çağırır): konuk taşınabilir mi; hedefin açacağı donanım yerleşimi.
async fn migrate_prepare(State(st): State<AppState>, Path(vmid): Path<VmId>) -> ApiResult<serde_json::Value> {
    Ok(Json(json!({ "layout": st.migrate_prepare(vmid)? })))
}

#[derive(serde::Deserialize)]
struct MigrateBody {
    /// Hedefin adresi; verilmezse isteğin geldiği adres.
    #[serde(default)]
    host: Option<String>,
    /// Hedefte disklerin ve belleğin beklendiği kapılar.
    nbd_port: u16,
    ram_port: u16,
    /// Bu taşımanın bağlantılarını şifreleyen tek kullanımlık anahtar (64 onaltılık hane).
    key: String,
}

/// Canlı taşımanın kaynağı (hedef sunucu çağırır): konuğu, taşımayı bekleyen hedefe gönderir. Görev döner.
async fn migrate_out(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(vmid): Path<VmId>,
    Json(body): Json<MigrateBody>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let host = body.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).unwrap_or(&ip).to_string();
    let (st2, user) = (st.clone(), me.user.clone());
    let task = tokio::task::spawn_blocking(move || st2.migrate_out(vmid, &host, body.nbd_port, body.ram_port, &body.key, &user)).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??;
    accepted(&st, task)
}

/// Askıdaki konuğun bellek durumunu indirir (başka sunucuya taşımak için). Konuğun belleğinin tamamıdır:
/// parolalar ve anahtarlar içerebilir; yalnızca yönetici indirebilir.
async fn suspend_state(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>) -> Response {
    use std::io::Read;
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "bellek durumunu yalnızca yönetici indirebilir", "code": "forbidden" }))).into_response();
    }
    let guest = match st.snapshot().guest(vmid).cloned() {
        Ok(g) => g,
        Err(e) => return ApiError::Core(e).into_response(),
    };
    let found = st.driver().suspend_state(&guest).filter(|_| guest.hw.suspended && guest.lock.is_none());
    let Some((path, running, layout)) = found else {
        return (StatusCode::CONFLICT, Json(json!({ "error": format!("{vmid} askıda değil ya da üzerinde işlem sürüyor"), "code": "not_suspended" }))).into_response();
    };
    let opened = std::fs::File::open(&path).and_then(|file| Ok((file.metadata()?.len(), file)));
    let Ok((len, file)) = opened else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "bellek durumu dosyası okunamadı", "code": "driver" }))).into_response();
    };
    let stream = futures_util::stream::unfold(file, |mut file| async move {
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; 256 * 1024];
            let n = file.read(&mut buf);
            (
                n.map(|n| {
                    buf.truncate(n);
                    buf
                }),
                file,
            )
        })
        .await;
        match read {
            Ok((Ok(buf), _)) if buf.is_empty() => None,
            Ok((result, file)) => Some((result, file)),
            Err(_) => None,
        }
    });
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (axum::http::header::CONTENT_LENGTH, len.to_string()),
            (axum::http::HeaderName::from_static("x-redox-state-running"), if running { "1" } else { "0" }.to_string()),
            (axum::http::HeaderName::from_static("x-redox-state-layout"), layout.to_string()),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// Birleşik görünümdeki eş sunucuların son okunan durumu (token içermez).
async fn peers(State(st): State<AppState>) -> Json<Vec<crate::peers::PeerView>> {
    Json(st.peers().views())
}

async fn nodes(State(st): State<AppState>) -> Json<Vec<Node>> {
    Json(st.snapshot().nodes)
}

async fn node(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Node> {
    Ok(Json(st.snapshot().node(&id)?.clone()))
}

async fn guests(State(st): State<AppState>) -> Json<Vec<Guest>> {
    Json(st.snapshot().guests)
}

async fn guest(State(st): State<AppState>, Path(vmid): Path<VmId>) -> ApiResult<Guest> {
    Ok(Json(st.snapshot().guest(vmid)?.clone()))
}

async fn create(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Json(spec): Json<GuestSpec>,
) -> Result<(StatusCode, Json<Guest>), ApiError> {
    let vmid = st.create_guest(&spec, &me.user)?;
    let guest = st.snapshot().guest(vmid)?.clone();
    Ok((StatusCode::CREATED, Json(guest)))
}

async fn delete(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(vmid): Path<VmId>,
) -> Result<Response, ApiError> {
    Ok(queued(st.request_guest_delete(vmid, &me.user)?))
}

/// Silme yanıtı: yapıldıysa 204; dört göz ilkesi yüzünden onay bekliyorsa 202 ve isteğin numarası.
fn queued(approval: Option<u64>) -> Response {
    match approval {
        None => StatusCode::NO_CONTENT.into_response(),
        Some(id) => (StatusCode::ACCEPTED, Json(json!({ "approval": id, "message": "ikinci bir yöneticinin onayı bekleniyor" }))).into_response(),
    }
}

async fn approvals(State(st): State<AppState>) -> Json<Vec<redox_core::Approval>> {
    Json(st.snapshot().pending_approvals(crate::state::now()).into_iter().cloned().collect())
}

async fn approval_approve(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<u64>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!({ "done": st.approve(id, &me.user)? })))
}

async fn approval_reject(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<u64>) -> Result<StatusCode, ApiError> {
    st.reject(id, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn act(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path((vmid, action)): Path<(VmId, String)>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let action: Action = action.parse().map_err(|()| ApiError::UnknownAction(action))?;
    let id = st.run_action(vmid, action, &me.user)?;
    let task = st.snapshot().task(id)?.clone();
    Ok((StatusCode::ACCEPTED, Json(task)))
}

async fn update(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(vmid): Path<VmId>,
    Json(patch): Json<GuestPatch>,
) -> ApiResult<Guest> {
    st.update_guest(vmid, &patch, &me.user)?;
    Ok(Json(st.snapshot().guest(vmid)?.clone()))
}

#[derive(serde::Deserialize)]
struct SnapshotBody {
    name: String,
    /// Üretim denetim noktası: bellek durumu olmadan, dosya sistemleri dondurularak.
    #[serde(default)]
    production: bool,
}

fn accepted(st: &AppState, id: TaskId) -> Result<(StatusCode, Json<Task>), ApiError> {
    Ok((StatusCode::ACCEPTED, Json(st.snapshot().task(id)?.clone())))
}

async fn snapshot_create(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(vmid): Path<VmId>,
    Json(body): Json<SnapshotBody>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let id = if body.production { st.snapshot_create_production(vmid, &body.name, &me.user)? } else { st.snapshot_create(vmid, &body.name, &me.user)? };
    accepted(&st, id)
}

async fn snapshot_delete(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path((vmid, name)): Path<(VmId, String)>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let id = st.snapshot_delete(vmid, &name, &me.user)?;
    accepted(&st, id)
}

async fn snapshot_rollback(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path((vmid, name)): Path<(VmId, String)>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let id = st.snapshot_rollback(vmid, &name, &me.user)?;
    accepted(&st, id)
}

#[derive(serde::Deserialize)]
struct ResizeBody {
    disk_gb: u64,
}

#[derive(serde::Deserialize)]
struct ExecBody {
    command: String,
}

async fn clone(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(vmid): Path<VmId>,
    Json(body): Json<CloneBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let new_id = st.clone_guest_as(vmid, &body.name, body.linked, &me.user)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "vmid": new_id }))))
}

#[derive(serde::Deserialize)]
struct CloneBody {
    name: String,
    /// Şablondan bağlı klon (disk kopyalanmaz).
    #[serde(default)]
    linked: bool,
}

async fn suspend(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>) -> Result<(StatusCode, Json<Task>), ApiError> {
    let task = st.suspend(vmid, &me.user)?;
    accepted(&st, task)
}

async fn discard_suspend(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>) -> Result<StatusCode, ApiError> {
    st.discard_suspend(vmid, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct TemplateBody {
    on: bool,
}

async fn template(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<TemplateBody>) -> Result<Json<Guest>, ApiError> {
    st.set_template(vmid, body.on, &me.user)?;
    Ok(Json(st.snapshot().guest(vmid)?.clone()))
}

async fn resize(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(vmid): Path<VmId>,
    Json(body): Json<ResizeBody>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let id = st.resize_disk(vmid, body.disk_gb, &me.user)?;
    accepted(&st, id)
}

/// Konteynerde komut çalıştırır; çıktıyı döndürür. Komut 20 saniyeye kadar sürebilir.
async fn exec(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(vmid): Path<VmId>,
    Json(body): Json<ExecBody>,
) -> ApiResult<serde_json::Value> {
    let output = tokio::task::spawn_blocking(move || st.exec(vmid, &body.command, &me.user))
        .await
        .map_err(|_| ApiError::Core(Error::Driver("komut çalıştırılamadı".to_string())))??;
    Ok(Json(json!({ "output": output })))
}

async fn backups(State(st): State<AppState>) -> Json<Vec<Backup>> {
    Json(st.snapshot().backups)
}

async fn backup_create(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(vmid): Path<VmId>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = st.backup_create(vmid, &me.user)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": id }))))
}

async fn backup_restore(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let task = st.backup_restore(&id, &me.user)?;
    accepted(&st, task)
}

#[derive(serde::Deserialize)]
struct DiskBody {
    size_gb: u64,
}

async fn disk_add(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<DiskBody>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = st.disk_add(vmid, body.size_gb, &me.user)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id, "size_gb": body.size_gb }))))
}

#[derive(serde::Deserialize)]
struct NicBody {
    network: String,
}

async fn nic_add(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<NicBody>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let network = redox_core::hw::normalize_network(&body.network);
    let index = st.nic_add(vmid, &network, &me.user)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "index": index, "network": network }))))
}

async fn disk_remove(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((vmid, id)): Path<(VmId, u8)>) -> Result<Response, ApiError> {
    Ok(queued(st.request_disk_remove(vmid, id, &me.user)?))
}

/// Yedeği dış hedefe gönderir; sonuç görev günlüğünde görünür.
async fn backup_push(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    st.backup_push(&id, &me.user)?;
    Ok(StatusCode::ACCEPTED)
}

/// Yedeğin makine durumu arşivi (UEFI değişkenleri, sanal TPM durumu): başka sunucuya taşıma içindir.
/// TPM durumu disk şifreleme anahtarlarını korur; yalnızca yönetici indirebilir.
async fn backup_machine(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Response {
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "makine durumunu yalnızca yönetici indirebilir", "code": "forbidden" }))).into_response();
    }
    let missing = || (StatusCode::NOT_FOUND, Json(json!({ "error": "bu yedekte makine durumu yok", "code": "not_found" }))).into_response();
    if st.snapshot().backup(&id).map(|b| !b.machine).unwrap_or(true) {
        return missing();
    }
    let Some(path) = st.driver().backup_machine_file(&id) else { return missing() };
    match tokio::task::spawn_blocking(move || std::fs::read(path)).await {
        Ok(Ok(bytes)) => ([(axum::http::header::CONTENT_TYPE, "application/x-tar")], bytes).into_response(),
        _ => missing(),
    }
}

#[derive(serde::Deserialize)]
struct PutBody {
    /// 0: asıl disk; diğerleri yedekteki ek disk numarası.
    #[serde(default)]
    disk: u8,
    /// Yedeğin içindeki yol.
    path: String,
    /// Yol bir klasör mü? (Klasör, konukta var olan hedef dizine tar ile açılır.)
    #[serde(default)]
    dir: bool,
    /// Konuktaki hedef yol; verilmezse yedekteki yol.
    #[serde(default)]
    target: String,
}

/// Yedeğin içindeki bir dosyayı ya da klasörü çalışan konuğun içine geri yükler; sonuç görev günlüğünde görünür.
async fn backup_put(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(id): Path<String>,
    Json(body): Json<PutBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    st.auth().audit.record(crate::state::now(), &me.user, &ip, &format!("backup put {id} {} {}", body.path, body.target), true);
    let vmid = st.backup_put(&id, body.disk, &body.path, body.dir, &body.target, &me.user)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "vmid": vmid }))))
}

/// Yedeğin içindeki bir dizini listeler (yalnızca yönetici; sunucuda guestfish gerekir).
async fn backup_files(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<crate::ui_ops::FilesQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !me.can_write() {
        return Err(ApiError::Core(Error::InvalidSpec("yedeğin içindeki dosyaları yalnızca yönetici görebilir".to_string())));
    }
    let entries = crate::ui_ops::list_backup_files(&st, &me.user, &ip, &id, query.disk, &query.path()).await?;
    Ok(Json(json!({ "path": crate::guestfs::normalize(&query.path()), "entries": entries })))
}

/// Yedeği geçici bir konuk olarak açıp sınar; sonuç yedeğin kaydına ve görev günlüğüne yazılır.
/// Yedek alanının doluluk tahmini; yeterli yedek yoksa `null`.
async fn backup_forecast(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(match st.snapshot().backup_forecast(crate::state::now()) {
        Some((storage, f)) => json!({ "storage": storage, "mb_per_day": f.mb_per_day, "days_left": f.days_left }),
        None => serde_json::Value::Null,
    })
}

/// Arşiv: yerelde silinmiş ama kopya hedefinde saklanan yedekler.
async fn backup_archive(State(st): State<AppState>) -> Json<Vec<Backup>> {
    Json(st.snapshot().archive)
}

async fn backup_recover(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    st.backup_recover(&id, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Olağan dışı değişim uyarısını kapatır.
async fn backup_ack(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    st.backup_ack(&id, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn backup_test(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    st.backup_test(&id, &me.user)?;
    Ok(StatusCode::ACCEPTED)
}

/// Anında kurtarma: yedeğin üzerinde çalışan yeni bir konuk oluşturur; yeni konuğun kimliğini döndürür.
async fn backup_instant(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>, Json(body): Json<RestoreNewBody>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let vmid = st.backup_instant(&id, &body.name, &me.user)?;
    Ok((StatusCode::CREATED, Json(json!({ "vmid": vmid }))))
}

/// Anında kurtarılmış konuğu yedekten bağımsızlaştırır.
async fn guest_detach(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>) -> Result<(StatusCode, Json<Task>), ApiError> {
    let id = st.guest_detach(vmid, &me.user)?;
    accepted(&st, id)
}

/// Şimdiki yedek raporu: son 24 saat, korumasız konuklar ve toplamlar.
async fn backup_report(State(st): State<AppState>) -> Json<serde_json::Value> {
    let report = st.backup_report();
    Json(json!({ "subject": report.subject, "body": report.body, "taken": report.taken, "failed": report.failed, "unprotected": report.unprotected }))
}

#[derive(serde::Deserialize)]
struct LockBody {
    days: u32,
}

/// Yedeği belirtilen gün kadar silinemez yapar; kilidin bittiği anı döndürür.
async fn backup_lock(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>, Json(body): Json<LockBody>) -> Result<Json<serde_json::Value>, ApiError> {
    let until = st.backup_lock(&id, body.days, &me.user)?;
    Ok(Json(json!({ "locked_until": until })))
}

#[derive(serde::Deserialize)]
struct RestoreNewBody {
    name: String,
}

/// Yedekten yeni bir sanal makine oluşturur; yeni konuğun kimliğini döndürür.
async fn backup_restore_new(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>, Json(body): Json<RestoreNewBody>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let vmid = st.backup_restore_new(&id, &body.name, &me.user)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "vmid": vmid }))))
}

#[derive(serde::Deserialize)]
struct MoveBody {
    storage: String,
}

/// Kapalı konuğun disklerini başka bir depolama alanına taşır.
async fn storage_move(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<MoveBody>) -> Result<(StatusCode, Json<Task>), ApiError> {
    let id = st.storage_move(vmid, body.storage.trim(), &me.user)?;
    accepted(&st, id)
}

#[derive(serde::Deserialize)]
struct CoresBody {
    /// Çıkılacak çekirdek sayısı (şimdikinden büyük, konuğun en fazla çekirdek değerinden büyük değil).
    cores: u32,
}

/// Çalışan sanal makineye çekirdek ekler. `online`: eklenenleri konuk ajanı etkinleştirdi mi?
async fn cores_add(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<CoresBody>) -> ApiResult<serde_json::Value> {
    let online = st.cores_add(vmid, body.cores, &me.user)?;
    Ok(Json(json!({ "cores": body.cores, "online": online })))
}

#[derive(serde::Deserialize)]
struct MemoryBody {
    /// Çıkılacak bellek (MB; şimdikinden büyük, konuğun en fazla bellek değerinden büyük değil).
    memory_mb: u64,
}

/// Çalışan sanal makineye bellek ekler. `online`: konuğun yeni belleği kullandığı konuk ajanıyla doğrulandı mı?
async fn memory_add(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<MemoryBody>) -> ApiResult<serde_json::Value> {
    let online = st.memory_add(vmid, body.memory_mb, &me.user)?;
    Ok(Json(json!({ "memory_mb": body.memory_mb, "online": online })))
}

#[derive(serde::Deserialize)]
struct MediaBody {
    /// CD sürücüsü: 1 ya da 2.
    drive: u8,
    /// Takılacak kalıp; boşsa sürücü boşaltılır.
    #[serde(default)]
    iso: String,
}

async fn media_change(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<MediaBody>) -> Result<StatusCode, ApiError> {
    st.media_change(vmid, body.drive, &body.iso, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct BalloonBody {
    /// Hedef bellek (MB); 0 belleğin tamamı.
    #[serde(default)]
    mb: u64,
    /// Dinamik bellek: alt sınır (MB); 0 kapatır. Verilirse `mb` yok sayılır.
    #[serde(default)]
    min: Option<u64>,
}

async fn balloon(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>, Json(body): Json<BalloonBody>) -> Result<StatusCode, ApiError> {
    match body.min {
        Some(min) => st.dynamic_memory_set(vmid, min, &me.user)?,
        None => st.balloon_set(vmid, body.mb, &me.user)?,
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Havuzlar, kotaları ve şimdiki kullanımlarıyla.
fn usage_json(c: &redox_core::Cluster, g: &redox_core::Guest) -> serde_json::Value {
    let u = c.usage_of(g.vmid);
    json!({
        "vmid": g.vmid, "name": g.name, "pool": g.pool, "since": u.since, "uptime_secs": u.uptime_secs,
        "cpu_core_hours": (u.cpu_core_hours() * 1000.0).round() / 1000.0, "mem_gb_hours": (u.mem_gb_hours() * 1000.0).round() / 1000.0,
        "disk_read_mb": u.disk_read_bytes >> 20, "disk_write_mb": u.disk_write_bytes >> 20,
        "net_rx_mb": u.net_rx_bytes >> 20, "net_tx_mb": u.net_tx_bytes >> 20,
    })
}

async fn replicas(State(st): State<AppState>) -> Json<Vec<redox_core::Replica>> {
    Json(st.snapshot().replicas)
}

#[derive(serde::Deserialize)]
struct ReplicaBody {
    peer: String,
    vmid: VmId,
    /// Çoğaltma aralığı (dakika); verilmezse 60.
    #[serde(default)]
    every_min: Option<u32>,
}

async fn replica_put(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(body): Json<ReplicaBody>) -> Result<StatusCode, ApiError> {
    st.replica_put(body.peer.trim(), body.vmid, body.every_min.unwrap_or(60), &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn replica_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((peer, vmid)): Path<(String, VmId)>) -> Result<StatusCode, ApiError> {
    st.replica_remove(&peer, vmid, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct FailoverBody {
    /// Kaynağa bu kadar dakika ulaşılamazsa yedek kopya başlatılır; `null` kapatır.
    #[serde(default)]
    minutes: Option<u32>,
}

async fn replica_failover(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((peer, vmid)): Path<(String, VmId)>, Json(body): Json<FailoverBody>) -> Result<StatusCode, ApiError> {
    st.replica_failover_set(&peer, vmid, body.minutes, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn openapi() -> Json<serde_json::Value> {
    Json(crate::openapi::document())
}

async fn replica_run(State(st): State<AppState>, Path((peer, vmid)): Path<(String, VmId)>) -> Result<StatusCode, ApiError> {
    tokio::task::spawn_blocking(move || st.replica_run(&peer, vmid)).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??;
    Ok(StatusCode::ACCEPTED)
}

async fn lockdown(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "on": st.in_lockdown() }))
}

#[derive(serde::Deserialize)]
struct LockdownBody {
    on: bool,
}

/// Kilit kipini açar ya da kapatır (yalnızca yönetici). Kilit açıkken kabul edilen tek değişiklik budur.
async fn lockdown_set(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(body): Json<LockdownBody>) -> Json<serde_json::Value> {
    st.lockdown_set(body.on, &me.user);
    Json(json!({ "on": st.in_lockdown() }))
}

async fn maintenance(State(st): State<AppState>) -> Json<serde_json::Value> {
    let c = st.snapshot();
    Json(json!({ "on": c.maintenance, "resume": c.maintenance_resume }))
}

#[derive(serde::Deserialize)]
struct MaintenanceBody {
    on: bool,
    /// Bakıma girerken çalışan konukları kapat (bakım bitince yeniden başlatılırlar).
    #[serde(default)]
    drain: bool,
    /// Bakıma girerken çalışan sanal makineleri kapatmadan bu eş sunucuya taşı.
    #[serde(default)]
    evacuate: Option<String>,
    /// Bu sunucunun o eşteki adı; verilmezse eşin eş listesinden bulunur.
    #[serde(default, rename = "as")]
    as_name: Option<String>,
}

async fn maintenance_set(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(body): Json<MaintenanceBody>) -> ApiResult<serde_json::Value> {
    if let Some(peer) = body.evacuate.as_deref().filter(|_| body.on) {
        let (st2, peer2, as_name, drain, user) = (st.clone(), peer.to_string(), body.as_name.clone(), body.drain, me.user.clone());
        // Eşe sorulur (ağ çağrısı): istek iş parçacığını tutmaz.
        let moving = tokio::task::spawn_blocking(move || st2.maintenance_evacuate(&peer2, as_name.as_deref(), drain, &user)).await.map_err(|e| ApiError::from(redox_core::Error::Driver(e.to_string())))??;
        return Ok(Json(json!({ "on": true, "evacuating": moving, "peer": peer })));
    }
    let guests = if body.on { st.maintenance_on(body.drain, &me.user) } else { st.maintenance_off(&me.user) };
    Ok(Json(json!({ "on": body.on, "guests": guests })))
}

/// Hazır bulut imajı kataloğu.
async fn media_catalog() -> Json<Vec<serde_json::Value>> {
    Json(crate::media::CATALOG.iter().map(|(key, label, file, url)| json!({ "key": key, "label": label, "file": file, "url": url })).collect())
}

async fn media_catalog_fetch(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(key): Path<String>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = st.catalog_fetch(&key, &me.user)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "name": name }))))
}

/// Konuğun içinden (konuk ajanıyla) okunan dosya sistemi dolulukları.
async fn filesystems(State(st): State<AppState>, Path(vmid): Path<VmId>) -> Result<Json<Vec<redox_core::FsUsage>>, ApiError> {
    let c = st.snapshot();
    c.guest(vmid)?;
    Ok(Json(c.filesystems_of(vmid).to_vec()))
}

/// Konuğun birikimli kaynak tüketimi.
async fn usage(State(st): State<AppState>, Path(vmid): Path<VmId>) -> Result<Json<serde_json::Value>, ApiError> {
    let c = st.snapshot();
    Ok(Json(usage_json(&c, c.guest(vmid)?)))
}

/// Bütün konukların tüketimi, CSV olarak.
async fn usage_csv(State(st): State<AppState>) -> Response {
    ([(axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8")], crate::csv::usage(&st.snapshot())).into_response()
}

/// Bütün konukların tüketimi (faturalama için dışa aktarım).
async fn usage_all(State(st): State<AppState>) -> Json<Vec<serde_json::Value>> {
    let c = st.snapshot();
    Json(c.guests.iter().map(|g| usage_json(&c, g)).collect())
}

async fn usage_reset(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(vmid): Path<VmId>) -> Result<StatusCode, ApiError> {
    st.usage_reset(vmid, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pools(State(st): State<AppState>) -> Json<Vec<serde_json::Value>> {
    let c = st.snapshot();
    Json(c.pools.iter().map(|p| json!({ "name": p.name, "cores": p.cores, "memory_mb": p.memory_mb, "disk_gb": p.disk_gb, "guests": p.guests, "used": c.pool_usage(&p.name, None) })).collect())
}

/// Havuzu ekler; aynı adda havuz varsa kotalarını günceller.
async fn pool_put(State(st): State<AppState>, Json(pool): Json<redox_core::Pool>) -> Result<StatusCode, ApiError> {
    st.pool_put(pool)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pool_delete(State(st): State<AppState>, Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    st.pool_remove(&name)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn jobs(State(st): State<AppState>) -> Json<Vec<redox_core::BackupJob>> {
    Json(st.snapshot().jobs)
}

#[derive(serde::Deserialize)]
struct JobBody {
    name: String,
    time: String,
    /// `1-5` biçiminde; verilmezse her gün.
    #[serde(default)]
    days: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    keep: Option<u32>,
    /// Saklanacak haftalık, aylık ve yıllık yedekler (0: kapalı).
    #[serde(default)]
    weekly: u32,
    #[serde(default)]
    monthly: u32,
    #[serde(default)]
    yearly: u32,
    /// İşin aldığı yedekler kaç gün silinemez olsun.
    #[serde(default)]
    lock: u32,
    /// Yedek penceresinin sonu (`SS:DD`); boşsa sınırsız.
    #[serde(default)]
    until: String,
    /// Her yedekten sonra bozulma denetimi yapılsın mı?
    #[serde(default)]
    verify: bool,
}

/// Yedek işini ekler; aynı adda iş varsa günceller.
async fn job_put(State(st): State<AppState>, Json(body): Json<JobBody>) -> Result<StatusCode, ApiError> {
    let gfs = redox_core::Gfs { weekly: body.weekly, monthly: body.monthly, yearly: body.yearly };
    st.job_put(redox_core::BackupJob::from_text(&body.name, &body.time, &body.days, &body.tags.join(","), body.keep)?.with_retention(gfs, body.lock)?.with_window(&body.until, body.verify)?)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn job_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>) -> Result<Response, ApiError> {
    Ok(queued(st.request_job_remove(&name, &me.user)?))
}

async fn job_run(State(st): State<AppState>, Path(name): Path<String>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let queued = st.job_run(&name)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "queued": queued }))))
}

async fn media_list(State(st): State<AppState>) -> Json<Vec<crate::media::Media>> {
    Json(crate::media::list(&st))
}

/// Ham gövdeyle yükleme: `curl -T dosya.iso -H "Authorization: Bearer …" URL/api/v1/media/dosya.iso`.
/// İsteğe bağlı `X-Sha256` başlığı verilirse dosya özetiyle doğrulanır.
async fn media_put(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>, headers: axum::http::HeaderMap, body: axum::body::Body) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    use futures_util::StreamExt;
    let (dest, _) = crate::media::target(&st, &name)?;
    let sha256 = headers.get("x-sha256").and_then(|v| v.to_str().ok()).map(str::to_string);
    let stored: Result<u64, String> = async {
        let mut upload = crate::media::Upload::begin(dest)?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            upload.write(chunk.map_err(|e| format!("yükleme yarıda kesildi: {e}"))?).await?;
        }
        upload.finish(sha256.as_deref()).await
    }
    .await;
    st.media_uploaded(&name, &stored.as_ref().map(|_| ()).map_err(Clone::clone), &me.user);
    let bytes = stored.map_err(redox_core::Error::InvalidSpec)?;
    Ok((StatusCode::CREATED, Json(json!({ "name": name, "bytes": bytes }))))
}

#[derive(serde::Deserialize)]
struct FetchBody {
    url: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    sha256: String,
}

async fn media_fetch(State(st): State<AppState>, Extension(me): Extension<Identity>, Json(body): Json<FetchBody>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = st.media_fetch(&body.url, &body.name, &body.sha256, &me.user)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "name": name }))))
}

async fn media_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    st.media_delete(&name, &me.user)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn backup_verify(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    st.backup_verify(&id, &me.user)?;
    Ok(StatusCode::ACCEPTED)
}

async fn backup_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>) -> Result<Response, ApiError> {
    Ok(queued(st.request_backup_delete(&id, &me.user)?))
}

async fn storage(State(st): State<AppState>) -> Json<Vec<Storage>> {
    Json(st.snapshot().storages)
}

async fn tasks(State(st): State<AppState>) -> Json<Vec<Task>> {
    Json(st.snapshot().tasks)
}

async fn task(State(st): State<AppState>, Path(id): Path<TaskId>) -> ApiResult<Task> {
    Ok(Json(st.snapshot().task(id)?.clone()))
}
