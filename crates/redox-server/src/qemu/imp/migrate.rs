//! Canlı taşıma: çalışan konuk, kapatılmadan başka bir sunucudaki QEMU'ya geçer.
//!
//! Hedef QEMU konuğun kaydındaki aynı donanımla, duraklatılmış ve taşıma bekler halde açılır; boş disklerini
//! ağdan yazılabilir sunar (NBD). Kaynak QEMU her diskini hedefe aynalar; aynalar yetişince belleği taşır ve
//! son anda kısa süre duraklar. Bellek hedefe varınca kaynak aynaları bırakıp kapanır, hedef konuğu sürdürür.
//! İki bağlantı da o taşıma için üretilen tek kullanımlık bir anahtarla şifrelenir (TLS, ön paylaşımlı anahtar).

use super::*;

const TLS_ID: &str = "migtls";
const JOB_PREFIX: &str = "mig-";

fn tls_dir(d: &QemuDriver, vmid: VmId) -> std::path::PathBuf {
    d.cfg.run_dir().join(format!("{vmid}.mig"))
}

/// Anahtarı yalnızca hizmet kullanıcısının okuyabildiği bir dosyaya yazar ve QEMU'ya şifreleme nesnesi olarak tanıtır.
fn add_tls(d: &QemuDriver, vmid: VmId, key: &str, endpoint: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if !redox_core::valid_migrate_key(key) {
        return Err("taşıma anahtarı geçersiz".to_string());
    }
    let dir = tls_dir(d, vmid);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(dir.join("keys.psk")).map_err(|e| format!("taşıma anahtarı yazılamadı: {e}"))?;
    file.write_all(format!("qemu:{key}\n").as_bytes()).map_err(|e| format!("taşıma anahtarı yazılamadı: {e}"))?;
    let object = serde_json::json!({ "execute": "object-add", "arguments": { "qom-type": "tls-creds-psk", "id": TLS_ID, "endpoint": endpoint, "dir": dir.display().to_string() } });
    qmp_values(&d.cfg.qmp_path(vmid), &[object]).map(|_| ())
}

/// Konuğun disklerinin QEMU'daki aygıt adları: önce asıl disk, sonra ek diskler. Aynı kayıttan açılan iki
/// QEMU'da adlar aynıdır; diskler iki sunucu arasında bu adlarla eşleşir.
fn disk_devices(d: &QemuDriver, g: &Guest) -> Result<Vec<String>, String> {
    let reply = qmp(&d.cfg.qmp_path(g.vmid), &["query-block"])?;
    let blocks = reply.first().and_then(Value::as_array).ok_or("konuğun diskleri QEMU'dan okunamadı")?;
    let mut wanted = vec![d.cfg.disk_of(g)];
    wanted.extend(g.hw.disks.iter().map(|disk| d.cfg.extra_disk_of(g, disk.id)));
    wanted
        .iter()
        .map(|path| {
            let file = path.display().to_string();
            blocks
                .iter()
                .find(|block| block["inserted"]["file"] == file.as_str())
                .and_then(|block| block["device"].as_str())
                .filter(|device| !device.is_empty())
                .map(str::to_string)
                .ok_or(format!("konuğun diski QEMU'da bulunamadı: {file}"))
        })
        .collect()
}

fn mirror_jobs(sock: &Path) -> Result<Vec<Value>, String> {
    let reply = qmp(sock, &["query-block-jobs"])?;
    Ok(reply.first().and_then(Value::as_array).map(|jobs| jobs.iter().filter(|job| job["device"].as_str().is_some_and(|id| id.starts_with(JOB_PREFIX))).cloned().collect()).unwrap_or_default())
}

fn kill_and_clean(d: &QemuDriver, vmid: VmId) {
    let sock = d.cfg.qmp_path(vmid);
    let alive = || live_pid(d, vmid).is_some();
    if alive() {
        let _ = qmp(&sock, &["quit"]);
        if !wait_until(Duration::from_secs(10), || !alive()) {
            if let Some(pid) = live_pid(d, vmid) {
                let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
            }
            wait_until(Duration::from_secs(3), || !alive());
        }
    }
    cleanup(d, vmid);
    let _ = std::fs::remove_dir_all(tls_dir(d, vmid));
}

/// Çalışan konuğun açıldığı donanım yerleşimi; konuk çalışmıyorsa `None`. Hedef aynı yerleşimle açılmalıdır.
pub fn migrate_layout(d: &QemuDriver, g: &Guest) -> Option<u8> {
    live_pid(d, g.vmid).map(|_| running_layout(d, g.vmid))
}

/// Hedef: konuğu taşıma bekler halde (duraklatılmış) açar, disklerini `nbd_port`'tan yazılabilir sunar ve
/// belleği `ram_port`'tan bekler.
pub fn migrate_listen(d: &QemuDriver, g: &Guest, layout: u8, bind: &str, nbd_port: u16, ram_port: u16, key: &str) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let alive = || live_pid(d, g.vmid).is_some();
    if alive() {
        return Err("konuk zaten çalışıyor".to_string());
    }
    if layout > crate::qemu::LAYOUT_CURRENT {
        return Err("kaynak sunucu daha yeni bir donanım yerleşimi kullanıyor; önce bu sunucuyu güncelleyin".to_string());
    }
    if !redox_core::valid_migrate_host(bind) {
        return Err(format!("geçersiz dinleme adresi: {bind}"));
    }
    if !d.cfg.disk_of(g).exists() {
        return Err(format!("disk imajı bulunamadı: {}", d.cfg.disk_of(g).display()));
    }
    cleanup(d, g.vmid);
    crate::qemu::write_seed(&d.cfg, g)?;
    if let Err(e) = prepare_vlan_taps(d, g) {
        remove_vlan_taps(d, g.vmid);
        return Err(e);
    }
    let mut args = crate::qemu::build_args_layout(&d.cfg, g, layout);
    // Konuk kendiliğinden başlamaz: bellek gelince duraklatılmış bekler, kaynak işini bitirince sürdürülür.
    args.extend(["-incoming".to_string(), "defer".to_string(), "-S".to_string()]);
    let started = std::process::Command::new(&d.cfg.qemu_bin)
        .args(args)
        .output()
        .map_err(|e| format!("{} çalıştırılamadı: {e}", d.cfg.qemu_bin))
        .and_then(|out| if out.status.success() { Ok(()) } else { Err(format!("QEMU başlatılamadı: {}", last_line(&out.stderr))) })
        .and_then(|()| if wait_until(Duration::from_secs(5), alive) { Ok(()) } else { Err("QEMU başladı ama süreç bulunamadı (hemen çıkmış olabilir)".to_string()) });
    if let Err(e) = started {
        remove_vlan_taps(d, g.vmid);
        return Err(e);
    }
    let address = |port: u16| if bind.contains(':') { format!("tcp:[{bind}]:{port}") } else { format!("tcp:{bind}:{port}") };
    let prepared = limit_network(d, g).and_then(|()| {
        add_tls(d, g.vmid, key, "server")?;
        let listen = serde_json::json!({ "execute": "nbd-server-start", "arguments": { "addr": { "type": "inet", "data": { "host": bind, "port": nbd_port.to_string() } }, "tls-creds": TLS_ID } });
        qmp_values(&sock, &[listen]).map_err(|e| format!("disk kapısı ({nbd_port}) açılamadı: {e}"))?;
        for device in disk_devices(d, g)? {
            qmp_values(&sock, &[serde_json::json!({ "execute": "nbd-server-add", "arguments": { "device": device, "writable": true } })])?;
        }
        let tls = serde_json::json!({ "execute": "migrate-set-parameters", "arguments": { "tls-creds": TLS_ID } });
        let incoming = serde_json::json!({ "execute": "migrate-incoming", "arguments": { "uri": address(ram_port) } });
        qmp_values(&sock, &[tls, incoming]).map(|_| ()).map_err(|e| format!("bellek kapısı ({ram_port}) açılamadı: {e}"))
    });
    prepared.map_err(|e| {
        kill_and_clean(d, g.vmid);
        format!("hedef taşımaya hazırlanamadı: {e}")
    })
}

/// Hedef: bellek geldi ve kaynak kapandı; disk sunumu kapatılır, konuk sürdürülür.
pub fn migrate_finish(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    if live_pid(d, g.vmid).is_none() {
        return Err("taşımayı bekleyen QEMU kapanmış".to_string());
    }
    let status = qmp(&sock, &["query-status"])?.first().and_then(|s| s["status"].as_str().map(str::to_string)).unwrap_or_default();
    if status != "paused" {
        return Err(format!("bellek hedefe tam ulaşmamış (QEMU durumu: {status}); konuk sürdürülmedi"));
    }
    qmp(&sock, &["nbd-server-stop"])?;
    qmp(&sock, &["cont"])?;
    let _ = std::fs::remove_dir_all(tls_dir(d, g.vmid));
    Ok(())
}

/// Hedef: taşıma olmadı; bekleyen QEMU kapatılır.
pub fn migrate_abort(d: &QemuDriver, g: &Guest) {
    kill_and_clean(d, g.vmid);
}

/// Kaynak: diskleri hedefe aynalar, aynalar yetişince belleği taşır. Başarıda konuk burada kapanmıştır ve
/// duraklama süresi (ms) döner. Hata, bellek hedefe geçmeden olursa konuk burada çalışmayı sürdürür.
pub fn migrate_send(d: &QemuDriver, g: &Guest, host: &str, nbd_port: u16, ram_port: u16, key: &str) -> Result<u64, String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let alive = || live_pid(d, g.vmid).is_some();
    if !alive() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    if !redox_core::valid_migrate_host(host) {
        return Err(format!("geçersiz hedef adresi: {host}"));
    }
    let devices = disk_devices(d, g)?;
    let node = |device: &str| format!("{JOB_PREFIX}{device}");
    // Taşıma olmadıysa geride iz bırakılmaz: aynalar durdurulur, hedefe açılan bağlantılar ve anahtar kaldırılır.
    let undo = || {
        let _ = qmp(&sock, &["migrate_cancel"]);
        for device in &devices {
            let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": node(device), "force": true } })]);
        }
        wait_until(Duration::from_secs(15), || mirror_jobs(&sock).is_ok_and(|jobs| jobs.is_empty()) || !alive());
        for device in &devices {
            let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "blockdev-del", "arguments": { "node-name": node(device) } })]);
        }
        let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "object-del", "arguments": { "id": TLS_ID } })]);
        let _ = std::fs::remove_dir_all(tls_dir(d, g.vmid));
    };
    let all_ready = |jobs: &[Value]| jobs.len() == devices.len() && jobs.iter().all(|job| job["ready"] == true);
    let moved = (|| -> Result<u64, String> {
        add_tls(d, g.vmid, key, "client")?;
        for device in &devices {
            let target = serde_json::json!({ "execute": "blockdev-add", "arguments": {
                "driver": "nbd", "node-name": node(device), "export": device, "tls-creds": TLS_ID,
                "server": { "type": "inet", "host": host, "port": nbd_port.to_string() } } });
            qmp_run(&sock, &[target], Duration::from_secs(30)).map_err(|e| format!("hedef sunucunun disk kapısına ({host}:{nbd_port}) bağlanılamadı: {e}"))?;
            let mirror = serde_json::json!({ "execute": "blockdev-mirror", "arguments": { "job-id": node(device), "device": device, "target": node(device), "sync": "full", "copy-mode": "write-blocking" } });
            qmp_values(&sock, &[mirror]).map_err(|e| format!("disk aynalanamadı: {e}"))?;
        }
        // Diskler: ilk kopya bitene ve aynalar konuğun yazdıklarına yetişene kadar beklenir.
        let deadline = Instant::now() + Duration::from_secs(48 * 3600);
        loop {
            let jobs = mirror_jobs(&sock)?;
            if all_ready(&jobs) {
                break;
            }
            if jobs.len() < devices.len() {
                return Err("disk aynalama yarıda kesildi (hedefe yazılamıyor ya da bağlantı koptu)".to_string());
            }
            if !alive() {
                return Err("konuk taşıma sırasında kapandı".to_string());
            }
            if Instant::now() >= deadline {
                return Err("diskler zamanında aynalanamadı".to_string());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        // Bellek: QEMU değişen sayfaları yollar, kalan az olunca konuğu kısa süre durdurup taşımayı bitirir.
        let uri = if host.contains(':') { format!("tcp:[{host}]:{ram_port}") } else { format!("tcp:{host}:{ram_port}") };
        let tls = serde_json::json!({ "execute": "migrate-set-parameters", "arguments": { "tls-creds": TLS_ID } });
        qmp_values(&sock, &[tls, serde_json::json!({ "execute": "migrate", "arguments": { "uri": uri } })]).map_err(|e| format!("bellek taşıması başlatılamadı: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(12 * 3600);
        loop {
            let info = qmp(&sock, &["query-migrate"]).ok().and_then(|r| r.into_iter().next());
            match info.as_ref().and_then(|m| m["status"].as_str()) {
                Some("completed") => return Ok(info.as_ref().and_then(|m| m["downtime"].as_u64()).unwrap_or(0)),
                Some("failed") => return Err(format!("bellek taşınamadı: {}", info.as_ref().and_then(|m| m["error-desc"].as_str()).unwrap_or("hedef kabul etmedi"))),
                Some("cancelled") => return Err("bellek taşıması iptal edildi".to_string()),
                _ if !alive() => return Err("konuk taşıma sırasında kapandı".to_string()),
                _ if Instant::now() >= deadline => return Err("bellek zamanında taşınamadı (konuk belleği çok hızlı değiştiriyor olabilir)".to_string()),
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    })();
    let downtime = match moved {
        Ok(downtime) => downtime,
        Err(e) => {
            undo();
            return Err(e);
        }
    };
    // Bellek hedefte ve konuk burada durdu. Aynalar hâlâ eşitse hedefin diskleri tamdır; değilse hedef eksik
    // diskle sürdürülmemelidir: konuk burada yeniden çalıştırılır.
    if !mirror_jobs(&sock).is_ok_and(|jobs| all_ready(&jobs)) {
        let resumed = qmp(&sock, &["cont"]).is_ok();
        undo();
        return Err(if resumed { "bellek taşındı ama disk aynası koptu; konuk bu sunucuda sürdürüldü" } else { "bellek taşındı ama disk aynası koptu; konuk bu sunucuda duraklatılmış durumda" }.to_string());
    }
    for device in &devices {
        let _ = qmp_values(&sock, &[serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": node(device) } })]);
    }
    wait_until(Duration::from_secs(30), || mirror_jobs(&sock).is_ok_and(|jobs| jobs.is_empty()) || !alive());
    kill_and_clean(d, g.vmid);
    Ok(downtime)
}
