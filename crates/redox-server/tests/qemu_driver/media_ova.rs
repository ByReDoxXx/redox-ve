//! Kurulum ortamları ve OVA.

use super::*;

#[tokio::test]
pub(super) async fn media_files_are_uploaded_listed_and_deleted() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "medya-test-tokeni").unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("izleyici", "test", "medya-izleyici-tokeni").unwrap();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let app = redox_server::app(state.clone());
    let call = |method: &str, uri: &str, token: &str, kind: Option<&str>, body: Vec<u8>| {
        let mut request = axum::http::Request::builder().method(method).uri(uri).header("host", "redox.test").header("authorization", format!("Bearer {token}"));
        if let Some(kind) = kind {
            request = request.header("content-type", kind);
        }
        app.clone().oneshot(request.body(axum::body::Body::from(body)).unwrap())
    };
    let text = |response: axum::response::Response| async move { String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap() };
    const ADMIN: &str = "medya-test-tokeni";
    const VIEWER: &str = "medya-izleyici-tokeni";

    // API: ham gövdeyle yükleme; dosya adı türü ve dizini belirler.
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let response = call("PUT", "/api/v1/media/debian-12.iso", ADMIN, None, big.clone()).await.unwrap();
    assert_eq!(response.status(), 201);
    assert!(std::fs::read(cfg.iso_dir().join("debian-12.iso")).unwrap() == big);
    assert_eq!(call("PUT", "/api/v1/media/bulut.qcow2", ADMIN, None, b"imaj".to_vec()).await.unwrap().status(), 201);
    assert_eq!(std::fs::read(cfg.cloud_dir().join("bulut.qcow2")).unwrap(), b"imaj");
    // Var olanın üzerine yazılmaz; geçersiz adlar ve dizin kaçışı reddedilir; boş dosya kabul edilmez.
    let again = call("PUT", "/api/v1/media/debian-12.iso", ADMIN, None, b"baska".to_vec()).await.unwrap();
    assert_eq!(again.status(), 422);
    assert!(text(again).await.contains("zaten var"));
    assert!(std::fs::read(cfg.iso_dir().join("debian-12.iso")).unwrap() == big);
    for bad in ["betik.sh", ".gizli.iso", "..%2F..%2Fetc%2Fpasswd.iso", "bos%20luk.iso"] {
        assert_eq!(call("PUT", &format!("/api/v1/media/{bad}"), ADMIN, None, b"x".to_vec()).await.unwrap().status(), 422, "{bad}");
    }
    assert_eq!(call("PUT", "/api/v1/media/bos.iso", ADMIN, None, Vec::new()).await.unwrap().status(), 422);
    // Özet başlığı tutmazsa dosya kaydedilmez.
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri("/api/v1/media/ozetli.iso")
        .header("host", "redox.test")
        .header("authorization", format!("Bearer {ADMIN}"))
        .header("x-sha256", "0".repeat(64))
        .body(axum::body::Body::from("veri"))
        .unwrap();
    assert_eq!(app.clone().oneshot(request).await.unwrap().status(), 422);
    assert!(!cfg.iso_dir().join("ozetli.iso").exists());
    let hidden = |dir: PathBuf| std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).count();
    assert_eq!(hidden(cfg.iso_dir()) + hidden(cfg.cloud_dir()), 0, "reddedilen yüklemeler geçici dosya bırakmaz");
    // Salt okunur kullanıcı yükleyemez ve silemez, ama listeyi görür.
    assert_eq!(call("PUT", "/api/v1/media/izinsiz.iso", VIEWER, None, b"x".to_vec()).await.unwrap().status(), 403);
    assert_eq!(call("DELETE", "/api/v1/media/debian-12.iso", VIEWER, None, Vec::new()).await.unwrap().status(), 403);
    let list: serde_json::Value = serde_json::from_str(&text(call("GET", "/api/v1/media", VIEWER, None, Vec::new()).await.unwrap()).await).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!((list[0]["name"].as_str(), list[0]["iso"].as_bool(), list[0]["bytes"].as_u64()), (Some("debian-12.iso"), Some(true), Some(300_000)));

    // Tarayıcı formu (çok parçalı): dosya parça parça diske akar; tarayıcının gönderdiği yol atılır.
    let boundary = "----redoxsinir";
    let mut form = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"C:\\\\indirilenler\\\\kurulum.iso\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    form.extend_from_slice(&big);
    form.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let response = call("POST", "/media/upload", ADMIN, Some(&format!("multipart/form-data; boundary={boundary}")), form).await.unwrap();
    assert_eq!(response.status(), 303);
    assert!(response.headers()["location"].to_str().unwrap().starts_with("/storage?notice="), "{:?}", response.headers()["location"]);
    assert!(std::fs::read(cfg.iso_dir().join("kurulum.iso")).unwrap() == big, "2 MB'lık varsayılan gövde sınırı bu adreste uygulanmaz");
    assert!(state.snapshot().tasks.iter().any(|t| t.command == "media upload kurulum.iso" && t.status == TaskStatus::Ok));
    let page = text(call("GET", "/storage", ADMIN, None, Vec::new()).await.unwrap()).await;
    assert!(page.contains("kurulum.iso") && page.contains("bulut.qcow2") && page.contains(r#"action="/media/upload" enctype="multipart/form-data""#), "{page}");

    // Bir konuğa takılı kalıp silinmez; takılı olmayan silinir.
    let id = state.create_guest(&GuestSpec { iso: Some("debian-12.iso".into()), ..spec("web", &node) }, "admin").unwrap();
    let refused = call("DELETE", "/api/v1/media/debian-12.iso", ADMIN, None, Vec::new()).await.unwrap();
    assert_eq!(refused.status(), 422);
    assert!(text(refused).await.contains(&id.to_string()));
    assert!(cfg.iso_dir().join("debian-12.iso").exists());
    assert_eq!(call("POST", "/media/kurulum.iso/delete", ADMIN, Some("application/x-www-form-urlencoded"), Vec::new()).await.unwrap().status(), 303);
    assert!(!cfg.iso_dir().join("kurulum.iso").exists());
    assert_eq!(call("DELETE", "/api/v1/media/yok.iso", ADMIN, None, Vec::new()).await.unwrap().status(), 422);
    // Adresten indirme: iç ağ ve şifresiz adresler istek anında reddedilir.
    for url in ["http://example.com/a.iso", "https://127.0.0.1/a.iso", "https://169.254.169.254/a.iso"] {
        let body = serde_json::json!({ "url": url }).to_string().into_bytes();
        assert_eq!(call("POST", "/api/v1/media/fetch", ADMIN, Some("application/json"), body).await.unwrap().status(), 422, "{url}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
pub(super) async fn ova_archives_are_imported_and_exported() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "ova-test-tokeni").unwrap();
    users.add_user("izleyici", "dogru-at-pil-zimba-2", redox_server::auth::Role::Viewer, Cost::Test).unwrap();
    users.add_token("izleyici", "test", "ova-izleyici-tokeni").unwrap();
    let driver = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let app = redox_server::app(state.clone());
    let call = |method: &str, uri: &str, token: &str, body: Vec<u8>| {
        let request = axum::http::Request::builder().method(method).uri(uri).header("host", "redox.test").header("authorization", format!("Bearer {token}"));
        app.clone().oneshot(request.body(axum::body::Body::from(body)).unwrap())
    };
    const ADMIN: &str = "ova-test-tokeni";

    // Başka bir sanallaştırıcıdan gelen arşiv: tanım, iki disk ve özet dosyası.
    let ovf = r#"<Envelope><References><File ovf:href="eski-disk1.vmdk" ovf:id="f1"/></References><VirtualSystem ovf:id="eski-sunucu"><Item><rasd:ResourceType>3</rasd:ResourceType><rasd:VirtualQuantity>4</rasd:VirtualQuantity></Item><Item><rasd:AllocationUnits>byte * 2^30</rasd:AllocationUnits><rasd:ResourceType>4</rasd:ResourceType><rasd:VirtualQuantity>2</rasd:VirtualQuantity></Item></VirtualSystem></Envelope>"#;
    let disk: Vec<u8> = (0..70_000u32).map(|i| (i % 249) as u8).collect();
    let archive = dir.join("kaynak.ova");
    redox_server::ova::write_tar(&archive, &[("eski.ovf", ovf.as_bytes()), ("../kacis.vmdk", b"yanlis disk"), ("eski-disk1.vmdk", &disk), ("eski.mf", b"x")]).unwrap();
    let response = call("PUT", "/api/v1/media/eski-sunucu.ova", ADMIN, std::fs::read(&archive).unwrap()).await.unwrap();
    assert_eq!(response.status(), 201);
    let list: serde_json::Value = serde_json::from_slice(&call("GET", "/api/v1/media", ADMIN, Vec::new()).await.unwrap().into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!((list[0]["ova"]["cores"].as_u64(), list[0]["ova"]["memory_mb"].as_u64(), list[0]["ova"]["name"].as_str()), (Some(4), Some(2048), Some("eski-sunucu")));
    let page = String::from_utf8(call("GET", "/storage", ADMIN, Vec::new()).await.unwrap().into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(page.contains("OVA arşivi") && page.contains("öneri: 4 çekirdek, 2048 MB"), "{page}");

    // Arşivden sanal makine: tanımın andığı disk çıkarılır (sahte qemu-img içeriği aynen kopyalar).
    let mut from_ova = spec("gelen", &node);
    from_ova.hw.image = Some("eski-sunucu.ova".into());
    let id = state.create_guest(&from_ova, "admin").unwrap();
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == disk);
    let leftovers = std::fs::read_dir(cfg.cloud_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).count();
    assert_eq!(leftovers, 0, "çıkarılan geçici disk silinir");
    assert!(!dir.join("kacis.vmdk").exists() && !cfg.data_dir.join("kacis.vmdk").exists(), "arşivdeki ad yol olarak kullanılmaz");

    // Dışa aktarma: tanım ve bütün diskler tek arşivde.
    assert_eq!(state.disk_add(id, 1, "admin").unwrap(), 1);
    std::fs::write(cfg.extra_disk_path(id, 1), b"ikinci diskin verisi").unwrap();
    let response = call("GET", &format!("/api/v1/guests/{id}/export"), ADMIN, Vec::new()).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-disposition"], "attachment; filename=\"gelen.ova\"");
    let declared: usize = response.headers()["content-length"].to_str().unwrap().parse().unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.len(), declared, "bildirilen uzunluk akan baytlarla aynı");
    let exported = dir.join("gelen.ova");
    std::fs::write(&exported, &body).unwrap();
    let members = redox_server::ova::members(&mut std::fs::File::open(&exported).unwrap()).unwrap();
    assert_eq!(members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["gelen.ovf", "gelen-disk1.vmdk", "gelen-disk2.vmdk"]);
    assert_eq!(&body[members[2].offset as usize..(members[2].offset + members[2].size) as usize], b"ikinci diskin verisi");
    let info = redox_server::ova::inspect(&exported).unwrap();
    assert_eq!((info.cores, info.memory_mb, info.disks.len()), (Some(1), Some(256), 2));
    // Sistemdeki tar da arşivi okuyabiliyor.
    let listing = std::process::Command::new("tar").arg("-tf").arg(&exported).output().unwrap();
    assert!(listing.status.success() && String::from_utf8_lossy(&listing.stdout).lines().count() == 3, "{listing:?}");
    let temporaries = std::fs::read_dir(cfg.backups_dir()).into_iter().flatten().flatten().count();
    assert_eq!(temporaries, 0, "dışa aktarma için üretilen dosyalar silinir");

    // Kendi arşivimiz yeniden içe aktarılabilir.
    assert_eq!(call("PUT", "/api/v1/media/gelen-kopya.ova", ADMIN, body.to_vec()).await.unwrap().status(), 201);
    let mut again = spec("donen", &node);
    again.hw.image = Some("gelen-kopya.ova".into());
    let second = state.create_guest(&again, "admin").unwrap();
    assert!(std::fs::read(cfg.disk_path(second)).unwrap() == disk, "dışa aktarılıp geri alınan disk aynı");

    // Çalışan konuk, olmayan konuk ve yetkisiz kullanıcı.
    state.run_action(id, Action::Start, "admin").unwrap();
    assert_eq!(call("GET", &format!("/api/v1/guests/{id}/export"), ADMIN, Vec::new()).await.unwrap().status(), 409);
    state.run_action(id, Action::Stop, "admin").unwrap();
    assert_eq!(call("GET", "/api/v1/guests/9999/export", ADMIN, Vec::new()).await.unwrap().status(), 404);
    assert_eq!(call("GET", &format!("/api/v1/guests/{id}/export"), "ova-izleyici-tokeni", Vec::new()).await.unwrap().status(), 403);
    let edit = String::from_utf8(call("GET", &format!("/guests/{id}?tab=cogalt"), ADMIN, Vec::new()).await.unwrap().into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(edit.contains(&format!("/guests/{id}/export")) && edit.contains("OVA Olarak İndir"));

    // Disksiz arşivden oluşturma açık hatayla reddedilir ve yarım konuk bırakılmaz.
    let empty = dir.join("bos.ova");
    redox_server::ova::write_tar(&empty, &[("bos.ovf", b"<a/>")]).unwrap();
    assert_eq!(call("PUT", "/api/v1/media/bos.ova", ADMIN, std::fs::read(&empty).unwrap()).await.unwrap().status(), 201);
    let mut broken = spec("olmaz", &node);
    broken.hw.image = Some("bos.ova".into());
    let before = state.snapshot().guests.len();
    assert!(matches!(state.create_guest(&broken, "admin"), Err(Error::Driver(m)) if m.contains("disk imajı")));
    assert_eq!(state.snapshot().guests.len(), before);
    std::fs::remove_dir_all(dir).unwrap();
}
