//! Dinamik bellek, disk sayaçları, iç içe sanallaştırma, sınama denetimleri, arşiv.

use super::*;

#[test]
pub(super) fn dynamic_memory_follows_the_balloon_driver_report() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&GuestSpec { memory_mb: 4096, ..spec("dinamik", &node) }, "admin").unwrap();
    let hot = || std::fs::read_to_string(format!("{}.hot", cfg.qmp_path(id).display())).unwrap_or_default();
    let report = |text: &str| std::fs::write(format!("{}.memstats", cfg.qmp_path(id).display()), text).unwrap();
    state.update_guest(id, &redox_core::GuestPatch { balloon: Some(true), ..Default::default() }, "admin").unwrap();
    state.dynamic_memory_set(id, 1024, "admin").unwrap();
    state.run_action(id, Action::Start, "admin").unwrap();

    // İlk tur yalnızca ölçüm yoklamasını açar; sürücü bildirmedikçe hedefe dokunulmaz.
    assert_eq!(state.dynamic_memory_tick(), 0);
    assert!(hot().contains("qom-set guest-stats-polling-interval=10"), "{}", hot());
    assert_eq!(state.dynamic_memory_tick(), 0, "balon sürücüsü olmayan konuk: ölçüm yok");
    assert!(!hot().contains("balloon "));

    // Sürücü "kullanılabilir bellek"i bildirmiyor (eski çekirdek): ölçüm sayılmaz, konuk kısılmaz.
    report("4096 yok");
    assert_eq!(state.dynamic_memory_tick(), 0);
    assert!(!hot().contains("balloon "), "{}", hot());
    // Konuk 4 GB'ın 600 MB'ını kullanıyor: 600 + 256 = 856 → 896, alt sınır 1024.
    report("4096 3496");
    assert_eq!(state.dynamic_memory_tick(), 1);
    assert!(hot().trim_end().ends_with(&format!("balloon {}", 1024u64 << 20)), "{}", hot());
    assert_eq!(state.snapshot().guest(id).unwrap().hw.balloon_mb, 1024);
    // Kullanım artar (konuk artık 1024 MB görüyor, 100 MB'ı boş): hedef büyür.
    report("1024 100");
    assert_eq!(state.dynamic_memory_tick(), 1);
    assert_eq!(state.snapshot().guest(id).unwrap().hw.balloon_mb, 1216);
    assert_eq!(hot().matches("qom-set").count(), 1, "yoklama bir kez açılır");

    // Kapatınca balon boşaltılır.
    state.dynamic_memory_set(id, 0, "admin").unwrap();
    assert!(hot().trim_end().ends_with(&format!("balloon {}", 4096u64 << 20)), "{}", hot());
    assert_eq!(state.dynamic_memory_tick(), 0);
    state.run_action(id, Action::Stop, "admin").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn disk_io_counters_come_from_blockstats() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let g = guest(&cfg, "sayac");
    d.create(&g).unwrap();
    assert_eq!(d.io_counters(&g), None, "kapalı konukta sayaç yok");
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(d.io_counters(&g), Some((1 << 20, 2 << 20)));
    assert_eq!(d.io_counters(&g), Some((2 << 20, 4 << 20)));
    d.apply(&g, Action::Stop).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn nested_guests_need_nested_kvm_on_the_host() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    cfg.kvm = true;
    cfg.sys_module = dir.join("sys-module");
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let mut g = guest(&cfg, "icice");
    g.hw.nested = true;
    d.create(&g).unwrap();
    assert!(!d.nested_virt());
    let err = d.apply(&g, Action::Start).unwrap_err();
    assert!(err.contains("iç içe sanallaştırma kapalı") && err.contains("kvm_intel nested=1"), "{err}");
    std::fs::create_dir_all(cfg.sys_module.join("kvm_intel/parameters")).unwrap();
    std::fs::write(cfg.sys_module.join("kvm_intel/parameters/nested"), "Y\n").unwrap();
    assert!(d.nested_virt());
    d.apply(&g, Action::Start).unwrap();
    let args = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(g.vmid).display())).unwrap();
    assert!(args.contains("-cpu\nhost\n"), "{args}");
    d.apply(&g, Action::Stop).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
pub(super) fn boot_test_checks_a_command_and_a_port_in_the_restored_guest() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    state.set_boot_test_wait(Duration::from_millis(300));
    let id = state.create_guest(&spec("uretim", &node), "admin").unwrap();
    state.update_guest(id, &redox_core::GuestPatch { forwards: Some(vec![redox_core::PortForward { host_port: 18080, guest_port: 80 }]), ..Default::default() }, "admin").unwrap();
    // Sınama konuklarında da konuk ajanı yanıt versin.
    std::fs::write(cfg.qmp_path(id).parent().unwrap().join("ajan-var"), "").unwrap();
    let test_of = |backup: &str| state.snapshot().backup(backup).unwrap().tested.clone().unwrap();
    let set = |port: u16, cmd: &str| state.update_guest(id, &redox_core::GuestPatch { test_port: Some(port), test_cmd: Some(cmd.into()), ..Default::default() }, "admin").unwrap();

    // Komut başarılı: sonuç notunda yazar; komut geçici konukta çalışır.
    set(0, "/usr/local/bin/saglik.sh --tam");
    let ok = state.backup_create(id, "admin").unwrap();
    state.backup_test(&ok, "admin").unwrap();
    let test = test_of(&ok);
    assert!(test.ok && test.note.contains("işletim sistemi yanıt verdi") && test.note.contains("sınama komutu başarıyla bitti"), "{}", test.note);
    let temp = state.snapshot().tasks.iter().find(|t| t.command.starts_with("vm restore-new")).and_then(|t| t.vmid).unwrap();
    assert!(std::fs::read_to_string(format!("{}.log", cfg.qga_path(temp).display())).unwrap().contains("guest-exec /usr/local/bin/saglik.sh --tam"));
    let launch = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(temp).display())).unwrap();
    assert!(!launch.contains("hostfwd"), "özgün konuğun kapı yönlendirmesi kopyaya verilmez: {launch}");

    // Komut hata koduyla biter: yedek açılsa da sınama başarısızdır.
    std::thread::sleep(Duration::from_millis(1100));
    set(0, "/usr/local/bin/basarisiz.sh");
    let bad = state.backup_create(id, "admin").unwrap();
    state.backup_test(&bad, "admin").unwrap();
    let test = test_of(&bad);
    assert!(!test.ok && test.note.contains("sınama komutu başarısız") && test.note.contains("3 koduyla bitti"), "{}", test.note);

    // Kapı denetimi: kapı yalnızca 127.0.0.1'de açılır; sahte konukta dinleyen olmadığı için başarısızdır.
    std::thread::sleep(Duration::from_millis(1100));
    set(22, "");
    let port = state.backup_create(id, "admin").unwrap();
    state.backup_test(&port, "admin").unwrap();
    let test = test_of(&port);
    assert!(!test.ok && test.note.contains("TCP 22 kapısındaki hizmet doğrulanamadı"), "{}", test.note);
    // Görevler en yeniden eskiye sıralıdır: ilk eşleşen bu sınamanın geçici konuğudur (kimlikler yeniden kullanılır).
    let temp = state.snapshot().tasks.iter().find(|t| t.command.starts_with("vm restore-new")).and_then(|t| t.vmid).unwrap();
    let launch = std::fs::read_to_string(format!("{}.args", cfg.qmp_path(temp).display())).unwrap();
    assert!(launch.contains("restrict=on,hostfwd=tcp:127.0.0.1:") && launch.contains("-:22"), "{launch}");
    assert_eq!(state.snapshot().guests.len(), 1, "geçici konuklar kalmaz");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn copy_targets_with_keep_days_archive_backups_deleted_locally() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&spec("arsivli", &node), "admin").unwrap();
    std::fs::write(cfg.disk_path(id), "arsivlik-disk").unwrap();
    let (first, vault) = (dir.join("hedef"), dir.join("arsiv"));
    for target in [&first, &vault] {
        std::fs::create_dir_all(target).unwrap();
    }
    let target = |path: &Path, extra: &str| redox_server::offsite::Config::parse(&format!("kind = dir
path = {}
passphrase = bir-hedef-parolasi
{extra}", path.display())).unwrap();
    assert!(matches!(state.backup_recover("vm-1-1", "admin"), Err(Error::InvalidSpec(m)) if m.contains("kopya hedefi tanımlı değil")));
    state.set_offsite(target(&first, ""));
    state.add_offsite_copy(target(&vault, "keep_days = 30
"));

    let backup = state.backup_create(id, "admin").unwrap();
    let (in_first, in_vault) = (first.join(format!("{backup}.qcow2.enc")), vault.join(format!("{backup}.qcow2.enc")));
    assert!(in_first.is_file() && in_vault.is_file());

    // Yerelde silinir: asıl hedeften de gider, saklama süresi olan kopya hedefinde kalır ve arşive geçer.
    state.backup_delete(&backup, "admin").unwrap();
    assert!(!cfg.backups_dir().join(format!("{backup}.qcow2")).exists() && !in_first.exists() && in_vault.is_file());
    let c = state.snapshot();
    assert!(c.backups.is_empty() && c.archive.len() == 1 && c.archive[0].id == backup);
    assert_eq!(state.archive_tick(), 0, "süresi dolmadı");

    // Geri getirilir: listeye elle alınmış bir yedek olarak döner ve kopya hedefinden geri yüklenir.
    state.backup_recover(&backup, "admin").unwrap();
    let c = state.snapshot();
    let record = c.backup(&backup).unwrap();
    assert!(c.archive.is_empty() && !record.auto && !record.offsite && record.copies == 1);
    assert!(matches!(state.backup_recover(&backup, "admin"), Err(Error::BackupNotFound(_))));
    std::fs::write(cfg.disk_path(id), "bozuldu").unwrap();
    state.backup_restore(&backup, "admin").unwrap();
    assert_eq!(state.snapshot().tasks[0].status, TaskStatus::Ok, "{:?}", state.snapshot().tasks[0].message);
    assert_eq!(std::fs::read_to_string(cfg.disk_path(id)).unwrap(), "arsivlik-disk");

    // Yeniden silinince yine arşive döner; süresi dolunca kopya hedefinden de silinir.
    state.backup_delete(&backup, "admin").unwrap();
    assert!(in_vault.is_file() && state.snapshot().archive.len() == 1);
    state.mutate(|c| c.archive[0].created -= 31 * 86_400);
    assert_eq!(state.archive_tick(), 1);
    assert!(!in_vault.exists() && state.snapshot().archive.is_empty());
    assert!(state.snapshot().tasks.iter().any(|t| t.command == format!("backup archive-expire {backup}")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn newest_backups_are_boot_tested_on_a_schedule() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    state.set_boot_test_wait(Duration::from_millis(200));
    let id = state.create_guest(&spec("uretim", &node), "admin").unwrap();
    let backup = state.backup_create(id, "admin").unwrap();
    // Kapalıyken ve saat penceresinin dışında bir şey yapılmaz.
    assert_eq!(state.auto_test_tick(200), None);
    state.set_auto_test(7, 180);
    assert_eq!(state.auto_test_tick(179), None);
    assert_eq!(state.auto_test_tick(180 + 240), None);
    // Pencerede: en yeni yedek sınanır, sonuç yedeğe yazılır; aynı yedek yedi gün dolmadan yeniden sınanmaz.
    assert_eq!(state.auto_test_tick(185).as_deref(), Some(backup.as_str()));
    let c = state.snapshot();
    assert!(c.backup(&backup).unwrap().tested.as_ref().is_some_and(|t| t.ok));
    assert!(c.tasks.iter().any(|t| t.command == format!("backup test {backup}") && t.user == redox_server::state::SCHEDULER));
    assert_eq!(c.guests.len(), 1);
    assert_eq!(state.auto_test_tick(186), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn disk_usage_inside_the_guest_is_read_through_the_agent() {
    if !python_available() {
        return;
    }
    let (cfg, dir) = config();
    let cluster = local_cluster(&cfg);
    let node = cluster.nodes[0].id.clone();
    let state = AppState::new(cluster, Box::new(QemuDriver::new(cfg.clone()).unwrap()), Duration::ZERO, None, Auth::new(UserFile::default(), None, false, Cost::Test));
    let id = state.create_guest(&spec("ajanli-disk", &node), "admin").unwrap();
    assert_eq!(state.guest_fs_tick(), 0, "kapalı konuk okunmaz");
    state.run_action(id, Action::Start, "admin").unwrap();
    state.refresh(None, None);
    assert_eq!(state.guest_fs_tick(), 1);
    let c = state.snapshot();
    assert_eq!(c.filesystems_of(id).iter().map(|f| (f.mount.as_str(), f.pct())).collect::<Vec<_>>(), [("/", 90)]);
    assert!(redox_server::alerts::evaluate(&c).iter().any(|a| a.key == format!("guest-fs:{id}:/")), "dolan disk uyarı üretir");
    state.run_action(id, Action::Stop, "admin").unwrap();
    state.refresh(None, None);
    state.guest_fs_tick();
    assert!(state.snapshot().filesystems_of(id).is_empty(), "duran konuğun ölçümü silinir");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
pub(super) fn cpu_shares_and_memory_reservation_are_written_to_the_guests_control_group() {
    if !python_available() {
        return;
    }
    let (mut cfg, dir) = config();
    let groups = dir.join("cgroup");
    std::fs::create_dir_all(&groups).unwrap();
    cfg.cgroup_base = Some(groups.clone());
    let d = QemuDriver::new(cfg.clone()).unwrap();
    let mut g = guest(&cfg, "payli");
    d.create(&g).unwrap();
    let group = groups.join(format!("vm-{}", g.vmid));
    let read = |file: &str| std::fs::read_to_string(group.join(file)).unwrap_or_default();

    // Pay ve garanti istenmemişse konuğun grubu açılmaz.
    d.apply(&g, Action::Start).unwrap();
    d.configure(&g).unwrap();
    assert!(!group.exists());

    // Çalışırken verilince hemen uygulanır: süreç gruba alınır, ağırlık ve garanti yazılır.
    (g.hw.cpu_shares, g.hw.mem_reserve_mb) = (400, 128);
    d.configure(&g).unwrap();
    let pid = std::fs::read_to_string(cfg.pid_path(g.vmid)).unwrap().trim().to_string();
    assert_eq!((read("cgroup.procs"), read("cpu.weight"), read("memory.min")), (pid, "400".to_string(), (128u64 << 20).to_string()));
    assert_eq!(std::fs::read_to_string(groups.join("cgroup.subtree_control")).unwrap(), "+memory", "alt gruplarda denetleyiciler açılır");
    // Sıfırlanınca olağan değerler yazılır.
    (g.hw.cpu_shares, g.hw.mem_reserve_mb) = (0, 0);
    d.configure(&g).unwrap();
    assert_eq!((read("cpu.weight"), read("memory.min")), ("100".to_string(), "0".to_string()));
    d.apply(&g, Action::Stop).unwrap();
    d.configure(&g).unwrap();

    // Başlatırken de uygulanır.
    let _ = std::fs::remove_dir_all(&group);
    g.hw.cpu_shares = 50;
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(read("cpu.weight"), "50");
    d.apply(&g, Action::Stop).unwrap();

    // Grup açılamıyorsa (yetki yok) pay istenen konuk başlatılmaz; istenmeyen konuk etkilenmez.
    std::fs::write(dir.join("dosya"), b"").unwrap();
    let mut locked = cfg.clone();
    locked.cgroup_base = Some(dir.join("dosya"));
    let d = QemuDriver::new(locked).unwrap();
    let refused = d.apply(&g, Action::Start).unwrap_err();
    assert!(refused.contains("denetim grubu") && refused.contains("Delegate=cpu memory"), "{refused}");
    assert_eq!(status(&d, &g), GuestStatus::Stopped, "söz verilemeyen konuk çalışır bırakılmaz");
    g.hw.cpu_shares = 0;
    d.apply(&g, Action::Start).unwrap();
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
