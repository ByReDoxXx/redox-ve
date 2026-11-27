//! QMP satırlarını ve /proc ölçümlerini ayrıştırma.

use super::*;

pub fn qmp_command(name: &str) -> String {
    format!("{}\n", serde_json::json!({ "execute": name }))
}

#[derive(Debug, Clone, PartialEq)]
pub enum QmpLine {
    Greeting,
    Return(Value),
    Error(String),
    /// Kendiliğinden gelen olay (SHUTDOWN, STOP …); yanıt beklerken atlanır.
    Event(String),
    Other,
}

pub fn parse_qmp_line(line: &str) -> QmpLine {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return QmpLine::Other;
    };
    if v.get("QMP").is_some() {
        QmpLine::Greeting
    } else if let Some(r) = v.get("return") {
        let mut r = r.clone();
        plain_block_files(&mut r);
        QmpLine::Return(r)
    } else if let Some(e) = v.get("error") {
        QmpLine::Error(e["desc"].as_str().unwrap_or("bilinmeyen QMP hatası").to_string())
    } else if let Some(e) = v.get("event") {
        QmpLine::Event(e.as_str().unwrap_or_default().to_string())
    } else {
        QmpLine::Other
    }
}

/// Şifreli (ya da ek seçenekle açılmış) diskte QEMU, `query-block` yanıtında dosya yolu yerine
/// `json:{"encrypt.key-secret": …, "file": {"filename": …}}` biçiminde bir ad bildirir. Sürücü diskleri yolundan
/// tanıdığı için bu ad, içindeki düz dosya yoluna çevrilir.
fn plain_block_files(reply: &mut Value) {
    let Some(blocks) = reply.as_array_mut() else { return };
    for block in blocks {
        let Some(file) = block.get_mut("inserted").and_then(|inserted| inserted.get_mut("file")) else { continue };
        let Some(path) = file.as_str().and_then(|name| name.strip_prefix("json:")).and_then(|text| {
            let options: Value = serde_json::from_str(text).ok()?;
            options["file"]["filename"].as_str().or_else(|| options["file.filename"].as_str()).map(str::to_string)
        }) else {
            continue;
        };
        *file = Value::String(path);
    }
}

/// `/proc/<pid>/stat` içinden toplam CPU tıkları (utime + stime).
pub fn parse_stat_ticks(stat: &str) -> Option<u64> {
    // Süreç adı parantez içinde boşluk içerebilir; son ')' sonrasından say.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

/// `/proc/<pid>/status` içinden bellek kullanımı (kB).
pub fn parse_vmrss_kb(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// İki örnek arasındaki tık farkından, konuğun çekirdeklerine göre CPU yüzdesi.
/// Linux'ta saniyede 100 tık (USER_HZ) varsayılır.
pub fn cpu_pct(delta_ticks: u64, elapsed: Duration, cores: u32) -> u8 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 || cores == 0 {
        return 0;
    }
    let used = delta_ticks as f64 / 100.0;
    (used / secs / cores as f64 * 100.0).round().clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod block_name_tests {
    use super::*;

    #[test]
    fn pseudo_file_names_of_encrypted_disks_become_paths() {
        // QEMU 10.2.1'in şifreli disk için gerçekten döndürdüğü biçim.
        let line = r#"{"return":[{"device":"virtio0","inserted":{"file":"json:{\"encrypt.key-secret\": \"diskkey\", \"driver\": \"qcow2\", \"file\": {\"driver\": \"file\", \"filename\": \"/d/vm-1.qcow2\"}}"}},{"device":"virtio1","inserted":{"file":"/d/duz.qcow2"}},{"device":"cd"},{"device":"x","inserted":{"file":"json:bozuk"}}]}"#;
        let QmpLine::Return(reply) = parse_qmp_line(line) else { panic!("yanıt bekleniyordu") };
        assert_eq!(reply[0]["inserted"]["file"], "/d/vm-1.qcow2");
        assert_eq!(reply[1]["inserted"]["file"], "/d/duz.qcow2");
        assert_eq!(reply[3]["inserted"]["file"], "json:bozuk", "anlaşılmayan ad olduğu gibi kalır");
        assert_eq!(parse_qmp_line(r#"{"return":{}}"#), QmpLine::Return(serde_json::json!({})));
    }
}
