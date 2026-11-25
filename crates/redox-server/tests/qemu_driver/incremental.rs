//! Artımlı yedekler ve ek disklerle yedek.

use super::*;

/// Her parçası (4 MiB) farklı içerikli, sonunda kısa bir kuyruk olan disk.
pub(super) fn patterned(chunks: usize) -> Vec<u8> {
    let size = redox_server::chunkstore::CHUNK_SIZE;
    let mut data: Vec<u8> = (0..chunks).flat_map(|i| std::iter::repeat_n(i as u8 + 1, size)).collect();
    data.extend_from_slice(b"kuyruk");
    data
}

pub(super) fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.path().is_dir() {
            found.extend(files_under(&entry.path()));
        } else {
            found.push(entry.path());
        }
    }
    found.sort();
    found
}

/// Yedek dizinindeki gizli (geçici) girdiler: iş bitince hiçbiri kalmamalı.
pub(super) fn temporaries(cfg: &QemuConfig) -> Vec<String> {
    std::fs::read_dir(cfg.backups_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with('.'))
        .collect()
}

#[test]
pub(super) fn incremental_backups_store_only_changes_and_restore_exactly() {
    let (mut cfg, dir) = config();
    cfg.incremental_backups = true;
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "deneme");
    d.create(&g).unwrap();
    let disk = cfg.disk_path(g.vmid);
    let chunks = || files_under(&cfg.backups_dir().join("chunks"));
    let chunk = redox_server::chunkstore::CHUNK_SIZE;

    let first = patterned(3);
    std::fs::write(&disk, &first).unwrap();
    let a = d.backup(&g, "vm-100-a").unwrap();
    assert!(a.new_mb.is_some(), "artımlı yedek eklenen miktarı bildirir");
    assert!(!cfg.backups_dir().join("vm-100-a.qcow2").exists(), "tek dosyalık kopya yazılmaz");
    assert!(cfg.backups_dir().join("vm-100-a.rdxi.json").is_file());
    assert_eq!(chunks().len(), 4);
    assert!(d.backup_present("vm-100-a") && !d.backup_present("vm-100-yok") && !d.backup_present("../x"));
    assert!(d.backup(&g, "vm-100-a").unwrap_err().contains("zaten var"));
    assert!(temporaries(&cfg).is_empty(), "geçici ham disk bırakılmaz: {:?}", temporaries(&cfg));

    // İkinci parçada birkaç bayt değişir: depoya yalnızca o parça eklenir.
    let mut second = first.clone();
    second[chunk..chunk + 5].copy_from_slice(b"YENI!");
    std::fs::write(&disk, &second).unwrap();
    let b = d.backup(&g, "vm-100-b").unwrap();
    assert_eq!(chunks().len(), 5, "yalnızca değişen parça eklenir");
    assert!(b.new_mb.unwrap() <= b.size_mb);
    // Değişmeyen diskin yedeği depoya hiçbir şey eklemez.
    assert_eq!(d.backup(&g, "vm-100-c").unwrap().new_mb, Some(0));
    assert_eq!(chunks().len(), 5);

    // Her yedek kendi anındaki diski bayt bayt geri verir.
    d.restore(&g, "vm-100-a", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == first, "ilk yedek ilk hali geri getirmeli");
    d.restore(&g, "vm-100-b", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == second, "ikinci yedek ikinci hali geri getirmeli");
    assert_eq!(d.verify_backup("vm-100-b", &[]).unwrap(), "4 parça doğrulandı");

    // En eski (temel) yedek silinir: sonrakiler etkilenmez.
    d.delete_backup("vm-100-a").unwrap();
    assert!(!d.backup_present("vm-100-a"));
    assert_eq!(chunks().len(), 4, "yalnızca silinen yedeğe özgü parça gider");
    std::fs::write(&disk, b"baska").unwrap();
    d.restore(&g, "vm-100-c", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == second);
    assert!(d.restore(&g, "vm-100-a", &[]).unwrap_err().contains("bulunamadı"));

    // Bozuk parça: doğrulama yakalar, geri yükleme reddedilir ve diske dokunulmaz.
    let victim = chunks().remove(0);
    let good = std::fs::read(&victim).unwrap();
    std::fs::write(&victim, b"bozuldu").unwrap();
    std::fs::write(&disk, b"canli-veri").unwrap();
    assert!(d.verify_backup("vm-100-b", &[]).unwrap_err().contains("parça bozuk"));
    assert!(d.restore(&g, "vm-100-b", &[]).unwrap_err().contains("parça bozuk"));
    assert_eq!(std::fs::read(&disk).unwrap(), b"canli-veri", "bozuk yedek diskin üzerine yazılmaz");
    assert_eq!(files_under(&cfg.images_dir()), vec![disk.clone()], "yarım geri yükleme dosyası bırakılmaz");
    assert!(d.export_backup("vm-100-b", 0).is_err());
    assert!(temporaries(&cfg).is_empty(), "başarısız dışa aktarma kalıntı bırakmaz");
    std::fs::write(&victim, &good).unwrap();

    // Dışa aktarma: tek dosyalık geçici kopya (sahte qemu-img içeriği aynen kopyalar).
    let (file, temporary) = d.export_backup("vm-100-b", 0).unwrap().unwrap();
    assert!(temporary && file.file_name().unwrap() == "vm-100-b.qcow2");
    assert!(std::fs::read(&file).unwrap() == second);
    redox_server::state::discard_export(&file);
    assert!(temporaries(&cfg).is_empty());

    d.delete_backup("vm-100-b").unwrap();
    d.delete_backup("vm-100-c").unwrap();
    assert!(chunks().is_empty(), "son yedek silinince depo boşalır");
    d.delete_backup("vm-100-c").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn incremental_and_full_backups_coexist_and_work_on_a_running_guest() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    let g = guest(&cfg, "deneme");
    let disk = cfg.disk_path(g.vmid);
    // Özellik açılmadan önce alınmış tam yedek.
    let old = QemuDriver::new(cfg.clone()).unwrap();
    old.create(&g).unwrap();
    std::fs::write(&disk, b"eski-hal").unwrap();
    assert_eq!(old.backup(&g, "vm-100-eski").unwrap().new_mb, None);
    // Yarım kalmış geçici dosyalar açılışta temizlenir.
    std::fs::write(cfg.backups_dir().join(".vm-100-x.raw.tmp"), b"x").unwrap();
    std::fs::create_dir_all(cfg.backups_dir().join(".export-1-1")).unwrap();
    std::fs::write(cfg.backups_dir().join(".export-1-1/vm-100-x.qcow2"), b"x").unwrap();

    cfg.incremental_backups = true;
    // Sunucu gerçek sürücüyü HostDriver üzerinden kullanır: yeni yöntemler iletilmeli.
    let d = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    assert!(temporaries(&cfg).is_empty(), "{:?}", temporaries(&cfg));
    assert!(d.backup_present("vm-100-eski"));
    assert_eq!(d.verify_backup("vm-100-eski", &[]).unwrap(), "imaj tutarlı");
    assert_eq!(d.export_backup("vm-100-eski", 0).unwrap().unwrap(), (cfg.backups_dir().join("vm-100-eski.qcow2"), false));
    assert!(d.verify_backup("ct-200-1", &[]).unwrap_err().contains("LXC kurulu değil"));
    assert!(d.verify_backup("vm-100-yok", &[]).unwrap_err().contains("bulunamadı"));

    // Çalışan konukta artımlı yedek: disk QEMU üzerinden ham olarak dökülür.
    let live = patterned(2);
    std::fs::write(&disk, &live).unwrap();
    d.apply(&g, Action::Start).unwrap();
    let size = d.backup(&g, "vm-100-canli").unwrap();
    assert!(size.new_mb.is_some() && d.backup_present("vm-100-canli"));
    assert!(!cfg.backups_dir().join("vm-100-canli.qcow2").exists());
    assert_eq!(d.verify_backup("vm-100-canli", &[]).unwrap(), "3 parça doğrulandı");
    let (file, temporary) = d.export_backup("vm-100-canli", 0).unwrap().unwrap();
    assert!(temporary && std::fs::read(&file).unwrap() == live);
    redox_server::state::discard_export(&file);
    // QEMU işi hatayla bitirirse yedek kaydedilmez, kalıntı kalmaz.
    std::fs::write(cfg.backups_dir().join(".vm-100-dolu.d0.raw.tmp.fail"), b"").unwrap();
    assert_eq!(d.backup(&g, "vm-100-dolu").unwrap_err(), "yedekleme başarısız: No space left on device");
    std::fs::remove_file(cfg.backups_dir().join(".vm-100-dolu.d0.raw.tmp.fail")).unwrap();
    assert!(!d.backup_present("vm-100-dolu") && temporaries(&cfg).is_empty());
    d.apply(&g, Action::Stop).unwrap();

    // İki tür de geri yüklenir.
    d.restore(&g, "vm-100-eski", &[]).unwrap();
    assert_eq!(std::fs::read(&disk).unwrap(), b"eski-hal");
    d.restore(&g, "vm-100-canli", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == live);
    // Bozulmuş tam yedek de yakalanır.
    std::fs::write(cfg.backups_dir().join("vm-100-eski.qcow2"), b"bozuk").unwrap();
    assert!(d.verify_backup("vm-100-eski", &[]).unwrap_err().starts_with("yedek bozuk"));
    d.delete_backup("vm-100-eski").unwrap();
    d.delete_backup("vm-100-canli").unwrap();
    assert!(files_under(&cfg.backups_dir()).is_empty(), "{:?}", files_under(&cfg.backups_dir()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
pub(super) async fn incremental_backups_reach_offsite_download_and_the_backups_page() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (mut cfg, dir) = config();
    cfg.incremental_backups = true;
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "artimli-test-tokeni").unwrap();
    let driver = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    let content = patterned(2);
    std::fs::write(cfg.disk_path(id), &content).unwrap();
    let remote = dir.join("uzak");
    std::fs::create_dir_all(&remote).unwrap();
    state.set_offsite(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\npassphrase = dogru-at-pil-zimba\n", remote.display())).unwrap());
    let remote_chunks = || files_under(&remote.join("chunks")).len();

    // Yedek artımlı alınır; dış hedefe de parça parça (şifreli) gider, tek dosyalık kopya üretilmez.
    let backup = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    assert!(c.tasks.iter().all(|t| t.status == TaskStatus::Ok), "{:?}", c.tasks);
    let record = c.backup(&backup).unwrap();
    assert!(record.new_mb.is_some() && record.offsite);
    assert!(remote.join(format!("{backup}.rdxi.json.enc")).is_file() && !remote.join(format!("{backup}.qcow2.enc")).exists());
    assert_eq!(remote_chunks(), 3);
    assert!(files_under(&remote).iter().all(|f| f.extension().is_some_and(|e| e == "enc")), "hedefteki her dosya şifrelidir");
    assert!(temporaries(&cfg).is_empty(), "{:?}", temporaries(&cfg));

    // İkinci yedek: tek parça değişti, hedefe yalnızca o gider.
    let first_content = content;
    let mut content = first_content.clone();
    content[5..10].copy_from_slice(b"YENI!");
    std::fs::write(cfg.disk_path(id), &content).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let first = backup;
    let backup = state.backup_create(id, "admin").unwrap();
    assert!(state.snapshot().backup(&backup).unwrap().offsite);
    assert_eq!(remote_chunks(), 4, "yalnızca değişen parça gönderilir");
    // Elle yeniden gönderme hedefte kaybolan parçayı tamamlar.
    std::fs::remove_file(files_under(&remote.join("chunks")).remove(0)).unwrap();
    state.backup_push(&first, "admin").unwrap();
    state.backup_push(&backup, "admin").unwrap();
    assert_eq!(remote_chunks(), 4);
    assert!(state.snapshot().tasks.iter().take(2).all(|t| t.command.starts_with("backup push") && t.status == TaskStatus::Ok));

    // Doğrulama görev günlüğüne başarıyla yazılır.
    state.backup_verify(&backup, "admin").unwrap();
    let last = state.snapshot().tasks[0].clone();
    assert_eq!((last.command, last.status), (format!("backup verify {backup}"), TaskStatus::Ok));

    // İndirme: artımlı yedek tek dosya olarak akar, geçici kopya kalmaz.
    let app = redox_server::app(state.clone());
    let fetch = |uri: String| {
        let request = axum::http::Request::builder().uri(uri).header("host", "redox.test").header("authorization", "Bearer artimli-test-tokeni").body(axum::body::Body::empty()).unwrap();
        app.clone().oneshot(request)
    };
    let response = fetch(format!("/api/v1/backups/{backup}/download")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-disposition"], format!("attachment; filename=\"{backup}.qcow2\"").as_str());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(body.as_ref() == content.as_slice(), "indirilen dosya diskin aynısı olmalı");
    assert!(temporaries(&cfg).is_empty());

    // Yedekler sayfası gerçek sürücüde etiketi ve doğrulama düğmesini gösterir.
    let response = fetch(format!("/guests/{id}?tab=yedekler")).await.unwrap();
    let html = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(html.contains("artımlı") && html.contains(&format!(r#"action="/backups/{backup}/verify""#)), "{html}");

    // Yerel depo tümüyle kaybolursa geri yükleme dış kopyanın parçalarını indirir (ikisi için de).
    std::fs::remove_dir_all(cfg.backups_dir()).unwrap();
    std::fs::write(cfg.disk_path(id), b"bozuk").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == content);
    state.backup_restore(&first, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == first_content);
    assert!(temporaries(&cfg).is_empty(), "{:?}", temporaries(&cfg));

    // Hedefte bozulan parça geri yüklenmez: konuğun diski olduğu gibi kalır.
    std::fs::remove_dir_all(cfg.backups_dir()).unwrap();
    let victim = files_under(&remote.join("chunks")).remove(0);
    let good = std::fs::read(&victim).unwrap();
    let mut bad = good.clone();
    let middle = bad.len() / 2;
    bad[middle] ^= 1;
    std::fs::write(&victim, &bad).unwrap();
    let _ = state.backup_restore(&first, "admin");
    let failed = state.snapshot().tasks[0].clone();
    assert_eq!(failed.status, TaskStatus::Failed, "{failed:?}");
    assert!(failed.message.as_deref().unwrap().contains("bozulmuş"), "{failed:?}");
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == first_content, "bozuk dış kopya diskin üzerine yazılmaz");
    std::fs::write(&victim, &good).unwrap();

    // İlk yedek silinir: hedefte yalnızca ona özgü parça gider, ikincisi hâlâ geri yüklenir.
    state.backup_delete(&first, "admin").unwrap();
    assert_eq!(remote_chunks(), 3);
    assert!(!remote.join(format!("{first}.rdxi.json.enc")).exists());
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert!(std::fs::read(cfg.disk_path(id)).unwrap() == content);

    // Son yedek de silinince hedefte hiçbir dosya kalmaz.
    state.backup_delete(&backup, "admin").unwrap();
    // Hedefte yalnızca depo işareti kalır.
    let left: Vec<String> = files_under(&remote).iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert!(left == ["redox-store.json.enc"] && state.snapshot().backups.is_empty(), "{left:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Ek diskli konuk: diskler sürücüde gerçekten oluşturulur.
pub(super) fn guest_with_disks(cfg: &QemuConfig, d: &QemuDriver, ids: &[u8]) -> Guest {
    let mut g = guest(cfg, "deneme");
    d.create(&g).unwrap();
    for id in ids {
        d.add_disk(&g, *id, 1).unwrap();
        g.hw.disks.push(redox_core::hw::ExtraDisk { id: *id, size_gb: 1, serial: *id as u64 });
    }
    g
}

#[test]
pub(super) fn extra_disks_are_backed_up_and_restored_together() {
    if !python_available() {
        return;
    }
    for incremental in [false, true] {
        let (mut cfg, dir) = config();
        cfg.incremental_backups = incremental;
        let d = QemuDriver::new(cfg.clone()).unwrap();
        let g = guest_with_disks(&cfg, &d, &[1, 2]);
        let disks = [cfg.disk_path(g.vmid), cfg.extra_disk_path(g.vmid, 1), cfg.extra_disk_path(g.vmid, 2)];
        let fill = |tag: &str| {
            for (i, disk) in disks.iter().enumerate() {
                std::fs::write(disk, format!("disk{i}-{tag}")).unwrap();
            }
        };
        let contents = || disks.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect::<Vec<_>>();
        let part = |id: &str, n: u8| cfg.backups_dir().join(if n == 0 { format!("{id}.qcow2") } else { format!("{id}.disk{n}.qcow2") });

        // Kapalı konuk: üç disk birlikte yedeklenir.
        fill("eski");
        d.backup(&g, "vm-100-a").unwrap();
        assert_eq!(part("vm-100-a", 0).exists() && part("vm-100-a", 1).exists() && part("vm-100-a", 2).exists(), !incremental);
        assert!(d.verify_backup("vm-100-a", &[1, 2]).is_ok(), "{incremental}");
        assert!(d.verify_backup("vm-100-a", &[3]).is_err(), "yedekte olmayan disk doğrulanamaz");

        // Çalışan konuk: bütün diskler tek QMP işlemiyle, aynı anda kopyalanır.
        fill("yeni");
        d.apply(&g, Action::Start).unwrap();
        d.backup(&g, "vm-100-b").unwrap();
        assert!(d.verify_backup("vm-100-b", &[1, 2]).is_ok());
        // Bir diskin kopyası başarısız olursa yedek alınmaz, diğer kopyalar durdurulur, kalıntı kalmaz.
        let marker = if incremental { cfg.backups_dir().join(".vm-100-dolu.d2.raw.tmp.fail") } else { cfg.backups_dir().join(".vm-100-dolu.disk2.qcow2.tmp.fail") };
        std::fs::write(&marker, b"").unwrap();
        assert_eq!(d.backup(&g, "vm-100-dolu").unwrap_err(), "yedekleme başarısız: No space left on device");
        std::fs::remove_file(&marker).unwrap();
        assert!(!d.backup_present("vm-100-dolu") && temporaries(&cfg).is_empty());
        assert!(!part("vm-100-dolu", 0).exists() && !part("vm-100-dolu", 1).exists() && !part("vm-100-dolu", 2).exists());
        // Başarısız denemeden sonra yeni yedek alınabilir (QMP bağlantısı temiz kapanmış olmalı).
        d.backup(&g, "vm-100-c").unwrap();
        d.apply(&g, Action::Stop).unwrap();

        // Geri yükleme: istenen bütün diskler birlikte döner.
        fill("bozuk");
        d.restore(&g, "vm-100-a", &[1, 2]).unwrap();
        assert_eq!(contents(), ["disk0-eski", "disk1-eski", "disk2-eski"]);
        // Yalnızca istenen ek diskler: diğerine dokunulmaz.
        d.restore(&g, "vm-100-b", &[2]).unwrap();
        assert_eq!(contents(), ["disk0-yeni", "disk1-eski", "disk2-yeni"]);
        // Yedekte olmayan bir disk istenirse hiçbir diske dokunulmaz.
        d.add_disk(&g, 3, 1).unwrap();
        fill("simdi");
        assert!(d.restore(&g, "vm-100-a", &[1, 2, 3]).is_err());
        assert_eq!(contents(), ["disk0-simdi", "disk1-simdi", "disk2-simdi"], "yarım geri yükleme olmaz");
        assert!(files_under(&cfg.images_dir()).iter().all(|f| !f.to_string_lossy().ends_with(".restore")));

        // Dışa aktarma: her disk kendi adıyla tek dosya.
        let (file, temporary) = d.export_backup("vm-100-a", 1).unwrap().unwrap();
        assert_eq!((temporary, file.file_name().unwrap().to_str().unwrap()), (incremental, "vm-100-a.disk1.qcow2"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "disk1-eski");
        if temporary {
            redox_server::state::discard_export(&file);
        }
        assert!(incremental == d.export_backup("vm-100-a", 5).is_err(), "artımlıda olmayan disk hata; tam yedekte dosya yolu döner");

        // Tam yedekte ek disk dosyası kaybolmuşsa: doğrulama ve geri yükleme bunu söyler.
        if !incremental {
            std::fs::remove_file(part("vm-100-b", 1)).unwrap();
            assert!(d.verify_backup("vm-100-b", &[1, 2]).unwrap_err().contains("bulunamadı"));
            assert!(d.restore(&g, "vm-100-b", &[1, 2]).unwrap_err().contains("bulunamadı"));
            assert_eq!(contents(), ["disk0-simdi", "disk1-simdi", "disk2-simdi"]);
        }
        // Dış hedeften inmiş olabilecek, başka bir dosyaya bağlanan (arkalıklı) imaj geri yüklenmez.
        if !incremental {
            std::fs::write(part("vm-100-c", 0), "arkalikli").unwrap();
            assert!(d.restore(&g, "vm-100-c", &[]).unwrap_err().contains("başka bir dosyaya bağlı"));
            assert_eq!(contents()[0], "disk0-simdi", "diske dokunulmaz");
        }
        // Yedek dosyaları doğrudan son adlarıyla yazılmaz: çökme sonrası kalan geçici dosyalar açılışta silinir.
        std::fs::write(cfg.backups_dir().join(".vm-100-yarim.qcow2.tmp"), b"x").unwrap();
        std::fs::write(cfg.backups_dir().join("vm-100-yarim.indiriliyor"), b"x").unwrap();
        std::fs::write(cfg.images_dir().join("vm-100-disk-0.qcow2.restore"), b"x").unwrap();
        drop(QemuDriver::new(cfg.clone()).unwrap());
        assert!(temporaries(&cfg).is_empty() && !cfg.backups_dir().join("vm-100-yarim.indiriliyor").exists());
        assert!(files_under(&cfg.images_dir()).iter().all(|f| !f.to_string_lossy().ends_with(".restore")));
        for id in ["vm-100-a", "vm-100-b", "vm-100-c"] {
            d.delete_backup(id).unwrap();
        }
        assert!(files_under(&cfg.backups_dir()).is_empty(), "{:?}", files_under(&cfg.backups_dir()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
pub(super) async fn full_backups_with_extra_disks_reach_offsite_and_download() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let mut users = UserFile::default();
    users.add_user("admin", "dogru-at-pil-zimba-1", redox_server::auth::Role::Admin, Cost::Test).unwrap();
    users.add_token("admin", "test", "ek-disk-test-tokeni").unwrap();
    let driver = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    let state = AppState::new(cluster, Box::new(driver), Duration::ZERO, None, Auth::new(users, None, false, Cost::Test));
    let id = state.create_guest(&spec("web", &node), "admin").unwrap();
    assert_eq!(state.disk_add(id, 1, "admin").unwrap(), 1);
    let (main, extra) = (cfg.disk_path(id), cfg.extra_disk_path(id, 1));
    std::fs::write(&main, "ana-ilk").unwrap();
    std::fs::write(&extra, "ek-ilk").unwrap();
    let remote = dir.join("uzak");
    std::fs::create_dir_all(&remote).unwrap();
    state.set_offsite(redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\n", remote.display())).unwrap());

    let backup = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    assert!(c.tasks.iter().all(|t| t.status == TaskStatus::Ok), "{:?}", c.tasks);
    assert_eq!(c.backup(&backup).unwrap().disks.len(), 1);
    assert!(c.backup(&backup).unwrap().offsite);
    // Dış hedefe iki dosya gider.
    assert_eq!(std::fs::read_to_string(remote.join(format!("{backup}.qcow2"))).unwrap(), "ana-ilk");
    assert_eq!(std::fs::read_to_string(remote.join(format!("{backup}.disk1.qcow2"))).unwrap(), "ek-ilk");

    // İndirme: asıl disk ve ek disk ayrı adreslerde.
    let app = redox_server::app(state.clone());
    let fetch = |uri: String| {
        let request = axum::http::Request::builder().uri(uri).header("host", "redox.test").header("authorization", "Bearer ek-disk-test-tokeni").body(axum::body::Body::empty()).unwrap();
        app.clone().oneshot(request)
    };
    let response = fetch(format!("/api/v1/backups/{backup}/download?disk=1")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-disposition"], format!("attachment; filename=\"{backup}.disk1.qcow2\"").as_str());
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(), b"ek-ilk");
    let response = fetch(format!("/api/v1/backups/{backup}/download")).await.unwrap();
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(), b"ana-ilk");
    assert_eq!(fetch(format!("/api/v1/backups/{backup}/download?disk=2")).await.unwrap().status(), 404, "yedekte olmayan disk");
    let response = fetch(format!("/guests/{id}?tab=yedekler")).await.unwrap();
    let html = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(html.contains("asıl disk + 1 ek disk") && html.contains(&format!("/backups/{backup}/download?disk=1")), "{html}");

    // Yerel yedekler kaybolur: geri yükleme iki dosyayı da hedeften indirir ve iki diski de geri getirir.
    std::fs::remove_dir_all(cfg.backups_dir()).unwrap();
    std::fs::write(&main, "ana-bozuk").unwrap();
    std::fs::write(&extra, "ek-bozuk").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert_eq!((std::fs::read_to_string(&main).unwrap(), std::fs::read_to_string(&extra).unwrap()), ("ana-ilk".to_string(), "ek-ilk".to_string()));
    state.backup_verify(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok);

    // Ek disk kaldırılırsa geri yükleme yalnızca asıl diski getirir; silinen diski geri eklemez.
    state.disk_remove(id, 1, "admin").unwrap();
    std::fs::write(&main, "ana-sonra").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok);
    assert_eq!(std::fs::read_to_string(&main).unwrap(), "ana-ilk");
    assert!(!extra.exists() && state.snapshot().guest(id).unwrap().hw.disks.is_empty());

    // Silme: yerel dosyalar ve dış kopyanın iki dosyası da gider.
    state.backup_delete(&backup, "admin").unwrap();
    assert!(files_under(&remote).is_empty() && files_under(&cfg.backups_dir()).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn local_backup_store_can_be_encrypted_with_a_passphrase() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    cfg.backup_passphrase = Some("dogru-at-pil-zimba".into());
    assert!(QemuDriver::new(cfg.clone()).err().unwrap().contains("--incremental-backups"), "tam yedekler şifrelenmez: açıkça reddedilir");
    cfg.incremental_backups = true;
    let d = redox_server::hostdriver::HostDriver::new(QemuDriver::new(cfg.clone()).unwrap(), None);
    assert!(d.backups_sealed());
    let g = guest(&cfg, "gizli");
    d.create(&g).unwrap();
    let disk = cfg.disk_path(g.vmid);
    let content = patterned(2);
    std::fs::write(&disk, &content).unwrap();
    d.backup(&g, "vm-100-a").unwrap();
    // Diskteki parçaların hiçbiri açık veri taşımaz.
    let chunks: Vec<PathBuf> = files_under(&cfg.backups_dir().join("chunks"));
    assert!(!chunks.is_empty() && chunks.iter().all(|f| std::fs::read(f).unwrap().starts_with(b"RDXSEAL1")));
    assert!(d.verify_backup("vm-100-a", &[]).is_ok());
    std::fs::write(&disk, b"bozuk").unwrap();
    d.restore(&g, "vm-100-a", &[]).unwrap();
    assert!(std::fs::read(&disk).unwrap() == content);
    // Artımlı çoğaltmanın açık önbelleği şifreli depoda üretilmez; ham içerik yine istenince çıkarılabilir.
    let raw = dir.join("ham");
    d.backup_raw("vm-100-a", 0, &raw).unwrap();
    assert!(std::fs::read(&raw).unwrap() == content);
    drop(d);
    // Parolasız ya da yanlış parolayla başlatma reddedilir.
    cfg.backup_passphrase = None;
    assert!(QemuDriver::new(cfg.clone()).err().unwrap().contains("--backup-passphrase-file"));
    cfg.backup_passphrase = Some("baska-bir-parola-123".into());
    assert!(QemuDriver::new(cfg.clone()).err().unwrap().contains("parolası yanlış"));
    std::fs::remove_dir_all(dir).unwrap();
}
