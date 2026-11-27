//! Çalışan konukta disk işlemleri: anlık görüntü, denetim noktası, yedek ve yedekten ayırma.

use super::*;

/// Çalışan konuğun disklerini arkalıklarından bağımsızlaştırır: QEMU arkalıktaki veriyi diske akıtır
/// (`block-stream`), bitince bağ kendiliğinden kalkar. Konuk bu sırada çalışmayı sürdürür.
pub(in crate::qemu) fn stream_disks(d: &QemuDriver, g: &Guest, disks: &[crate::qemu::PathBuf]) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let blocks = qmp(&sock, &["query-block"])?;
    let devices = blocks.first().and_then(Value::as_array).ok_or("konuğun diskleri QEMU'da bulunamadı")?;
    let prefix = format!("redox-flat-{}-", g.vmid);
    for (i, disk) in disks.iter().enumerate() {
        let file = disk.display().to_string();
        let device = devices
            .iter()
            .find(|b| b["inserted"]["file"] == file.as_str())
            .and_then(|b| b["device"].as_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("disk QEMU'da bulunamadı: {file}"))?;
        let start = serde_json::json!({ "execute": "block-stream", "arguments": { "device": device, "job-id": format!("{prefix}{i}") } });
        qmp_values(&sock, &[start]).map_err(|e| format!("disk yedekten ayrılamadı: {e}"))?;
    }
    // İşler bitince listeden düşer; sonuç çağıran tarafta imajın kendisine bakılarak doğrulanır.
    let deadline = std::time::Instant::now() + Duration::from_secs(24 * 3600);
    loop {
        let jobs = qmp(&sock, &["query-block-jobs"])?;
        let busy = jobs.first().and_then(Value::as_array).is_some_and(|list| list.iter().any(|job| job["device"].as_str().is_some_and(|name| name.starts_with(&prefix))));
        if !busy {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline || !live_pid(d, g.vmid).is_some() {
            return Err("disklerin yedekten ayrılması tamamlanamadı (konuk kapandı ya da süre doldu)".to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Çalışan konuğun disklerinin, bellek durumu olmadan alınan anlık görüntüsü. Bütün diskler tek bir QMP
/// işlemiyle (transaction) aynı anda görüntülenir. Konuk ajanı varsa dosya sistemleri önce dondurulur ve
/// görüntü alınır alınmaz çözülür: görüntü, uygulamaların yarım yazdığı bir anı değil temiz bir anı içerir.
pub(in crate::qemu) fn disk_snapshot(d: &QemuDriver, g: &Guest, name: &str, disks: &[crate::qemu::PathBuf]) -> Result<bool, String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let blocks = qmp(&sock, &["query-block"])?;
    let devices = blocks.first().and_then(Value::as_array).ok_or("konuğun diskleri QEMU'da bulunamadı")?;
    let mut actions = Vec::new();
    for disk in disks {
        let file = disk.display().to_string();
        let device = devices
            .iter()
            .find(|b| b["inserted"]["file"] == file.as_str())
            .and_then(|b| b["device"].as_str())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("disk QEMU'da bulunamadı: {file}"))?;
        actions.push(serde_json::json!({ "type": "blockdev-snapshot-internal-sync", "data": { "device": device, "name": name } }));
    }
    // UEFI değişken deposu da (takılıysa) aynı işleme katılır: kapalıyken geri dönüş onu da arar.
    for secure in [false, true] {
        let file = d.cfg.efivars_path(g.vmid, secure).display().to_string();
        if let Some(device) = devices.iter().find(|b| b["inserted"]["file"] == file.as_str()).and_then(|b| b["device"].as_str()).filter(|n| !n.is_empty()) {
            actions.push(serde_json::json!({ "type": "blockdev-snapshot-internal-sync", "data": { "device": device, "name": name } }));
        }
    }
    let frozen = d.agent_call(g.vmid, "guest-fsfreeze-freeze", Duration::from_secs(60)).is_some();
    let taken = qmp_values(&sock, &[serde_json::json!({ "execute": "transaction", "arguments": { "actions": actions } })]).map(|_| ()).map_err(|e| format!("denetim noktası alınamadı: {e}"));
    if frozen {
        // Konuk donmuş bırakılmaz: çözme başarısızsa görüntü alınmış olsa da hata bildirilir.
        let qga = d.cfg.qga_path(g.vmid);
        let thawed = (0..5).any(|i| {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(500));
            }
            crate::qga::call(&qga, "guest-fsfreeze-thaw", Duration::from_secs(30)).is_ok()
        });
        if !thawed {
            return Err("konuğun dosya sistemi denetim noktasından sonra çözülemedi; konukta 'fsfreeze -u' çalıştırın ya da konuğu yeniden başlatın".to_string());
        }
    }
    taken.map(|()| frozen)
}

/// Çalışan konuğun disklerindeki değişen blok haritalarını siler (geri dönüşten önce; hata yok sayılır).
pub fn drop_bitmaps(d: &QemuDriver, g: &Guest) {
    let sock = d.cfg.qmp_path(g.vmid);
    let Ok(blocks) = qmp(&sock, &["query-block"]) else { return };
    for block in blocks.first().and_then(Value::as_array).into_iter().flatten() {
        let has = [&block["dirty-bitmaps"], &block["inserted"]["dirty-bitmaps"]].into_iter().filter_map(Value::as_array).flatten().any(|m| m["name"] == crate::qemu::CBT_BITMAP);
        if let (true, Some(device)) = (has, block["device"].as_str()) {
            let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "block-dirty-bitmap-remove", "arguments": { "node": device, "name": crate::qemu::CBT_BITMAP } })]);
        }
    }
}

/// Çalışan konukta anlık görüntü: disk ve bellek durumu birlikte kaydedilir (savevm).
pub fn live_snapshot(d: &QemuDriver, g: &Guest, op: SnapshotOp<'_>) -> Result<(), String> {
    match op {
        SnapshotOp::Create(name) => hmp(d, g, &format!("savevm {name}")),
        SnapshotOp::Rollback(name) => hmp(d, g, &format!("loadvm {name}")).map_err(|e| {
            format!("{e}; konuk kapalıyken alınmış bir görüntüye dönmek için önce konuğu kapatın")
        }),
        SnapshotOp::Delete(name) => hmp(d, g, &format!("delvm {name}")),
    }
}

/// Çalışan konuğun disklerini, konuğu durdurmadan hedef dosyalara kopyalar. Bütün kopyalar tek bir
/// QMP işlemiyle (transaction) başlar: hepsi aynı andaki tutarlı disk görüntüsüdür.
/// `parts`: (konuğun disk dosyası, hedef dosya). `raw`: sıkıştırılmış qcow2 yerine ham imaj.
///
/// Konukta ajan çalışıyorsa kopyalar başlamadan önce dosya sistemleri dondurulur ve kopyalar başlar
/// başlamaz çözülür: yedek, uygulamaların yarım yazdığı bir anı değil temiz bir anı içerir. Dönen değer
/// dondurmanın yapılıp yapılmadığıdır. Ajan yoksa yedek yine alınır (elektrik kesilmiş gibi tutarlıdır).
pub fn live_backup(d: &QemuDriver, g: &Guest, parts: &[(std::path::PathBuf, std::path::PathBuf)], raw: bool, mode: crate::qemu::Cbt) -> Result<bool, String> {
    // Yedek öncesi komut (ör. veritabanını tutarlı duruma getirir): başarısızsa yedek alınmaz, çünkü
    // komutu yazan kişi yedeğin o durumda alınmasını şart koşmuştur.
    let hook = |command: &str, when: &str| -> Result<(), String> {
        let Some((path, args)) = redox_core::hw::split_hook(command) else { return Ok(()) };
        match crate::qga::exec(&d.cfg.qga_path(g.vmid), &path, &args, Duration::from_secs(300)) {
            Ok((0, _)) => Ok(()),
            Ok((code, last)) => Err(format!("yedek {when} komutu {code} koduyla bitti: {path} ({last})")),
            Err(e) => Err(format!("yedek {when} komutu çalıştırılamadı: {e}")),
        }
    };
    hook(&g.hw.backup_pre, "öncesi").map_err(|e| format!("{e}; yedek alınmadı"))?;
    let mut post_done = g.hw.backup_post.is_empty();
    let mut post_result: Result<(), String> = Ok(());
    let frozen = d.agent_call(g.vmid, "guest-fsfreeze-freeze", Duration::from_secs(60)).is_some();
    let mut thawed = !frozen;
    // Çözme başarısız olursa konuk donmuş kalır: birkaç kez denenir ve sonuç mutlaka bildirilir.
    let thaw = |d: &QemuDriver| -> Result<(), String> {
        let sock = d.cfg.qga_path(g.vmid);
        let mut last = String::new();
        for _ in 0..5 {
            match crate::qga::call(&sock, "guest-fsfreeze-thaw", Duration::from_secs(30)) {
                Ok(_) => return Ok(()),
                Err(e) => last = e,
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(format!("konuğun dosya sistemi yedekten sonra çözülemedi; konukta 'fsfreeze -u' çalıştırın ya da konuğu yeniden başlatın ({last})"))
    };
    let outcome = live_copy(d, g, parts, raw, mode, &mut || {
        if !thawed {
            thawed = true;
            thaw(d)?;
        }
        // Kopyalar başladı: disk görüntüsü alındı, konuk normal çalışmasına dönebilir.
        if !post_done {
            post_done = true;
            post_result = hook(&g.hw.backup_post, "sonrası");
        }
        Ok(())
    });
    // Kopyalar başlayamadıysa da (hata) konuk donmuş bırakılmaz ve yedek sonrası komut yine çalıştırılır.
    let unfrozen = if thawed { Ok(()) } else { thaw(d) };
    if !post_done {
        post_result = hook(&g.hw.backup_post, "sonrası");
    }
    let post = post_result.map_err(|e| format!("{e}; yedek geçersiz sayıldı, komutu konukta elle çalıştırın"));
    outcome.and(unfrozen).and(post).map(|()| frozen)
}

/// `started`: kopyalar başladığı (anlık görüntü alındığı) an bir kez çağrılır.
pub(super) fn live_copy(
    d: &QemuDriver,
    g: &Guest,
    parts: &[(std::path::PathBuf, std::path::PathBuf)],
    raw: bool,
    mode: crate::qemu::Cbt,
    started: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let mut added = Vec::new();
    let outcome = copy_disks(d, g, parts, raw, mode, started, &mut added);
    let sock = d.cfg.qmp_path(g.vmid);
    for node in added {
        // İş bittikten hemen sonra düğüm kısa süre meşgul olabilir.
        let removed = wait_until(Duration::from_secs(10), || qmp_values(&sock, &[serde_json::json!({ "execute": "blockdev-del", "arguments": { "node-name": node } })]).is_ok() || live_pid(d, g.vmid).is_none());
        if !removed {
            eprintln!("uyarı: {} konuğunda yedek hedefi ({node}) kaldırılamadı", g.vmid);
        }
    }
    outcome
}

fn copy_disks(
    d: &QemuDriver,
    g: &Guest,
    parts: &[(std::path::PathBuf, std::path::PathBuf)],
    raw: bool,
    mode: crate::qemu::Cbt,
    started: &mut dyn FnMut() -> Result<(), String>,
    added: &mut Vec<String>,
) -> Result<(), String> {
    use crate::qemu::{Cbt, CBT_BITMAP};
    let sock = d.cfg.qmp_path(g.vmid);
    // Sürücü adı QEMU sürümüne göre değişebilir: diskler dosya yollarından bulunur.
    let blocks = qmp(&sock, &["query-block"])?;
    let devices = blocks.first().and_then(Value::as_array).ok_or("konuğun diski QEMU'da bulunamadı")?;
    let mut nonce = [0u8; 4];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("rastgele sayı üretilemedi: {e}"))?;
    let attempt = u32::from_le_bytes(nonce);
    // Harita işlemleri kopyalarla aynı QMP işlemindedir: harita tam olarak kopyanın alındığı anda sıfırlanır.
    let (mut bitmaps, mut actions, mut pending) = (Vec::new(), Vec::new(), Vec::new());
    for (i, (source, target)) in parts.iter().enumerate() {
        let file = source.display().to_string();
        let block = devices.iter().find(|b| b["inserted"]["file"] == file.as_str());
        let device = block
            .and_then(|b| b["device"].as_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| if i == 0 { "konuğun diski QEMU'da bulunamadı".to_string() } else { format!("konuğun ek diski QEMU'da bulunamadı: {file}") })?;
        // Haritalar QEMU sürümüne göre aygıtın ya da takılı imajın altında listelenir.
        let bitmap = block.and_then(|b| {
            [&b["dirty-bitmaps"], &b["inserted"]["dirty-bitmaps"]].into_iter().filter_map(Value::as_array).flatten().find(|m| m["name"] == CBT_BITMAP)
        });
        let on_bitmap = |kind: &str, extra: Value| {
            let mut data = serde_json::json!({ "node": device, "name": CBT_BITMAP });
            if let (Some(data), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
                data.extend(extra.clone());
            }
            serde_json::json!({ "type": kind, "data": data })
        };
        match (mode, bitmap) {
            (Cbt::Off, _) => {}
            // Düzgün kapanmamış konuğun haritası tutarsızdır: silinip yeniden oluşturulur.
            (Cbt::Arm, Some(b)) if b["inconsistent"] == true => {
                bitmaps.push(on_bitmap("block-dirty-bitmap-remove", Value::Null));
                bitmaps.push(on_bitmap("block-dirty-bitmap-add", serde_json::json!({ "persistent": true })));
            }
            (Cbt::Arm, Some(_)) => bitmaps.push(on_bitmap("block-dirty-bitmap-clear", Value::Null)),
            (Cbt::Arm, None) => bitmaps.push(on_bitmap("block-dirty-bitmap-add", serde_json::json!({ "persistent": true }))),
            (Cbt::Delta, Some(b)) if b["inconsistent"] != true && b["recording"] != false && b["persistent"] != false && b["busy"] != true => {}
            (Cbt::Delta, _) => return Err("değişen blok haritası yok ya da tutarsız (konuk düzgün kapanmamış olabilir)".to_string()),
        }
        let delta = mode == Cbt::Delta;
        // Şifreli konuk: QEMU'nun kendisinin oluşturacağı hedef şifresiz olurdu. Hedef önceden şifreli kurulur,
        // anahtarla eklenir ve yedek onun içine yazılır.
        let sealed = if g.hw.encrypted {
            if raw || delta {
                return Err("şifreli diskli konukta ham ya da değişen blok yedeği alınmaz".to_string());
            }
            let size = block.and_then(|b| b["inserted"]["image"]["virtual-size"].as_u64()).ok_or("diskin boyutu okunamadı")?;
            d.create_encrypted_bytes(target, size)?;
            let node = format!("yedek-{attempt}-{i}");
            let add = serde_json::json!({ "execute": "blockdev-add", "arguments": {
                "driver": "qcow2", "node-name": node, "encrypt": { "format": "luks", "key-secret": crate::qemu::SECRET_ID },
                "file": { "driver": "file", "filename": target.display().to_string() } } });
            qmp_values(&sock, &[add]).map_err(|e| format!("yedekleme başlatılamadı: {e}"))?;
            added.push(node.clone());
            Some(node)
        } else {
            None
        };
        // İş adı her denemede farklıdır: önceki bir denemeden kalmış işin olayları bizimkilerle karışamaz.
        let job = format!("redox-backup-{}-{i}-{attempt}", g.vmid);
        let mut data = serde_json::json!({
            "job-id": job, "device": device, "target": target.display().to_string(),
            "format": if raw && !delta { "raw" } else { "qcow2" }, "sync": if delta { "incremental" } else { "full" },
        });
        if d.cfg.backup_mbps > 0 {
            // Çalışan konuk: QEMU'nun yedek işi bu hızı (bayt/sn) aşmaz.
            data["speed"] = (d.cfg.backup_mbps * 1024 * 1024).into();
        }
        if delta {
            // İş başarıyla biterse QEMU kopyalanan bitleri haritadan siler; başarısız olursa harita olduğu gibi kalır.
            data["bitmap"] = CBT_BITMAP.into();
        } else if !raw {
            data["compress"] = true.into();
        }
        match &sealed {
            None => actions.push(serde_json::json!({ "type": "drive-backup", "data": data })),
            Some(node) => {
                let mut sealed_data = serde_json::json!({ "job-id": job, "device": device, "target": node, "sync": "full" });
                if let Some(speed) = data.get("speed") {
                    sealed_data["speed"] = speed.clone();
                }
                actions.push(serde_json::json!({ "type": "blockdev-backup", "data": sealed_data }));
            }
        }
        pending.push(job);
    }
    let actions: Vec<Value> = bitmaps.into_iter().chain(actions).collect();
    let start = serde_json::json!({ "execute": "transaction", "arguments": { "actions": actions } });
    let cancel = |job: &str| serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": job } });
    // Önceki denemeden kalmış (ör. sunucu yedek sırasında yeniden başlamış) işler diskleri meşgul tutar: durdurulur.
    let prefix = format!("redox-backup-{}-", g.vmid);
    if let Ok(reply) = qmp(&sock, &["query-block-jobs"]) {
        let stale: Vec<String> = reply
            .first()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|job| job["device"].as_str())
            .filter(|name| name.starts_with(&prefix))
            .map(str::to_string)
            .collect();
        for job in &stale {
            let _ = qmp_values(&sock, &[cancel(job)]);
        }
        if !stale.is_empty() {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    let all_jobs = pending.clone();

    let err = |e: std::io::Error| format!("QMP ({}): {e}", sock.display());
    let stream = UnixStream::connect(&sock).map_err(err)?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(err)?;
    let mut writer = stream.try_clone().map_err(err)?;
    let mut reader = BufReader::new(stream);
    let deadline = Instant::now() + Duration::from_secs(6 * 3600);
    // Okuma zaman aşımı yalnızca "henüz satır yok" demektir; iş bitene kadar beklenir.
    let next = |reader: &mut BufReader<UnixStream>| -> Result<Value, String> {
        let mut line = String::new();
        loop {
            match reader.read_line(&mut line) {
                Ok(0) => return Err("QMP bağlantısı kapandı (konuk kapanmış olabilir)".to_string()),
                Ok(_) => return serde_json::from_str(line.trim()).map_err(|e| format!("QMP yanıtı okunamadı: {e}")),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    if Instant::now() >= deadline {
                        return Err("yedekleme zaman aşımına uğradı".to_string());
                    }
                }
                Err(e) => return Err(err(e)),
            }
        }
    };
    let mut send = |command: &Value| writer.write_all(format!("{command}\n").as_bytes()).map_err(err);
    let outcome = (|| -> Result<(), String> {
        next(&mut reader)?; // karşılama
        send(&serde_json::json!({ "execute": "qmp_capabilities" }))?;
        // 0: yetenek yanıtı bekleniyor; 1: işlemin başlama yanıtı bekleniyor; 2: işler sürüyor.
        let mut stage = 0;
        let mut failure: Option<String> = None;
        loop {
            let message = next(&mut reader)?;
            if stage < 2 {
                if let Some(error) = message.get("error") {
                    return Err(format!("yedekleme başlatılamadı: {}", error["desc"].as_str().unwrap_or("bilinmeyen hata")));
                }
                if message.get("return").is_some() {
                    if stage == 0 {
                        send(&start)?;
                    } else {
                        // Kopyalar başladı: bu andaki disk görüntüsü alındı, konuk yazmaya devam edebilir.
                        started()?;
                    }
                    stage += 1;
                    // Kopyalanacak bir şey yoksa işler, başlama yanıtı gelmeden bitmiş olabilir.
                    if stage == 2 && pending.is_empty() {
                        return failure.map_or(Ok(()), Err);
                    }
                    continue;
                }
                if stage == 0 {
                    continue;
                }
                // Başlama isteği gönderildi: QEMU, hızlı biten işin "bitti" olayını yanıttan önce yollayabilir.
            }
            // Bu aşamadaki yanıtlar (durdurma isteklerine) önemsizdir; yalnızca kendi işlerimizin olayları izlenir.
            let (event, data) = (message["event"].as_str().unwrap_or_default(), &message["data"]);
            let Some(job) = data["device"].as_str().filter(|job| pending.iter().any(|p| p == job)).map(str::to_string) else {
                continue;
            };
            let failed = match event {
                "BLOCK_JOB_COMPLETED" => {
                    pending.retain(|p| *p != job);
                    data["error"].as_str().map(|e| format!("yedekleme başarısız: {e}"))
                }
                "BLOCK_JOB_CANCELLED" => {
                    pending.retain(|p| *p != job);
                    Some("yedekleme yarıda kesildi".to_string())
                }
                "BLOCK_JOB_ERROR" => Some("yedekleme yarıda kesildi".to_string()),
                _ => None,
            };
            if let (Some(reason), None) = (failed, &failure) {
                failure = Some(reason);
                // Bir disk kopyalanamadıysa yedek geçersizdir: süren diğer kopyalar da durdurulur.
                for other in &pending {
                    send(&cancel(other))?;
                }
            }
            // Başlama yanıtı (ve konuğun çözülmesi) beklenmeden çıkılmaz.
            if pending.is_empty() && stage == 2 {
                return failure.map_or(Ok(()), Err);
            }
        }
    })();
    // QMP aynı anda tek istemciye hizmet verir: yeni bağlantı açmadan önce bu bağlantı kapatılır.
    drop(reader);
    drop(writer);
    if outcome.is_err() {
        // Zaman aşımı, kopan bağlantı ya da okunamayan yanıtla çıkılıyorsa işler QEMU'da sürüyor olabilir:
        // hedef dosyalar silinmeden önce durdurulurlar (bitmiş iş için istek zararsızca reddedilir).
        for job in &all_jobs {
            let _ = qmp_values(&sock, &[cancel(job)]);
        }
    }
    outcome
}
