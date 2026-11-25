//! Askı, şablon, klon, anlık görüntü, onay sayfası ve toplu işlemler.

use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NameForm {
    name: String,
    /// Klon formunda: şablondan bağlı klon.
    linked: Option<String>,
    /// Şablon formunda: `1` şablona çevirir, `0` çıkarır.
    on: Option<String>,
    /// Anlık görüntü formunda: belleksiz (üretim) denetim noktası.
    production: Option<String>,
}

pub async fn suspend(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}");
    match st.suspend(vmid, &me.user) {
        Ok(_) => flash(&back, "notice", "Konuk askıya alınıyor: bellek durumu diske yazılıp konuk kapatılacak."),
        Err(e) => failed(&back, &e),
    }
}

pub async fn discard_suspend(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}");
    match st.discard_suspend(vmid, &me.user) {
        Ok(()) => flash(&back, "notice", "Askı durumu atıldı; konuk bir sonraki başlatmada sıfırdan açılır."),
        Err(e) => failed(&back, &e),
    }
}

pub async fn template(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<NameForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let on = f.on.as_deref() == Some("1");
    let back = format!("/guests/{vmid}?tab=cogalt");
    match st.set_template(vmid, on, &me.user) {
        Ok(()) => flash(&back, "notice", if on { "Konuk şablona çevrildi." } else { "Konuk şablonluktan çıkarıldı." }),
        Err(e) => failed(&back, &e),
    }
}

pub async fn clone(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<NameForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match st.clone_guest_as(vmid, f.name.trim(), f.linked.is_some(), &me.user) {
        Ok(new_id) => flash(&format!("/guests/{new_id}"), "notice", &format!("{vmid} klonlanıyor: yeni konuk {new_id}.")),
        Err(e) => failed(&format!("/guests/{vmid}?tab=cogalt"), &e),
    }
}

pub async fn snapshot_create(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>, Form(f): Form<NameForm>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=goruntuler");
    let started = if f.production.is_some() { st.snapshot_create_production(vmid, f.name.trim(), &me.user) } else { st.snapshot_create(vmid, f.name.trim(), &me.user) };
    match started {
        Ok(_) => flash(&back, "notice", &format!("Anlık görüntü alınıyor: {}", f.name.trim())),
        Err(e) => failed(&back, &e),
    }
}

pub async fn snapshot_action(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Path((raw, name, op)): Path<(String, String, String)>,
) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=goruntuler");
    let result = match op.as_str() {
        "rollback" => st.snapshot_rollback(vmid, &name, &me.user).map(|_| format!("{name} anlık görüntüsüne dönülüyor.")),
        "delete" => st.snapshot_delete(vmid, &name, &me.user).map(|_| format!("{name} siliniyor.")),
        _ => return flash(&back, "error", &format!("Bilinmeyen işlem: {op}")),
    };
    match result {
        Ok(notice) => flash(&back, "notice", &notice),
        Err(e) => failed(&back, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfirmQuery {
    op: String,
    name: String,
}

/// Geri alınamayan işlemlerden önce gösterilen onay sayfası (GET hiçbir şeyi değiştirmez).
pub async fn confirm(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(q): Query<ConfirmQuery>,
) -> Response {
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    let Some(g) = raw.parse().ok().and_then(|id| ctx.cluster.guest(id).ok()) else {
        return ctx.not_found(&format!("“{raw}” kimlikli bir konuk yok."));
    };
    let base = format!("/guests/{}", g.vmid);
    let name = q.name.as_str();
    let has_snapshot = g.snapshots.iter().any(|s| s.name == name);
    let has_backup = ctx.cluster.backups_of(g.vmid).any(|b| b.id == name);
    let c = match q.op.as_str() {
        "rollback" if has_snapshot => Confirm {
            title: "Anlık görüntüye dönülsün mü?",
            message: html! { b { (g.vmid) " (" (g.name) ")" } " diski " b { (name) } " anındaki haline dönecek. O andan sonra diske yazılan her şey kaybolur." },
            action: format!("{base}/snapshots/{name}/rollback"),
            button: "Geri Dön",
            back: format!("{base}?tab=goruntuler"),
            cli: format!("{} rollback {} {name}", g.kind, g.vmid),
        },
        "delsnapshot" if has_snapshot => Confirm {
            title: "Anlık görüntü silinsin mi?",
            message: html! { b { (name) } " anlık görüntüsü silinecek. Diskin şimdiki hali etkilenmez, ama o ana bir daha dönemezsiniz." },
            action: format!("{base}/snapshots/{name}/delete"),
            button: "Anlık Görüntüyü Sil",
            back: format!("{base}?tab=goruntuler"),
            cli: format!("{} delsnapshot {} {name}", g.kind, g.vmid),
        },
        "restore" if has_backup => Confirm {
            title: "Yedek geri yüklensin mi?",
            message: html! { b { (g.vmid) " (" (g.name) ")" } " diskinin yerine " b { (name) } " yedeği konacak. Diskin şimdiki içeriği ve bütün anlık görüntüler kaybolur."
                @if let Ok(disks) = st.snapshot().restore_disks(name) {
                    @if !disks.is_empty() { " Yedekteki ek diskler de (" (disks.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ")) ") geri yüklenir; şimdiki içerikleri kaybolur." }
                }
            },
            action: format!("/backups/{name}/restore"),
            button: "Geri Yükle",
            back: format!("{base}?tab=yedekler"),
            cli: format!("{} restore {} {name}", g.kind, g.vmid),
        },
        "delbackup" if has_backup => Confirm {
            title: "Yedek silinsin mi?",
            message: html! { b { (name) } " yedeği kalıcı olarak silinecek. Bu işlem geri alınamaz." },
            action: format!("/backups/{name}/delete"),
            button: "Yedeği Sil",
            back: format!("{base}?tab=yedekler"),
            cli: format!("backup delete {name}"),
        },
        "deldisk" if g.hw.disks.iter().any(|d| d.id.to_string() == name) => Confirm {
            title: "Ek disk silinsin mi?",
            message: html! { b { (g.vmid) " (" (g.name) ")" } " konuğunun " b { (name) } " numaralı ek diski ve içindeki bütün veriler kalıcı olarak silinecek. Bu işlem geri alınamaz." },
            action: format!("{base}/disks/{name}/delete"),
            button: "Diski Sil",
            back: format!("{base}?tab=donanim"),
            cli: format!("vm disk remove {} {name}", g.vmid),
        },
        _ => return ctx.not_found("Onaylanacak işlem bulunamadı; anlık görüntü ya da yedek silinmiş olabilir."),
    };
    let page = ctx.page(Current::Guest(g.vmid), format!("{base}/confirm"));
    Html(view_ops::confirm_page(&page, g, &c)).into_response()
}

/// Seçili konuklara aynı eylemi uygular. Form aynı adlı birden çok `id` alanı taşır.
pub async fn bulk(State(st): State<AppState>, Extension(me): Extension<Identity>, RawForm(body): RawForm) -> Redirect {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("");
    let back = if value("back").starts_with("/nodes/") { value("back").to_string() } else { "/guests".to_string() };
    let Ok(action) = value("action").parse::<Action>() else {
        return flash(&back, "error", "Toplu işlem için bir eylem seçin.");
    };
    let ids: Vec<VmId> = pairs.iter().filter(|(k, _)| k == "id").filter_map(|(_, v)| v.parse().ok()).collect();
    if ids.is_empty() {
        return flash(&back, "error", "Önce en az bir konuk seçin.");
    }
    let (mut done, mut skipped) = (0, Vec::new());
    for id in &ids {
        match st.run_action(*id, action, &me.user) {
            Ok(_) => done += 1,
            Err(_) => skipped.push(id.to_string()),
        }
    }
    let mut message = format!("{} isteği {done} konuk için alındı.", action.label());
    if !skipped.is_empty() {
        message.push_str(&format!(" Uygun durumda olmadığı için atlananlar: {}.", skipped.join(", ")));
    }
    flash(&back, if done > 0 { "notice" } else { "error" }, &message)
}
