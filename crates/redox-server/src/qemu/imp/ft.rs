//! Hata toleransı (QEMU COLO) adımları. Sıra gerçek QEMU'da doğrulanmıştır:
//!
//! 1. Yedek sunucuda ikiz açılır (`standby_start`): boş diskler, taşıma bekler halde.
//! 2. Birincil, diskini ikizin asıl kopyasına yansıtır ve yetişmesini bekler (`sync`).
//! 3. Birincil konuk kısa süre durdurulur; yansıtma bırakılır, ağ süzgeçleri ve ikinci disk çocuğu takılır ve
//!    COLO taşıması başlatılır (`engage`). Bundan sonra ikiz eşzamanlı çalışır.
//! 4. Bir taraf kaybolursa kalan tarafta `drop_secondary` ya da `take_over` çalışır.
//!
//! İkiz donarsa birincil çalışmayı sürdürür ama **ağdan hiçbir şey gönderemez** (her paket ikizinkiyle
//! karşılaştırılır); `drop_secondary` bu yüzden önce bağlantıları zorla keser.
//!
//! Şifreleme: disk yazmaları ve bellek aktarımı tek kullanımlık anahtarla TLS'lidir; paket karşılaştırma
//! kanalları şifresizdir (konuğun ağ trafiğinin bir kopyası o iki bağlantıdan geçer).

use super::*;
use crate::qemu::ft_args::{secondary_args, PrimaryLink, DISK_ID, PARENT_ID};
use redox_core::{FtLink, FtStatus};
use std::path::PathBuf;

const TLS_ID: &str = "fttls";
const SYNC_JOB: &str = "ft-esitle";
const NBD_CHILD: &str = "replication0";
/// Birincilin sonradan takılan süzgeçleri (kaldırma sırasıyla) ve karakter aygıtları.
const PRIMARY_OBJECTS: [&str; 5] = ["comp0", "m0", "redire0", "redire1", "iothread1"];
const PRIMARY_CHARDEVS: [&str; 6] = ["compare_out0", "compare0-0", "compare_out", "compare0", "compare1", "mirror0"];

fn ft_dir(d: &QemuDriver, vmid: VmId) -> PathBuf {
    d.cfg.data_dir.join("ft").join(vmid.to_string())
}

fn tls_dir(d: &QemuDriver, vmid: VmId) -> PathBuf {
    d.cfg.run_dir().join(format!("{vmid}.ft"))
}

/// Paket kapılarının (giden paketlerin kopyası ve karşılaştırma) anahtar dizini. Asıl dizinden ayrıdır: aynı
/// QEMU bir kapıda sunucu, öbüründe istemcidir ve her uç kendi nesnesini ister.
fn packet_tls_dir(d: &QemuDriver, vmid: VmId) -> PathBuf {
    d.cfg.run_dir().join(format!("{vmid}.ftp"))
}

/// Birincilin kendi içindeki karşılaştırma kapıları: ağ kapısı değil, çalışma dizinindeki (0700) soketler.
/// Yerel kapıda olsalardı sunucudaki başka bir hesap bağlanıp konuğun ağına paket sokabilirdi.
fn local_socket(d: &QemuDriver, vmid: VmId, name: &str) -> PathBuf {
    d.cfg.run_dir().join(format!("{vmid}.ft-{name}"))
}

/// Birincilde paket kapılarının şifreleme nesnesi.
const PACKET_TLS_ID: &str = crate::qemu::ft_args::PACKET_TLS_ID;

fn run(sock: &Path, command: Value) -> Result<Value, String> {
    qmp_run(sock, &[command], Duration::from_secs(30)).map(|mut r| r.pop().unwrap_or(Value::Null))
}

/// Hata olsa da sürdürülen temizlik komutu.
fn quietly(sock: &Path, command: Value) {
    let _ = qmp_values(sock, &[command]);
}

fn hmp_line(sock: &Path, line: &str) -> Result<String, String> {
    Ok(run(sock, serde_json::json!({ "execute": "human-monitor-command", "arguments": { "command-line": line } }))?.as_str().unwrap_or_default().trim().to_string())
}

fn colo_capability(sock: &Path) -> Result<(), String> {
    run(sock, serde_json::json!({ "execute": "migrate-set-capabilities", "arguments": { "capabilities": [{ "capability": "x-colo", "state": true }] } })).map(drop)
}

fn address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("tcp:[{host}]:{port}")
    } else {
        format!("tcp:{host}:{port}")
    }
}

pub fn ft_status(d: &QemuDriver, g: &Guest) -> Option<FtStatus> {
    live_pid(d, g.vmid)?;
    let sock = d.cfg.qmp_path(g.vmid);
    // Yanıt vermeyen süreç "yok" sayılmaz: anlık bir gecikme yüzünden sağlam bir ikiz silinmemeli.
    let ask = |command: &str| qmp(&sock, &[command]).ok().and_then(|mut r| r.pop());
    let (Some(colo), Some(state)) = (ask("query-colo-status"), ask("query-status")) else {
        return Some(FtStatus { unresponsive: true, ..Default::default() });
    };
    Some(FtStatus {
        unresponsive: false,
        engaged: colo["mode"] == "primary" || colo["mode"] == "secondary",
        // Denetim noktaları sırasında QEMU "colo" ve "finish-migrate" durumlarından geçer; bunlar duraklama değildir.
        paused: matches!(state["status"].as_str(), Some("paused" | "prelaunch" | "inmigrate" | "postmigrate")),
        error: match colo["reason"].as_str() {
            Some("error") => Some("QEMU eşzamanlı çalışmada hata bildirdi".to_string()),
            _ => None,
        },
    })
}

/// Yedek taraf: ikizi açar. Konuk kaydı birincilinkiyle aynı donanımı (aynı kimlik ve MAC adresi) anlatmalıdır.
pub fn ft_standby_start(d: &QemuDriver, g: &Guest, disk_bytes: u64, link: &FtLink, key: &str) -> Result<(), String> {
    let alive = || live_pid(d, g.vmid).is_some();
    if alive() {
        return Err("konuk zaten çalışıyor".to_string());
    }
    for host in [&link.primary_host, &link.bind] {
        if !redox_core::valid_migrate_host(host) {
            return Err(format!("geçersiz adres: {host}"));
        }
    }
    cleanup(d, g.vmid);
    let dir = ft_dir(d, g.vmid);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
    let size = disk_bytes.to_string();
    let make = |name: &str| -> Result<PathBuf, String> {
        let file = dir.join(name);
        tool(&d.cfg.qemu_img_bin, &["create", "-q", "-f", "qcow2", &file.display().to_string(), &size])?;
        Ok(file)
    };
    // Asıl kopya konuğun disk dosyasıdır: devralmadan sonra konuk olağan yerindeki diskiyle sürer.
    let parent = d.cfg.disk_of(g);
    if let Some(folder) = parent.parent() {
        std::fs::create_dir_all(folder).map_err(|e| format!("{} oluşturulamadı: {e}", folder.display()))?;
    }
    tool(&d.cfg.qemu_img_bin, &["create", "-q", "-f", "qcow2", &parent.display().to_string(), &size])?;
    let (active, hidden) = (make("etkin.qcow2")?, make("gizli.qcow2")?);
    let packet_keys = packet_tls_dir(d, g.vmid);
    super::migrate::write_psk_dir(&packet_keys, key)?;
    let primary = PrimaryLink { host: &link.primary_host, mirror_port: link.mirror_port, compare_port: link.compare_port, tls_dir: &packet_keys };
    let mut args = secondary_args(crate::qemu::build_args(&d.cfg, g), &parent, (&parent, &active, &hidden), primary)?;
    args.extend(["-incoming".to_string(), "defer".to_string()]);
    let out = std::process::Command::new(&d.cfg.qemu_bin).args(args).output().map_err(|e| format!("{} çalıştırılamadı: {e}", d.cfg.qemu_bin))?;
    if !out.status.success() {
        return Err(format!("ikiz başlatılamadı: {}", last_line(&out.stderr)));
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let prepared = (|| -> Result<(), String> {
        if !wait_until(Duration::from_secs(5), alive) {
            return Err("ikiz başladı ama süreç bulunamadı".to_string());
        }
        add_tls_object(&sock, &tls_dir(d, g.vmid), TLS_ID, key, "server")?;
        colo_capability(&sock)?;
        run(&sock, serde_json::json!({ "execute": "nbd-server-start", "arguments": { "addr": { "type": "inet", "data": { "host": link.bind, "port": link.nbd_port.to_string() } }, "tls-creds": TLS_ID } }))
            .map_err(|e| format!("disk kapısı ({}) açılamadı: {e}", link.nbd_port))?;
        run(&sock, serde_json::json!({ "execute": "nbd-server-add", "arguments": { "device": PARENT_ID, "writable": true } }))?;
        run(&sock, serde_json::json!({ "execute": "migrate-set-parameters", "arguments": { "tls-creds": TLS_ID } }))?;
        run(&sock, serde_json::json!({ "execute": "migrate-incoming", "arguments": { "uri": address(&link.bind, link.migrate_port) } })).map_err(|e| format!("bellek kapısı ({}) açılamadı: {e}", link.migrate_port))?;
        Ok(())
    })();
    prepared.inspect_err(|_| ft_standby_stop(d, g))
}

/// Yedek taraf: ikizi kapatır ve katmanları siler (devralınmayacak).
pub fn ft_standby_stop(d: &QemuDriver, g: &Guest) {
    let sock = d.cfg.qmp_path(g.vmid);
    let alive = || live_pid(d, g.vmid).is_some();
    if alive() {
        let _ = qmp(&sock, &["quit"]);
        if !wait_until(Duration::from_secs(5), || !alive()) {
            if let Some(pid) = live_pid(d, g.vmid) {
                let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
            }
            wait_until(Duration::from_secs(3), || !alive());
        }
    }
    cleanup(d, g.vmid);
    let _ = std::fs::remove_dir_all(ft_dir(d, g.vmid));
    let _ = std::fs::remove_dir_all(tls_dir(d, g.vmid));
    let _ = std::fs::remove_dir_all(packet_tls_dir(d, g.vmid));
}

/// Birincil: korumayı kurar. Dönüşte ikiz eşzamanlı çalışmaktadır. Hata olursa konuk korumasız ve çalışır kalır.
pub fn ft_protect(d: &QemuDriver, g: &Guest, link: &FtLink, key: &str) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    let alive = || live_pid(d, g.vmid).is_some();
    if !alive() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    for host in [&link.secondary_host, &link.bind] {
        if !redox_core::valid_migrate_host(host) {
            return Err(format!("geçersiz adres: {host}"));
        }
    }
    // Önceki denemenin kalıntıları temizlenir.
    ft_drop_secondary(d, g)?;
    let engaged = (|| -> Result<(), String> {
        add_tls_object(&sock, &tls_dir(d, g.vmid), TLS_ID, key, "client")?;
        // 1. Disk: ikizin asıl kopyası birincilinkine eşitlenir.
        run(&sock, serde_json::json!({ "execute": "blockdev-add", "arguments": { "driver": "nbd", "node-name": SYNC_JOB, "export": PARENT_ID, "tls-creds": TLS_ID,
            "server": { "type": "inet", "host": link.secondary_host, "port": link.nbd_port.to_string() } } }))
        .map_err(|e| format!("ikizin disk kapısına ({}:{}) bağlanılamadı: {e}", link.secondary_host, link.nbd_port))?;
        run(&sock, serde_json::json!({ "execute": "blockdev-mirror", "arguments": { "job-id": SYNC_JOB, "device": DISK_ID, "target": SYNC_JOB, "sync": "full", "copy-mode": "write-blocking" } })).map_err(|e| format!("disk eşitlenemedi: {e}"))?;
        let (mut best, mut moved) = (0u64, Instant::now());
        loop {
            let jobs = block_jobs(&sock, SYNC_JOB)?;
            let Some(job) = jobs.first() else { return Err("disk eşitlemesi yarıda kesildi".to_string()) };
            if job["ready"] == true {
                break;
            }
            let done = job["offset"].as_u64().unwrap_or(0);
            if done != best {
                (best, moved) = (done, Instant::now());
            }
            if moved.elapsed() > Duration::from_secs(15) {
                return Err("disk eşitlemesi ilerlemiyor (ikiz yanıt vermiyor)".to_string());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        // 2. Konuk durur; iki disk aynıyken yansıtma bırakılır.
        run(&sock, serde_json::json!({ "execute": "stop" }))?;
        run(&sock, serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": SYNC_JOB } }))?;
        if !wait_until(Duration::from_secs(15), || block_jobs(&sock, SYNC_JOB).is_ok_and(|j| j.is_empty())) {
            return Err("disk eşitleme işi bırakılamadı".to_string());
        }
        quietly(&sock, serde_json::json!({ "execute": "blockdev-del", "arguments": { "node-name": SYNC_JOB } }));
        // 3. Ağ: giden paketler ikize kopyalanır ve ikizinkiyle karşılaştırılmadan dışarı çıkmaz.
        // İkizin bağlandığı iki kapı ağdadır ve anahtarla şifrelenir; birincilin kendi içindeki iki kapı ise
        // çalışma dizinindeki soketlerdir.
        add_tls_object(&sock, &packet_tls_dir(d, g.vmid), PACKET_TLS_ID, key, "server")?;
        let add = |id: &str, addr: Value, server: bool, tls: bool| {
            let mut data = serde_json::json!({ "addr": addr, "server": server });
            if server {
                data["wait"] = false.into();
            }
            if tls {
                data["tls-creds"] = PACKET_TLS_ID.into();
            }
            run(&sock, serde_json::json!({ "execute": "chardev-add", "arguments": { "id": id, "backend": { "type": "socket", "data": data } } }))
        };
        for (id, port) in [("mirror0", link.mirror_port), ("compare1", link.compare_port)] {
            let addr = serde_json::json!({ "type": "inet", "data": { "host": link.bind, "port": port.to_string() } });
            add(id, addr, true, true).map_err(|e| format!("karşılaştırma kapısı ({port}) açılamadı: {e}"))?;
        }
        for (id, name, server) in [("compare0", "c0", true), ("compare0-0", "c0", false), ("compare_out", "out", true), ("compare_out0", "out", false)] {
            let path = local_socket(d, g.vmid, name);
            if server {
                let _ = std::fs::remove_file(&path);
            }
            let addr = serde_json::json!({ "type": "unix", "data": { "path": path.display().to_string() } });
            add(id, addr, server, false).map_err(|e| format!("karşılaştırma kapısı ({id}) açılamadı: {e}"))?;
        }
        for object in [
            serde_json::json!({ "qom-type": "filter-mirror", "id": "m0", "netdev": "net0", "queue": "tx", "outdev": "mirror0", "vnet_hdr_support": true }),
            serde_json::json!({ "qom-type": "filter-redirector", "id": "redire0", "netdev": "net0", "queue": "rx", "indev": "compare_out", "vnet_hdr_support": true }),
            serde_json::json!({ "qom-type": "filter-redirector", "id": "redire1", "netdev": "net0", "queue": "rx", "outdev": "compare0", "vnet_hdr_support": true }),
            serde_json::json!({ "qom-type": "iothread", "id": "iothread1" }),
            serde_json::json!({ "qom-type": "colo-compare", "id": "comp0", "primary_in": "compare0-0", "secondary_in": "compare1", "outdev": "compare_out0", "iothread": "iothread1", "vnet_hdr_support": true }),
        ] {
            run(&sock, serde_json::json!({ "execute": "object-add", "arguments": object })).map_err(|e| format!("ağ süzgeci takılamadı: {e}"))?;
        }
        // 4. İkizin diski ikinci çocuk olur ve COLO başlar.
        let drive = format!(
            "drive_add -n buddy driver=replication,mode=primary,file.driver=nbd,file.host={},file.port={},file.export={PARENT_ID},file.tls-creds={TLS_ID},node-name={NBD_CHILD}",
            link.secondary_host, link.nbd_port
        );
        let said = hmp_line(&sock, &drive)?;
        if !said.is_empty() && said != "OK" {
            return Err(format!("ikizin diski eklenemedi: {said}"));
        }
        run(&sock, serde_json::json!({ "execute": "x-blockdev-change", "arguments": { "parent": DISK_ID, "node": NBD_CHILD } })).map_err(|e| format!("ikizin diski bağlanamadı: {e}"))?;
        colo_capability(&sock)?;
        run(&sock, serde_json::json!({ "execute": "migrate-set-parameters", "arguments": { "tls-creds": TLS_ID } }))?;
        run(&sock, serde_json::json!({ "execute": "migrate", "arguments": { "uri": address(&link.secondary_host, link.migrate_port) } })).map_err(|e| format!("bellek aktarımı başlatılamadı: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(600);
        loop {
            let info = qmp(&sock, &["query-migrate"])?.pop().unwrap_or(Value::Null);
            match info["status"].as_str() {
                Some("colo") if ft_status(d, g).is_some_and(|s| s.engaged) => break,
                Some("failed") => return Err(format!("bellek aktarılamadı: {}", info["error-desc"].as_str().unwrap_or("ikiz kabul etmedi"))),
                _ if !alive() => return Err("konuk kurulum sırasında kapandı".to_string()),
                _ if Instant::now() >= deadline => return Err("ikiz zamanında hazır olmadı".to_string()),
                _ => std::thread::sleep(Duration::from_millis(300)),
            }
        }
        if ft_status(d, g).is_some_and(|s| s.paused) {
            quietly(&sock, serde_json::json!({ "execute": "cont" }));
        }
        Ok(())
    })();
    engaged.inspect_err(|_| {
        let _ = ft_drop_secondary(d, g);
        quietly(&sock, serde_json::json!({ "execute": "cont" }));
    })
}

/// Birincil: ikizi bırakır ve tek başına sürer. İkiz donmuş olsa da çalışır; yinelenmesi zararsızdır.
pub fn ft_drop_secondary(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    if live_pid(d, g.vmid).is_none() {
        return Ok(());
    }
    // Takılı bağlantılar önce zorla kesilir; yoksa aşağıdaki komutlar yanıt bekler.
    if let Ok(mut reply) = qmp(&sock, &["query-yank"]) {
        let instances: Vec<Value> = reply
            .pop()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            // İzleyici ve seri kapı bağlantılarına dokunulmaz.
            .filter(|i| i["type"] != "chardev" || matches!(i["id"].as_str(), Some("mirror0" | "compare1")))
            .collect();
        if !instances.is_empty() {
            quietly(&sock, serde_json::json!({ "execute": "yank", "arguments": { "instances": instances } }));
        }
    }
    quietly(&sock, serde_json::json!({ "execute": "block-job-cancel", "arguments": { "device": SYNC_JOB, "force": true } }));
    wait_until(Duration::from_secs(10), || block_jobs(&sock, SYNC_JOB).is_ok_and(|j| j.is_empty()));
    quietly(&sock, serde_json::json!({ "execute": "blockdev-del", "arguments": { "node-name": SYNC_JOB } }));
    let engaged = ft_status(d, g).is_some_and(|s| s.engaged);
    // İkinci çocuk varsa çıkarılır (adı QEMU'da "children.N" biçimindedir).
    for child in ["children.1", "children.2", "children.3"] {
        quietly(&sock, serde_json::json!({ "execute": "x-blockdev-change", "arguments": { "parent": DISK_ID, "child": child } }));
    }
    let _ = hmp_line(&sock, &format!("drive_del {NBD_CHILD}"));
    for id in PRIMARY_OBJECTS {
        quietly(&sock, serde_json::json!({ "execute": "object-del", "arguments": { "id": id } }));
    }
    if engaged {
        run(&sock, serde_json::json!({ "execute": "x-colo-lost-heartbeat" })).map_err(|e| format!("eşzamanlı çalışmadan çıkılamadı: {e}"))?;
    } else {
        quietly(&sock, serde_json::json!({ "execute": "migrate_cancel" }));
    }
    for id in PRIMARY_CHARDEVS {
        wait_until(Duration::from_secs(2), || qmp_values(&sock, &[serde_json::json!({ "execute": "chardev-remove", "arguments": { "id": id } })]).map_or_else(|e| e.contains("not found"), |_| true));
    }
    quietly(&sock, serde_json::json!({ "execute": "object-del", "arguments": { "id": TLS_ID } }));
    quietly(&sock, serde_json::json!({ "execute": "object-del", "arguments": { "id": PACKET_TLS_ID } }));
    let _ = std::fs::remove_dir_all(tls_dir(d, g.vmid));
    let _ = std::fs::remove_dir_all(packet_tls_dir(d, g.vmid));
    for name in ["c0", "out"] {
        let _ = std::fs::remove_file(local_socket(d, g.vmid, name));
    }
    quietly(&sock, serde_json::json!({ "execute": "cont" }));
    Ok(())
}

/// Yedek taraf: birincil kayboldu; ikiz konuğun kendisi olur.
pub fn ft_take_over(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    let sock = d.cfg.qmp_path(g.vmid);
    if live_pid(d, g.vmid).is_none() {
        return Err("ikiz çalışmıyor".to_string());
    }
    if !ft_status(d, g).is_some_and(|s| s.engaged) {
        return Err("ikiz henüz eşzamanlı çalışmıyor; devralınamaz".to_string());
    }
    quietly(&sock, serde_json::json!({ "execute": "nbd-server-stop" }));
    run(&sock, serde_json::json!({ "execute": "x-colo-lost-heartbeat" })).map_err(|e| format!("ikiz devralınamadı: {e}"))?;
    for id in ["f2", "f1", "rew0"] {
        quietly(&sock, serde_json::json!({ "execute": "object-del", "arguments": { "id": id } }));
    }
    for id in ["red0", "red1"] {
        quietly(&sock, serde_json::json!({ "execute": "chardev-remove", "arguments": { "id": id } }));
    }
    if ft_status(d, g).is_some_and(|s| s.paused) {
        quietly(&sock, serde_json::json!({ "execute": "cont" }));
    }
    Ok(())
}

/// Birincil: konuğu duraklatır ya da sürdürür (hakeme ulaşılamazken iki yerde birden çalışmasın).
pub fn ft_hold(d: &QemuDriver, g: &Guest, hold: bool) -> Result<(), String> {
    run(&d.cfg.qmp_path(g.vmid), serde_json::json!({ "execute": if hold { "stop" } else { "cont" } })).map(drop)
}

/// Hakem karşı tarafı seçti: bu taraftaki QEMU hemen sonlandırılır.
pub fn ft_fence(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    if let Some(pid) = live_pid(d, g.vmid) {
        let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
        if !wait_until(Duration::from_secs(10), || live_pid(d, g.vmid).is_none()) {
            return Err("konuğun QEMU süreci sonlandırılamadı".to_string());
        }
    }
    cleanup(d, g.vmid);
    Ok(())
}
