//! OVA dışa aktarma ve kurulum ortamları.

use super::*;

/// Yedek dosyasını indirir (dışa aktarma). Yalnızca yöneticiler; dosya parça parça akıtılır.
/// Kapalı sanal makineyi OVA arşivi olarak indirir. Arşiv diske yazılmaz: tanım ve diskler sırayla akıtılır.
pub async fn guest_export(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(raw): Path<String>) -> Response {
    use std::io::Read;
    if !me.can_write() {
        return (StatusCode::FORBIDDEN, "Dışa aktarmak için yönetici yetkisi gerekir.").into_response();
    }
    let Ok(vmid) = raw.parse::<redox_core::VmId>() else {
        return (StatusCode::NOT_FOUND, "Konuk bulunamadı.").into_response();
    };
    // Disk dönüştürme uzun sürebilir: istek iş parçacığını tutmamalı.
    let exporter = st.clone();
    let prepared = tokio::task::spawn_blocking(move || -> Result<(String, u64, Box<dyn Read + Send>), redox_core::Error> {
        let (guest, dir, disks) = exporter.export_ova(vmid)?;
        let built = (|| -> Result<(u64, Box<dyn Read + Send>), String> {
            let mut sized = Vec::with_capacity(disks.len());
            let mut files = Vec::with_capacity(disks.len());
            for (name, path, capacity) in &disks {
                let file = std::fs::File::open(path).map_err(|e| format!("dışa aktarılan disk açılamadı: {e}"))?;
                let size = file.metadata().map_err(|e| e.to_string())?.len();
                sized.push((name.clone(), size, *capacity));
                files.push((name.clone(), size, file));
            }
            let ovf = crate::ova::ovf_xml(&guest.name, guest.cores, guest.mem_total_mb, &sized).into_bytes();
            let pad = |size: u64| std::io::Cursor::new(vec![0u8; crate::ova::padding(size) as usize]);
            let mut total = 512 + ovf.len() as u64 + crate::ova::padding(ovf.len() as u64) + 1024;
            let ovf_len = ovf.len() as u64;
            let mut stream: Box<dyn Read + Send> = Box::new(std::io::Cursor::new(crate::ova::tar_header(&format!("{}.ovf", guest.name), ovf_len).to_vec()).chain(std::io::Cursor::new(ovf)).chain(pad(ovf_len)));
            for (name, size, file) in files {
                total += 512 + size + crate::ova::padding(size);
                stream = Box::new(stream.chain(std::io::Cursor::new(crate::ova::tar_header(&name, size).to_vec())).chain(file.take(size)).chain(pad(size)));
            }
            Ok((total, Box::new(stream.chain(std::io::Cursor::new(vec![0u8; 1024])))))
        })();
        // Açık dosyalar silinse de okunmaya devam eder; geçici dizin diskte kalmaz.
        let _ = std::fs::remove_dir_all(&dir);
        let (total, stream) = built.map_err(redox_core::Error::Driver)?;
        Ok((guest.name, total, stream))
    })
    .await;
    let (name, total, reader) = match prepared {
        Ok(Ok(ready)) => ready,
        Ok(Err(e)) => {
            let status = match e.kind() {
                redox_core::ErrorKind::NotFound => StatusCode::NOT_FOUND,
                redox_core::ErrorKind::Conflict => StatusCode::CONFLICT,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            return (status, capitalize(&e.to_string())).into_response();
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Dışa aktarma hazırlanamadı.").into_response(),
    };
    let stream = futures_util::stream::unfold(reader, |mut reader| async move {
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
            Ok((result, reader)) => Some((result, reader)),
            Err(_) => None,
        }
    });
    (
        [
            (CONTENT_TYPE, "application/x-tar".to_string()),
            (axum::http::header::CONTENT_LENGTH, total.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}.ova\"")),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// Tarayıcıdan kalıp ya da imaj yükleme (çok parçalı form). Dosya parça parça diske akar; belleğe alınmaz.
pub async fn media_upload(State(st): State<AppState>, Extension(me): Extension<Identity>, mut form: axum::extract::Multipart) -> Redirect {
    let back = "/storage";
    let stored: Result<String, String> = async {
        while let Some(mut field) = form.next_field().await.map_err(|e| format!("yükleme okunamadı: {e}"))? {
            if field.name() != Some("file") {
                continue;
            }
            // Tarayıcı tam yol gönderebilir: yalnızca dosya adı alınır, o da ad kuralından geçer.
            let name = field.file_name().unwrap_or_default().rsplit(['/', '\\']).next().unwrap_or_default().to_string();
            if name.is_empty() {
                return Err("yüklenecek dosyayı seçin".to_string());
            }
            let (dest, _) = crate::media::target(&st, &name).map_err(|e| e.to_string())?;
            let mut upload = crate::media::Upload::begin(dest)?;
            while let Some(chunk) = field.chunk().await.map_err(|e| format!("yükleme yarıda kesildi: {e}"))? {
                upload.write(chunk).await?;
            }
            upload.finish(None).await?;
            return Ok(name);
        }
        Err("yüklenecek dosyayı seçin".to_string())
    }
    .await;
    match stored {
        Ok(name) => {
            st.media_uploaded(&name, &Ok(()), &me.user);
            flash(back, "notice", &format!("{name} yüklendi."))
        }
        Err(e) => flash(back, "error", &capitalize(&e)),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FetchForm {
    url: String,
    name: String,
    sha256: String,
}

pub async fn media_fetch(State(st): State<AppState>, Extension(me): Extension<Identity>, Form(f): Form<FetchForm>) -> Redirect {
    match st.media_fetch(&f.url, &f.name, &f.sha256, &me.user) {
        Ok(name) => flash("/storage", "notice", &format!("{name} indiriliyor; bitince görev günlüğünde görünür.")),
        Err(e) => failed("/storage", &e),
    }
}

pub async fn media_delete(State(st): State<AppState>, Extension(me): Extension<Identity>, Path(name): Path<String>) -> Redirect {
    match st.media_delete(&name, &me.user) {
        Ok(()) => flash("/storage", "notice", &format!("{name} silindi.")),
        Err(e) => failed("/storage", &e),
    }
}
