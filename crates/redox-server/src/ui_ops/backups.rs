//! Yedek işlemleri: alma, geri yükleme ve diğerleri, yedek işleri, dosya tarayıcısı ve indirme.

use super::*;

/// Anında kurtarılmış konuğu yedekten ayırır.
pub async fn guest_detach(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=cogalt");
    match st.guest_detach(vmid, &me.user) {
        Ok(_) => flash(&back, "notice", (tx("Diskler yedekten ayrılıyor; bitince yedek serbest kalır. Sonuç görevlerde görünür."))),
        Err(e) => failed(&back, &e),
    }
}

/// Yedekler sayfasındaki iş formu: ekler ya da aynı adlı işi günceller.
pub async fn job_save(State(st): State<AppState>, RawForm(body): RawForm) -> Redirect {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.trim()).unwrap_or("");
    let days: String = pairs.iter().filter(|(k, _)| k == "day").filter_map(|(_, v)| v.parse::<u8>().ok()).filter(|d| (1..=7).contains(d)).map(|d| d.to_string()).collect();
    if days.is_empty() {
        return flash("/backups", "error", (tx("En az bir gün seçin.")));
    }
    let keep = match value("keep") {
        "" => None,
        text => match text.parse::<u32>() {
            Ok(n) => Some(n),
            Err(_) => return flash("/backups", "error", (tx("Saklanacak yedek sayısı bir sayı olmalı."))),
        },
    };
    let count = |key: &str, label: &str| -> Result<u32, String> {
        match value(key) {
            "" => Ok(0),
            text => text.parse().map_err(|_| tfmt(format!("{label} bir sayı olmalı."))),
        }
    };
    let extras = (|| Ok::<_, String>((redox_core::Gfs { weekly: count("weekly", (tx("Haftalık yedek sayısı")))?, monthly: count("monthly", (tx("Aylık yedek sayısı")))?, yearly: count("yearly", (tx("Yıllık yedek sayısı")))? }, count("lock", (tx("Kilit süresi")))?)))();
    let (gfs, lock) = match extras {
        Ok(extras) => extras,
        Err(message) => return flash("/backups", "error", &message),
    };
    let job = match redox_core::BackupJob::from_text(value("name"), value("time"), &days, value("tags"), keep).and_then(|job| job.with_retention(gfs, lock)).and_then(|job| job.with_window(value("until"), !value("verify").is_empty())) {
        Ok(job) => job,
        Err(e) => return failed("/backups", &e),
    };
    let name = job.name.clone();
    match st.job_put(job) {
        Ok(()) => flash("/backups", "notice", &tfmt(format!("{name} işi kaydedildi."))),
        Err(e) => failed("/backups", &e),
    }
}

/// `run`: işi şimdi başlatır; `delete`: siler (aldığı yedekler kalır).
pub async fn job_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((name, op)): Path<(String, String)>) -> Redirect {
    match op.as_str() {
        "run" => match st.job_run(&name) {
            Ok(0) => flash("/backups", "error", &tfmt(format!("{name} işinin kapsamında konuk yok."))),
            Ok(n) => flash("/backups", "notice", &tfmt(format!("{name} işi başlatıldı: {n} konuk sırayla yedeklenecek; sonuç görevlerde görünür."))),
            Err(e) => failed("/backups", &e),
        },
        "delete" => match st.request_job_remove(&name, &me.user) {
            Ok(Some(n)) => flash("/backups", "notice", &tfmt(format!("{name} işi silinmedi: ikinci bir yöneticinin onayı bekleniyor (istek #{n})."))),
            Ok(None) => flash("/backups", "notice", &tfmt(format!("{name} işi silindi; aldığı yedekler duruyor."))),
            Err(e) => failed("/backups", &e),
        },
        _ => flash("/backups", "error", (tx("Bilinmeyen işlem."))),
    }
}

pub async fn backup_create(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Redirect {
    let vmid = match vmid_of(&raw) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let back = format!("/guests/{vmid}?tab=yedekler");
    match st.backup_create(vmid, &me.user) {
        Ok(id) => flash(&back, "notice", &tfmt(format!("Yedek alınıyor: {id}"))),
        Err(e) => failed(&back, &e),
    }
}

pub async fn backup_action(State(st): State<AppState>, Extension(me): Extension<Identity>, Path((id, op)): Path<(String, String)>) -> Redirect {
    // Yedekten yeni konuk: özgün konuk silinmiş de olabilir; başarılıysa yeni konuğun sayfasına gidilir.
    if op == "restore-new" || op == "instant" {
        let name = match st.snapshot().backup(&id) {
            Ok(b) => redox_core::restored_name(&b.guest_name),
            Err(e) => return flash("/backups", "error", &capitalize(&e.to_string())),
        };
        let made = if op == "instant" { st.backup_instant(&id, &name, &me.user) } else { st.backup_restore_new(&id, &name, &me.user) };
        return match made {
            Ok(vmid) if op == "instant" => flash(&format!("/guests/{vmid}"), "notice", &tfmt(format!("{name}, {id} yedeğinin üzerinde açılmaya hazır. Kalıcı olması için Çoğalt ve Taşı sekmesinden yedekten ayırın."))),
            Ok(vmid) => flash(&format!("/guests/{vmid}"), "notice", &tfmt(format!("{id} yedeğinden {name} oluşturuluyor; sonuç görevlerde görünür."))),
            Err(e) => failed("/backups", &e),
        };
    }
    // Arşivdeki yedek henüz listede değildir: sahibi aranmadan geri getirilir.
    if op == "recover" {
        return match st.backup_recover(&id, &me.user) {
            Ok(()) => flash("/backups", "notice", &tfmt(format!("{id} arşivden geri getirildi; geri yüklenirken kopya hedefinden iner."))),
            Err(e) => failed("/backups", &e),
        };
    }
    let owner = st.snapshot().backup(&id).map(|b| b.vmid);
    let back = match owner {
        Ok(vmid) => format!("/guests/{vmid}?tab=yedekler"),
        Err(e) => return flash("/", "error", &capitalize(&e.to_string())),
    };
    let result = match op.as_str() {
        "restore" => st.backup_restore(&id, &me.user).map(|_| tfmt(format!("{id} geri yükleniyor."))),
        "delete" => st.request_backup_delete(&id, &me.user).map(|queued| match queued {
            Some(n) => tfmt(format!("{id} silinmedi: ikinci bir yöneticinin onayı bekleniyor (istek #{n}).")),
            None => format!("{id} silindi."),
        }),
        "push" => st.backup_push(&id, &me.user).map(|()| tfmt(format!("{id} dış hedefe gönderiliyor."))),
        "verify" => st.backup_verify(&id, &me.user).map(|()| tfmt(format!("{id} doğrulanıyor; sonuç görev günlüğüne yazılır."))),
        "test" => st.backup_test(&id, &me.user).map(|()| tfmt(format!("{id} geçici bir konuk olarak açılıp sınanıyor; sonuç görev günlüğüne ve yedeğin satırına yazılır."))),
        "ack" => st.backup_ack(&id, &me.user).map(|()| tfmt(format!("{id}: olağan dışı değişim uyarısı kapatıldı."))),
        "lock" => st.backup_lock(&id, redox_core::retention::DEFAULT_LOCK_DAYS, &me.user).map(|until| tfmt(format!("{id}, {} tarihine kadar silinemez.", redox_core::retention::date_label(until)))),
        _ => return flash(&back, "error", &tfmt(format!("Bilinmeyen işlem: {op}"))),
    };
    match result {
        Ok(notice) => flash(&back, "notice", &notice),
        Err(e) => failed(&back, &e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FilesQuery {
    /// 0: asıl disk; diğerleri yedekteki ek disk numarası.
    pub disk: u8,
    path: String,
    /// Dizin, tar arşivi olarak indirilsin mi?
    tar: Option<String>,
    /// Çalışan konuğa geri yüklenmek üzere seçilen yol ve bunun bir klasör olup olmadığı.
    pick: Option<String>,
    dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PutForm {
    disk: u8,
    path: String,
    tar: Option<String>,
    target: String,
}

/// Yedekteki dosyalar sayfasındaki "çalışan konuğa geri yükle" formu.
pub async fn backup_put(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(id): Path<String>,
    Form(f): Form<PutForm>,
) -> Redirect {
    let folder = if crate::guestfs::valid_path(&f.path) { crate::guestfs::parent(&crate::guestfs::normalize(&f.path)) } else { "/".to_string() };
    let back = format!("/backups/{}/files?disk={}&path={}", crate::view::enc(&id), f.disk, crate::view::enc(&folder));
    st.auth().audit.record(now(), &me.user, &ip, &format!("backup put {id} {} {}", f.path, f.target), true);
    match st.backup_put(&id, f.disk, &f.path, f.tar.is_some(), &f.target, &me.user) {
        Ok(vmid) => flash(&back, "notice", &tfmt(format!("Geri yükleme başladı: konuk {vmid}. Sonuç Görevler sayfasına yazılır."))),
        Err(e) => failed(&back, &e),
    }
}

impl FilesQuery {
    /// Verilmemiş yol köktür.
    pub fn path(&self) -> String {
        if self.path.is_empty() { "/".to_string() } else { self.path.clone() }
    }
}

/// Yedeğin içindeki dizini listeler. Okuma birkaç saniye sürer (ve artımlı yedekte önce imaj dökülür): istek iş
/// parçacığını tutmaz. Her erişim denetim günlüğüne yazılır: yedeğin içeriği konuğun bütün dosyalarıdır.
pub async fn list_backup_files(st: &AppState, user: &str, ip: &str, id: &str, disk: u8, path: &str) -> Result<Vec<crate::guestfs::Entry>, Error> {
    if !crate::guestfs::valid_path(path) {
        return Err(Error::InvalidSpec(tfmt(format!("geçersiz yol: {path}"))));
    }
    st.auth().audit.record(now(), user, ip, &format!("backup ls {id} {}", crate::guestfs::normalize(path)), true);
    let (state, id, path) = (st.clone(), id.to_string(), path.to_string());
    tokio::task::spawn_blocking(move || {
        let (bin, image, temporary) = state.backup_image(&id, disk)?;
        let listed = crate::guestfs::list(&bin, &image, &path).map_err(Error::Driver);
        if temporary {
            crate::state::discard_export(&image);
        }
        listed
    })
    .await
    .unwrap_or_else(|_| Err(Error::Driver((tx("dosya listesi okunamadı")).to_string())))
}

/// Yedeğin içindeki dosyaları gösteren sayfa (yalnızca yönetici).
pub async fn backup_files(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<FilesQuery>,
) -> Response {
    let ctx = Ctx::new(&st, &headers, PageQuery::default(), &me);
    if !me.can_write() {
        return ctx.not_found((tx("Yedeğin içindeki dosyaları yalnızca yönetici görebilir.")));
    }
    let Ok(backup) = st.snapshot().backup(&id).cloned() else {
        return ctx.not_found((tx("Yedek bulunamadı.")));
    };
    let path = query.path();
    let listed = list_backup_files(&st, &me.user, &ip, &id, query.disk, &path).await.map_err(|e| capitalize(&e.to_string()));
    let pick = query.pick.as_deref().filter(|p| crate::guestfs::valid_path(p)).map(|p| (crate::guestfs::normalize(p), query.dir.is_some())).filter(|(p, _)| p != "/");
    let running = st.snapshot().guest(backup.vmid).is_ok_and(|g| g.kind == redox_core::GuestKind::Vm && g.status == redox_core::GuestStatus::Running);
    Html(crate::view::backup_files(&ctx.page(Current::Backups, "/backups"), &backup, query.disk, &crate::guestfs::normalize(&path), &listed, pick.as_ref().map(|(p, dir)| (p.as_str(), *dir)), running)).into_response()
}

/// Yedeğin içinden tek bir dosyayı ya da (`tar=1` ile) bir dizinin tar arşivini indirir. Yalnızca yönetici.
pub async fn backup_file(
    State(st): State<AppState>,
    Extension(me): Extension<Identity>,
    Extension(crate::ClientIp(ip)): Extension<crate::ClientIp>,
    Path(id): Path<String>,
    Query(query): Query<FilesQuery>,
) -> Response {
    use std::io::Read;
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, (tx("Yedekten dosya indirmek için yönetici yetkisi gerekir."))).into_response();
    }
    let (path, tar) = (query.path(), query.tar.is_some());
    if !crate::guestfs::valid_path(&path) || (!tar && crate::guestfs::normalize(&path) == "/") {
        return (StatusCode::UNPROCESSABLE_ENTITY, (tx("Geçersiz yol."))).into_response();
    }
    st.auth().audit.record(now(), &me.user, &ip, &format!("backup {} {id} {}", if tar { "tar" } else { "get" }, crate::guestfs::normalize(&path)), true);
    let (state, wanted, disk, guest_path) = (st.clone(), id.clone(), query.disk, path.clone());
    let opened = tokio::task::spawn_blocking(move || {
        let (bin, image, temporary) = state.backup_image(&wanted, disk)?;
        let cleanup: Option<Box<dyn FnOnce() + Send>> = if temporary {
            let image = image.clone();
            Some(Box::new(move || crate::state::discard_export(&image)))
        } else {
            None
        };
        let mut stream = crate::guestfs::open(&bin, &image, &guest_path, tar, cleanup).map_err(Error::Driver)?;
        // İlk parça burada okunur: dosya yoksa ya da okunamıyorsa boş bir indirme yerine açık bir hata döner.
        let mut first = vec![0u8; 64 * 1024];
        let n = stream.read(&mut first).map_err(|e| Error::Driver(tfmt(format!("yedekteki dosya okunamadı: {e}"))))?;
        first.truncate(n);
        Ok::<_, Error>((first, stream))
    })
    .await;
    let (first, reader) = match opened {
        Ok(Ok(ready)) if !ready.0.is_empty() => ready,
        Ok(Ok(_)) => return (StatusCode::NOT_FOUND, (tx("Dosya yedekte yok, boş ya da okunamıyor."))).into_response(),
        Ok(Err(e)) => return (StatusCode::UNPROCESSABLE_ENTITY, capitalize(&e.to_string())).into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, (tx("Dosya okunamadı."))).into_response(),
    };
    let stream = futures_util::stream::unfold((Some(first), reader), |(first, mut reader)| async move {
        if let Some(first) = first {
            return Some((Ok::<_, std::io::Error>(first), (None, reader)));
        }
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; 256 * 1024];
            let n = reader.read(&mut buf);
            (
                n.map(|n| {
                    buf.truncate(n);
                    buf
                }),
                reader,
            )
        })
        .await;
        match read {
            Ok((Ok(buf), _)) if buf.is_empty() => None,
            Ok((result, reader)) => Some((result, (None, reader))),
            Err(_) => None,
        }
    });
    // Dosya adı başlığa güvenle konabilsin diye sadeleştirilir.
    let leaf = crate::guestfs::normalize(&path).rsplit('/').next().unwrap_or("").to_string();
    let safe: String = leaf.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    let name = match (safe.is_empty(), tar) {
        (true, _) => format!("{id}.tar"),
        (false, true) => format!("{safe}.tar"),
        (false, false) => safe,
    };
    (
        [
            (CONTENT_TYPE, if tar { "application/x-tar" } else { "application/octet-stream" }.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
            (HeaderName::from_static("x-content-type-options"), "nosniff".to_string()),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DownloadQuery {
    /// 0: asıl disk; diğerleri yedekteki ek disk numarası.
    disk: u8,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct BlockQuery {
    disk: u8,
    index: u64,
}

/// Artımlı çoğaltma: yedekteki diskin blok özetleri (yalnızca yönetici).
pub async fn backup_blocks(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>, Query(query): Query<BlockQuery>) -> Response {
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, tx("Yedek indirmek için yönetici yetkisi gerekir.")).into_response();
    }
    let listed = tokio::task::spawn_blocking(move || {
        let cache = st.backup_raw_cache(&id, query.disk).map_err(|e| e.to_string())?;
        crate::delta::block_hashes(&cache)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match listed {
        Ok((size, hashes)) => axum::Json(serde_json::json!({ "size": size, "block": crate::delta::BLOCK, "hashes": hashes })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, crate::i18n::tfmt(e)).into_response(),
    }
}

/// Artımlı çoğaltma: yedekteki diskin bir bloğu (yalnızca yönetici).
pub async fn backup_block(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>, Query(query): Query<BlockQuery>) -> Response {
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, tx("Yedek indirmek için yönetici yetkisi gerekir.")).into_response();
    }
    let read = tokio::task::spawn_blocking(move || {
        let cache = st.backup_raw_cache(&id, query.disk).map_err(|e| e.to_string())?;
        crate::delta::block(&cache, query.index)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match read {
        Ok(data) => ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], data).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, crate::i18n::tfmt(e)).into_response(),
    }
}

pub async fn backup_download(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(id): Path<String>, Query(query): Query<DownloadQuery>) -> Response {
    use std::io::Read;
    let disk = query.disk;
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, (tx("Yedek indirmek için yönetici yetkisi gerekir."))).into_response();
    }
    match st.snapshot().backup(&id) {
        Err(_) => return (StatusCode::NOT_FOUND, (tx("Yedek bulunamadı."))).into_response(),
        Ok(backup) if disk != 0 && !backup.disks.iter().any(|d| d.id == disk) => {
            return (StatusCode::NOT_FOUND, (tx("Bu yedekte o numarada ek disk yok."))).into_response();
        }
        Ok(_) => {}
    }
    // Artımlı yedek önce tek dosyaya dökülür; bu uzun sürebilir, istek iş parçacığını tutmamalı.
    let (exporter, wanted) = (st.clone(), id.clone());
    let opened = tokio::task::spawn_blocking(move || -> Result<Option<(std::fs::File, u64, String)>, String> {
        let Some((path, temporary)) = exporter.driver().export_backup(&wanted, disk)? else {
            return Ok(None);
        };
        let opened = std::fs::File::open(&path).ok().and_then(|file| {
            let len = file.metadata().ok()?.len();
            Some((file, len, path.file_name()?.to_str()?.to_string()))
        });
        // Açık dosya silinse de okunmaya devam eder; geçici kopya diskte kalmaz.
        if temporary {
            crate::state::discard_export(&path);
        }
        Ok(opened)
    })
    .await;
    let (file, len, name) = match opened {
        Ok(Ok(Some(found))) => found,
        Ok(Err(e)) => return (StatusCode::INTERNAL_SERVER_ERROR, tfmt(format!("Yedek hazırlanamadı: {e}"))).into_response(),
        _ => return (StatusCode::NOT_FOUND, (tx("Yedek dosyası bu sunucuda yok (yalnızca dış kopyası olabilir)."))).into_response(),
    };
    let stream = futures_util::stream::unfold(file, |mut file| async move {
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; 256 * 1024];
            let n = file.read(&mut buf);
            (n.map(|n| {
                buf.truncate(n);
                buf
            }), file)
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
            (CONTENT_TYPE, "application/octet-stream".to_string()),
            (axum::http::header::CONTENT_LENGTH, len.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// `/backups/{id}/{op}` adresinin GET karşılığı: yalnızca `download`.
pub async fn backup_get(state: State<AppState>, me: Extension<Identity>, Path((id, op)): Path<(String, String)>, query: Query<DownloadQuery>) -> Response {
    if op == "download" { backup_download(state, me, Path(id), query).await } else { StatusCode::NOT_FOUND.into_response() }
}
