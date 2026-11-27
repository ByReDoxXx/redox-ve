//! Çalışan konuğun disklerini başka bir depolama alanına taşıma.
//!
//! QEMU her diski yeni yerindeki dosyaya aynalar (konuk o sırada çalışır ve yazar); aynalar yetişince disk
//! yeni dosyaya geçirilir ve eski dosya silinir. Konuk hiç durmaz. Aynalama yalnızca diskin şimdiki içeriğini
//! taşır: dosyanın içindeki anlık görüntüler gelmez, bu yüzden görüntüsü olan konuk bu yolla taşınmaz.

use super::*;

const JOB_PREFIX: &str = "tasi-";

/// Aygıtın şu an kullandığı dosya.
fn device_file(sock: &Path, device: &str) -> Option<String> {
    let reply = qmp(sock, &["query-block"]).ok()?;
    reply.first()?.as_array()?.iter().find(|block| block["device"] == device).and_then(|block| block["inserted"]["file"].as_str().map(str::to_string))
}

/// Çalışan konuğun bütün disklerini `target` alanına taşır. Başarıda diskler yeni alandadır ve eski dosyalar
/// silinmiştir. Hata olursa geçmemiş diskler eski yerinde kalır; bir kısmı geçtiyse bunu açıkça söyler.
pub fn move_storage_live(d: &QemuDriver, g: &Guest, target: &str) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let alive = || live_pid(d, g.vmid).is_some();
    if !alive() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    let dest_dir = d.cfg.storage_dir(target);
    if dest_dir == d.cfg.storage_dir(&g.storage) {
        return Err(format!("{target} alanı konuğun şimdiki alanıyla aynı dizinde"));
    }
    std::fs::create_dir_all(&dest_dir).map_err(|e| format!("{} oluşturulamadı: {e}", dest_dir.display()))?;
    let mut sources = vec![d.cfg.disk_of(g)];
    sources.extend(g.hw.disks.iter().map(|disk| d.cfg.extra_disk_of(g, disk.id)));
    let blocks = qmp(&sock, &["query-block"])?;
    let blocks = blocks.first().and_then(Value::as_array).ok_or("konuğun diskleri QEMU'dan okunamadı")?;
    // (aygıt, eski dosya, yeni dosya). Önceki yarım taşımada yeni alana geçmiş disk yeniden taşınmaz.
    let mut plan = Vec::new();
    for source in &sources {
        let name = source.file_name().and_then(|n| n.to_str()).ok_or("disk adı okunamadı")?;
        let dest = dest_dir.join(name);
        let uses = |path: &Path| blocks.iter().find(|b| b["inserted"]["file"] == path.display().to_string().as_str()).and_then(|b| b["device"].as_str()).filter(|device| !device.is_empty()).map(str::to_string);
        if uses(&dest).is_some() {
            continue;
        }
        let device = uses(source).ok_or(format!("konuğun diski QEMU'da bulunamadı: {}", source.display()))?;
        if dest.exists() {
            return Err(format!("{} zaten var; önce onu kaldırın", dest.display()));
        }
        plan.push((device, source.clone(), dest));
    }
    let job = |device: &str| format!("{JOB_PREFIX}{device}");
    let cancel = |upto: usize| {
        for (device, _, dest) in plan.iter().take(upto) {
            let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": job(device), "force": true } })]);
            wait_until(Duration::from_secs(15), || block_jobs(&sock, JOB_PREFIX).is_ok_and(|jobs| !jobs.iter().any(|j| j["device"] == job(device).as_str())) || !alive());
            let _ = std::fs::remove_file(dest);
        }
    };
    // Aynalar: her disk yeni yerine kopyalanır, konuğun o sırada yazdıkları iki yere birden gider.
    for (i, (device, _, dest)) in plan.iter().enumerate() {
        let mirror = serde_json::json!({ "execute": "drive-mirror", "arguments": {
            "job-id": job(device), "device": device, "target": dest.display().to_string(), "format": "qcow2", "sync": "full", "mode": "absolute-paths", "copy-mode": "write-blocking" } });
        if let Err(e) = qmp_run(&sock, &[mirror], Duration::from_secs(60)) {
            cancel(i + 1);
            return Err(format!("disk yeni alana aynalanamadı: {e}"));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(48 * 3600);
    loop {
        let jobs = match block_jobs(&sock, JOB_PREFIX) {
            Ok(jobs) => jobs,
            Err(e) => {
                cancel(plan.len());
                return Err(e);
            }
        };
        if jobs.len() == plan.len() && jobs.iter().all(|j| j["ready"] == true) {
            break;
        }
        let problem = if jobs.len() < plan.len() {
            Some("disk aynalama yarıda kesildi (hedef alan dolmuş ya da yazılamıyor olabilir)")
        } else if !alive() {
            Some("konuk taşıma sırasında kapandı")
        } else if Instant::now() >= deadline {
            Some("diskler zamanında aynalanamadı")
        } else {
            None
        };
        if let Some(problem) = problem {
            cancel(plan.len());
            return Err(problem.to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // Geçiş: her disk yeni dosyasına alınır. Geçen diskin eski dosyası bayatlar ve hemen silinir: yarıda
    // kalan bir taşımadan sonra konuk eski alandan açılırsa bayat diskle değil, açık bir hatayla karşılaşılır.
    for (i, (device, source, dest)) in plan.iter().enumerate() {
        let wanted = dest.display().to_string();
        let switched = qmp_values(&sock, &[serde_json::json!({ "execute": "block-job-complete", "arguments": { "device": job(device) } })])
            .map(|_| wait_until(Duration::from_secs(120), || device_file(&sock, device).as_deref() == Some(wanted.as_str()) || !alive()) && device_file(&sock, device).as_deref() == Some(wanted.as_str()));
        if !matches!(switched, Ok(true)) {
            // Kalan aynalar bırakılır; geçmemiş diskler eski yerinde çalışmayı sürdürür.
            for (rest, _, rest_dest) in plan.iter().skip(i) {
                let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": job(rest), "force": true } })]);
                if device_file(&sock, rest).as_deref() != Some(rest_dest.display().to_string().as_str()) {
                    let _ = std::fs::remove_file(rest_dest);
                }
            }
            let why = switched.err().unwrap_or_else(|| "QEMU yeni dosyaya geçmedi".to_string());
            return Err(if i == 0 {
                format!("diskler yeni alana geçirilemedi: {why}; konuk eski alanında çalışmayı sürdürüyor")
            } else {
                format!("{i} disk yeni alana geçti, kalanı geçirilemedi: {why}. Konuk çalışıyor ama diskleri iki alana bölündü: konuğu KAPATMADAN taşımayı yeniden deneyin")
            });
        }
        let _ = std::fs::remove_file(source);
    }
    wait_until(Duration::from_secs(30), || block_jobs(&sock, JOB_PREFIX).is_ok_and(|jobs| jobs.is_empty()) || !alive());
    Ok(())
}
