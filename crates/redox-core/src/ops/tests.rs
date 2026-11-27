use super::*;
use crate::model::GuestSpec;

const NOW: u64 = 1_800_000_000;

fn cluster() -> Cluster {
    Cluster::demo(NOW)
}

fn patch() -> GuestPatch {
    GuestPatch::default()
}

fn fwd(host_port: u16, guest_port: u16) -> PortForward {
    PortForward { host_port, guest_port }
}

#[test]
fn begin_op_locks_and_complete_unlocks() {
    let mut c = cluster();
    assert_eq!(
        c.begin_op(100, "vm snapshot 100".into(), "u", NOW, Some("anlık görüntü almak")),
        Err(Error::RequiresStopped { vmid: 100, what: "anlık görüntü almak".into() })
    );
    assert_eq!(c.begin_op(999, "x".into(), "u", NOW, None), Err(Error::GuestNotFound(999)));
    // Durdurma şartı olmayan işlem çalışan konukta da başlar.
    let live = c.begin_op(100, "vm backup 100".into(), "u", NOW, None).unwrap();
    c.complete_task(live, Ok(()), NOW).unwrap();

    let t = c.begin_op(103, "vm snapshot 103 temiz".into(), "ayse", NOW, Some("x")).unwrap();
    assert_eq!(c.guest(103).unwrap().lock, Some(t));
    assert_eq!(c.begin_op(103, "y".into(), "u", NOW, None), Err(Error::Locked { vmid: 103, task: t }));
    assert_eq!(c.begin_action(103, Action::Start, "u", NOW), Err(Error::Locked { vmid: 103, task: t }));

    c.complete_task(t, Err("disk dolu".into()), NOW + 3).unwrap();
    let g = c.guest(103).unwrap();
    assert_eq!((g.lock, g.status), (None, GuestStatus::Stopped), "işlem görevi durumu değiştirmez");
    let task = c.task(t).unwrap();
    assert_eq!((task.status, task.message.as_deref(), task.user.as_str()), (TaskStatus::Failed, Some("disk dolu"), "ayse"));
}

#[test]
fn update_changes_fields_and_logs() {
    let mut c = cluster();
    let p = GuestPatch {
        name: Some("derleme".into()),
        cores: Some(4),
        memory_mb: Some(8192),
        os: Some("windows-2025".into()),
        iso: Some("win.iso".into()),
        onboot: Some(false),
        tags: Some(vec!["prod".into(), "ci".into(), "prod".into()]),
        forwards: Some(vec![fwd(8080, 80), fwd(2222, 22)]),
        bridge: None,
        backup_time: Some("03:30".into()),
        backup_keep: Some(3),
        ..Default::default()
    };
    c.update_guest(103, &p, "ayse", NOW).unwrap();
    let g = c.guest(103).unwrap();
    assert_eq!((g.name.as_str(), g.cores, g.mem_total_mb, g.os.as_str()), ("derleme", 4, 8192, "windows-2025"));
    assert_eq!((g.iso.as_deref(), g.onboot), (Some("win.iso"), false));
    assert_eq!(g.tags, ["ci", "prod"], "etiketler sıralı ve tekil");
    assert_eq!(g.forwards, [fwd(8080, 80), fwd(2222, 22)]);
    assert_eq!((c.tasks[0].command.as_str(), c.tasks[0].user.as_str()), ("vm set 103", "ayse"));

    // Boş ISO kalıbı çıkarır; verilmeyen alanlar değişmez.
    c.update_guest(103, &GuestPatch { iso: Some(String::new()), ..patch() }, "u", NOW).unwrap();
    let g = c.guest(103).unwrap();
    assert_eq!((g.iso.as_deref(), g.cores, g.tags.len()), (None, 4, 2));
}

#[test]
fn hardware_changes_need_a_stopped_guest() {
    let mut c = cluster();
    let before = c.clone();
    for p in [
        GuestPatch { cores: Some(2), ..patch() },
        GuestPatch { memory_mb: Some(4096), ..patch() },
        GuestPatch { iso: Some("x.iso".into()), ..patch() },
        GuestPatch { forwards: Some(vec![fwd(8080, 80)]), ..patch() },
    ] {
        assert!(matches!(c.update_guest(100, &p, "u", NOW), Err(Error::RequiresStopped { vmid: 100, .. })), "{p:?}");
    }
    assert_eq!(c, before);
    // Aynı değeri yeniden göndermek değişiklik sayılmaz; ad ve etiket çalışırken de değişir.
    let p = GuestPatch { cores: Some(4), name: Some("web-yeni".into()), tags: Some(vec!["web".into()]), onboot: Some(false), ..patch() };
    c.update_guest(100, &p, "u", NOW).unwrap();
    assert_eq!(c.guest(100).unwrap().name, "web-yeni");
}

#[test]
fn update_validates_values() {
    let mut c = cluster();
    let before = c.clone();
    let bad: Vec<(GuestPatch, &str)> = vec![
        (GuestPatch { name: Some("Kötü".into()), ..patch() }, "geçersiz ad"),
        (GuestPatch { cores: Some(0), ..patch() }, "çekirdek"),
        (GuestPatch { cores: Some(999), ..patch() }, "çekirdek"),
        (GuestPatch { memory_mb: Some(1), ..patch() }, "bellek"),
        (GuestPatch { iso: Some("../x.iso".into()), ..patch() }, "geçersiz ISO"),
        (GuestPatch { tags: Some(vec!["Büyük".into()]), ..patch() }, "geçersiz etiket"),
        (GuestPatch { tags: Some((0..9).map(|i| format!("t{i}")).collect()), ..patch() }, "en fazla 8 etiket"),
        (GuestPatch { forwards: Some(vec![fwd(80, 80)]), ..patch() }, "geçersiz yönlendirme"),
        (GuestPatch { forwards: Some(vec![fwd(8080, 0)]), ..patch() }, "geçersiz yönlendirme"),
        (GuestPatch { forwards: Some((0..17).map(|i| fwd(9000 + i, 80)).collect()), ..patch() }, "en fazla 16"),
    ];
    for (p, needle) in bad {
        let err = c.update_guest(103, &p, "u", NOW).unwrap_err().to_string();
        assert!(err.contains(needle), "{p:?}: {err}");
    }
    assert!(matches!(c.update_guest(202, &GuestPatch { iso: Some("a.iso".into()), ..patch() }, "u", NOW), Err(Error::InvalidSpec(_))), "konteynere ISO takılmaz");
    assert_eq!(c.update_guest(999, &patch(), "u", NOW), Err(Error::GuestNotFound(999)));
    assert_eq!(c, before);
}

#[test]
fn bridge_setting_and_mac() {
    let mut c = cluster();
    for ok in ["nat", "vmbr0", "br-lan", "eth0.100", "docker0"] {
        assert!(valid_bridge(ok), "{ok}");
    }
    for bad in ["", "-x", "..", ".", "a b", "br0;reboot", "çok-uzun-köprü-adı", "br0,script=/x", "abcdefghijklmnop"] {
        assert!(!valid_bridge(bad), "{bad}");
    }
    assert_eq!(mac_for(100), "52:54:00:00:00:64");
    assert_eq!(mac_for(0x00AB_CDEF), "52:54:00:ab:cd:ef");
    assert_ne!(mac_for(100), mac_for(101));

    c.update_guest(103, &GuestPatch { bridge: Some("br-lan".into()), ..patch() }, "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().bridge, "br-lan");
    assert!(matches!(c.update_guest(103, &GuestPatch { bridge: Some("br0,x=1".into()), ..patch() }, "u", NOW), Err(Error::InvalidSpec(_))));
    assert!(matches!(c.update_guest(100, &GuestPatch { bridge: Some("nat".into()), ..patch() }, "u", NOW), Err(Error::RequiresStopped { .. })), "ağ ayarı çalışırken değişmez");

    assert!(c.observe_ip(100, Some("10.0.0.7".into())));
    assert!(!c.observe_ip(100, Some("10.0.0.7".into())), "aynı adres değişiklik sayılmaz");
    assert!(c.observe_ip(100, None));
    assert_eq!(c.guest(100).unwrap().ip, None);
    assert!(!c.observe_ip(999, None));
}

#[test]
fn host_ports_are_unique_across_guests() {
    let mut c = cluster();
    c.update_guest(103, &GuestPatch { forwards: Some(vec![fwd(8080, 80)]), ..patch() }, "u", NOW).unwrap();
    assert_eq!(c.update_guest(202, &GuestPatch { forwards: Some(vec![fwd(8080, 8080)]), ..patch() }, "u", NOW), Err(Error::PortTaken(8080)));
    assert_eq!(c.update_guest(202, &GuestPatch { forwards: Some(vec![fwd(9000, 1), fwd(9000, 2)]), ..patch() }, "u", NOW), Err(Error::PortTaken(9000)));
    // Kendi portunu yeniden göndermek çakışma değildir.
    c.update_guest(103, &GuestPatch { forwards: Some(vec![fwd(8080, 80), fwd(8443, 443)]), ..patch() }, "u", NOW).unwrap();
}

#[test]
fn snapshot_bookkeeping() {
    let mut c = cluster();
    assert!(matches!(c.check_snapshot_new(103, "Kötü Ad"), Err(Error::InvalidSpec(_))));
    assert!(matches!(c.check_snapshot_new(103, ""), Err(Error::InvalidSpec(_))));
    c.check_snapshot_new(103, "kurulum_oncesi").unwrap();
    c.add_snapshot(103, "kurulum_oncesi", NOW);
    assert_eq!(c.check_snapshot_new(103, "kurulum_oncesi"), Err(Error::SnapshotExists("kurulum_oncesi".into())));
    assert_eq!(c.check_snapshot(103, "yok"), Err(Error::SnapshotNotFound("yok".into())));
    c.check_snapshot(103, "kurulum_oncesi").unwrap();

    for i in 1..MAX_SNAPSHOTS {
        c.add_snapshot(103, &format!("s{i}"), NOW);
    }
    assert!(matches!(c.check_snapshot_new(103, "bir-daha"), Err(Error::InvalidSpec(m)) if m.contains("en fazla")));
    c.remove_snapshot(103, "s1");
    c.check_snapshot_new(103, "bir-daha").unwrap();
    c.after_restore(103, 0);
    assert!(c.guest(103).unwrap().snapshots.is_empty());
}

#[test]
fn disk_can_only_grow_within_free_space() {
    let mut c = cluster();
    assert!(matches!(c.check_grow(103, 200), Err(Error::InvalidSpec(_))), "aynı boyut");
    assert!(matches!(c.check_grow(103, 100), Err(Error::InvalidSpec(_))), "küçültme");
    assert_eq!(
        c.check_grow(103, 1000),
        Err(Error::InsufficientStorage { storage: "local-zfs".into(), needed_gb: 800, free_gb: 410 })
    );
    c.check_grow(103, 300).unwrap();
    let used = c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb;
    c.apply_grow(103, 300);
    assert_eq!(c.guest(103).unwrap().disk_gb, 300);
    assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used + 100);
}

#[test]
fn clone_copies_config_but_not_identity() {
    let mut c = cluster();
    c.update_guest(103, &GuestPatch { tags: Some(vec!["ci".into()]), forwards: Some(vec![fwd(8080, 80)]), ..patch() }, "u", NOW).unwrap();
    c.add_snapshot(103, "s", NOW);
    let used = c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb;

    assert_eq!(c.clone_guest(103, "Kötü"), Err(Error::InvalidName("Kötü".into())));
    assert_eq!(c.clone_guest(999, "x"), Err(Error::GuestNotFound(999)));
    let id = c.clone_guest(103, "win-build-2").unwrap();
    assert_eq!(id, 105);
    let (src, dst) = (c.guest(103).unwrap().clone(), c.guest(id).unwrap().clone());
    assert_eq!((dst.cores, dst.mem_total_mb, dst.disk_gb, &dst.os, &dst.tags), (src.cores, src.mem_total_mb, src.disk_gb, &src.os, &src.tags));
    assert_eq!((dst.status, dst.name.as_str()), (GuestStatus::Stopped, "win-build-2"));
    assert!(dst.forwards.is_empty() && dst.snapshots.is_empty(), "port ve anlık görüntü kopyalanmaz");
    assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used + 200);

    c.discard_clone(id);
    assert!(c.guest(id).is_err());
    assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used);

    let mut full = cluster();
    full.storages[1].used_gb = full.storages[1].total_gb - 10;
    assert!(matches!(full.clone_guest(103, "x"), Err(Error::InsufficientStorage { .. })));
}

#[test]
fn extra_disks_reserve_and_release_storage() {
    let mut c = Cluster::demo(NOW);
    let stopped = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).unwrap().clone();
    let running = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
    let used = |c: &Cluster| c.storages.iter().find(|s| s.id == stopped.storage && s.available_on(&stopped.node)).unwrap().used_gb;
    let before = used(&c);

    assert_eq!(c.add_disk(stopped.vmid, 20).unwrap(), 1);
    assert_eq!(c.add_disk(stopped.vmid, 30).unwrap(), 2);
    assert_eq!(used(&c), before + 50);
    assert_eq!(c.guest(stopped.vmid).unwrap().total_disk_gb(), stopped.disk_gb + 50);
    // Aradaki disk silinince numarası boşalır ve yeni disk onu alır; diğerinin numarası değişmez.
    assert_eq!(c.remove_disk(stopped.vmid, 1).unwrap().size_gb, 20);
    assert_eq!(used(&c), before + 30);
    assert_eq!(c.add_disk(stopped.vmid, 5).unwrap(), 1);
    assert_eq!(c.guest(stopped.vmid).unwrap().hw.disks.iter().map(|d| (d.id, d.size_gb)).collect::<Vec<_>>(), [(1, 5), (2, 30)]);

    assert!(matches!(c.add_disk(running, 10), Err(Error::RequiresStopped { .. })));
    assert!(matches!(c.add_disk(ct, 10), Err(Error::InvalidSpec(m)) if m.contains("yalnızca sanal makinelere")));
    assert!(matches!(c.add_disk(stopped.vmid, 0), Err(Error::InvalidSpec(_))));
    assert!(matches!(c.add_disk(stopped.vmid, 10_000_000), Err(Error::InsufficientStorage { .. })));
    assert!(matches!(c.remove_disk(stopped.vmid, 7), Err(Error::InvalidSpec(m)) if m.contains("7 numaralı ek disk yok")));
    for _ in 0..6 {
        c.add_disk(stopped.vmid, 1).unwrap();
    }
    assert!(matches!(c.add_disk(stopped.vmid, 1), Err(Error::InvalidSpec(m)) if m.contains("en fazla 8")));

    // Klon ek diskleri de sayar; silme hepsini iade eder.
    let with_disks = used(&c);
    let total = c.guest(stopped.vmid).unwrap().total_disk_gb();
    let copy = c.clone_guest(stopped.vmid, "kopya").unwrap();
    assert_eq!(used(&c), with_disks + total);
    c.discard_clone(copy);
    assert_eq!(used(&c), with_disks);
    c.delete_guest(stopped.vmid, "u", NOW).unwrap();
    assert_eq!(used(&c), before - stopped.disk_gb);
}

#[test]
fn backup_registry() {
    let mut c = cluster();
    let b = |id: &str, vmid| Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW, size_mb: 5, auto: false, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 };
    c.add_backup(b("vm-103-1", 103));
    c.add_backup(b("vm-100-1", 100));
    c.add_backup(b("vm-103-2", 103));
    assert_eq!(c.backups_of(103).map(|x| x.id.as_str()).collect::<Vec<_>>(), ["vm-103-2", "vm-103-1"], "en yeni başta");
    assert_eq!(c.backup("vm-100-1").unwrap().vmid, 100);
    assert_eq!(c.remove_backup("yok"), Err(Error::BackupNotFound("yok".into())));
    assert_eq!(c.remove_backup("vm-103-1").unwrap().id, "vm-103-1");
    assert_eq!(c.backups.len(), 2);
    // Yedekler durumla birlikte saklanır.
    let back: Cluster = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(back.backups, c.backups);
}

#[test]
fn schedule_rules() {
    for (text, minutes) in [("00:00", Some(0)), ("03:30", Some(210)), ("23:59", Some(1439)), ("24:00", None), ("3:30", None), ("03:60", None), ("0330", None), ("ab:cd", None), ("", None), ("03:30:00", None), ("+3:30", None)] {
        assert_eq!(parse_time(text), minutes, "{text}");
    }
    let mut c = cluster();
    let set = |c: &mut Cluster, vmid, time: &str, keep| c.update_guest(vmid, &GuestPatch { backup_time: Some(time.into()), backup_keep: keep, ..patch() }, "u", NOW);
    set(&mut c, 103, "03:30", Some(2)).unwrap();
    set(&mut c, 100, "23:00", None).unwrap();
    assert_eq!((c.guest(103).unwrap().backup_keep, c.guest(100).unwrap().backup_keep), (2, DEFAULT_KEEP));
    assert!(matches!(set(&mut c, 103, "25:00", None), Err(Error::InvalidSpec(_))));
    assert!(matches!(set(&mut c, 103, "03:30", Some(0)), Err(Error::InvalidSpec(_))));
    assert!(matches!(set(&mut c, 103, "03:30", Some(366)), Err(Error::InvalidSpec(_))));

    assert!(c.auto_backups_due("2026-11-24", 209).is_empty(), "saati gelmeden alınmaz");
    assert_eq!(c.auto_backups_due("2026-11-24", 210), [103]);
    assert_eq!(c.auto_backups_due("2026-11-24", 1439), [100, 103], "sunucu kapalıyken kaçan saat gün içinde telafi edilir");
    c.mark_auto_backup(103, "2026-11-24");
    assert_eq!(c.auto_backups_due("2026-11-24", 1439), [100], "aynı gün ikinci kez alınmaz");
    assert_eq!(c.auto_backups_due("2026-11-25", 300), [103], "ertesi gün yeniden");

    let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    assert!(c.auto_backups_due("2026-11-25", 300).is_empty(), "kilitli konuk bekletilir");
    c.complete_task(t, Ok(()), NOW).unwrap();
    set(&mut c, 103, "", None).unwrap();
    assert_eq!(c.guest(103).unwrap().backup_time, None, "boş saat zamanlamayı kapatır");
    assert!(c.auto_backups_due("2026-11-25", 1439).iter().all(|v| *v != 103));
}

#[test]
fn restore_covers_extra_disks_still_attached_and_unchanged() {
    use crate::hw::ExtraDisk;
    let mut c = cluster();
    let disk = |id, size_gb| ExtraDisk { id, size_gb, serial: id as u64 };
    let with = |c: &mut Cluster, disks: Vec<ExtraDisk>| c.guest_mut_for_import(103).unwrap().hw.disks = disks;
    with(&mut c, vec![disk(1, 10), disk(2, 20)]);
    c.add_backup(Backup { id: "b".into(), vmid: 103, guest_name: "x".into(), created: NOW, size_mb: 1, auto: false, offsite: false, new_mb: None, disks: vec![disk(1, 10), disk(2, 20)], disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    assert_eq!(c.restore_disks("b").unwrap(), [1, 2]);
    // Yedekten sonra kaldırılan disk geri getirilmez; sonradan eklenen (3) listeye girmez.
    with(&mut c, vec![disk(2, 20), disk(3, 5)]);
    assert_eq!(c.restore_disks("b").unwrap(), [2]);
    // Aynı numaraya sonradan takılmış disk (boyutu aynı olsa da) başka bir disktir: üzerine yazılmaz.
    with(&mut c, vec![ExtraDisk { id: 1, size_gb: 10, serial: 9 }, disk(2, 20)]);
    assert_eq!(c.restore_disks("b").unwrap(), [2]);
    // Sıra numarası tutmayan eski kayıtlar ayırt edilemez: boyut farklıysa açık hata.
    let old = |id, size_gb| ExtraDisk { id, size_gb, serial: 0 };
    c.add_backup(Backup { id: "eski".into(), vmid: 103, guest_name: "x".into(), created: NOW, size_mb: 1, auto: false, offsite: false, new_mb: None, disks: vec![old(1, 10)], disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    with(&mut c, vec![old(1, 10)]);
    assert_eq!(c.restore_disks("eski").unwrap(), [1]);
    with(&mut c, vec![old(1, 50)]);
    assert!(matches!(c.restore_disks("eski"), Err(Error::InvalidSpec(m)) if m.contains("ek disk 1") && m.contains("10 GB") && m.contains("50 GB")));
    assert_eq!(c.restore_disks("yok"), Err(Error::BackupNotFound("yok".into())));

    // Eklenen her disk yeni bir sıra numarası alır; aynı numara yeniden kullanılsa da.
    let mut c = cluster();
    let vm = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).unwrap().vmid;
    c.add_disk(vm, 1).unwrap();
    let first = c.guest(vm).unwrap().hw.disks[0].clone();
    c.remove_disk(vm, 1).unwrap();
    c.add_disk(vm, 1).unwrap();
    let second = c.guest(vm).unwrap().hw.disks[0].clone();
    assert_eq!((first.id, second.id), (1, 1));
    assert_ne!(first.serial, second.serial);

    // Yedekten sonra büyütülen asıl disk geri yüklenince kayıt da eski boyuta döner.
    let (was, storage) = (c.guest(vm).unwrap().disk_gb, c.guest(vm).unwrap().storage.clone());
    let used = |c: &Cluster| c.storages.iter().find(|s| s.id == storage).unwrap().used_gb;
    let before = used(&c);
    c.apply_grow(vm, was + 7);
    c.after_restore(vm, was);
    assert_eq!((c.guest(vm).unwrap().disk_gb, used(&c)), (was, before));
    c.after_restore(vm, 0);
    assert_eq!(c.guest(vm).unwrap().disk_gb, was, "boyutu bilinmeyen eski yedek kaydı değiştirmez");
    // Eski kayıtlarda alan yoktur: yalnızca asıl disk.
    let old: Backup = serde_json::from_str(r#"{"id":"e","vmid":103,"guest_name":"x","created":1,"size_mb":1}"#).unwrap();
    assert!(old.disks.is_empty());
}

#[test]
fn suspended_guests_keep_their_disk_and_hardware() {
    let mut c = cluster();
    let vm = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
    c.after_suspend(vm);
    let g = c.guest(vm).unwrap();
    assert!(g.hw.suspended && g.status == GuestStatus::Stopped && g.started_at.is_none() && g.ip.is_none());
    assert!(c.not_suspended(vm).is_err());
    assert!(matches!(c.add_disk(vm, 1), Err(Error::InvalidSpec(m)) if m.contains("askıda")));
    assert!(matches!(c.set_template(vm, true), Err(Error::InvalidSpec(m)) if m.contains("askıda")));
    // Klon askıda başlamaz: kendi bellek durumu yoktur.
    let copy = c.clone_guest(vm, "kopya").unwrap();
    assert!(!c.guest(copy).unwrap().hw.suspended);
    // Başlatılabilir; devam edince işaret kalkar.
    assert!(c.begin_action(vm, Action::Start, "u", NOW).is_ok());
    c.clear_suspended(vm);
    assert!(c.not_suspended(vm).is_ok());
}

#[test]
fn templates_cannot_change_and_linked_clones_protect_them() {
    let mut c = cluster();
    let vm = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Stopped).unwrap().vmid;
    let running = c.guests.iter().find(|g| g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
    assert!(matches!(c.clone_guest_as(vm, "bagli", true), Err(Error::InvalidSpec(m)) if m.contains("yalnızca şablondan")));
    assert!(matches!(c.set_template(running, true), Err(Error::RequiresStopped { .. })));
    assert!(matches!(c.set_template(ct, true), Err(Error::InvalidSpec(_))));

    c.update_guest(vm, &GuestPatch { backup_time: Some("03:30".into()), onboot: Some(true), ..patch() }, "u", NOW).unwrap();
    c.set_template(vm, true).unwrap();
    let t = c.guest(vm).unwrap();
    assert!(t.hw.template && !t.onboot && t.backup_time.is_none(), "şablon kendiliğinden açılmaz, yedeği zamanlanmaz");
    assert!(matches!(c.begin_action(vm, Action::Start, "u", NOW), Err(Error::InvalidSpec(m)) if m.contains("şablon")));
    assert!(c.not_template(vm).is_err() && c.not_template(running).is_ok());
    assert!(matches!(c.add_disk(vm, 1), Err(Error::InvalidSpec(m)) if m.contains("şablon")));

    let linked = c.clone_guest_as(vm, "bagli", true).unwrap();
    let full = c.clone_guest_as(vm, "tam", false).unwrap();
    assert_eq!((c.guest(linked).unwrap().hw.base, c.guest(linked).unwrap().hw.template), (Some(vm), false));
    assert_eq!((c.guest(full).unwrap().hw.base, c.guest(full).unwrap().hw.template), (None, false));
    // Bağlı klon varken şablon ne silinir ne şablonluktan çıkar; bağlı klon şablon olamaz.
    assert!(matches!(c.delete_guest(vm, "u", NOW), Err(Error::InvalidSpec(m)) if m.contains(&linked.to_string())));
    assert!(matches!(c.set_template(vm, false), Err(Error::InvalidSpec(m)) if m.contains("bağlı klonları")));
    assert!(matches!(c.set_template(linked, true), Err(Error::InvalidSpec(m)) if m.contains("bağlı klon")));
    c.delete_guest(linked, "u", NOW).unwrap();
    c.set_template(vm, false).unwrap();
    assert!(c.begin_action(vm, Action::Start, "u", NOW).is_ok());

    // İstekle şablon ya da bağlı klon ilişkisi kurulamaz.
    let like = c.guest(vm).unwrap().clone();
    let mut spec = GuestSpec {
        vmid: None, name: "sahte".into(), kind: like.kind, node: like.node, cores: 1, memory_mb: 512, disk_gb: 1,
        storage: like.storage, os: like.os, bridge: like.bridge, onboot: false, iso: None, hw: Default::default(),
    };
    spec.hw.template = true;
    spec.hw.base = Some(vm);
    let made = c.create_guest(&spec, "u", NOW).unwrap();
    assert_eq!((c.guest(made).unwrap().hw.template, c.guest(made).unwrap().hw.base), (false, None));
}

#[test]
fn retention_prunes_only_old_automatic_backups() {
    let mut c = cluster();
    c.update_guest(103, &GuestPatch { backup_keep: Some(2), ..patch() }, "u", NOW).unwrap();
    let mut add = |id: &str, vmid, auto| c.add_backup(Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW, size_mb: 1, auto, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    add("a1", 103, true);
    add("elle", 103, false);
    add("a2", 103, true);
    add("a3", 103, true);
    add("a4", 103, true);
    add("baska", 100, true);
    add("yetim", 999, true);
    // En yeni iki otomatik yedek (a4, a3) kalır; elle alınan, başka konuğunki ve yetim olan korunur.
    assert_eq!(c.prunable_backups(NOW), ["a2", "a1"]);
    c.remove_backup("a2").unwrap();
    c.remove_backup("a1").unwrap();
    assert!(c.prunable_backups(NOW).is_empty());
}

#[test]
fn gfs_and_locks_shape_what_is_pruned() {
    use crate::retention::{Gfs, DAY_SECS};
    let mut c = cluster();
    c.update_guest(103, &GuestPatch { backup_keep: Some(2), backup_weekly: Some(3), ..patch() }, "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().backup_gfs, Gfs { weekly: 3, monthly: 0, yearly: 0 });
    assert!(c.update_guest(103, &GuestPatch { backup_yearly: Some(100_000), ..patch() }, "u", NOW).is_err());
    // 30 gün boyunca her gün bir otomatik yedek (g0 en yeni).
    for day in (0..30u64).rev() {
        c.add_backup(Backup { id: format!("g{day}"), vmid: 103, guest_name: "x".into(), created: NOW - day * DAY_SECS, size_mb: 1, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    }
    let kept = |c: &Cluster| {
        let doomed = c.prunable_backups(NOW);
        c.backups.iter().filter(|b| !doomed.contains(&b.id)).map(|b| b.id.clone()).collect::<Vec<_>>()
    };
    let survivors = kept(&c);
    // Son 2 yedek + yedeği olan son 3 haftanın en yenileri (en yeni hafta zaten ilk ikide).
    assert_eq!(survivors.len(), 4, "{survivors:?}");
    assert!(survivors.starts_with(&["g0".to_string(), "g1".to_string()]));
    let weekly: Vec<u64> = survivors[2..].iter().map(|id| id[1..].parse().unwrap()).collect();
    assert_eq!(weekly[1] - weekly[0], 7, "haftalık yedekler birer hafta arayla: {weekly:?}");
    // Kilitli yedek, fazlalık olsa da süresi dolana kadar budanmaz; kilit bitince budanır.
    assert!(c.prunable_backups(NOW).contains(&"g20".to_string()));
    let until = c.lock_backup("g20", 10, NOW).unwrap();
    assert_eq!(until, NOW + 10 * DAY_SECS);
    assert!(!c.prunable_backups(NOW).contains(&"g20".to_string()));
    assert!(c.prunable_backups(until + 1).contains(&"g20".to_string()));
    // Kilit yalnızca uzatılır; silme kilitliyken reddedilir.
    assert!(c.lock_backup("g20", 5, NOW).unwrap_err().to_string().contains("kısaltılamaz"));
    assert_eq!(c.lock_backup("g20", 20, NOW).unwrap(), NOW + 20 * DAY_SECS);
    assert!(c.lock_backup("g20", 0, NOW).is_err() && c.lock_backup("g20", 99_999, NOW).is_err() && c.lock_backup("yok", 1, NOW).is_err());
    assert!(c.backup_deletable("g20", NOW).unwrap_err().to_string().contains("tarihine kadar kilitli"));
    c.backup_deletable("g20", NOW + 21 * DAY_SECS).unwrap();
    c.backup_deletable("g0", NOW).unwrap();
    assert!(c.backup("g20").unwrap().is_locked(NOW) && !c.backup("g0").unwrap().is_locked(NOW));
    // İşin aldığı yedekler işin kurallarına uyar.
    c.put_job(crate::jobs::BackupJob::new("gece", "03:00", crate::hw::EVERY_DAY, Vec::new(), 1).with_retention(Gfs { weekly: 0, monthly: 2, yearly: 0 }, 0).unwrap()).unwrap();
    for day in [0u64, 1, 40, 41, 80] {
        c.add_backup(Backup { id: format!("j{day}"), vmid: 100, guest_name: "x".into(), created: NOW - day * DAY_SECS, size_mb: 1, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: Some("gece".into()), machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    }
    let doomed = c.prunable_backups(NOW);
    let job_kept: Vec<&str> = c.backups.iter().filter(|b| b.job.is_some() && !doomed.contains(&b.id)).map(|b| b.id.as_str()).collect();
    assert_eq!(job_kept.len(), 2, "en yeni yedek ve önceki ayın en yenisi: {job_kept:?}");
    assert!(job_kept.contains(&"j0"));
}

#[test]
fn old_state_files_load_without_new_fields() {
    // v0.1 durum dosyasında tags/forwards/snapshots/backups alanları yoktu.
    let mut v = serde_json::to_value(cluster()).unwrap();
    v.as_object_mut().unwrap().remove("backups");
    for g in v["guests"].as_array_mut().unwrap() {
        for key in ["tags", "forwards", "snapshots", "iso", "backup_time", "backup_keep", "last_auto_backup"] {
            g.as_object_mut().unwrap().remove(key);
        }
    }
    let c: Cluster = serde_json::from_value(v).unwrap();
    assert!(c.backups.is_empty() && c.guests.iter().all(|g| g.tags.is_empty() && g.snapshots.is_empty() && g.backup_keep == DEFAULT_KEEP));
}

#[test]
fn log_records_instant_operations() {
    let mut c = cluster();
    c.log("pve-ist-01", "u", "backup delete vm-1".into(), &Ok(()), NOW, None);
    c.log("pve-ist-01", "u", "vm clone 103".into(), &Err("yer yok".into()), NOW, Some(103));
    assert_eq!((c.tasks[1].status, c.tasks[1].ended), (TaskStatus::Ok, Some(NOW)));
    assert_eq!((c.tasks[0].status, c.tasks[0].message.as_deref()), (TaskStatus::Failed, Some("yer yok")));
}

#[test]
fn text_parsers() {
    assert_eq!(parse_tags(" Prod, web  ci ,,"), ["prod", "web", "ci"]);
    assert!(parse_tags("").is_empty());
    assert_eq!(parse_forwards("8080:80, 2222:22"), Ok(vec![fwd(8080, 80), fwd(2222, 22)]));
    assert_eq!(parse_forwards("  "), Ok(vec![]));
    for bad in ["8080", "a:b", "8080:80:1", "99999:80", "8080-80"] {
        assert!(parse_forwards(bad).is_err(), "{bad}");
    }
    assert!(valid_tag("a-1") && !valid_tag("") && !valid_tag("a b") && !valid_tag(&"a".repeat(25)));
    assert!(valid_snapshot_name("a_b-1") && !valid_snapshot_name("a b") && !valid_snapshot_name("../x") && !valid_snapshot_name(&"a".repeat(33)));
}

#[test]
fn boot_plan_orders_guests_and_skips_running_ones() {
    let mut c = cluster();
    let stopped: Vec<VmId> = c.guests.iter().filter(|g| g.status == GuestStatus::Stopped).map(|g| g.vmid).collect();
    assert!(stopped.contains(&103) && stopped.contains(&202), "{stopped:?}");
    for g in c.guests.iter_mut() {
        g.onboot = false;
    }
    assert!(c.boot_plan().is_empty());
    let set = |c: &mut Cluster, vmid, order, delay| c.update_guest(vmid, &GuestPatch { onboot: Some(true), boot_order: Some(order), boot_delay: Some(delay), ..patch() }, "u", NOW);
    set(&mut c, 202, 0, 0).unwrap();
    set(&mut c, 103, 2, 30).unwrap();
    set(&mut c, 100, 1, 5).unwrap();
    assert_eq!(c.boot_plan(), [(103, 30), (202, 0)], "çalışan 100 listede yok; sıralı olan sırasızdan önce");
    c.guest_mut(100).unwrap().status = GuestStatus::Stopped;
    assert_eq!(c.boot_plan(), [(100, 5), (103, 30), (202, 0)]);
    c.set_template(103, true).unwrap();
    assert_eq!(c.boot_plan(), [(100, 5), (202, 0)], "şablon başlatılmaz");
    let t = c.begin_action(202, Action::Start, "u", NOW).unwrap();
    assert_eq!(c.boot_plan(), [(100, 5)], "kilitli konuk atlanır");
    c.complete_task(t, Ok(()), NOW).unwrap();
    assert!(set(&mut c, 100, 1000, 0).is_err() && set(&mut c, 100, 1, 601).is_err());
    assert_eq!(c.guest(100).unwrap().hw.boot_order, 1, "geçersiz değer kaydı değiştirmez");
}

#[test]
fn power_schedules_run_once_within_the_grace_window() {
    use crate::hw::{parse_schedules, SCHEDULE_GRACE_MIN};
    let mut c = cluster();
    let set = |c: &mut Cluster, vmid, text: &str| c.update_guest(vmid, &GuestPatch { schedules: Some(parse_schedules(text).unwrap()), ..patch() }, "u", NOW);
    set(&mut c, 103, "start@08:00@1-5, stop@20:00").unwrap();
    // 2026-11-24 Salı (1), 2026-11-28 Cumartesi (5).
    assert!(c.schedules_due("2026-11-24", 1, 479).is_empty(), "saati gelmeden çalışmaz");
    assert_eq!(c.schedules_due("2026-11-24", 1, 480), [(103, 0, Action::Start)]);
    assert_eq!(c.schedules_due("2026-11-24", 1, 480 + SCHEDULE_GRACE_MIN), [(103, 0, Action::Start)]);
    assert!(c.schedules_due("2026-11-24", 1, 481 + SCHEDULE_GRACE_MIN).is_empty(), "kaçan saat gün içinde telafi edilmez");
    assert!(c.schedules_due("2026-11-28", 5, 480).is_empty(), "hafta sonu çalışmaz");
    assert_eq!(c.schedules_due("2026-11-28", 5, 1200), [(103, 1, Action::Stop)]);

    c.mark_schedule(103, 0, "2026-11-24");
    assert!(c.schedules_due("2026-11-24", 1, 481).is_empty(), "aynı gün ikinci kez çalışmaz");
    assert_eq!(c.schedules_due("2026-11-25", 2, 481), [(103, 0, Action::Start)], "ertesi gün yeniden");
    // Listeyi yeniden kaydetmek değişmeyen satırın kaydını korur; değişen satır yenidir.
    set(&mut c, 103, "stop@20:00, start@08:00@1-5").unwrap();
    assert!(c.schedules_due("2026-11-24", 1, 481).is_empty());
    set(&mut c, 103, "start@08:01@1-5").unwrap();
    assert_eq!(c.schedules_due("2026-11-24", 1, 481), [(103, 0, Action::Start)]);

    let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    assert!(c.schedules_due("2026-11-24", 1, 481).is_empty(), "kilitli konuk bekletilir");
    c.complete_task(t, Ok(()), NOW).unwrap();
    c.mark_schedule(103, 9, "2026-11-24");
    c.mark_schedule(999, 0, "2026-11-24");
    assert!(set(&mut c, 103, "").is_ok() && c.guest(103).unwrap().hw.schedules.is_empty(), "boş liste zamanlamayı kaldırır");
    let many = vec![parse_schedules("stop@01:00").unwrap().remove(0); 9];
    assert!(c.update_guest(103, &GuestPatch { schedules: Some(many), ..patch() }, "u", NOW).is_err());
}

#[test]
fn rate_limits_need_a_stopped_vm_and_a_bridged_nic() {
    let mut c = cluster();
    let limits = |disk, iops, net| GuestPatch { disk_mbps: Some(disk), disk_iops: Some(iops), net_mbps: Some(net), ..patch() };
    assert!(matches!(c.update_guest(100, &limits(50, 0, 0), "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
    assert!(c.update_guest(100, &limits(0, 0, 0), "u", NOW).is_ok(), "aynı değer değişiklik sayılmaz");
    c.update_guest(103, &limits(50, 2000, 100), "u", NOW).unwrap();
    let hw = c.guest(103).unwrap().hw.clone();
    assert_eq!((hw.disk_mbps, hw.disk_iops, hw.net_mbps), (50, 2000, 100));
    // Köprüden NAT'a geçmek, ağ sınırı dururken reddedilir; sınırla birlikte kaldırılırsa olur.
    let nat = || GuestPatch { bridge: Some("nat".into()), ..patch() };
    assert!(c.update_guest(103, &nat(), "u", NOW).unwrap_err().to_string().contains("köprüye bağlı"));
    assert_eq!(c.guest(103).unwrap().bridge, "vmbr0");
    c.update_guest(103, &GuestPatch { net_mbps: Some(0), ..nat() }, "u", NOW).unwrap();
    assert!(c.update_guest(103, &limits(50, 2000, 10), "u", NOW).is_err(), "NAT'ta ağ sınırı konamaz");
    assert!(c.update_guest(103, &GuestPatch { nics: Some(vec!["vmbr1".into()]), ..limits(50, 2000, 10) }, "u", NOW).is_ok(), "köprüdeki ek kart yeter");
    assert!(c.update_guest(103, &limits(10_001, 0, 0), "u", NOW).is_err());
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
    assert!(c.update_guest(ct, &limits(1, 0, 0), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
}

#[test]
fn devices_can_be_added_to_a_running_vm_only_when_hot() {
    let mut c = Cluster::demo(NOW);
    let find = |c: &Cluster, kind, status| c.guests.iter().find(|g| g.kind == kind && g.status == status).unwrap().vmid;
    let (running, stopped, paused) = (find(&c, GuestKind::Vm, GuestStatus::Running), find(&c, GuestKind::Vm, GuestStatus::Stopped), find(&c, GuestKind::Vm, GuestStatus::Paused));
    let ct = find(&c, GuestKind::Ct, GuestStatus::Running);

    assert!(matches!(c.add_disk(running, 5), Err(Error::RequiresStopped { .. })));
    assert_eq!(c.add_disk_hot(running, 5).unwrap(), 1);
    assert_eq!(c.add_disk_hot(stopped, 5).unwrap(), 1, "kapalı konukta da çalışır");
    assert!(matches!(c.add_disk_hot(paused, 5), Err(Error::RequiresStopped { .. })), "duraklatılmış konuğa takılmaz");
    assert!(c.add_disk_hot(ct, 5).is_err(), "konteynere takılmaz");
    // Çıkarmak her zaman kapalı konuk ister; takılamayan diskin kaydı ise durum denetimi olmadan geri alınır.
    assert!(matches!(c.remove_disk(running, 1), Err(Error::RequiresStopped { .. })));
    assert_eq!(c.forget_disk(running, 1).unwrap().size_gb, 5);
    assert!(c.guest(running).unwrap().hw.disks.is_empty() && c.forget_disk(running, 1).is_err());

    assert!(matches!(c.add_nic(running, "vmbr1", false), Err(Error::RequiresStopped { .. })));
    assert_eq!(c.add_nic(running, "vmbr1", true).unwrap(), 1);
    assert_eq!(c.add_nic(running, "nat", true).unwrap(), 2);
    assert!(c.add_nic(running, "br0,script=/x", true).is_err());
    assert_eq!(c.add_nic(running, "vmbr2", true).unwrap(), 3);
    assert!(c.add_nic(running, "vmbr3", true).unwrap_err().to_string().contains("en fazla 3"));
    assert_eq!(c.guest(running).unwrap().hw.nics, ["vmbr1", "nat", "vmbr2"]);
    c.forget_nic(running);
    assert_eq!(c.guest(running).unwrap().hw.nics, ["vmbr1", "nat"]);
    assert_eq!(c.add_nic(stopped, "nat", false).unwrap(), 1);
    assert!(c.add_nic(ct, "nat", true).is_err() && c.add_nic(paused, "nat", true).is_err());
    c.set_template(stopped, true).unwrap();
    assert!(c.add_nic(stopped, "nat", false).is_err(), "şablonun donanımı değişmez");
}

#[test]
fn guests_move_between_storages_with_their_share() {
    use crate::model::{Storage, StorageKind};
    let mut c = cluster();
    let node = c.guest(103).unwrap().node.clone();
    c.storages.push(Storage { id: "hizli".into(), node: Some(node.clone()), kind: StorageKind::Dir, content: vec!["disk".into()], used_gb: 10, total_gb: 1000 });
    c.storages.push(Storage { id: "dar".into(), node: Some(node.clone()), kind: StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 50 });
    c.storages.push(Storage { id: "baska-dugum".into(), node: Some("yok".into()), kind: StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 1000 });
    c.add_disk(103, 20).unwrap();
    let size = c.guest(103).unwrap().total_disk_gb();
    let from = c.guest(103).unwrap().storage.clone();
    let used = |c: &Cluster, id: &str| c.storages.iter().find(|s| s.id == id && s.available_on(&node)).unwrap().used_gb;
    let before = used(&c, &from);

    assert!(c.check_storage_move(103, &from).unwrap_err().to_string().contains("zaten"));
    assert!(c.check_storage_move(103, "nfs-yedek").unwrap_err().to_string().contains("konuk diski tutmuyor"));
    assert!(matches!(c.check_storage_move(103, "dar"), Err(Error::InsufficientStorage { .. })));
    assert!(c.check_storage_move(103, "baska-dugum").is_err() && c.check_storage_move(103, "yok").is_err());
    c.check_storage_move(103, "hizli").unwrap();
    c.apply_storage_move(103, "hizli");
    assert_eq!((c.guest(103).unwrap().storage.as_str(), used(&c, "hizli"), used(&c, &from)), ("hizli", 10 + size, before - size), "ek diskler dahil bütün pay taşınır");

    // Şablon ve bağlı klon: disk yolları birbirine bağlı olduğu için taşınmaz; askıdaki konuk da taşınmaz.
    c.set_template(103, true).unwrap();
    let linked = c.clone_guest_as(103, "bagli", true).unwrap();
    assert!(c.check_storage_move(103, &from).unwrap_err().to_string().contains("bağlı klonları olan şablon"));
    assert!(c.check_storage_move(linked, &from).unwrap_err().to_string().contains("bağlı klon taşınamaz"));
    let full = c.clone_guest_as(103, "tam", false).unwrap();
    c.check_storage_move(full, &from).unwrap();
    c.guest_mut_for_import(full).unwrap().hw.suspended = true;
    assert!(c.check_storage_move(full, &from).is_err(), "askıdaki konuğun bellek durumu şimdiki diskine aittir");
}

#[test]
fn guests_without_a_recent_backup_are_unprotected() {
    use crate::retention::DAY_SECS;
    let mut c = cluster();
    let all = c.guests.len();
    assert_eq!(c.unprotected(NOW, 7).len(), all, "hiç yedeği olmayan her konuk korumasızdır");
    let add = |c: &mut Cluster, id: &str, vmid, age_days: u64| c.add_backup(Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW - age_days * DAY_SECS, size_mb: 1, auto: false, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    add(&mut c, "yeni", 100, 1);
    add(&mut c, "eski", 101, 30);
    add(&mut c, "daha-eski", 101, 60);
    let list = c.unprotected(NOW, 7);
    assert!(!list.iter().any(|(vmid, _, _)| *vmid == 100), "bir gün önce yedeklenen konuk korunuyor");
    assert_eq!(list.iter().find(|(vmid, _, _)| *vmid == 101).unwrap().2, Some(NOW - 30 * DAY_SECS), "en yeni yedeğin anı döner");
    assert!(!c.unprotected(NOW, 45).iter().any(|(vmid, _, _)| *vmid == 101), "süre uzayınca korunuyor sayılır");
    c.set_template(103, true).unwrap();
    assert!(!c.unprotected(NOW, 7).iter().any(|(vmid, _, _)| *vmid == 103), "şablonlar sayılmaz");
}

#[test]
fn archived_backups_return_as_manual_backups() {
    let mut c = cluster();
    let b = |id: &str, created| Backup { id: id.into(), vmid: 103, guest_name: "x".into(), created, size_mb: 5, auto: true, offsite: true, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: true, job: Some("gece".into()), machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 1 };
    c.add_backup(b("yeni", NOW));
    c.archive_backup(b("eski", NOW - 100));
    c.archive_backup(b("daha-eski", NOW - 200));
    c.archive_backup(b("eski", NOW - 100));
    assert_eq!(c.archive.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), ["eski", "daha-eski"], "yinelenmez, en yeni başta");
    c.unarchive_backup("daha-eski").unwrap();
    let back = c.backup("daha-eski").unwrap();
    assert_eq!((back.auto, back.offsite, back.job.clone(), back.copies, back.offsite_bound), (false, false, None, 1, true));
    assert_eq!(c.backups.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), ["yeni", "daha-eski"], "tarih sırası korunur");
    assert!(matches!(c.unarchive_backup("yok"), Err(Error::BackupNotFound(_))));
    c.drop_archived("eski");
    assert!(c.archive.is_empty());
}

#[test]
fn boot_test_checks_are_validated_and_change_while_running() {
    let mut c = cluster();
    c.update_guest(100, &GuestPatch { test_port: Some(22), test_cmd: Some(" /usr/local/bin/saglik.sh --hizli ".into()), ..patch() }, "u", NOW).unwrap();
    let hw = c.guest(100).unwrap().hw.clone();
    assert_eq!((hw.test_port, hw.test_cmd.as_str(), hw.test_fwd), (22, "/usr/local/bin/saglik.sh --hizli", 0));
    assert!(c.update_guest(100, &GuestPatch { test_cmd: Some("saglik.sh".into()), ..patch() }, "u", NOW).unwrap_err().to_string().contains("mutlak bir program yoluyla"));
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
    assert!(c.update_guest(ct, &GuestPatch { test_port: Some(22), ..patch() }, "u", NOW).is_err());
    c.set_test_forward(100, 40123);
    assert_eq!(c.guest(100).unwrap().hw.test_fwd, 40123);
    c.update_guest(100, &GuestPatch { test_port: Some(0), test_cmd: Some(String::new()), ..patch() }, "u", NOW).unwrap();
}

#[test]
fn rpo_overrides_the_general_backup_age_and_anomalies_are_flagged() {
    use crate::retention::DAY_SECS;
    let mut c = cluster();
    let total = c.guests.iter().filter(|g| !g.hw.template).count();
    assert_eq!(c.unprotected(NOW, 0).len(), 0, "genel ayar kapalı, hedefi olan konuk yok");
    // 100'ün hedefi 4 saat: genel ayar kapalıyken de denetlenir.
    c.update_guest(100, &GuestPatch { rpo_hours: Some(4), ..patch() }, "u", NOW).unwrap();
    assert_eq!(c.unprotected(NOW, 0).iter().map(|u| u.0).collect::<Vec<_>>(), [100]);
    assert!(c.update_guest(100, &GuestPatch { rpo_hours: Some(9000), ..patch() }, "u", NOW).is_err());
    let add = |c: &mut Cluster, id: &str, vmid, age_secs: u64, new_mb: Option<u64>| {
        c.add_backup(Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW - age_secs, size_mb: 20_000, auto: false, offsite: false, new_mb, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    };
    // 5 saatlik yedek 4 saatlik hedefi kaçırır, 7 günlük genel ayarı kaçırmaz.
    add(&mut c, "a", 100, 5 * 3600, None);
    add(&mut c, "b", 103, 5 * 3600, None);
    assert!(c.unprotected(NOW, 7).iter().any(|u| u.0 == 100) && !c.unprotected(NOW, 7).iter().any(|u| u.0 == 103));
    assert_eq!(c.unprotected(NOW, 7).len(), total - 1);
    add(&mut c, "c", 100, 3600, None);
    assert!(!c.unprotected(NOW, 0).iter().any(|u| u.0 == 100));

    // Olağan dışı değişim: üç olağan artımlı yedekten sonra diskin yarısı değişir.
    for (i, mb) in [100u64, 140, 90].iter().enumerate() {
        add(&mut c, &format!("n{i}"), 103, 4000 - i as u64 * DAY_SECS / 100, Some(*mb));
    }
    assert!(c.backups.iter().all(|b| b.suspicious.is_none()));
    add(&mut c, "sifreli", 103, 10, Some(10_000));
    let flagged = c.backups.iter().find(|b| b.id == "sifreli").unwrap();
    assert!(flagged.suspicious.as_deref().is_some_and(|n| n.contains("fidye yazılımı")), "{:?}", flagged.suspicious);
    add(&mut c, "baska", 100, 5, Some(10_000));
    assert!(c.backups[0].suspicious.is_none(), "başka konuğun geçmişi sayılmaz");
}

#[test]
fn nested_virtualization_is_a_hardware_change() {
    let mut c = cluster();
    let on = GuestPatch { nested: Some(true), ..patch() };
    assert!(matches!(c.update_guest(100, &on, "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
    c.update_guest(103, &on, "u", NOW).unwrap();
    assert!(c.guest(103).unwrap().hw.nested);
    assert!(c.update_guest(103, &GuestPatch { cpu_type: Some("qemu64".into()), ..patch() }, "u", NOW).is_err(), "uzantıyı taşıyamayan model reddedilir");
    c.update_guest(103, &GuestPatch { cpu_type: Some("qemu64".into()), nested: Some(false), ..patch() }, "u", NOW).unwrap();
}

#[test]
fn nic_guard_is_a_vm_option_that_changes_while_running() {
    let mut c = cluster();
    // 100 çalışıyor: koruma donanım değişikliği değildir, kapatmadan açılır.
    c.update_guest(100, &GuestPatch { nic_guard: Some(true), ..patch() }, "u", NOW).unwrap();
    assert!(c.guest(100).unwrap().hw.nic_guard);
    c.update_guest(100, &GuestPatch { nic_guard: Some(false), ..patch() }, "u", NOW).unwrap();
    assert!(!c.guest(100).unwrap().hw.nic_guard);
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
    assert!(c.update_guest(ct, &GuestPatch { nic_guard: Some(true), ..patch() }, "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
}

#[test]
fn dynamic_memory_needs_a_balloon_and_sane_bounds() {
    let mut c = cluster();
    assert!(c.set_dynamic_memory(103, 1024).unwrap_err().to_string().contains("bellek balonu yok"));
    c.update_guest(103, &GuestPatch { balloon: Some(true), ..patch() }, "u", NOW).unwrap();
    let total = c.guest(103).unwrap().mem_total_mb;
    assert!(c.set_dynamic_memory(103, 100).is_err() && c.set_dynamic_memory(103, total + 1).is_err());
    c.set_dynamic_memory(103, 1024).unwrap();
    assert!(c.set_balloon_target(103, 2048).unwrap_err().to_string().contains("dinamik bellek açık"));
    c.set_dynamic_target(103, 1536);
    assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 1536);
    c.set_dynamic_target(103, total);
    assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0, "tamamı 0 olarak saklanır");
    // Kapatınca elle hedef yeniden verilebilir; kapalıyken hesaplanan hedef kaydedilmez.
    c.set_dynamic_memory(103, 0).unwrap();
    c.set_dynamic_target(103, 1536);
    assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0);
    c.set_balloon_target(103, 2048).unwrap();
    // Balon çıkarılınca dinamik bellek de kapanır.
    c.set_balloon_target(103, 0).unwrap();
    c.set_dynamic_memory(103, 1024).unwrap();
    c.update_guest(103, &GuestPatch { balloon: Some(false), ..patch() }, "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.mem_min_mb, 0);
}

#[test]
fn production_checkpoints_need_a_stopped_guest_to_roll_back() {
    let mut c = cluster();
    c.add_snapshot(100, "bellekli", NOW);
    c.add_snapshot_kind(100, "uretim", NOW, true, true);
    let snaps = c.guest(100).unwrap().snapshots.clone();
    assert_eq!(snaps.iter().map(|s| (s.name.as_str(), s.disk_only, s.quiesced)).collect::<Vec<_>>(), [("bellekli", false, false), ("uretim", true, true)]);
    // 100 çalışıyor: bellekli görüntüye dönülebilir, belleksize dönmek için kapatmak gerekir.
    c.check_rollback_state(100, "bellekli").unwrap();
    assert!(matches!(c.check_rollback_state(100, "uretim"), Err(Error::RequiresStopped { .. })));
    c.add_snapshot_kind(103, "uretim", NOW, true, false);
    c.check_rollback_state(103, "uretim").unwrap();
    // Eski kayıtlar (alanlar yok) bellekli sayılır.
    let old: Snapshot = serde_json::from_str(r#"{"name":"eski","created":1}"#).unwrap();
    assert!(!old.disk_only && !old.quiesced);
}

#[test]
fn instantly_recovered_guests_pin_their_backup() {
    let mut c = cluster();
    c.update_guest(103, &GuestPatch { backup_keep: Some(1), ..patch() }, "u", NOW).unwrap();
    for (i, id) in ["eski", "yeni"].iter().enumerate() {
        c.add_backup(Backup { id: id.to_string(), vmid: 103, guest_name: "x".into(), created: NOW + i as u64, size_mb: 1, auto: true, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested: None, copies: 0 });
    }
    assert_eq!(c.prunable_backups(NOW), ["eski"]);
    assert!(c.check_detach(100).unwrap_err().to_string().contains("üzerinde çalışmıyor"));
    // 100 numaralı konuk "eski" yedeğinin üzerinde çalışıyor: yedek silinemez ve budanmaz.
    c.guest_mut_for_import(100).unwrap().hw.instant = Some("eski".into());
    assert!(c.backup_deletable("eski", NOW).unwrap_err().to_string().contains("bu yedeğin üzerinde çalışıyor"));
    assert!(c.prunable_backups(NOW).is_empty());
    c.backup_deletable("yeni", NOW).unwrap();
    c.check_detach(100).unwrap();
    c.apply_detach(100);
    assert_eq!(c.guest(100).unwrap().hw.instant, None);
    c.backup_deletable("eski", NOW).unwrap();
    assert_eq!(c.prunable_backups(NOW), ["eski"]);
}

#[test]
fn imported_disks_keep_their_numbers_and_serials() {
    use crate::hw::ExtraDisk;
    let mut c = cluster();
    let used = |c: &Cluster| c.storages.iter().map(|s| s.used_gb).sum::<u64>();
    let before = used(&c);
    c.add_disk_exact(103, &ExtraDisk { id: 5, size_gb: 10, serial: 9 }).unwrap();
    c.add_disk_exact(103, &ExtraDisk { id: 2, size_gb: 4, serial: 3 }).unwrap();
    let hw = c.guest(103).unwrap().hw.clone();
    assert_eq!(hw.disks.iter().map(|d| (d.id, d.serial)).collect::<Vec<_>>(), [(2, 3), (5, 9)], "numara ve seri korunur, sıralı durur");
    assert_eq!((hw.disk_seq, used(&c) - before), (9, 14), "sonraki disk çakışmayan bir seri alır; yer ayrılır");
    assert_eq!(c.add_disk(103, 1).unwrap(), 1);
    assert_eq!(c.guest(103).unwrap().hw.disks.iter().find(|d| d.id == 1).unwrap().serial, 10);
    for bad in [ExtraDisk { id: 5, size_gb: 1, serial: 1 }, ExtraDisk { id: 0, size_gb: 1, serial: 1 }, ExtraDisk { id: 99, size_gb: 1, serial: 1 }, ExtraDisk { id: 3, size_gb: 0, serial: 1 }] {
        assert!(c.add_disk_exact(103, &bad).is_err(), "{bad:?}");
    }
    assert!(matches!(c.add_disk_exact(103, &ExtraDisk { id: 3, size_gb: 1_000_000, serial: 1 }), Err(Error::InsufficientStorage { .. })));
}

#[test]
fn cpu_type_and_balloon_follow_hardware_rules() {
    let mut c = cluster();
    let set = |cpu: &str, balloon: bool| GuestPatch { cpu_type: Some(cpu.into()), balloon: Some(balloon), ..patch() };
    assert!(matches!(c.update_guest(100, &set("Haswell", true), "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
    c.update_guest(100, &set("", false), "u", NOW).unwrap();
    assert!(c.update_guest(103, &set("Pentium9", false), "u", NOW).unwrap_err().to_string().contains("bilinmeyen işlemci türü"));
    assert!(c.set_balloon_target(103, 4096).unwrap_err().to_string().contains("bellek balonu yok"));
    c.update_guest(103, &set("Haswell", true), "u", NOW).unwrap();
    let total = c.guest(103).unwrap().mem_total_mb;
    assert_eq!((c.guest(103).unwrap().hw.cpu_type.as_str(), c.guest(103).unwrap().hw.balloon), ("Haswell", true));
    // Hedef: alt sınır ile bellek arasında; 0 ya da belleğin tamamı balonu boşaltır.
    assert_eq!(c.set_balloon_target(103, 4096).unwrap(), 0);
    assert_eq!(c.set_balloon_target(103, 8192).unwrap(), 4096, "önceki hedef döner");
    assert!(c.set_balloon_target(103, 100).is_err() && c.set_balloon_target(103, total + 1).is_err());
    c.set_balloon_target(103, total).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0);
    c.set_balloon_target(103, 4096).unwrap();
    c.restore_balloon_target(103, 8192);
    assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 8192);
    // Bellek hedefin altına inerse ya da balon kaldırılırsa hedef sıfırlanır.
    c.update_guest(103, &GuestPatch { memory_mb: Some(4096), ..patch() }, "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.balloon_mb, 0);
    c.set_balloon_target(103, 2048).unwrap();
    c.update_guest(103, &set("", false), "u", NOW).unwrap();
    let hw = c.guest(103).unwrap().hw.clone();
    assert_eq!((hw.cpu_type.as_str(), hw.balloon, hw.balloon_mb), ("", false, 0));
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
    assert!(c.update_guest(ct, &set("Haswell", false), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    assert!(c.set_balloon_target(ct, 512).is_err());
}

#[test]
fn cloud_init_is_edited_on_a_stopped_vm() {
    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGtMF0qv2yBAZ8lfmTtk1mKkZHjPl7Yj8d0Lr0oQ1X2w ali@dizustu";
    let mut c = cluster();
    let ci = |key: &str, ip: &str| GuestPatch { ci_user: Some("ali".into()), ci_ssh_key: Some(key.into()), ci_ip: Some(ip.into()), ci_gw: Some(String::new()), ci_dns: Some(String::new()), ..patch() };
    assert!(matches!(c.update_guest(100, &ci(KEY, ""), "u", NOW), Err(Error::RequiresStopped { .. })), "çalışan konukta değişmez");
    c.update_guest(103, &ci(KEY, "10.0.0.5/24"), "u", NOW).unwrap();
    let hw = c.guest(103).unwrap().hw.clone();
    assert_eq!((hw.ci_user.as_deref(), hw.ci_ip.as_deref(), hw.ci_gen, hw.cloud_init()), (Some("ali"), Some("10.0.0.5/24"), 1, true));
    // Aynı değerleri yeniden kaydetmek sürümü artırmaz; başka bir ayar değişikliği de artırmaz.
    c.update_guest(103, &ci(KEY, "10.0.0.5/24"), "u", NOW).unwrap();
    c.update_guest(103, &GuestPatch { cores: Some(2), ..patch() }, "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.ci_gen, 1);
    assert!(c.update_guest(103, &ci(KEY, "10.0.0.500/24"), "u", NOW).unwrap_err().to_string().contains("geçersiz cloud-init adresi"));
    assert!(c.update_guest(103, &ci("ssh-rsa kisa", ""), "u", NOW).unwrap_err().to_string().contains("geçersiz SSH"));
    assert_eq!(c.guest(103).unwrap().hw.ci_gen, 1, "reddedilen değişiklik kaydı bozmaz");
    // Anahtarı boşaltmak cloud-init'i kapatır.
    c.update_guest(103, &ci("", "10.0.0.5/24"), "u", NOW).unwrap();
    let hw = c.guest(103).unwrap().hw.clone();
    assert_eq!((hw.cloud_init(), hw.ci_user, hw.ci_ip, hw.ci_gen), (false, None, None, 2));
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
    assert!(c.update_guest(ct, &ci(KEY, ""), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
}

#[test]
fn vlan_tags_need_a_stopped_vm_on_a_bridge() {
    let mut c = cluster();
    let vlan = |v| GuestPatch { vlan: Some(v), ..patch() };
    assert!(matches!(c.update_guest(100, &vlan(20), "u", NOW), Err(Error::RequiresStopped { .. })));
    c.update_guest(103, &vlan(20), "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.vlan, 20);
    assert!(c.update_guest(103, &vlan(4095), "u", NOW).is_err());
    // NAT'a geçmek etiket dururken reddedilir; etiketle birlikte kaldırılırsa olur.
    let nat = || GuestPatch { bridge: Some("nat".into()), ..patch() };
    assert!(c.update_guest(103, &nat(), "u", NOW).unwrap_err().to_string().contains("VLAN etiketi"));
    c.update_guest(103, &GuestPatch { vlan: Some(0), ..nat() }, "u", NOW).unwrap();
    assert!(c.update_guest(103, &vlan(5), "u", NOW).is_err(), "NAT'taki karta etiket konmaz");
    // Ek kartlar: etiket tanımın içindedir.
    c.update_guest(103, &GuestPatch { nics: Some(vec!["vmbr1:30".into(), "nat".into()]), ..patch() }, "u", NOW).unwrap();
    assert!(c.update_guest(103, &GuestPatch { nics: Some(vec!["nat:30".into()]), ..patch() }, "u", NOW).is_err());
    assert_eq!(c.add_nic(103, "vmbr2:4094", false).unwrap(), 3);
    c.forget_nic(103);
    assert!(c.add_nic(103, "vmbr2:0", false).is_err());
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
    assert!(c.update_guest(ct, &vlan(5), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
}

#[test]
fn shares_and_other_hardware_cannot_change_while_suspended() {
    use crate::hw::parse_shares;
    let mut c = cluster();
    let shares = |text: &str| GuestPatch { shares: Some(parse_shares(text).unwrap()), ..patch() };
    assert!(matches!(c.update_guest(100, &shares("ortak"), "u", NOW), Err(Error::RequiresStopped { .. })));
    c.update_guest(103, &shares("ortak, veri:ro"), "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.shares.iter().map(ToString::to_string).collect::<Vec<_>>(), ["ortak", "veri:ro"]);
    assert!(c.update_guest(103, &shares("ortak, ortak"), "u", NOW).is_err());
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
    assert!(c.update_guest(ct, &shares("ortak"), "u", NOW).is_err());

    // Askıdaki konuk: donanım değişmez (bellek durumu şimdiki donanımı bekler); ad ve etiket değişir.
    c.after_suspend(103);
    assert!(c.update_guest(103, &shares(""), "u", NOW).unwrap_err().to_string().contains("askıda"));
    assert!(c.update_guest(103, &GuestPatch { cores: Some(2), ..patch() }, "u", NOW).unwrap_err().to_string().contains("askıda"));
    assert!(c.update_guest(103, &shares("ortak, veri:ro"), "u", NOW).is_ok(), "aynı liste değişiklik sayılmaz");
    assert!(c.update_guest(103, &GuestPatch { tags: Some(vec!["x".into()]), ..patch() }, "u", NOW).is_ok());
    c.clear_suspended(103);
    assert!(c.update_guest(103, &shares(""), "u", NOW).is_ok());
}

#[test]
fn scheduled_boot_tests_pick_the_newest_backup_that_is_due() {
    use crate::retention::DAY_SECS;
    let mut c = cluster();
    assert_eq!(c.next_backup_to_test(NOW, 7), None, "yedek yok");
    let add = |c: &mut Cluster, id: &str, vmid, age_days: u64, tested_days_ago: Option<u64>| {
        let tested = tested_days_ago.map(|d| BackupTest { at: NOW - d * DAY_SECS, ok: true, note: String::new() });
        c.add_backup(Backup { id: id.into(), vmid, guest_name: "x".into(), created: NOW - age_days * DAY_SECS, size_mb: 5, auto: false, offsite: false, new_mb: None, disks: Vec::new(), disk_gb: 0, quiesced: None, cbt: false, offsite_bound: false, job: None, machine: false, spec: None, suspicious: None, locked_until: None, tested, copies: 0 });
    };
    // 100: en yeni yedeği 2 gün önce sınandı (sırası gelmedi); eski yedeği hiç sınanmadı ama en yenisi değil.
    add(&mut c, "vm-100-eski", 100, 9, None);
    add(&mut c, "vm-100-yeni", 100, 1, Some(2));
    assert_eq!(c.next_backup_to_test(NOW, 7), None);
    // 103: en yeni yedeği 8 gün önce sınandı; 101: hiç sınanmadı (önce o).
    add(&mut c, "vm-103-1", 103, 3, Some(8));
    assert_eq!(c.next_backup_to_test(NOW, 7).as_deref(), Some("vm-103-1"));
    add(&mut c, "vm-101-1", 101, 3, None);
    assert_eq!(c.next_backup_to_test(NOW, 7).as_deref(), Some("vm-101-1"));
    // Konteyner yedekleri açılarak sınanmaz.
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
    c.backups.retain(|b| b.vmid == 100);
    add(&mut c, &format!("ct-{ct}-1"), ct, 1, None);
    assert_eq!(c.next_backup_to_test(NOW, 7), None);
}

#[test]
fn cores_are_added_to_a_running_vm_only_up_to_its_limit() {
    let mut c = cluster();
    let limit = |max: u32| GuestPatch { max_cores: Some(max), ..patch() };
    let node_cores = c.node(&c.guest(103).unwrap().node.clone()).unwrap().cores;
    // Sınır yalnızca kapalı sanal makinede, çekirdek sayısı ile düğümün çekirdekleri arasında verilir.
    assert!(matches!(c.update_guest(100, &limit(4), "u", NOW), Err(Error::RequiresStopped { .. })));
    c.update_guest(103, &GuestPatch { cores: Some(2), ..patch() }, "u", NOW).unwrap();
    for bad in [1, node_cores + 1] {
        assert!(c.update_guest(103, &limit(bad), "u", NOW).unwrap_err().to_string().contains("en fazla çekirdek 2 ile"), "{bad}");
    }
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
    assert!(c.update_guest(ct, &limit(4), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    c.update_guest(103, &limit(4), "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.max_cores, 4);
    // Çekirdek sayısı sınırın üstüne çıkarılamaz; sınırla birlikte değiştirilebilir.
    assert!(c.update_guest(103, &GuestPatch { cores: Some(5), ..patch() }, "u", NOW).unwrap_err().to_string().contains("en fazla çekirdek"));
    c.update_guest(103, &GuestPatch { cores: Some(3), max_cores: Some(4), ..patch() }, "u", NOW).unwrap();
    c.update_guest(103, &GuestPatch { cores: Some(2), ..patch() }, "u", NOW).unwrap();

    // Kapalı konuğa, sınırı olmayan konuğa ve konteynere çalışırken eklenmez.
    assert!(c.check_core_add(103, 3).unwrap_err().to_string().contains("çalışmıyor"));
    assert!(c.check_core_add(100, 64).unwrap_err().to_string().contains("çalışırken çekirdek ekleme kapalı"));
    let running_ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Running).unwrap().vmid;
    assert!(c.check_core_add(running_ct, 64).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));

    c.guest_mut(103).unwrap().status = GuestStatus::Running;
    assert_eq!(c.check_core_add(103, 4).unwrap(), 2, "şimdiki sayı döner");
    assert!(c.check_core_add(103, 2).unwrap_err().to_string().contains("yalnızca artırılır"));
    assert!(c.check_core_add(103, 1).unwrap_err().to_string().contains("yalnızca artırılır"));
    assert!(c.check_core_add(103, 5).unwrap_err().to_string().contains("en fazla 4 çekirdeğe"));
    c.note_core_added(103);
    assert_eq!(c.guest(103).unwrap().cores, 3);
    // Çalışırken sınır değişmez; 0 (kapalıyken) özelliği kapatır.
    assert!(matches!(c.update_guest(103, &limit(0), "u", NOW), Err(Error::RequiresStopped { .. })));
    c.guest_mut(103).unwrap().status = GuestStatus::Stopped;
    c.update_guest(103, &limit(0), "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.max_cores, 0);
}

#[test]
fn memory_is_added_to_a_running_vm_in_steps_up_to_its_limit() {
    let mut c = cluster();
    let limit = |max: u64| GuestPatch { max_memory_mb: Some(max), ..patch() };
    let node_mem = c.node(&c.guest(103).unwrap().node.clone()).unwrap().mem_total_mb;
    // Sınır yalnızca kapalı sanal makinede, bellek ile düğümün belleği arasında verilir.
    assert!(matches!(c.update_guest(100, &limit(65536), "u", NOW), Err(Error::RequiresStopped { .. })));
    c.update_guest(103, &GuestPatch { memory_mb: Some(2048), ..patch() }, "u", NOW).unwrap();
    for bad in [1024, node_mem + 1] {
        assert!(c.update_guest(103, &limit(bad), "u", NOW).unwrap_err().to_string().contains("en fazla bellek 2048 MB ile"), "{bad}");
    }
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Stopped).unwrap().vmid;
    assert!(c.update_guest(ct, &limit(4096), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    c.update_guest(103, &limit(4096), "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.max_mem_mb, 4096);
    assert!(c.update_guest(103, &GuestPatch { memory_mb: Some(8192), ..patch() }, "u", NOW).unwrap_err().to_string().contains("en fazla bellek"));

    assert!(c.check_memory_add(103, 3072).unwrap_err().to_string().contains("çalışmıyor"));
    assert!(c.check_memory_add(100, 1 << 30).unwrap_err().to_string().contains("çalışırken bellek ekleme kapalı"));
    c.guest_mut(103).unwrap().status = GuestStatus::Running;
    assert_eq!(c.check_memory_add(103, 3072).unwrap(), 1024, "eklenecek miktar döner");
    assert!(c.check_memory_add(103, 2048).unwrap_err().to_string().contains("yalnızca artırılır"));
    assert!(c.check_memory_add(103, 4224).unwrap_err().to_string().contains("en fazla 4096 MB"));
    // Adım dışı değerde en yakın geçerli değer önerilir.
    assert!(c.check_memory_add(103, 3000).unwrap_err().to_string().contains("en yakın geçerli değer 3072 MB"));
    assert!(c.check_memory_add(103, 4090).unwrap_err().to_string().contains("en yakın geçerli değer 4096 MB"));

    // Eklenen parça toplam belleğe ve parça listesine işlenir; elle verilmiş balon hedefi de aynı kadar yükselir.
    {
        let g = c.guest_mut(103).unwrap();
        (g.hw.balloon, g.hw.balloon_mb) = (true, 1024);
    }
    c.note_memory_added(103, 1024);
    let g = c.guest(103).unwrap();
    assert_eq!((g.mem_total_mb, g.hw.dimms.clone(), g.hw.balloon_mb), (3072, vec![1024], 2048));
    // Sınıra bir adımdan az kalmışsa bunu söyler.
    c.guest_mut(103).unwrap().hw.max_mem_mb = 3100;
    assert!(c.check_memory_add(103, 3090).unwrap_err().to_string().contains("bir adımlık yer kalmadı"));
    c.guest_mut(103).unwrap().hw.max_mem_mb = 8192;
    // Yuvalar dolunca konuğun bir kez kapatılıp açılması gerekir.
    for _ in 1..crate::hw::MEM_SLOTS {
        c.note_memory_added(103, 128);
    }
    assert!(c.check_memory_add(103, 8192).unwrap_err().to_string().contains("bir kez kapatıp açın"));
}

#[test]
fn hot_added_memory_becomes_boot_memory_on_a_cold_start_but_not_on_resume() {
    let mut c = cluster();
    {
        let g = c.guest_mut(103).unwrap();
        (g.mem_total_mb, g.hw.max_mem_mb, g.hw.dimms) = (3072, 4096, vec![512, 512]);
    }
    // Askıdaki konuk parçalarıyla döner.
    c.guest_mut(103).unwrap().hw.suspended = true;
    let task = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().hw.dimms, vec![512, 512]);
    c.complete_task(task, Ok(()), NOW).unwrap();
    c.guest_mut(103).unwrap().status = GuestStatus::Stopped;
    // Sıfırdan açılışta parçalar açılış belleğine katılır: toplam aynı, yuvalar boş.
    c.guest_mut(103).unwrap().hw.suspended = false;
    c.begin_action(103, Action::Start, "u", NOW).unwrap();
    let g = c.guest(103).unwrap();
    assert_eq!((g.mem_total_mb, g.hw.dimms.is_empty()), (3072, true));
}

#[test]
fn a_memory_snapshot_is_not_loaded_into_a_running_guest_whose_cores_or_memory_changed() {
    let mut c = cluster();
    {
        let g = c.guest_mut(103).unwrap();
        (g.status, g.cores, g.mem_total_mb, g.hw.max_cores, g.hw.max_mem_mb) = (GuestStatus::Running, 2, 2048, 4, 4096);
    }
    c.add_snapshot(103, "once", NOW);
    c.check_rollback_state(103, "once").unwrap();
    c.note_core_added(103);
    assert!(matches!(c.check_rollback_state(103, "once"), Err(Error::RequiresStopped { .. })));
    c.guest_mut(103).unwrap().cores = 2;
    c.note_memory_added(103, 512);
    assert!(matches!(c.check_rollback_state(103, "once"), Err(Error::RequiresStopped { .. })));
    // Aynı donanımla alınan görüntüye dönülür; kapalı konukta (yalnızca disk döner) kısıt yoktur.
    c.add_snapshot(103, "sonra", NOW);
    c.check_rollback_state(103, "sonra").unwrap();
    c.guest_mut(103).unwrap().status = GuestStatus::Stopped;
    c.check_rollback_state(103, "once").unwrap();
    // Eski kayıtlarda (donanım bilgisi yok) davranış değişmez.
    c.guest_mut(103).unwrap().status = GuestStatus::Running;
    c.guest_mut(103).unwrap().snapshots[0].cores = 0;
    c.check_rollback_state(103, "once").unwrap();
}

#[test]
fn only_plain_running_vms_are_migrated_live_and_the_source_copy_does_not_restart() {
    let mut c = cluster();
    // Çalışan, sunucuya bağlı donanımı olmayan sanal makine taşınır.
    assert!(c.check_migrate_out(103).unwrap_err().to_string().contains("çalışmıyor"));
    c.guest_mut(103).unwrap().status = GuestStatus::Running;
    c.check_migrate_out(103).unwrap();
    let running_ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.status == GuestStatus::Running).unwrap().vmid;
    assert!(c.check_migrate_out(running_ct).unwrap_err().to_string().contains("yalnızca sanal makineler"));
    // Kayıttan yeniden kurulamayan ya da sunucuya bağlı donanım taşımayı engeller.
    let blocked = |change: &dyn Fn(&mut Guest)| {
        let mut c = cluster();
        let g = c.guest_mut(103).unwrap();
        g.status = GuestStatus::Running;
        change(g);
        c.check_migrate_out(103).unwrap_err().to_string()
    };
    assert!(blocked(&|g| g.hw.firmware = crate::hw::Firmware::Uefi).contains("UEFI"));
    assert!(blocked(&|g| g.hw.tpm = true).contains("TPM"));
    assert!(blocked(&|g| g.hw.usb.push("046d:c52b".into())).contains("sunucu aygıtı"));
    assert!(blocked(&|g| g.iso = Some("debian.iso".into())).contains("kalıbı"));
    assert!(blocked(&|g| g.hw.ci_ssh_key = Some("ssh-ed25519 AAAA ali".into())).contains("cloud-init"));
    assert!(blocked(&|g| g.hw.base = Some(100)).contains("bağlı"));
    let task = c.begin_op(103, "vm snapshot 103".into(), "u", NOW, None).unwrap();
    assert!(matches!(c.check_migrate_out(103), Err(Error::Locked { .. })));
    c.complete_task(task, Ok(()), NOW).unwrap();

    // Taşınan konuğun kaynaktaki kaydı kapalıdır ve kendiliğinden açılmaz.
    {
        let g = c.guest_mut(103).unwrap();
        (g.onboot, g.hw.restart, g.hw.dimms) = (true, true, vec![512]);
    }
    c.after_migrated_out(103);
    let g = c.guest(103).unwrap();
    assert_eq!((g.status, g.onboot, g.hw.restart, g.hw.dimms.is_empty()), (GuestStatus::Stopped, false, false, true));
    assert!(g.tags.iter().any(|t| t == crate::MIGRATED_TAG));
    c.after_migrated_out(103);
    assert_eq!(c.guest(103).unwrap().tags.iter().filter(|t| *t == crate::MIGRATED_TAG).count(), 1);

    // Anahtar ve adres QEMU'ya argüman olarak gider: yalnızca beklenen biçim kabul edilir.
    assert!(crate::valid_migrate_key(&"0a".repeat(32)));
    for bad in ["", "0a", &"0A".repeat(32), &"zz".repeat(32), &"0a".repeat(33)] {
        assert!(!crate::valid_migrate_key(bad), "{bad}");
    }
    for good in ["10.0.0.7", "sunucu-2.ornek.net", "fd00::7"] {
        assert!(crate::valid_migrate_host(good), "{good}");
    }
    for bad in ["", "-x", "a b", "a,b", "a/b", "a;b"] {
        assert!(!crate::valid_migrate_host(bad), "{bad}");
    }
}

#[test]
fn cpu_shares_and_memory_reservations_are_validated_and_reservations_must_fit_the_node() {
    let mut c = cluster();
    let set = |shares: Option<u32>, reserve: Option<u64>| GuestPatch { cpu_shares: shares, mem_reserve_mb: reserve, ..patch() };
    // Pay: 0 olağan; sınırların dışı ve konteyner reddedilir. Çalışan konukta da değişir.
    for bad in [1, 9, 10_001] {
        assert!(c.update_guest(100, &set(Some(bad), None), "u", NOW).unwrap_err().to_string().contains("işlemci payı 10 ile 10000"), "{bad}");
    }
    c.update_guest(100, &set(Some(400), None), "u", NOW).unwrap();
    assert_eq!((c.guest(100).unwrap().hw.cpu_shares, c.guest(100).unwrap().status), (400, GuestStatus::Running));
    c.update_guest(100, &set(Some(0), None), "u", NOW).unwrap();
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct).unwrap().vmid;
    assert!(c.update_guest(ct, &set(Some(200), None), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    assert!(c.update_guest(ct, &set(None, Some(128)), "u", NOW).unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    assert_eq!((crate::hw::cpu_weight(0), crate::hw::cpu_weight(400), crate::hw::cpu_weight(99_999)), (100, 400, 10_000));

    // Garanti: konuğun belleğini aşamaz; bellek garantinin altına indirilemez.
    let mem = c.guest(103).unwrap().mem_total_mb;
    assert!(c.update_guest(103, &set(None, Some(mem + 1)), "u", NOW).unwrap_err().to_string().contains("büyük olamaz"));
    c.update_guest(103, &set(None, Some(4096)), "u", NOW).unwrap();
    assert!(c.update_guest(103, &GuestPatch { memory_mb: Some(2048), ..patch() }, "u", NOW).unwrap_err().to_string().contains("büyük olamaz"));
    // Balon garantinin altına inemez; garanti de balonla inilen belleğin üstüne çıkamaz.
    c.guest_mut(103).unwrap().hw.balloon = true;
    assert!(c.set_balloon_target(103, 2048).unwrap_err().to_string().contains("garantisinin (4096 MB) altına"));
    assert!(c.set_dynamic_memory(103, 1024).unwrap_err().to_string().contains("garantisinin (4096 MB) altına"));
    c.set_balloon_target(103, 8192).unwrap();
    assert!(c.update_guest(103, &set(None, Some(12288)), "u", NOW).unwrap_err().to_string().contains("balonla inilebilen bellekten (8192 MB)"));
    c.set_balloon_target(103, 0).unwrap();

    // Çalışan konukların garantileri düğümün belleğini aşamaz: fazlası başlatılmaz, çalışırken de verilmez.
    let node = c.guest(103).unwrap().node.clone();
    let total = c.node(&node).unwrap().mem_total_mb;
    let other = c.guests.iter().find(|g| g.node == node && g.kind == GuestKind::Vm && g.status == GuestStatus::Running).unwrap().vmid;
    c.guest_mut(other).unwrap().hw.mem_reserve_mb = total - 2048;
    let refused = c.begin_action(103, Action::Start, "u", NOW).unwrap_err().to_string();
    assert!(refused.contains("garanti edilebilecek bellek yetmiyor") && refused.contains(&format!("{} MB garanti verilmiş", total - 2048)), "{refused}");
    assert_eq!(c.guest(103).unwrap().lock, None, "reddedilen başlatma konuğu kilitlemez");
    c.update_guest(103, &set(None, Some(2048)), "u", NOW).unwrap();
    let task = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    c.complete_task(task, Ok(()), NOW).unwrap();
    assert!(c.update_guest(103, &set(None, Some(4096)), "u", NOW).unwrap_err().to_string().contains("garanti edilebilecek bellek yetmiyor"));
    // Kapalı konukların garantisi sayılmaz.
    c.guest_mut(other).unwrap().status = GuestStatus::Stopped;
    c.update_guest(103, &set(None, Some(4096)), "u", NOW).unwrap();
}

#[test]
fn a_running_vm_moves_between_storages_only_without_snapshots() {
    use crate::model::{Storage, StorageKind};
    let mut c = cluster();
    let node = c.guest(100).unwrap().node.clone();
    c.storages.push(Storage { id: "hizli".into(), node: Some(node.clone()), kind: StorageKind::Dir, content: vec!["disk".into()], used_gb: 0, total_gb: 5000 });
    assert_eq!(c.guest(100).unwrap().status, GuestStatus::Running);
    c.check_storage_move_live(100, "hizli").unwrap();
    // Kapalıyken geçerli olan kurallar burada da geçerlidir.
    let here = c.guest(100).unwrap().storage.clone();
    assert!(c.check_storage_move_live(100, &here).unwrap_err().to_string().contains("zaten"));
    assert!(c.check_storage_move_live(100, "yok").is_err());
    // Görüntüler disk dosyasının içindedir ve aynalanmaz: görüntüsü olan konuk çalışırken taşınmaz.
    c.add_snapshot(100, "once", NOW);
    assert!(c.check_storage_move_live(100, "hizli").unwrap_err().to_string().contains("anlık görüntüsü olan konuk çalışırken taşınamaz"));
    c.check_storage_move(100, "hizli").unwrap();
    c.remove_snapshot(100, "once");
    c.guest_mut(100).unwrap().hw.instant = Some("vm-100-1".into());
    assert!(c.check_storage_move_live(100, "hizli").unwrap_err().to_string().contains("yedeğe dayanan"));
    c.guest_mut(100).unwrap().hw.instant = None;
    let ct = c.guests.iter().find(|g| g.kind == GuestKind::Ct && g.node == node).map(|g| g.vmid);
    if let Some(ct) = ct {
        assert!(c.check_storage_move_live(ct, "hizli").unwrap_err().to_string().contains("yalnızca sanal makinelerde"));
    }
}
