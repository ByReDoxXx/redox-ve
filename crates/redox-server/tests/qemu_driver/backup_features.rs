//! Yedekten yeni konuk, dosya okuma, açarak sınama, anında kurtarma, kopya hedefleri, konuk komutları, hız sınırı.

use super::*;

#[test]
pub(super) fn a_backup_is_opened_as_a_new_guest_even_after_the_original_is_gone() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let ok = |what: &str| assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{what}: {:?}", state.snapshot().tasks[0].message);
    let source = state.create_guest(&GuestSpec { cores: 2, memory_mb: 1024, ..spec("ozgun", &node) }, "admin").unwrap();
    state.update_guest(source, &redox_core::GuestPatch { tags: Some(vec!["web".into()]), cpu_type: Some("kvm64".into()), onboot: Some(true), ..Default::default() }, "admin").unwrap();
    assert_eq!(state.disk_add(source, 2, "admin").unwrap(), 1);
    let original = state.snapshot().guest(source).unwrap().clone();
    std::fs::write(cfg.disk_path(source), "ana-disk-yedegi").unwrap();
    std::fs::write(cfg.extra_disk_of(&original, 1), "ek-disk-yedegi").unwrap();
    let backup = state.backup_create(source, "admin").unwrap();
    ok("yedek");
    assert!(state.snapshot().backup(&backup).unwrap().spec.is_some(), "konuğun tanımı yedekle saklanır");
    // Özgün konuk sonradan değişir.
    std::fs::write(cfg.disk_path(source), "sonradan-degisti").unwrap();

    let copy = state.backup_restore_new(&backup, "kurtarilan", "admin").unwrap();
    ok("yeni konuk");
    assert_eq!(state.snapshot().tasks[0].command, format!("vm restore-new {copy} {backup}"));
    let g = state.snapshot().guest(copy).unwrap().clone();
    assert_ne!(copy, source);
    assert_eq!((g.name.as_str(), g.cores, g.mem_total_mb, g.status, g.hw.cpu_type.as_str()), ("kurtarilan", 2, 1024, GuestStatus::Stopped, "kvm64"));
    assert!(!g.onboot && g.lock.is_none(), "kopya kendiliğinden başlatılmaz");
    assert_eq!(g.hw.disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>(), [(1, 2)]);
    assert_eq!(std::fs::read_to_string(cfg.disk_path(copy)).unwrap(), "ana-disk-yedegi");
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_of(&g, 1)).unwrap(), "ek-disk-yedegi");
    assert_eq!(std::fs::read_to_string(cfg.disk_path(source)).unwrap(), "sonradan-degisti", "özgün konuğa dokunulmaz");
    state.run_action(copy, Action::Start, "admin").unwrap();
    ok("kopyayı başlatma");
    state.run_action(copy, Action::Stop, "admin").unwrap();

    // Özgün konuk silinse de yedekten yeni konuk açılır (tanım yedekle saklı).
    state.delete_guest(source, "admin").unwrap();
    let again = state.backup_restore_new(&backup, "ikinci", "admin").unwrap();
    ok("silinmiş konuğun yedeği");
    assert_eq!(std::fs::read_to_string(cfg.disk_path(again)).unwrap(), "ana-disk-yedegi");
    assert_eq!(state.snapshot().guest(again).unwrap().mem_total_mb, 1024);

    // Geçersiz ad ve olmayan yedek: konuk oluşmaz. Tanımı olmayan eski yedek, konuğu da yoksa açık hatayla reddedilir.
    let before = state.snapshot().guests.len();
    assert!(state.backup_restore_new(&backup, "Geçersiz Ad", "admin").is_err());
    assert!(state.backup_restore_new("vm-999-1", "x", "admin").is_err());
    state.mutate(|c| c.backups.iter_mut().for_each(|b| b.spec = None));
    assert!(matches!(state.backup_restore_new(&backup, "ucuncu", "admin"), Err(Error::InvalidSpec(m)) if m.contains("ayarları bilinmiyor")));
    assert_eq!(state.snapshot().guests.len(), before);
    // Yedek dosyası kaybolmuşsa görev başarısız olur; yarım konuk kayıtta kalır ama disk boş kalmaz diye uyarı görevdedir.
    let _ = std::fs::remove_dir_all(dir);
}

/// Sahte guestfish: `<imaj>.d` dizinini konuğun kökü sayar (gerçeği bir yardımcı sanal makine başlatır).
pub(super) const FAKE_GUESTFISH: &str = r#"[ "$1" = --version ] && { echo "guestfish 1.52.0"; exit 0; }
ROOT="$3.d"; VERB=$5; P=$6
[ -d "$ROOT" ] || { echo "libguestfs: error: inspect_os: no operating system was found on this disk" >&2; exit 1; }
case "$VERB" in
  readdir)
    [ -d "$ROOT$P" ] || { echo "libguestfs: error: readdir: $P: No such file or directory" >&2; exit 1; }
    i=0
    for f in $(ls -A "$ROOT$P"); do
      if [ -d "$ROOT$P/$f" ]; then t=d; else t=r; fi
      printf '[%d] = {\n  ino: %d\n  ftyp: %s\n  name: %s\n}\n' $i $i $t "$f"; i=$((i+1))
    done ;;
  download) cat "$ROOT$P" ;;
  tar-out) tar -C "$ROOT$P" -cf - . ;;
esac
"#;

#[tokio::test]
pub(super) async fn single_files_are_read_out_of_a_backup() {
    if !python_available() {
        return;
    }
    use std::io::Read;
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&spec("dosyali", &node), "admin").unwrap();
    let backup = state.backup_create(id, "admin").unwrap();
    // guestfish tanımlı değilken açık hata.
    assert!(matches!(state.backup_image(&backup, 0), Err(Error::InvalidSpec(m)) if m.contains("guestfish gerekir")));
    state.set_guestfish(Some(script(&dir, "guestfish", FAKE_GUESTFISH)));
    let (bin, image, temporary) = state.backup_image(&backup, 0).unwrap();
    assert_eq!((image.clone(), temporary), (cfg.backups_dir().join(format!("{backup}.qcow2")), false), "tam yedek yerinde okunur, kopyalanmaz");
    assert!(matches!(state.backup_image(&backup, 3), Err(Error::InvalidSpec(m)) if m.contains("ek disk yok")));
    assert!(state.backup_image("vm-999-1", 0).is_err());

    // Yedeğin "içi": sahte araç imajın yanındaki dizini okur.
    let root = PathBuf::from(format!("{}.d", image.display()));
    std::fs::create_dir_all(root.join("etc/ssh")).unwrap();
    std::fs::write(root.join("etc/hostname"), "dosyali-konuk\n").unwrap();
    std::fs::write(root.join("etc/ssh/sshd_config"), "Port 2222\n").unwrap();
    let listed = redox_server::ui_ops::list_backup_files(&state, "admin", "127.0.0.1", &backup, 0, "/etc").await.unwrap();
    assert_eq!(listed.iter().map(|e| (e.name.as_str(), e.kind)).collect::<Vec<_>>(), [("ssh", 'd'), ("hostname", 'r')]);
    assert!(redox_server::ui_ops::list_backup_files(&state, "admin", "127.0.0.1", &backup, 0, "/yok").await.unwrap_err().to_string().contains("No such file"));
    assert!(redox_server::ui_ops::list_backup_files(&state, "admin", "127.0.0.1", &backup, 0, "../etc").await.is_err());
    let mut content = String::new();
    redox_server::guestfs::open(&bin, &image, "/etc/hostname", false, None).unwrap().read_to_string(&mut content).unwrap();
    assert_eq!(content, "dosyali-konuk\n");
    let mut archive = Vec::new();
    redox_server::guestfs::open(&bin, &image, "/etc/ssh", true, None).unwrap().read_to_end(&mut archive).unwrap();
    assert!(archive.windows(9).any(|w| w == b"Port 2222"));
    // Yedek ve konuğun diski değişmedi; denetim günlüğünde iz var.
    assert_eq!(std::fs::read_to_string(&image).unwrap(), "disk");
    assert!(format!("{:?}", state.auth().audit.recent("", 20)).contains(&format!("backup ls {backup} /etc")));

    // Konteyner yedeği tar arşividir: dosya tarayıcısı açılmaz.
    state.mutate(|c| c.add_backup(redox_core::Backup { id: "ct-200-1".into(), ..c.backups[0].clone() }));
    assert!(matches!(state.backup_image("ct-200-1", 0), Err(Error::InvalidSpec(m)) if m.contains("tar arşividir")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn a_backup_is_proven_by_booting_it_in_an_isolated_temporary_guest() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    state.set_boot_test_wait(Duration::from_millis(300));
    let id = state.create_guest(&GuestSpec { hw: redox_core::Hardware { nics: vec!["nat".into()], usb: vec!["046d:c52b".into()], ..Default::default() }, ..spec("uretim", &node) }, "admin").unwrap();
    std::fs::write(cfg.disk_path(id), "uretim-diski").unwrap();
    let backup = state.backup_create(id, "admin").unwrap();
    let guests_before = state.snapshot().guests.len();
    let used_before: u64 = state.snapshot().storages.iter().map(|s| s.used_gb).sum();

    state.backup_test(&backup, "admin").unwrap();
    let c = state.snapshot();
    let task = c.tasks.iter().find(|t| t.command == format!("backup test {backup}")).expect("sınama görevi").clone();
    assert_eq!(task.status, TaskStatus::Ok, "{:?}", task.message);
    let test = c.backup(&backup).unwrap().tested.clone().expect("sonuç yedeğe yazılır");
    assert!(test.ok && test.note.contains("çalıştı") && test.note.contains("doğrulanamadı"), "ajan yokken ne doğrulandığı dürüstçe yazılır: {}", test.note);
    // Geçici konuk silinir: kayıt, disk ve ayrılan yer geri gelir.
    assert_eq!(c.guests.len(), guests_before, "geçici konuk kalmaz");
    assert_eq!(c.storages.iter().map(|s| s.used_gb).sum::<u64>(), used_before);
    assert_eq!(std::fs::read_dir(cfg.images_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".qcow2")).count(), 1, "yalnızca üretim konuğunun diski durur");
    assert_eq!(std::fs::read_to_string(cfg.disk_path(id)).unwrap(), "uretim-diski", "özgün konuğa dokunulmaz");
    // Geçici konuk ağdan yalıtılmış başlatılır ve sunucunun USB aygıtını üstlenmez.
    let temp_id = c.tasks.iter().find(|t| t.command.starts_with("vm restore-new")).and_then(|t| t.vmid).unwrap();
    let launch = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(temp_id).display())).unwrap();
    assert!(launch.contains("user,id=net0,restrict=on") && launch.contains("user,id=net1,restrict=on") && !launch.contains("usb-host"), "{launch}");

    // Bozuk yedek: sınama başarısız olur, neden yazar, yine de geçici konuk kalmaz.
    std::thread::sleep(Duration::from_millis(1100));
    let broken = state.backup_create(id, "admin").unwrap();
    std::fs::remove_file(cfg.backups_dir().join(format!("{broken}.qcow2"))).unwrap();
    state.backup_test(&broken, "admin").unwrap();
    let c = state.snapshot();
    let test = c.backup(&broken).unwrap().tested.clone().unwrap();
    assert!(!test.ok && test.note.contains("geri yüklenemedi"), "{}", test.note);
    assert_eq!(c.tasks.iter().find(|t| t.command == format!("backup test {broken}")).unwrap().status, TaskStatus::Failed);
    assert_eq!(c.guests.len(), guests_before);
    assert!(state.backup_test("vm-999-1", "admin").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn a_guest_runs_straight_from_its_backup_and_is_detached_later() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let ok = |what: &str| assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{what}: {:?}", state.snapshot().tasks[0].message);
    let source = state.create_guest(&spec("ozgun", &node), "admin").unwrap();
    assert_eq!(state.disk_add(source, 2, "admin").unwrap(), 1);
    let original = state.snapshot().guest(source).unwrap().clone();
    std::fs::write(cfg.disk_path(source), "yedekteki-ana-disk").unwrap();
    std::fs::write(cfg.extra_disk_of(&original, 1), "yedekteki-ek-disk").unwrap();
    let backup = state.backup_create(source, "admin").unwrap();
    let (main_part, extra_part) = (cfg.backups_dir().join(format!("{backup}.qcow2")), cfg.backups_dir().join(format!("{backup}.disk1.qcow2")));

    // Anında aç: diskler kopyalanmaz, yedeğe dayanan katmanlar kurulur; konuk hemen başlar.
    let new = state.backup_instant(&backup, "acil", "admin").unwrap();
    ok("anında aç");
    let g = state.snapshot().guest(new).unwrap().clone();
    assert_eq!((g.hw.instant.as_deref(), g.status), (Some(backup.as_str()), GuestStatus::Stopped));
    assert_eq!(std::fs::read_to_string(cfg.disk_path(new)).unwrap(), format!("bagli:{}", main_part.display()), "asıl disk yedeğe dayanır");
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_of(&g, 1)).unwrap(), format!("bagli:{}", extra_part.display()));
    state.run_action(new, Action::Start, "admin").unwrap();
    ok("başlatma");
    // Yedek, üzerinde konuk çalışırken silinemez.
    assert!(matches!(state.backup_delete(&backup, "admin"), Err(Error::InvalidSpec(m)) if m.contains("bu yedeğin üzerinde çalışıyor")));
    assert!(main_part.is_file());

    // Çalışırken ayır: QEMU arkalığı diske akıtır; bitince konuk bağımsızdır ve yedek silinebilir.
    state.guest_detach(new, "admin").unwrap();
    ok("çalışırken ayırma");
    assert_eq!(state.snapshot().tasks[0].command, format!("vm detach {new}"));
    assert_eq!(std::fs::read_to_string(cfg.disk_path(new)).unwrap(), "yedekteki-ana-disk");
    assert_eq!(std::fs::read_to_string(cfg.extra_disk_of(&g, 1)).unwrap(), "yedekteki-ek-disk");
    assert_eq!(state.snapshot().guest(new).unwrap().hw.instant, None);
    let hot = std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(new).display())).unwrap();
    assert_eq!(hot.matches("block-stream").count(), 2, "{hot}");
    assert!(matches!(state.guest_detach(new, "admin"), Err(Error::InvalidSpec(m)) if m.contains("üzerinde çalışmıyor")));
    state.run_action(new, Action::Stop, "admin").unwrap();

    // Kapalıyken ayır: qemu-img rebase.
    let second = state.backup_instant(&backup, "ikinci", "admin").unwrap();
    state.guest_detach(second, "admin").unwrap();
    ok("kapalıyken ayırma");
    assert_eq!(std::fs::read_to_string(cfg.disk_path(second)).unwrap(), "yedekteki-ana-disk");
    state.backup_delete(&backup, "admin").unwrap();
    assert!(!main_part.exists());
    assert_eq!(std::fs::read_to_string(cfg.disk_path(new)).unwrap(), "yedekteki-ana-disk", "ayrılan konuk yedek silinince etkilenmez");

    // Yedek dosyası yoksa ya da zincirliyse konuk oluşmaz.
    std::thread::sleep(Duration::from_millis(1100));
    let gone = state.backup_create(source, "admin").unwrap();
    let before = state.snapshot().guests.len();
    std::fs::write(cfg.backups_dir().join(format!("{gone}.qcow2")), "arkalikli").unwrap();
    assert!(matches!(state.backup_instant(&gone, "zincirli", "admin"), Err(Error::Driver(m)) if m.contains("arkalıklı")));
    std::fs::remove_file(cfg.backups_dir().join(format!("{gone}.qcow2"))).unwrap();
    assert!(matches!(state.backup_instant(&gone, "yok", "admin"), Err(Error::Driver(m)) if m.contains("yerelde yok")));
    assert_eq!(state.snapshot().guests.len(), before, "başarısız denemeler konuk bırakmaz");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn backups_are_copied_to_extra_targets_and_restored_from_whichever_survives() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&spec("uc-iki-bir", &node), "admin").unwrap();
    std::fs::write(cfg.disk_path(id), "korunan-disk").unwrap();
    let (first, second, third) = (dir.join("hedef-1"), dir.join("hedef-2"), dir.join("hedef-3"));
    for target in [&first, &second] {
        std::fs::create_dir_all(target).unwrap();
    }
    let target = |path: &Path, pass: &str| redox_server::offsite::Config::parse(&format!("kind = dir\npath = {}\npassphrase = {pass}\n", path.display())).unwrap();
    state.set_offsite(target(&first, "birinci-hedef-parolasi"));
    state.add_offsite_copy(target(&second, "ikinci-hedef-parolasi"));
    // Üçüncü hedefin dizini yok: o kopya başarısız olur ama yedek ve öbür kopyalar geçerlidir.
    state.add_offsite_copy(target(&third, "ucuncu-hedef-parolasi"));

    let backup = state.backup_create(id, "admin").unwrap();
    let c = state.snapshot();
    let record = c.backup(&backup).unwrap().clone();
    assert_eq!((record.offsite, record.copies), (true, 1), "asıl hedef ve erişilebilen bir kopya hedefi");
    assert!(first.join(format!("{backup}.qcow2.enc")).is_file() && second.join(format!("{backup}.qcow2.enc")).is_file());
    let failed = c.tasks.iter().find(|t| t.command.starts_with(&format!("backup copy {backup}"))).expect("başarısız kopya günlükte");
    assert_eq!(failed.status, TaskStatus::Failed);
    assert!(c.tasks.iter().any(|t| t.command == format!("vm backup {id}") && t.status == TaskStatus::Ok), "yedeğin kendisi başarılı");
    assert_ne!(std::fs::read(first.join(format!("{backup}.qcow2.enc"))).unwrap(), std::fs::read(second.join(format!("{backup}.qcow2.enc"))).unwrap(), "her hedef kendi parolasıyla şifrelenir");

    // Yerel yedek ve asıl hedefteki kopya kaybolur: geri yükleme ikinci hedeften gelir.
    let local = cfg.backups_dir().join(format!("{backup}.qcow2"));
    std::fs::remove_file(&local).unwrap();
    std::fs::remove_file(first.join(format!("{backup}.qcow2.enc"))).unwrap();
    std::fs::write(cfg.disk_path(id), "bozuldu").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert_eq!(std::fs::read_to_string(cfg.disk_path(id)).unwrap(), "korunan-disk");

    // Elle yeniden gönderme asıl hedefi onarır ve kopya sayısını günceller (üçüncü hedef artık var).
    std::fs::create_dir_all(&third).unwrap();
    state.backup_push(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().backup(&backup).unwrap().copies, 2);
    assert!(first.join(format!("{backup}.qcow2.enc")).is_file() && third.join(format!("{backup}.qcow2.enc")).is_file());

    // Silme bütün hedeflerdeki kopyaları kaldırır.
    state.backup_delete(&backup, "admin").unwrap();
    for target in [&first, &second, &third] {
        assert!(!target.join(format!("{backup}.qcow2.enc")).exists(), "{}", target.display());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn commands_run_inside_the_guest_before_and_after_a_live_backup() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    // Sahte QEMU, adı "ajanli" ile başlayan konukta konuk ajanını canlandırır.
    let id = state.create_guest(&spec("ajanli-vt", &node), "admin").unwrap();
    let hooks = |pre: &str, post: &str| redox_core::GuestPatch { backup_pre: Some(pre.into()), backup_post: Some(post.into()), ..Default::default() };
    assert!(state.update_guest(id, &hooks("once.sh", ""), "admin").is_err(), "mutlak yol şart");
    state.update_guest(id, &hooks("/usr/local/bin/once.sh --sessiz", "/usr/local/bin/sonra.sh"), "admin").unwrap();
    let log = || std::fs::read_to_string(format!("{}.log", cfg.qga_path(id).display())).unwrap_or_default();
    let last = || state.snapshot().tasks[0].clone();

    // Kapalı konukta komut çalışmaz (konuk kapalı: çalıştıracak bir şey yok).
    state.backup_create(id, "admin").unwrap();
    assert_eq!(last().status, TaskStatus::Ok, "{:?}", last().message);
    assert!(log().is_empty());

    // Çalışan konukta sıra: önce komut, dondurma, çözme, sonra komut.
    state.run_action(id, Action::Start, "admin").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let backup = state.backup_create(id, "admin").unwrap();
    assert_eq!(last().status, TaskStatus::Ok, "{:?}", last().message);
    let lines: Vec<String> = log().lines().filter(|l| l.starts_with("guest-exec") || l.contains("fsfreeze")).map(|l| l.trim_end().to_string()).collect();
    assert_eq!(lines, ["guest-exec /usr/local/bin/once.sh --sessiz", "guest-fsfreeze-freeze", "guest-fsfreeze-thaw", "guest-exec /usr/local/bin/sonra.sh"], "{lines:?}");
    assert_eq!(state.snapshot().backup(&backup).unwrap().quiesced, Some(true));

    // Öncesi komutu başarısız: yedek alınmaz, dosya bırakılmaz, konuk dondurulmaz; neden görevde yazar.
    state.update_guest(id, &hooks("/usr/local/bin/basarisiz-once.sh", "/usr/local/bin/sonra.sh"), "admin").unwrap();
    std::fs::remove_file(format!("{}.log", cfg.qga_path(id).display())).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let count = state.snapshot().backups.len();
    state.backup_create(id, "admin").unwrap();
    let task = last();
    assert_eq!(task.status, TaskStatus::Failed);
    let message = task.message.unwrap();
    assert!(message.contains("yedek öncesi komutu 3 koduyla bitti") && message.contains("veritabani mesgul") && message.contains("yedek alınmadı"), "{message}");
    assert_eq!(state.snapshot().backups.len(), count);
    assert!(!log().contains("fsfreeze") && !log().contains("sonra.sh"), "{}", log());

    // Sonrası komutu başarısız: konuk çözülmüştür ama yedek geçersiz sayılır ve neden yazılır.
    state.update_guest(id, &hooks("/usr/local/bin/once.sh", "/usr/local/bin/basarisiz-sonra.sh"), "admin").unwrap();
    std::fs::remove_file(format!("{}.log", cfg.qga_path(id).display())).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    state.backup_create(id, "admin").unwrap();
    let message = last().message.unwrap();
    assert!(last().status == TaskStatus::Failed && message.contains("yedek sonrası komutu") && message.contains("elle çalıştırın"), "{message}");
    assert!(log().contains("guest-fsfreeze-thaw"), "konuk donmuş bırakılmaz: {}", log());
    assert_eq!(state.snapshot().backups.len(), count);
    assert!(std::fs::read_dir(cfg.backups_dir()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')), "yarım yedek dosyası kalmaz");

    // Komutlar kaldırılınca yedek eskisi gibi alınır.
    state.update_guest(id, &hooks("", ""), "admin").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    state.backup_create(id, "admin").unwrap();
    assert_eq!(last().status, TaskStatus::Ok, "{:?}", last().message);
    state.run_action(id, Action::Stop, "admin").unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn backup_copies_respect_the_configured_speed_limit() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    // Sahte qemu-img kendisine verilen argümanları kaydeder.
    cfg.qemu_img_bin = script(&dir, "qemu-img-kayitli", &format!("echo \"$@\" >> '{}/img.log'\n{FAKE_IMG}", dir.display()));
    cfg.backup_mbps = 50;
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "sinirli");
    d.create(&g).unwrap();
    // Kapalı konuk: qemu-img convert -r <bayt/sn>.
    d.backup(&g, "vm-100-kapali").unwrap();
    let log = std::fs::read_to_string(dir.join("img.log")).unwrap();
    assert!(log.lines().any(|l| l.starts_with(&format!("convert -r {} -O qcow2 -c ", 50u64 * 1024 * 1024))), "{log}");
    // Çalışan konuk: QEMU'nun yedek işine hız verilir.
    d.apply(&g, Action::Start).unwrap();
    d.backup(&g, "vm-100-canli").unwrap();
    let hot = std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(g.vmid).display())).unwrap();
    assert!(hot.contains(&format!("backup-speed {}", 50u64 * 1024 * 1024)), "{hot}");
    d.apply(&g, Action::Stop).unwrap();
    // Sınır yokken hız argümanı verilmez.
    let mut free = cfg.clone();
    free.backup_mbps = 0;
    std::fs::remove_file(dir.join("img.log")).unwrap();
    QemuDriver::new(free).unwrap().backup(&g, "vm-100-sinirsiz").unwrap();
    assert!(!std::fs::read_to_string(dir.join("img.log")).unwrap().contains(" -r "));
    let _ = std::fs::remove_dir_all(dir);
}
