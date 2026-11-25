use super::*;

const NOW: u64 = 1_800_000_000;

fn cluster() -> Cluster {
    Cluster::demo(NOW)
}

fn spec(name: &str) -> GuestSpec {
    GuestSpec {
        vmid: None,
        name: name.to_string(),
        kind: GuestKind::Vm,
        node: "pve-ist-01".to_string(),
        cores: 2,
        memory_mb: 2048,
        disk_gb: 32,
        storage: "local-zfs".to_string(),
        os: "debian-12".to_string(),
        bridge: "vmbr0".to_string(),
        onboot: false,
        iso: None,
        hw: Default::default(),
    }
}

fn run(c: &mut Cluster, vmid: VmId, action: Action) -> Result<TaskId, Error> {
    let id = c.begin_action(vmid, action, "root@pam", NOW)?;
    c.complete_task(id, Ok(()), NOW + 2)?;
    Ok(id)
}

#[test]
fn demo_summary_adds_up() {
    let s = cluster().summary();
    assert_eq!((s.nodes_online, s.nodes_total), (3, 3));
    assert!(s.quorate);
    assert_eq!(s.cores, 80);
    // (34*32 + 61*32 + 12*16) / 80 = 40.4
    assert_eq!(s.cpu_pct, 40);
    assert_eq!(s.mem_total_mb, 320 * 1024);
    assert_eq!(s.mem_pct, 63);
    assert_eq!((s.storage_used_gb, s.storage_total_gb), (9612, 12_500));
    assert_eq!(s.storage_pct, 77);
    assert_eq!((s.guests_running, s.guests_total), (5, 8));
}

#[test]
fn quorum_needs_strict_majority() {
    let mut c = cluster();
    c.nodes[0].status = NodeStatus::Offline;
    assert!(c.summary().quorate, "2/3 çoğunluktur");
    c.nodes[1].status = NodeStatus::Offline;
    assert!(!c.summary().quorate, "1/3 çoğunluk değildir");
    assert_eq!(c.summary().cores, 16, "çevrimdışı düğümler toplamdan düşer");
}

#[test]
fn start_stop_cycle_updates_state_and_uptime() {
    let mut c = cluster();
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);

    run(&mut c, 103, Action::Start).unwrap();
    let g = c.guest(103).unwrap();
    assert_eq!(g.status, GuestStatus::Running);
    assert_eq!(g.started_at, Some(NOW + 2));
    assert_eq!(g.uptime_secs(NOW + 62), Some(60));
    assert!(g.mem_used_mb > 0 && g.lock.is_none());

    run(&mut c, 103, Action::Stop).unwrap();
    let g = c.guest(103).unwrap();
    assert_eq!(g.status, GuestStatus::Stopped);
    assert_eq!((g.cpu_pct, g.mem_used_mb, g.started_at), (0, 0, None));
}

#[test]
fn pause_resume_and_reboot() {
    let mut c = cluster();
    run(&mut c, 100, Action::Pause).unwrap();
    assert_eq!(c.guest(100).unwrap().status, GuestStatus::Paused);
    run(&mut c, 100, Action::Resume).unwrap();
    assert_eq!(c.guest(100).unwrap().status, GuestStatus::Running);

    let before = c.guest(100).unwrap().started_at;
    run(&mut c, 100, Action::Reboot).unwrap();
    let g = c.guest(100).unwrap();
    assert_eq!(g.status, GuestStatus::Running);
    assert_ne!(g.started_at, before, "yeniden başlatma çalışma süresini sıfırlar");
}

#[test]
fn invalid_transitions_are_rejected_without_side_effects() {
    let mut c = cluster();
    let before = c.clone();
    assert_eq!(
        c.begin_action(100, Action::Start, "u", NOW),
        Err(Error::InvalidTransition {
            vmid: 100,
            action: Action::Start,
            status: GuestStatus::Running
        })
    );
    assert_eq!(
        c.begin_action(103, Action::Reboot, "u", NOW).unwrap_err().kind(),
        crate::ErrorKind::Conflict
    );
    assert_eq!(c.begin_action(999, Action::Start, "u", NOW), Err(Error::GuestNotFound(999)));
    assert_eq!(c, before, "reddedilen eylem durum ya da görev bırakmamalı");
}

#[test]
fn guest_is_locked_while_a_task_runs() {
    let mut c = cluster();
    let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    assert!(c.has_running_tasks());
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped, "bitene kadar durum değişmez");
    assert_eq!(
        c.begin_action(103, Action::Start, "u", NOW),
        Err(Error::Locked { vmid: 103, task: t })
    );
    assert_eq!(c.delete_guest(103, "u", NOW), Err(Error::Locked { vmid: 103, task: t }));

    c.complete_task(t, Ok(()), NOW + 1).unwrap();
    assert!(!c.has_running_tasks());
    assert_eq!(c.complete_task(t, Ok(()), NOW + 1), Err(Error::TaskNotFound(t)), "görev iki kez kapanmaz");
}

#[test]
fn failed_task_leaves_guest_unchanged_and_unlocked() {
    let mut c = cluster();
    let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    c.complete_task(t, Err("depolama kilidi alınamadı".into()), NOW + 1).unwrap();
    let g = c.guest(103).unwrap();
    assert_eq!((g.status, g.lock), (GuestStatus::Stopped, None));
    let task = c.task(t).unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.message.as_deref(), Some("depolama kilidi alınamadı"));
    assert_eq!(task.ended, Some(NOW + 1));
    // Başarısızlıktan sonra yeniden denenebilir.
    assert!(c.begin_action(103, Action::Start, "u", NOW + 2).is_ok());
}

#[test]
fn action_refused_on_offline_node() {
    let mut c = cluster();
    c.nodes[1].status = NodeStatus::Offline;
    assert_eq!(
        c.begin_action(103, Action::Start, "u", NOW),
        Err(Error::NodeOffline("pve-ist-02".into()))
    );
}

#[test]
fn next_vmid_fills_first_gap() {
    let mut c = cluster();
    assert_eq!(c.next_vmid(), 105, "100-104 dolu, 200+ ayrı blok");
    c.guests.retain(|g| g.vmid != 101);
    assert_eq!(c.next_vmid(), 101);
    c.guests.clear();
    assert_eq!(c.next_vmid(), 100);
}

#[test]
fn create_inserts_sorted_reserves_disk_and_logs() {
    let mut c = cluster();
    let free_before = c.storage_for("local-zfs", "pve-ist-01").unwrap().free_gb();
    let vmid = c.create_guest(&spec("yeni-vm"), "ayse@pve", NOW).unwrap();
    assert_eq!(vmid, 105);
    let ids: Vec<VmId> = c.guests.iter().map(|g| g.vmid).collect();
    assert_eq!(ids, [100, 101, 102, 103, 104, 105, 200, 201, 202]);
    let g = c.guest(105).unwrap();
    assert_eq!((g.status, g.mem_total_mb, g.ip.as_deref()), (GuestStatus::Stopped, 2048, None));
    assert_eq!(c.storage_for("local-zfs", "pve-ist-01").unwrap().free_gb(), free_before - 32);
    // Aynı adlı depolama başka düğümde etkilenmez.
    assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, 1390);
    let t = &c.tasks[0];
    assert_eq!((t.command.as_str(), t.user.as_str(), t.status), ("vm create 105", "ayse@pve", TaskStatus::Ok));
}

#[test]
fn create_validates_every_field() {
    let mut c = cluster();
    let before = c.clone();
    let cases: Vec<(GuestSpec, Error)> = vec![
        (GuestSpec { vmid: Some(100), ..spec("a") }, Error::VmidTaken(100)),
        (GuestSpec { vmid: Some(99), ..spec("a") }, Error::InvalidVmid(99)),
        (GuestSpec { vmid: Some(1_000_000_000), ..spec("a") }, Error::InvalidVmid(1_000_000_000)),
        (spec("Kötü Ad"), Error::InvalidName("Kötü Ad".into())),
        (GuestSpec { node: "yok".into(), ..spec("a") }, Error::NodeNotFound("yok".into())),
        (GuestSpec { storage: "yok".into(), ..spec("a") }, Error::StorageNotFound("yok".into())),
        (
            GuestSpec { storage: "local-lvm".into(), ..spec("a") },
            Error::StorageNotOnNode { storage: "local-lvm".into(), node: "pve-ist-01".into() },
        ),
        (
            GuestSpec { disk_gb: 5000, ..spec("a") },
            Error::InsufficientStorage { storage: "local-zfs".into(), needed_gb: 5000, free_gb: 1188 },
        ),
    ];
    for (s, expected) in cases {
        assert_eq!(c.create_guest(&s, "u", NOW), Err(expected));
    }
    for s in [
        GuestSpec { cores: 0, ..spec("a") },
        GuestSpec { cores: 33, ..spec("a") },
        GuestSpec { memory_mb: 32, ..spec("a") },
        GuestSpec { memory_mb: 999_999_999, ..spec("a") },
        GuestSpec { disk_gb: 0, ..spec("a") },
    ] {
        assert!(matches!(c.create_guest(&s, "u", NOW), Err(Error::InvalidSpec(_))), "{s:?}");
    }
    assert_eq!(c, before, "reddedilen istek hiçbir şeyi değiştirmemeli");
}

#[test]
fn container_may_use_less_memory_than_vm_and_shared_storage() {
    let mut c = cluster();
    let s = GuestSpec { kind: GuestKind::Ct, memory_mb: 32, storage: "nfs-yedek".into(), ..spec("kucuk") };
    let id = c.create_guest(&s, "u", NOW).unwrap();
    assert_eq!(c.guest(id).unwrap().kind, GuestKind::Ct);
    assert_eq!(c.tasks[0].command, format!("ct create {id}"));
}

#[test]
fn create_refused_on_offline_node() {
    let mut c = cluster();
    c.nodes[0].status = NodeStatus::Offline;
    assert_eq!(c.create_guest(&spec("a"), "u", NOW), Err(Error::NodeOffline("pve-ist-01".into())));
}

#[test]
fn delete_requires_stopped_and_frees_disk() {
    let mut c = cluster();
    assert_eq!(c.delete_guest(100, "u", NOW), Err(Error::NotStopped(100)));
    assert_eq!(c.delete_guest(104, "u", NOW), Err(Error::NotStopped(104)), "duraklatılmış da silinemez");
    assert_eq!(c.delete_guest(999, "u", NOW), Err(Error::GuestNotFound(999)));

    let used = c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb;
    c.delete_guest(103, "u", NOW).unwrap();
    assert_eq!(c.guest(103), Err(Error::GuestNotFound(103)));
    assert_eq!(c.storage_for("local-zfs", "pve-ist-02").unwrap().used_gb, used - 200);
    assert_eq!(c.tasks[0].command, "vm destroy 103");
}

#[test]
fn create_then_delete_restores_storage() {
    let mut c = cluster();
    let before = c.storages.clone();
    let id = c.create_guest(&spec("gecici"), "u", NOW).unwrap();
    c.delete_guest(id, "u", NOW).unwrap();
    assert_eq!(c.storages, before);
}

#[test]
fn task_log_is_capped_but_keeps_running_tasks() {
    let mut c = cluster();
    let running = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    for i in 0..(MAX_TASKS as u64 + 50) {
        run(&mut c, 100, if i % 2 == 0 { Action::Pause } else { Action::Resume }).unwrap();
    }
    assert_eq!(c.tasks.len(), MAX_TASKS);
    assert_eq!(c.task(running).unwrap().status, TaskStatus::Running);
    let ids: Vec<TaskId> = c.tasks.iter().map(|t| t.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(ids, sorted, "günlük en yeniden eskiye sıralı kalır");
}

#[test]
fn iso_is_validated_and_only_for_vms() {
    let mut c = cluster();
    let with_iso = |iso: &str| GuestSpec { iso: Some(iso.to_string()), ..spec("a") };
    assert!(matches!(c.create_guest(&with_iso("../x.iso"), "u", NOW), Err(Error::InvalidSpec(_))));
    assert!(matches!(c.create_guest(&with_iso("x.img"), "u", NOW), Err(Error::InvalidSpec(_))));
    let ct = GuestSpec { kind: GuestKind::Ct, ..with_iso("debian.iso") };
    assert!(matches!(c.create_guest(&ct, "u", NOW), Err(Error::InvalidSpec(_))), "konteynere ISO takılmaz");
    let id = c.create_guest(&with_iso("debian.iso"), "u", NOW).unwrap();
    assert_eq!(c.guest(id).unwrap().iso.as_deref(), Some("debian.iso"));
}

#[test]
fn abort_create_rolls_everything_back() {
    let mut c = cluster();
    let before = c.storages.clone();
    let id = c.create_guest(&spec("yarim"), "u", NOW).unwrap();
    c.abort_create(id, "qemu-img bulunamadı", NOW + 1);
    assert_eq!(c.guest(id), Err(Error::GuestNotFound(id)));
    assert_eq!(c.storages, before, "disk payı iade edilmeli");
    let t = &c.tasks[0];
    assert_eq!((t.status, t.message.as_deref()), (TaskStatus::Failed, Some("qemu-img bulunamadı")));
    assert_eq!(c.next_vmid(), id, "kimlik yeniden kullanılabilir");
    c.abort_create(999, "yok", NOW); // bilinmeyen kimlik sessizce geçilir
}

#[test]
fn check_deletable_matches_delete_rules() {
    let mut c = cluster();
    assert!(c.check_deletable(103).is_ok());
    assert_eq!(c.check_deletable(100).unwrap_err(), Error::NotStopped(100));
    assert_eq!(c.check_deletable(999).unwrap_err(), Error::GuestNotFound(999));
    let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    assert_eq!(c.check_deletable(103).unwrap_err(), Error::Locked { vmid: 103, task: t });
}

#[test]
fn observed_state_reconciles_guest() {
    use crate::driver::Probe;
    let mut c = cluster();

    // Çalışan konuk kendi kendine kapanmış.
    let stopped = Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None };
    assert!(c.observe_guest(100, &stopped, NOW));
    let g = c.guest(100).unwrap();
    assert_eq!((g.status, g.cpu_pct, g.mem_used_mb, g.started_at), (GuestStatus::Stopped, 0, 0, None));
    assert!(!c.observe_guest(100, &stopped, NOW), "aynı gözlem değişiklik sayılmaz");

    // Durmuş görünen konuk aslında çalışıyor (sunucu yeniden başlamış).
    let running = Probe { status: GuestStatus::Running, cpu_pct: Some(250), mem_used_mb: Some(999_999) };
    assert!(c.observe_guest(103, &running, NOW + 5));
    let g = c.guest(103).unwrap();
    assert_eq!((g.status, g.cpu_pct, g.mem_used_mb, g.started_at), (GuestStatus::Running, 100, g.mem_total_mb, Some(NOW + 5)));

    // Ölçüm yoksa eldeki değerler korunur; duraklatılmış konuğun CPU'su sıfırdır.
    let paused = Probe { status: GuestStatus::Paused, cpu_pct: Some(40), mem_used_mb: None };
    c.observe_guest(103, &paused, NOW + 9);
    let g = c.guest(103).unwrap();
    assert_eq!((g.cpu_pct, g.mem_used_mb, g.started_at), (0, g.mem_total_mb, Some(NOW + 5)));

    assert!(!c.observe_guest(999, &running, NOW));
}

#[test]
fn observation_does_not_override_a_running_task() {
    use crate::driver::Probe;
    let mut c = cluster();
    let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    let running = Probe { status: GuestStatus::Running, cpu_pct: Some(5), mem_used_mb: Some(64) };
    assert!(!c.observe_guest(103, &running, NOW));
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);
    c.complete_task(t, Ok(()), NOW).unwrap();
    assert_eq!(c.guest(103).unwrap().status, GuestStatus::Running);
}

#[test]
fn local_cluster_and_host_observations() {
    let node = Node { id: "ev".into(), status: NodeStatus::Online, cores: 8, cpu_pct: 0, mem_used_mb: 0, mem_total_mb: 1, uptime_secs: 0, kernel: "6".into(), ip: "10.0.0.2".into() };
    let storage = Storage { id: "local".into(), node: Some("ev".into()), kind: StorageKind::Dir, content: vec![], used_gb: 0, total_gb: 0 };
    let mut c = Cluster::local(node, storage);
    assert_eq!((c.name.as_str(), c.guests.len(), c.next_vmid()), ("ev", 0, 100));

    c.observe_node("ev", 140, 9000, 8000, 77);
    let n = c.node("ev").unwrap();
    assert_eq!((n.cpu_pct, n.mem_used_mb, n.mem_total_mb, n.uptime_secs), (100, 8000, 8000, 77));
    c.observe_storage("local", "ev", 30, 100);
    assert_eq!(c.storage_for("local", "ev").unwrap().free_gb(), 70);
    c.observe_node("yok", 1, 1, 1, 1);
    c.observe_storage("yok", "ev", 1, 1);

    let s = GuestSpec { node: "ev".into(), storage: "local".into(), ..spec("ilk") };
    assert_eq!(c.create_guest(&s, "u", NOW), Ok(100));
}

#[test]
fn state_survives_json_round_trip() {
    let mut c = cluster();
    c.create_guest(&spec("kalici"), "u", NOW).unwrap();
    let t = c.begin_action(103, Action::Start, "u", NOW).unwrap();
    let json = serde_json::to_string(&c).unwrap();
    let mut back: Cluster = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
    // Görev sayacı da korunur: yeni görev eski kimlikle çakışmaz.
    back.complete_task(t, Ok(()), NOW).unwrap();
    let next = back.begin_action(100, Action::Pause, "u", NOW).unwrap();
    assert!(next > t);
}
