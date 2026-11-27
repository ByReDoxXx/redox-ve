//! Çalışan konuktan çekirdek ve bellek parçası çıkarma.
//!
//! Çıkarma konuk işletim sisteminin işbirliğiyle olur: QEMU isteği iletir (ACPI), konuk kaynağı bırakınca aygıt
//! kaybolur. Bu yüzden aygıt gerçekten kaybolana kadar beklenir; kaybolmazsa işlem başarısız sayılır.

use super::*;

/// Konuğun aygıtı bırakması için tanınan süre.
const RELEASE: Duration = Duration::from_secs(20);

fn place(slot: &Value) -> [i64; 6] {
    ["socket-id", "die-id", "cluster-id", "module-id", "core-id", "thread-id"].map(|key| slot["props"][key].as_i64().unwrap_or(0))
}

/// Takılı çekirdekler, yuva sırasıyla: her birinin QOM yolu.
fn present_cores(sock: &Path) -> Result<Vec<String>, String> {
    let reply = qmp(sock, &["query-hotpluggable-cpus"])?;
    let slots = reply.first().and_then(Value::as_array).ok_or("konuğun çekirdek yuvaları QEMU'dan okunamadı")?;
    let mut used: Vec<&Value> = slots.iter().filter(|slot| slot.get("qom-path").is_some()).collect();
    used.sort_by_key(|slot| place(slot));
    Ok(used.iter().filter_map(|slot| slot["qom-path"].as_str().map(str::to_string)).collect())
}

/// Sonradan takılan bellek parçalarının aygıt adları.
fn present_dimms(sock: &Path) -> Result<Vec<String>, String> {
    let reply = qmp(sock, &["query-memory-devices"])?;
    let list = reply.first().and_then(Value::as_array).ok_or("konuğun bellek aygıtları QEMU'dan okunamadı")?;
    Ok(list.iter().filter_map(|device| device["data"]["id"].as_str().map(str::to_string)).collect())
}

fn delete_device(sock: &Path, id: &str, what: &str) -> Result<(), String> {
    qmp_values(sock, &[serde_json::json!({ "execute": "device_del", "arguments": { "id": id } })]).map(|_| ()).map_err(|e| format!("{what} konuktan çıkarılamadı: {e}"))
}

/// Konukta şu anda takılı olan `(çekirdek sayısı, sonradan takılan parça sayısı)`.
pub fn inventory(d: &QemuDriver, g: &Guest) -> Option<(u32, usize)> {
    live_pid(d, g.vmid)?;
    let sock = d.cfg.qmp_path(g.vmid);
    let cores = present_cores(&sock).ok()?.len() as u32;
    let dimms = if g.hw.dimms.is_empty() { 0 } else { present_dimms(&sock).ok()?.len() };
    Some((cores, dimms))
}

/// En yüksek numaralı çekirdeği çıkarır. İlk çekirdek çıkarılamaz.
pub fn hot_remove_core(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    let sock = d.cfg.qmp_path(g.vmid);
    let cores = present_cores(&sock)?;
    if cores.len() <= 1 {
        return Err("son çekirdek çıkarılamaz".to_string());
    }
    let last = cores.last().expect("en az iki çekirdek var");
    delete_device(&sock, last, "çekirdek")?;
    let gone = wait_until(RELEASE, || present_cores(&sock).is_ok_and(|now| !now.contains(last)));
    if gone {
        Ok(())
    } else {
        Err("konuk çekirdeği bırakmadı (işletim sistemi desteklemiyor ya da çekirdek meşgul)".to_string())
    }
}

/// En son takılan bellek parçasını çıkarır; sıra bozulmadığı için askıdan dönüşte yuvalar aynı kalır.
pub fn hot_remove_memory(d: &QemuDriver, g: &Guest) -> Result<(), String> {
    if live_pid(d, g.vmid).is_none() {
        return Err("konuk çalışmıyor (QEMU süreci yok)".to_string());
    }
    let Some(index) = g.hw.dimms.len().checked_sub(1) else {
        return Err("çıkarılabilecek bellek parçası yok (yalnızca çalışırken eklenen bellek çıkarılır)".to_string());
    };
    let sock = d.cfg.qmp_path(g.vmid);
    let (backend, dimm) = (crate::qemu::dimm_backend(index), crate::qemu::dimm_device(index));
    delete_device(&sock, &dimm, "bellek")?;
    if !wait_until(RELEASE, || present_dimms(&sock).is_ok_and(|now| !now.contains(&dimm))) {
        return Err("konuk belleği bırakmadı: parça kullanımda. Linux konukta yeni belleğin taşınabilir açılması gerekir (memhp_default_state=online_movable)".to_string());
    }
    release_backend(&sock, &backend);
    Ok(())
}

/// Parçanın arkasındaki bellek nesnesini siler; aygıt yeni kaybolduysa nesne kısa süre daha kullanımda olabilir.
fn release_backend(sock: &Path, backend: &str) {
    let removed = wait_until(Duration::from_secs(5), || qmp_values(sock, &[serde_json::json!({ "execute": "object-del", "arguments": { "id": backend } })]).is_ok());
    if !removed {
        eprintln!("uyarı: {backend} bellek nesnesi silinemedi; konuk kapanınca silinir");
    }
}

/// Konuk bir parçayı istek zaman aşımından sonra bırakmışsa arkasındaki nesneyi temizler.
pub fn release_dimm_backend(d: &QemuDriver, g: &Guest, index: usize) {
    let sock = d.cfg.qmp_path(g.vmid);
    release_backend(&sock, &crate::qemu::dimm_backend(index));
}
