//! Arayüz istek işleyicileri: sayfalar GET, işlemler POST + 303 yönlendirme.

#[allow(unused_imports)]
use crate::i18n::{tfmt, tx};
use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use redox_core::command::{self, Resolved};
use redox_core::{Action, Cluster, Error, GuestKind, GuestSpec, VmId};
use serde::Deserialize;

use crate::api::status_for;
use crate::auth::{Identity, LoginError, SESSION_COOKIE};
use crate::{cookie, ClientIp};
use crate::state::{now, AppState};
use crate::view::{self, enc, Current, FieldErrors, NewGuestForm, Page};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PageQuery {
    pub(crate) q: String,
    notice: Option<String>,
    error: Option<String>,
    tab: String,
    kind: String,
    /// Grafik aralığı: boş (10 dakika), `24s`, `30g`.
    range: String,
}

/// Bir isteğin sayfa bağlamı: küme görüntüsü, tema ve bildirimler.
pub(crate) struct Ctx {
    pub(crate) cluster: Cluster,
    theme: &'static str,
    query: PageQuery,
    now: u64,
    user: String,
    admin: bool,
    operator: bool,
    scope: Vec<String>,
    real: bool,
    ct: bool,
    live: bool,
    live_ct: bool,
    hotplug: bool,
    files: bool,
    backup_age: u32,
    offsite: Option<String>,
    sealed: bool,
    range: crate::history::Range,
    history: crate::state::History,
}

impl Ctx {
    pub(crate) fn new(st: &AppState, headers: &HeaderMap, query: PageQuery, me: &Identity) -> Self {
        let range = crate::history::Range::parse(&query.range);
        Ctx {
            cluster: st.snapshot(),
            theme: theme_of(headers),
            now: now(),
            user: me.user.clone(),
            admin: me.can_write(),
            operator: me.role == crate::auth::Role::Operator,
            scope: me.scope.clone(),
            real: st.driver().is_real(),
            ct: st.driver().supports(GuestKind::Ct),
            live: st.driver().live_disk_ops(GuestKind::Vm),
            live_ct: st.driver().live_backup(GuestKind::Ct),
            hotplug: st.driver().hotplug(GuestKind::Vm),
            files: st.guestfish().is_some(),
            backup_age: st.backup_age_days(),
            offsite: st.offsite().map(|c| c.describe()),
            sealed: st.driver().backups_sealed(),
            range,
            query,
            history: if st.driver().is_real() { st.history_for(range) } else { Default::default() },
        }
    }

    pub(crate) fn page(&self, current: Current, back: impl Into<String>) -> Page<'_> {
        Page {
            cluster: &self.cluster,
            theme: self.theme,
            current,
            q: self.query.q.trim(),
            notice: self.query.notice.as_deref(),
            error: self.query.error.as_deref(),
            now: self.now,
            // Seçili aralık sayfa yenilenince ve tema değişince korunur.
            back: {
                let back: String = back.into();
                match self.range.as_str() {
                    "" => back,
                    range => format!("{back}{}range={range}", if back.contains('?') { '&' } else { '?' }),
                }
            },
            range: self.range,
            user: &self.user,
            admin: self.admin,
            operator: self.operator,
            scope: &self.scope,
            real: self.real,
            ct: self.ct,
            live: self.live,
            live_ct: self.live_ct,
            hotplug: self.hotplug,
            files: self.files,
            backup_age: self.backup_age,
            offsite: self.offsite.as_deref(),
            sealed: self.sealed,
            history: &self.history,
        }
    }

    pub(crate) fn not_found(&self, message: &str) -> Response {
        let page = self.page(Current::Other, "/");
        (StatusCode::NOT_FOUND, Html(view::error_page(&page, (tx("Bulunamadı")), message))).into_response()
    }
}

fn theme_of(headers: &HeaderMap) -> &'static str {
    let cookies = headers.get(COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("");
    if cookies.split(';').any(|c| c.trim() == "theme=light") {
        "light"
    } else {
        "dark"
    }
}

fn with_q(path: &str, q: &str) -> String {
    if q.trim().is_empty() {
        path.to_string()
    } else {
        format!("{path}?q={}", enc(q.trim()))
    }
}

pub async fn datacenter(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    Html(view::datacenter(&ctx.page(Current::Datacenter, "/")))
}

/// Konuk listesi: `q` metne, `tab` duruma göre süzer.
pub async fn guests(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    let only = ctx.query.tab.clone();
    let mut parts = Vec::new();
    if ["calisan", "kapali", "sablon"].contains(&only.as_str()) {
        parts.push(format!("tab={only}"));
    }
    if !ctx.query.q.trim().is_empty() {
        parts.push(format!("q={}", enc(ctx.query.q.trim())));
    }
    let back = if parts.is_empty() { "/guests".to_string() } else { format!("/guests?{}", parts.join("&")) };
    Html(view::guests(&ctx.page(Current::Guests, back), &only))
}

pub async fn tasks(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    let back = with_q("/tasks", &ctx.query.q);
    Html(view::tasks(&ctx.page(Current::Tasks, back)))
}

pub async fn backups(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    Html(view::backups(&ctx.page(Current::Backups, "/backups")))
}

pub async fn pools(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    Html(view::pools(&ctx.page(Current::Other, "/pools")))
}

pub async fn node(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Response {
    let ctx = Ctx::new(&st, &headers, query, &me);
    let Ok(node) = ctx.cluster.node(&id) else {
        return ctx.not_found(&tfmt(format!("“{id}” adında bir düğüm yok.")));
    };
    let back = with_q(&format!("/nodes/{id}"), &ctx.query.q);
    Html(view::node(&ctx.page(Current::Node(id.clone()), back), node)).into_response()
}

fn parse_vmid(raw: &str) -> Option<VmId> {
    raw.parse().ok()
}

pub async fn guest(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(query): Query<PageQuery>,
) -> Response {
    let ctx = Ctx::new(&st, &headers, query, &me);
    let Some(g) = parse_vmid(&raw).and_then(|id| ctx.cluster.guest(id).ok()) else {
        return ctx.not_found(&tfmt(format!("“{raw}” kimlikli bir konuk yok. Silinmiş olabilir.")));
    };
    let tab = ctx.query.tab.as_str();
    let back = if tab.is_empty() || tab == "ozet" {
        format!("/guests/{}", g.vmid)
    } else {
        format!("/guests/{}?tab={}", g.vmid, enc(tab))
    };
    let opts = view::FormOptions { vnc: st.driver().vnc_socket(g).is_some(), ..form_options(&st) };
    Html(view::guest(&ctx.page(Current::Guest(g.vmid), back), g, tab, &opts)).into_response()
}

pub async fn storage(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    let media = crate::media::list(&st);
    Html(view::storage(&ctx.page(Current::Storage, "/storage"), &media))
}

pub async fn peers(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    Html(view::peers(&ctx.page(Current::Peers, "/cluster"), &st.peers().views()))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ImportForm {
    peer: String,
    vmid: String,
}

pub async fn peer_import(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<ImportForm>) -> Redirect {
    let Some(vmid) = parse_vmid(&f.vmid) else {
        return flash("/cluster", "error", (tx("Geçersiz konuk kimliği.")));
    };
    match st.import_from_peer(f.peer.trim(), vmid, &me.user) {
        Ok(()) => flash("/cluster", "notice", &tfmt(format!("{} sunucusundaki {vmid} buraya kopyalanıyor; sonuç görevlerde görünecek.", f.peer.trim()))),
        Err(e) => flash("/cluster", "error", &(capitalize(&e.to_string()) + ".")),
    }
}

/// Küme sayfasındaki "Canlı Taşı" düğmesi: eşteki çalışan konuk kapatılmadan buraya geçer.
pub async fn peer_migrate(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<ImportForm>) -> Redirect {
    let Some(vmid) = parse_vmid(&f.vmid) else {
        return flash("/cluster", "error", (tx("Geçersiz konuk kimliği.")));
    };
    let (st2, peer, user) = (st.clone(), f.peer.trim().to_string(), me.user.clone());
    match tokio::task::spawn_blocking(move || st2.migrate_from_peer(&peer, vmid, &user)).await {
        Ok(Ok(())) => flash("/cluster", "notice", &tfmt(format!("{} sunucusundaki {vmid} kapatılmadan buraya taşınıyor; sonuç görevlerde görünecek.", f.peer.trim()))),
        Ok(Err(e)) => flash("/cluster", "error", &(capitalize(&e.to_string()) + ".")),
        Err(_) => flash("/cluster", "error", (tx("İşlem başlatılamadı."))),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReplicaForm {
    peer: String,
    vmid: String,
    every_min: String,
    failover_min: String,
    /// `add`, `run`, `failover` ya da `remove`.
    op: String,
}

/// Birleşik görünümdeki çoğaltma formları.
pub async fn replica_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<ReplicaForm>) -> Redirect {
    let Some(vmid) = parse_vmid(&f.vmid) else {
        return flash("/cluster", "error", (tx("Geçersiz konuk kimliği.")));
    };
    let peer = f.peer.trim().to_string();
    let result = match f.op.as_str() {
        "run" => {
            let (st, peer) = (st.clone(), peer.clone());
            tokio::task::spawn_blocking(move || st.replica_run(&peer, vmid)).await.unwrap_or_else(|e| Err(Error::Driver(e.to_string()))).map(|()| (tx("Çoğaltma başladı; sonuç görevlerde görünecek.")).to_string())
        }
        "save" => match (f.every_min.trim().parse::<u32>(), f.failover_min.trim()) {
            (Err(_), _) => return flash("/cluster", "error", (tx("Aralık dakika olarak bir tam sayı olmalı."))),
            (Ok(minutes), failover) => {
                let failover = match failover {
                    "" | "0" => Ok(None),
                    text => text.parse::<u32>().map(Some),
                };
                match failover {
                    Err(_) => return flash("/cluster", "error", (tx("Devralma süresi dakika olarak bir tam sayı olmalı (0 kapatır)."))),
                    Ok(failover) => st
                        .replica_put(&peer, vmid, minutes, &me.user)
                        .and_then(|()| if st.snapshot().replicas.iter().any(|r| r.peer == peer && r.source == vmid && r.failover_min == failover) { Ok(()) } else { st.replica_failover_set(&peer, vmid, failover, &me.user) })
                        .map(|()| (tx("Çoğaltma ayarları kaydedildi.")).to_string()),
                }
            }
        },
        "failover" => match f.failover_min.trim() {
            "" | "0" => st.replica_failover_set(&peer, vmid, None, &me.user).map(|()| (tx("Kendiliğinden devralma kapatıldı.")).to_string()),
            text => match text.parse::<u32>() {
                Ok(minutes) => st.replica_failover_set(&peer, vmid, Some(minutes), &me.user).map(|()| tfmt(format!("{peer} sunucusuna {minutes} dakika ulaşılamazsa ve bir hakem onaylarsa yedek kopya kendiliğinden başlatılacak."))),
                Err(_) => return flash("/cluster", "error", (tx("Devralma süresi dakika olarak bir tam sayı olmalı (0 kapatır)."))),
            },
        },
        "remove" => st.replica_remove(&peer, vmid, &me.user).map(|()| (tx("Çoğaltma kaldırıldı; yedek kopya olağan bir konuk olarak duruyor.")).to_string()),
        _ => match f.every_min.trim().parse::<u32>() {
            Ok(minutes) => st.replica_put(&peer, vmid, minutes, &me.user).map(|()| tfmt(format!("{peer} sunucusundaki {vmid}, {minutes} dakikada bir buraya çoğaltılacak."))),
            Err(_) => return flash("/cluster", "error", (tx("Aralık dakika olarak bir tam sayı olmalı."))),
        },
    };
    match result {
        Ok(text) => flash("/cluster", "notice", &text),
        Err(e) => flash("/cluster", "error", &(capitalize(&e.to_string()) + ".")),
    }
}

pub async fn help(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let ctx = Ctx::new(&st, &headers, query, &me);
    Html(view::help(&ctx.page(Current::Other, "/help")))
}

pub async fn not_found(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap) -> Response {
    Ctx::new(&st, &headers, PageQuery::default(), &me).not_found((tx("Bu adreste bir sayfa yok.")))
}

pub(crate) fn flash(path: &str, key: &str, message: &str) -> Redirect {
    let sep = if path.contains('?') { '&' } else { '?' };
    Redirect::to(&format!("{path}{sep}{key}={}", enc(message)))
}

#[derive(Debug, Deserialize)]
pub struct ActionForm {
    action: String,
}

pub async fn action(State(st): State<AppState>,
    Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(form): Form<ActionForm>) -> Redirect {
    let Some(vmid) = parse_vmid(&raw) else {
        return flash("/", "error", &tfmt(format!("Geçersiz konuk kimliği: {raw}")));
    };
    let path = format!("/guests/{vmid}");
    let Ok(action) = form.action.parse::<Action>() else {
        return flash(&path, "error", &format!("Bilinmeyen eylem: {}", form.action));
    };
    act(&st, vmid, action, &me.user)
}

fn act(st: &AppState, vmid: VmId, action: Action, user: &str) -> Redirect {
    let path = format!("/guests/{vmid}");
    match st.run_action(vmid, action, user) {
        Ok(_) => flash(&path, "notice", &tfmt(format!("{} isteği alındı: {vmid}", action.label()))),
        Err(Error::GuestNotFound(_)) => flash("/", "error", &Error::GuestNotFound(vmid).to_string()),
        Err(e) => flash(&path, "error", &capitalize(&e.to_string())),
    }
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub async fn new_form(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Query(query): Query<PageQuery>) -> Html<String> {
    let kind = query.kind.parse().unwrap_or(GuestKind::Vm);
    let ctx = Ctx::new(&st, &headers, query, &me);
    let mut form = NewGuestForm::defaults(kind, &ctx.cluster);
    if st.driver().is_real() {
        form.bridge = redox_core::NAT.to_string();
        if let (GuestKind::Ct, Some(first)) = (kind, st.driver().templates().first()) {
            form.os = first.clone();
        }
    }
    let back = format!("/guests/new?kind={kind}");
    Html(view::new_guest(&ctx.page(Current::Other, back), &form, &Vec::new(), &form_options(&st)))
}

/// Oluşturma ve düzenleme formlarının sürücüden gelen seçenekleri.
pub(crate) fn form_options(st: &AppState) -> view::FormOptions {
    let d = st.driver();
    view::FormOptions { isos: d.isos(), bridges: d.bridges(), templates: d.templates(), images: d.images(), vnc: false }
}

fn number<T: std::str::FromStr>(raw: &str, field: &'static str, label: &str, errors: &mut FieldErrors) -> Option<T> {
    match raw.trim().parse() {
        Ok(v) => Some(v),
        Err(_) => {
            errors.push((field, tfmt(format!("{label} bir tam sayı olmalı."))));
            None
        }
    }
}

/// Boş bırakılan form alanı `None` olur.
fn filled(raw: &str) -> Option<String> {
    Some(raw.trim()).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Çekirdek hatasını ilgili form alanına bağlar.
fn field_of(e: &Error) -> &'static str {
    match e {
        Error::VmidTaken(_) | Error::InvalidVmid(_) => "vmid",
        Error::InvalidName(_) => "name",
        Error::NodeNotFound(_) | Error::NodeOffline(_) => "node",
        Error::StorageNotFound(_) | Error::StorageNotOnNode { .. } | Error::InsufficientStorage { .. } => "storage",
        _ => "",
    }
}

pub async fn create(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Form(form): Form<NewGuestForm>) -> Response {
    let mut errors: FieldErrors = Vec::new();
    let vmid = match form.vmid.trim() {
        "" => None,
        raw => number::<VmId>(raw, "vmid", (tx("Kimlik")), &mut errors),
    };
    let cores = number::<u32>(&form.cores, "cores", (tx("Çekirdek")), &mut errors);
    let memory_mb = number::<u64>(&form.memory_mb, "memory_mb", (tx("Bellek")), &mut errors);
    let disk_gb = number::<u64>(&form.disk_gb, "disk_gb", "Disk boyutu", &mut errors);

    // Seçim kutusu gönderilmediyse (konteyner formu) BIOS varsayılır.
    let (firmware, secure_boot) = redox_core::hw::parse_firmware_choice(form.firmware.trim()).unwrap_or_default();
    if let (true, Some(cores), Some(memory_mb), Some(disk_gb)) = (errors.is_empty(), cores, memory_mb, disk_gb) {
        let spec = GuestSpec {
            vmid,
            name: form.name.trim().to_string(),
            kind: form.kind(),
            node: form.node.clone(),
            cores,
            memory_mb,
            disk_gb,
            storage: form.storage.clone(),
            os: form.os.trim().to_string(),
            bridge: form.bridge.trim().to_string(),
            onboot: form.onboot.is_some(),
            iso: Some(form.iso.trim()).filter(|s| !s.is_empty()).map(str::to_string),
            hw: redox_core::Hardware {
                image: filled(&form.image),
                ci_user: filled(&form.ci_user),
                // Yapıştırılan anahtarın sonundaki satır sonu atılır; içindeki satır sonu doğrulamada reddedilir.
                ci_ssh_key: filled(&form.ci_ssh_key),
                firmware,
                secure_boot,
                tpm: form.tpm.is_some(),
                ..Default::default()
            },
        };
        match st.create_guest(&spec, &me.user) {
            Ok(id) => {
                return flash(&format!("/guests/{id}"), "notice", &tfmt(format!("{} {id} oluşturuldu.", spec.kind.long_label())))
                    .into_response()
            }
            Err(e) => {
                let status = status_for(e.kind());
                errors.push((field_of(&e), capitalize(&e.to_string()) + "."));
                return render_form(&st, &headers, &me, &form, &errors, status);
            }
        }
    }
    render_form(&st, &headers, &me, &form, &errors, StatusCode::UNPROCESSABLE_ENTITY)
}

fn render_form(st: &AppState, headers: &HeaderMap, me: &Identity, form: &NewGuestForm, errors: &FieldErrors, status: StatusCode) -> Response {
    let ctx = Ctx::new(st, headers, PageQuery::default(), me);
    let back = format!("/guests/new?kind={}", form.kind());
    (status, Html(view::new_guest(&ctx.page(Current::Other, back), form, errors, &form_options(st)))).into_response()
}

pub async fn delete_confirm(State(st): State<AppState>,
    Extension(me): Extension<Identity>, headers: HeaderMap, Path(raw): Path<String>) -> Response {
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Some(g) = parse_vmid(&raw).and_then(|id| ctx.cluster.guest(id).ok()) else {
        return ctx.not_found(&tfmt(format!("“{raw}” kimlikli bir konuk yok.")));
    };
    let back = format!("/guests/{}/delete", g.vmid);
    Html(view::delete_confirm(&ctx.page(Current::Guest(g.vmid), back), g)).into_response()
}

pub async fn delete(State(st): State<AppState>,
    Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let Some(vmid) = parse_vmid(&raw) else {
        return flash("/", "error", &tfmt(format!("Geçersiz konuk kimliği: {raw}")));
    };
    match st.request_guest_delete(vmid, &me.user) {
        Ok(Some(id)) => flash(&format!("/guests/{vmid}"), "notice", &tfmt(format!("Silinmedi: ikinci bir yöneticinin onayı bekleniyor (istek #{id})."))),
        Ok(None) => flash("/", "notice", &format!("{vmid} silindi.")),
        Err(Error::GuestNotFound(_)) => flash("/", "error", &Error::GuestNotFound(vmid).to_string()),
        Err(e) => flash(&format!("/guests/{vmid}"), "error", &capitalize(&e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct CommandForm {
    #[serde(default)]
    q: String,
}

/// Komut satırı: gezinme komutları yönlendirir, eylem komutları görev başlatır.
pub async fn command(State(st): State<AppState>,
    Extension(me): Extension<Identity>, Form(form): Form<CommandForm>) -> Redirect {
    let resolved = command::parse(&form.q).and_then(|c| command::resolve(&c, &st.snapshot()));
    match resolved {
        Ok(Resolved::Home) => Redirect::to("/"),
        Ok(Resolved::Storage) => Redirect::to("/storage"),
        Ok(Resolved::Help) => Redirect::to("/help"),
        Ok(Resolved::Node(id)) => Redirect::to(&format!("/nodes/{id}")),
        Ok(Resolved::Guest(vmid)) => Redirect::to(&format!("/guests/{vmid}")),
        Ok(Resolved::Search(text)) => Redirect::to(&with_q("/guests", &text)),
        Ok(Resolved::Act { vmid, action }) => act(&st, vmid, action, &me.user),
        Err(e) => flash("/", "error", &capitalize(&e.to_string())),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ThemeQuery {
    to: String,
    back: String,
}

/// Yalnızca site içi yollara izin verir (açık yönlendirme olmasın).
fn safe_back(back: &str) -> &str {
    if back.starts_with('/') && !back.starts_with("//") && !back.contains('\\') {
        back
    } else {
        "/"
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LoginQuery {
    next: String,
    /// Tek oturum açma dönüşünde gösterilecek hata.
    error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LoginForm {
    username: String,
    password: String,
    code: String,
    next: String,
}

pub async fn login_form(State(st): State<AppState>, headers: HeaderMap, Query(query): Query<LoginQuery>) -> Response {
    if crate::identify(&st, &headers).is_some() {
        return Redirect::to(safe_back(&query.next)).into_response();
    }
    Html(view::login(theme_of(&headers), safe_back(&query.next), "", query.error.as_deref(), st.oidc().is_some())).into_response()
}

pub async fn login(
    State(st): State<AppState>,
    headers: HeaderMap,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Form(form): Form<LoginForm>,
) -> Response {
    let name = form.username.trim();
    let next = safe_back(&form.next);
    let audit = |action: &str, ok: bool| st.auth().audit.record(now(), name, &ip, action, ok);
    match st.auth().login_with_code(&ip, name, &form.password, &form.code, now()) {
        Ok(token) => {
            println!("giriş: {name} ({ip})");
            audit("giriş", true);
            (
                StatusCode::SEE_OTHER,
                [(SET_COOKIE, st.auth().session_cookie(&token)), (LOCATION, next.to_string())],
            )
                .into_response()
        }
        Err(LoginError::Invalid) => {
            println!("başarısız giriş denemesi: {name:?} ({ip})");
            audit("giriş (hatalı parola ya da kod)", false);
            let page = view::login(theme_of(&headers), next, name, Some((tx("Kullanıcı adı ya da parola hatalı."))), st.oidc().is_some());
            (StatusCode::UNAUTHORIZED, Html(page)).into_response()
        }
        Err(LoginError::Locked(secs)) => {
            println!("giriş kilitli: {name:?} ({ip}), {secs} sn");
            audit("giriş (kilitli)", false);
            let message = tfmt(format!("Çok fazla hatalı deneme. {} dakika sonra yeniden deneyin.", secs.div_ceil(60)));
            (StatusCode::TOO_MANY_REQUESTS, Html(view::login(theme_of(&headers), next, name, Some(&message), st.oidc().is_some()))).into_response()
        }
    }
}

fn oidc_cookie(st: &AppState, value: &str, max_age: u64) -> String {
    // Lax: kimlik sağlayıcıdan dönen üst düzey gezinmede gönderilir (Strict gönderilmezdi).
    format!("{}={value}; Path=/login/oidc; HttpOnly; SameSite=Lax; Max-Age={max_age}{}", crate::oidc::STATE_COOKIE, if st.auth().behind_tls_proxy { "; Secure" } else { "" })
}

fn login_error(message: &str) -> Response {
    Redirect::to(&format!("/login?error={}", enc(message))).into_response()
}

/// Tek oturum açmayı başlatır: tarayıcıyı kimlik sağlayıcıya yollar.
pub async fn oidc_start(State(st): State<AppState>) -> Response {
    let Some(oidc) = st.oidc() else { return StatusCode::NOT_FOUND.into_response() };
    match tokio::task::spawn_blocking(move || oidc.begin(now())).await {
        Ok(Ok((location, state))) => (StatusCode::SEE_OTHER, [(SET_COOKIE, oidc_cookie(&st, &state, 600)), (LOCATION, location)]).into_response(),
        Ok(Err(e)) => login_error(&(capitalize(&e) + ".")),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct OidcCallback {
    code: String,
    state: String,
    error: String,
}

/// Kimlik sağlayıcıdan dönüş: kimliği yerel kullanıcıyla eşleştirir ve oturum açar.
pub async fn oidc_callback(State(st): State<AppState>, headers: HeaderMap, Extension(ClientIp(ip)): Extension<ClientIp>, Query(q): Query<OidcCallback>) -> Response {
    let Some(oidc) = st.oidc() else { return StatusCode::NOT_FOUND.into_response() };
    if !q.error.is_empty() || q.code.is_empty() {
        return login_error((tx("Kimlik sağlayıcı girişi tamamlamadı.")));
    }
    let cookie_state = cookie(&headers, crate::oidc::STATE_COOKIE).map(str::to_string);
    let finished = tokio::task::spawn_blocking(move || oidc.finish(&q.code, &q.state, cookie_state.as_deref(), now())).await;
    let subject = match finished {
        Ok(Ok(subject)) => subject,
        Ok(Err(e)) => return login_error(&(capitalize(&e) + ".")),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Some((token, name)) = st.auth().login_sso(&subject, now()) else {
        st.auth().audit.record(now(), &subject, &ip, "giriş (tek oturum açma, eşleşen kullanıcı yok)", false);
        return login_error((tx("Bu kimlik hiçbir RedoX VE kullanıcısıyla eşleşmiyor. Yöneticiniz redox user sso ile eşlemelidir.")));
    };
    println!("giriş (tek oturum açma): {name} ({ip})");
    st.auth().audit.record(now(), &name, &ip, "giriş (tek oturum açma)", true);
    // Yönlendirme yerine sayfa: dış siteden başlayan yönlendirme zincirinde tarayıcı Strict çerezi göndermez;
    // bu sayfanın kendi yenilemesi aynı siteden yeni bir gezinmedir.
    let mut response = Html(view::signed_in(theme_of(&headers))).into_response();
    for value in [st.auth().session_cookie(&token), oidc_cookie(&st, "", 0)] {
        if let Ok(value) = value.parse() {
            response.headers_mut().append(SET_COOKIE, value);
        }
    }
    response
}

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        st.auth().logout(token);
    }
    (StatusCode::SEE_OTHER, [(SET_COOKIE, st.auth().clear_cookie()), (LOCATION, "/login".to_string())]).into_response()
}

/// Arayüz dilini değiştirir: çerezi yazar ve gelinen sayfaya döner.
pub async fn lang(Query(query): Query<ThemeQuery>) -> Response {
    let lang = crate::i18n::Lang::parse(&query.to).unwrap_or_default();
    (
        StatusCode::SEE_OTHER,
        [
            (SET_COOKIE, format!("lang={}; Path=/; Max-Age=31536000; SameSite=Lax", lang.code())),
            (LOCATION, safe_back(&query.back).to_string()),
        ],
    )
        .into_response()
}

pub async fn theme(Query(query): Query<ThemeQuery>) -> Response {
    let theme = if query.to == "light" { "light" } else { "dark" };
    (
        StatusCode::SEE_OTHER,
        [
            (SET_COOKIE, format!("theme={theme}; Path=/; Max-Age=31536000; SameSite=Lax")),
            (LOCATION, safe_back(&query.back).to_string()),
        ],
    )
        .into_response()
}

pub async fn asset(Path(file): Path<String>) -> Response {
    let (mime, body): (&str, &str) = match file.as_str() {
        "style.css" => ("text/css; charset=utf-8", view::STYLE_CSS),
        "logo.svg" => ("image/svg+xml", view::LOGO_SVG),
        "wordmark.svg" => ("image/svg+xml", view::WORDMARK_SVG),
        "icons.svg" => ("image/svg+xml", view::ICONS_SVG),
        "console.js" => ("text/javascript; charset=utf-8", view::CONSOLE_JS),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(CONTENT_TYPE, mime), (CACHE_CONTROL, "public, max-age=3600")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_links_stay_on_site() {
        assert_eq!(safe_back("/guests/102?tab=gorevler"), "/guests/102?tab=gorevler");
        for evil in ["//evil.example", "https://evil.example", "/\\evil.example", "", "javascript:alert(1)"] {
            assert_eq!(safe_back(evil), "/", "{evil}");
        }
    }

    #[test]
    fn theme_cookie_parsing() {
        let mut h = HeaderMap::new();
        assert_eq!(theme_of(&h), "dark");
        h.insert(COOKIE, "a=1; theme=light; b=2".parse().unwrap());
        assert_eq!(theme_of(&h), "light");
        h.insert(COOKIE, "theme=lightning".parse().unwrap());
        assert_eq!(theme_of(&h), "dark");
    }

    #[test]
    fn capitalize_handles_turkish_and_empty() {
        assert_eq!(capitalize((tx("çekirdek"))), (tx("Çekirdek")));
        assert_eq!(capitalize(""), "");
    }
}
