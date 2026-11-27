//! Güç işlemleri, askıya alma, durum yoklama, ekran görüntüsü ve tuş gönderme.

use super::*;

pub fn apply(d: &QemuDriver, g: &Guest, action: Action) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let alive = || live_pid(d, g.vmid).is_some();
    match action {
        Action::Start => {
            if alive() {
                return Err("konuk zaten çalışıyor".to_string());
            }
            if !d.cfg.disk_of(g).exists() {
                return Err(format!("disk imajı bulunamadı: {}", d.cfg.disk_of(g).display()));
            }
            cleanup(d, g.vmid);
            crate::qemu::write_seed(&d.cfg, g)?;
            crate::qemu::prepare_efivars(d, g)?;
            // Paylaşılan klasörlerin dizinleri yoksa oluşturulur (yalnızca hizmet kullanıcısına açık).
            for share in g.hw.shares.iter().filter(|s| s.valid()) {
                let dir = d.cfg.shares_dir().join(&share.name);
                if !dir.is_dir() {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("paylaşılan klasör {} oluşturulamadı: {e}", dir.display()))?;
                }
            }
            if g.hw.nested && d.cfg.kvm && !crate::qemu::nested_enabled(&d.cfg.sys_module) {
                return Err(format!("konuk iç içe sanallaştırma istiyor ama {}", crate::qemu::NESTED_HINT));
            }
            start_tpm(d, g)?;
            if let Err(e) = prepare_vlan_taps(d, g) {
                stop_tpm(d, g.vmid);
                remove_vlan_taps(d, g.vmid);
                return Err(e);
            }
            // Askıdan dönüş: kayıtlı bellek durumu varsa QEMU onu yükleyerek başlar ve konuk kaldığı yerden sürer.
            let state = d.cfg.state_path(g.vmid);
            let resuming = state.is_file();
            // Askıdaki konuk, askıya alındığı donanım yerleşimiyle döner (eski kayıtlarda bağlantı noktası yoktur).
            let layout = if !resuming {
                crate::qemu::LAYOUT_CURRENT
            } else {
                // İşaret dosyası yoksa en eski yerleşim; içeriği yerleşim numarasıdır (boşsa 1).
                match std::fs::read_to_string(state.with_extension("state.hp")) {
                    Err(_) => crate::qemu::LAYOUT_LEGACY,
                    Ok(text) => text.trim().parse().unwrap_or(crate::qemu::LAYOUT_PORTS),
                }
            };
            let mut args = crate::qemu::build_args_layout(&d.cfg, g, layout);
            if resuming {
                args.extend(["-incoming".to_string(), format!("exec:cat {}", shell_quote(&state))]);
            }
            let started = crate::qemu::perf::preflight(g, &crate::host::performance())
                .and_then(|()| std::process::Command::new(&d.cfg.qemu_bin).args(args).output().map_err(|e| format!("{} çalıştırılamadı: {e}", d.cfg.qemu_bin)))
                .and_then(|out| if out.status.success() { Ok(()) } else { Err(format!("QEMU başlatılamadı: {}", last_line(&out.stderr))) })
                .and_then(|()| {
                    if wait_until(Duration::from_secs(5), alive) { Ok(()) } else { Err("QEMU başladı ama süreç bulunamadı (hemen çıkmış olabilir)".to_string()) }
                });
            if started.is_err() {
                stop_tpm(d, g.vmid);
                remove_vlan_taps(d, g.vmid);
                return started;
            }
            // Ağ hız sınırı konuk başladıktan sonra TAP arayüzüne uygulanır. Uygulanamıyorsa konuk
            // sınırsız çalışmaya bırakılmaz: kapatılır ve hata bildirilir.
            if let Err(e) = limit_network(d, g) {
                let _ = qmp(&sock, &["quit"]);
                wait_until(Duration::from_secs(5), || !alive());
                cleanup(d, g.vmid);
                return Err(e);
            }
            // İşlemci payı ve bellek garantisi: istenmiş ama uygulanamıyorsa konuk bu sözler olmadan çalıştırılmaz.
            if let Err(e) = apply_limits(d, g) {
                let _ = qmp(&sock, &["quit"]);
                wait_until(Duration::from_secs(5), || !alive());
                cleanup(d, g.vmid);
                return Err(e);
            }
            // Kayıtlı bellek hedefi yeniden uygulanır (QEMU her açılışta balonu boş başlatır). Uygulanamazsa
            // konuk belleğinin tamamıyla çalışmayı sürdürür; bu bir başlatma hatası değildir.
            if g.hw.balloon && g.hw.balloon_mb > 0 && !resuming {
                if let Err(e) = set_balloon(d, g, g.hw.balloon_mb) {
                    eprintln!("uyarı: konuk {} için {e}", g.vmid);
                }
            }
            if resuming {
                // Durum yüklenene kadar konuk "inmigrate" görünür; yüklenince kendiliğinden çalışır.
                let running = || qmp(&sock, &["query-status"]).ok().and_then(|r| r.first().and_then(|s| s["status"].as_str().map(|s| s == "running" || s == "paused"))).unwrap_or(false);
                if !wait_until(Duration::from_secs(600), || running() || !alive()) || !alive() {
                    let _ = qmp(&sock, &["quit"]);
                    cleanup(d, g.vmid);
                    return Err("konuk askıdan döndürülemedi (bellek durumu bu donanımla uyuşmuyor olabilir); askı durumunu atıp yeniden başlatın".to_string());
                }
                // Durum, konuk durdurulmuşken kaydedilir ve QEMU onu durdurulmuş olarak geri yükler:
                // askıya alınırken çalışıyorduysa yeniden çalıştırılır.
                let marker = state.with_extension("state.running");
                if marker.exists() {
                    let _ = qmp(&sock, &["cont"]);
                }
                let _ = std::fs::remove_file(&marker);
                let _ = std::fs::remove_file(state.with_extension("state.hp"));
                // Durum kullanıldı: aynı andan ikinci kez devam edilmemeli.
                let _ = std::fs::remove_file(&state);
            }
            Ok(())
        }
        Action::Stop => {
            if !alive() {
                cleanup(d, g.vmid);
                return Ok(());
            }
            // Önce konuktan kapanmasını iste (ACPI); süre dolarsa zorla kapat.
            let asked = qmp(&sock, &["system_powerdown"]).is_ok();
            if !(asked && wait_until(d.cfg.shutdown_timeout, || !alive())) {
                let _ = qmp(&sock, &["quit"]);
                if !wait_until(Duration::from_secs(5), || !alive()) {
                    if let Some(pid) = live_pid(d, g.vmid) {
                        let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
                    }
                    if !wait_until(Duration::from_secs(3), || !alive()) {
                        return Err("QEMU süreci sonlandırılamadı".to_string());
                    }
                }
            }
            cleanup(d, g.vmid);
            Ok(())
        }
        Action::Reboot | Action::Pause | Action::Resume => {
            if !alive() {
                return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
            }
            let command = match action {
                Action::Reboot => "system_reset",
                Action::Pause => "stop",
                _ => "cont",
            };
            qmp(&sock, &[command]).map(|_| ())
        }
    }
}

/// Askıya alma: konuk durdurulur, bellek ve aygıt durumu dosyaya yazılır, QEMU kapatılır.
pub fn suspend(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let alive = || live_pid(d, g.vmid).is_some();
    if !alive() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    let state = d.cfg.state_path(g.vmid);
    let temp = state.with_extension("state.tmp");
    let marker = state.with_extension("state.running");
    let _ = std::fs::remove_file(&temp);
    let _ = std::fs::remove_file(&marker);
    // Konuk elle duraklatılmış olabilir: geri dönünce de öyle kalmalı.
    let was_running = qmp(&sock, &["query-status"]).ok().and_then(|r| r.first().map(|s| s["status"] == "running")).unwrap_or(true);
    // Konuk hangi yerleşimle açıldıysa onunla dönmelidir.
    let layout = running_layout(d, g.vmid);
    let _ = std::fs::remove_file(state.with_extension("state.hp"));
    let migrate = serde_json::json!({ "execute": "migrate", "arguments": { "uri": format!("exec:cat > {}", shell_quote(&temp)) } });
    let saved = qmp_values(&sock, &[serde_json::json!({ "execute": "stop" }), migrate]).and_then(|_| {
        let mut outcome = Err("bellek durumu yazılamadı: zaman aşımı".to_string());
        wait_until(Duration::from_secs(1800), || {
            let status = qmp(&sock, &["query-migrate"]).ok().and_then(|r| r.first().and_then(|m| m["status"].as_str().map(str::to_string)));
            match status.as_deref() {
                Some("completed") => outcome = Ok(()),
                Some("failed") | Some("cancelled") => {
                    outcome = Err(if g.hw.shares.is_empty() {
                        "bellek durumu yazılamadı (disk dolu olabilir)".to_string()
                    } else {
                        "bellek durumu yazılamadı: paylaşılan klasör konukta bağlıyken konuk askıya alınamaz; önce konukta ayırın (umount) ya da disk dolu olabilir".to_string()
                    })
                }
                None if !alive() => outcome = Err("QEMU askıya alma sırasında kapandı".to_string()),
                _ => return false,
            }
            true
        });
        outcome
    });
    if let Err(e) = saved {
        // Başarısızsa konuk çalışmaya devam eder; yarım dosya bırakılmaz.
        let _ = std::fs::remove_file(&temp);
        let _ = qmp(&sock, &["cont"]);
        return Err(e);
    }
    let _ = qmp(&sock, &["quit"]);
    if !wait_until(Duration::from_secs(10), || !alive()) {
        if let Some(pid) = live_pid(d, g.vmid) {
            let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
        }
        wait_until(Duration::from_secs(3), || !alive());
    }
    cleanup(d, g.vmid);
    if was_running {
        std::fs::write(&marker, b"").map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))?;
    }
    if layout > crate::qemu::LAYOUT_LEGACY {
        std::fs::write(state.with_extension("state.hp"), layout.to_string()).map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))?;
    }
    // Durum dosyası ancak QEMU kapandıktan sonra geçerli adını alır: yarım ya da hâlâ yazılan dosyadan dönülmez.
    std::fs::rename(&temp, &state).map_err(|e| format!("bellek durumu kaydedilemedi: {e}"))
}

pub fn screenshot(d: &QemuDriver, g: &Guest) -> Result<Vec<u8>, String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor; ekranı görmek için başlatın".to_string());
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let file = d.cfg.run_dir().join(format!("{}.screen", g.vmid));
    let dump = |format: Option<&str>| {
        let mut arguments = serde_json::json!({ "filename": file.display().to_string() });
        if let Some(f) = format {
            arguments["format"] = f.into();
        }
        qmp_values(&sock, &[serde_json::json!({ "execute": "screendump", "arguments": arguments })])
    };
    // Yeni QEMU doğrudan PNG yazar; eskisi yalnızca PPM bilir.
    let png = dump(Some("png")).is_ok();
    if !png {
        dump(None)?;
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("ekran dökümü okunamadı: {e}"));
    let _ = std::fs::remove_file(&file);
    let bytes = bytes?;
    if png && bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Ok(bytes)
    } else {
        crate::console::ppm_to_png(&bytes)
    }
}

pub fn send_keys(d: &QemuDriver, g: &Guest, chords: &[Vec<String>]) -> Result<(), String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor".to_string());
    }
    if chords.len() > crate::console::MAX_CHORDS {
        return Err(format!("tek seferde en fazla {} tuş gönderilebilir", crate::console::MAX_CHORDS));
    }
    let commands: Vec<Value> = chords
        .iter()
        .map(|keys| {
            let keys: Vec<Value> = keys.iter().map(|k| serde_json::json!({ "type": "qcode", "data": k })).collect();
            serde_json::json!({ "execute": "send-key", "arguments": { "keys": keys } })
        })
        .collect();
    qmp_values(&d.cfg.qmp_path(g.vmid), &commands).map(|_| ())
}

pub fn probe(d: &QemuDriver, g: &Guest) -> Probe {
    let Some(pid) = live_pid(d, g.vmid) else {
        lock(&d.samples).remove(&g.vmid);
        return Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None };
    };
    // QMP yanıt vermezse (meşgul) süreç yaşadığı için çalışıyor sayılır.
    let paused = qmp(&d.cfg.qmp_path(g.vmid), &["query-status"])
        .ok()
        .and_then(|r| r.first().map(|v| v["status"] == "paused" || v["running"] == false))
        .unwrap_or(false);

    let now = Instant::now();
    let ticks = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| parse_stat_ticks(&s));
    let cpu = ticks.and_then(|t| {
        let previous = lock(&d.samples).insert(g.vmid, (t, now));
        previous.map(|(old, at)| cpu_pct(t.saturating_sub(old), now.duration_since(at), g.cores))
    });
    let mem = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| parse_vmrss_kb(&s))
        .map(|kb| kb / 1024);
    Probe {
        status: if paused { GuestStatus::Paused } else { GuestStatus::Running },
        cpu_pct: cpu,
        mem_used_mb: mem,
    }
}
