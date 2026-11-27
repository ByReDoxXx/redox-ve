//! Dağıtık anahtarın sunucu tarafı.
//!
//! Yönetim sunucusu yetkisizdir ve ağ aygıtı oluşturmaz: tanımları bir dosyaya yazar (`write_spec`). Ağı kuran,
//! root yetkisiyle çalışan küçük bir birimdir (`redox net sync <dosya>`, `sync`). O birim dosyaya güvenmez:
//! her alanı yeniden doğrular, komutları kabuksuz ve ayrı argümanlar olarak çalıştırır, yalnızca `vds` önekli ve
//! kendi işaretlediği aygıtlara dokunur.

pub mod plan;
pub mod status;
pub mod sync;

use std::path::Path;

use redox_core::VSwitch;

/// Ayrıcalıklı yardımcının okuyacağı en büyük dosya.
pub const MAX_SPEC_BYTES: u64 = 64 * 1024;

/// İstenen anahtarları dosyaya yazar. Dosya tek adımda değiştirilir: yardımcı yarım dosya görmez.
pub fn write_spec(path: &Path, switches: &[VSwitch]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(switches).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.yeni");
    std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path)).map_err(|e| format!("{} yazılamadı: {e}", path.display()))
}

/// `qemu-bridge-helper` izin listesi: konuk kartları yalnızca izin verilen köprülere bağlanabilir. Bu dosya
/// `/etc/qemu/bridge.conf` içinden `include` ile okunur; adlar doğrulanmış tanımlardan gelir.
pub fn bridge_conf(switches: &[VSwitch]) -> String {
    let mut out = String::from("# RedoX VE dağıtık anahtarları; `redox net sync` yazar, elle düzenlemeyin.\n");
    for switch in switches.iter().filter(|s| s.check().is_ok()) {
        out.push_str(&format!("allow {}\n", switch.name));
    }
    out
}

pub fn write_bridge_conf(path: &Path, switches: &[VSwitch]) -> Result<(), String> {
    let temp = path.with_extension("conf.yeni");
    std::fs::write(&temp, bridge_conf(switches))
        .and_then(|()| {
            #[cfg(unix)]
            std::fs::set_permissions(&temp, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
            std::fs::rename(&temp, path)
        })
        .map_err(|e| format!("{} yazılamadı: {e}", path.display()))
}

/// Dosyayı ayrıcalıklı yardımcı için okur: sembolik bağ ve olağan dışı dosya kabul edilmez, boyut sınırlıdır,
/// tanımların tümü yeniden doğrulanır.
pub fn read_spec(path: &Path) -> Result<Vec<VSwitch>, String> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!("{} olağan bir dosya değil", path.display()));
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_SPEC_BYTES + 1).read_to_string(&mut text))
        .map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
    if text.len() as u64 > MAX_SPEC_BYTES {
        return Err(format!("{} çok büyük", path.display()));
    }
    // Ayrıştırma hatasının ayrıntısı yazılmaz: dosyanın içeriği günlüğe sızmasın.
    let switches: Vec<VSwitch> = serde_json::from_str(&text).map_err(|e| format!("{} anahtar tanımı değil (satır {})", path.display(), e.line()))?;
    redox_core::vswitch::check_all(&switches)?;
    Ok(switches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_file_is_validated_again_when_read() {
        let dir = std::env::temp_dir().join(format!("redox-anahtar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vswitch.json");
        let good = vec![VSwitch { name: "vds-a".to_string(), vni: 7, mtu: 1450, members: vec!["10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap()] }];
        write_spec(&path, &good).unwrap();
        assert_eq!(read_spec(&path).unwrap(), good);
        let conf = dir.join("redox-vds.conf");
        write_bridge_conf(&conf, &good).unwrap();
        assert_eq!(std::fs::read_to_string(&conf).unwrap().lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>(), ["allow vds-a"]);
        for (text, needle) in [
            (r#"[{"name":"vmbr0","vni":7,"members":["10.0.0.1","10.0.0.2"]}]"#.to_string(), "ile başlamalı"),
            (r#"[{"name":"vds-a","vni":7,"members":["10.0.0.1","10.0.0.2"]},{"name":"vds-b","vni":7,"members":["10.0.0.1","10.0.0.2"]}]"#.to_string(), "zaten vds-a"),
            ("gizli-icerik".to_string(), "anahtar tanımı değil"),
            (" ".repeat(MAX_SPEC_BYTES as usize + 1), "çok büyük"),
        ] {
            std::fs::write(&path, text).unwrap();
            let err = read_spec(&path).unwrap_err();
            assert!(err.contains(needle) && !err.contains("gizli-icerik"), "{err}");
        }
        assert!(read_spec(&dir).unwrap_err().contains("olağan bir dosya değil"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
