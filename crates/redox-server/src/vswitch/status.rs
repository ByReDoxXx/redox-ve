//! Anahtarın bu sunucudaki durumu. Yetki gerektirmez: `/sys/class/net` okunur.

use std::path::Path;

use redox_core::VSwitch;
use serde::Serialize;

use super::plan::{device_name, MARK};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Local {
    /// Köprü ve tünel aygıtı kurulu, ayarı tanıma uyuyor.
    Ready,
    /// Aygıtlardan biri yok: ağ yardımcısı henüz çalışmadı ya da başarısız oldu.
    Missing,
    /// Aynı adda, RedoX VE'nin kurmadığı bir aygıt var.
    Foreign,
    /// Aygıtlar kurulu ama ayarı farklı (MTU, bağlantı ya da kapalı).
    Differs,
}

impl Local {
    pub fn label(self) -> &'static str {
        match self {
            Local::Ready => "hazır",
            Local::Missing => "kurulmadı",
            Local::Foreign => "aynı adda yabancı aygıt",
            Local::Differs => "ayarı farklı",
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim().to_string()
}

/// `root`: olağan çalışmada `/sys/class/net`.
pub fn local_status_in(root: &Path, switch: &VSwitch) -> Local {
    let (bridge, device) = (root.join(&switch.name), root.join(device_name(switch.vni)));
    if !bridge.exists() || !device.exists() {
        // Tek aygıt var ve bizim değilse bu da yabancı aygıttır.
        let foreign = [&bridge, &device].iter().any(|d| d.exists() && read(&d.join("ifalias")) != MARK);
        return if foreign { Local::Foreign } else { Local::Missing };
    }
    if [&bridge, &device].iter().any(|d| read(&d.join("ifalias")) != MARK) {
        return Local::Foreign;
    }
    let mtu = switch.mtu.to_string();
    // IFF_UP: bayrakların en düşük biti.
    let up = |dir: &Path| u32::from_str_radix(read(&dir.join("flags")).trim_start_matches("0x"), 16).is_ok_and(|f| f & 1 == 1);
    let attached = device.join("master").join("ifindex");
    if read(&bridge.join("mtu")) != mtu || read(&device.join("mtu")) != mtu || !up(&bridge) || !up(&device) || read(&attached) != read(&bridge.join("ifindex")) {
        return Local::Differs;
    }
    Local::Ready
}

pub fn local_status(switch: &VSwitch) -> Local {
    local_status_in(Path::new("/sys/class/net"), switch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_read_from_sysfs() {
        let root = std::env::temp_dir().join(format!("redox-sys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let switch = VSwitch { name: "vds-a".to_string(), vni: 7, mtu: 1450, members: vec!["10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap()] };
        let write = |dir: &str, file: &str, value: &str| {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(file), value).unwrap();
        };
        assert_eq!(local_status_in(&root, &switch), Local::Missing);
        write("vds-a", "ifalias", "");
        assert_eq!(local_status_in(&root, &switch), Local::Foreign);
        for dir in ["vds-a", "vx7"] {
            write(dir, "ifalias", "redox-vds\n");
            write(dir, "mtu", "1450\n");
            write(dir, "flags", "0x1003\n");
        }
        write("vds-a", "ifindex", "5\n");
        write("vx7/master", "ifindex", "5\n");
        assert_eq!(local_status_in(&root, &switch), Local::Ready);
        write("vds-a", "flags", "0x1002\n");
        assert_eq!(local_status_in(&root, &switch), Local::Differs, "kapalı köprü hazır sayılmaz");
        write("vds-a", "flags", "0x1003\n");
        write("vx7/master", "ifindex", "9\n");
        assert_eq!(local_status_in(&root, &switch), Local::Differs, "başka köprüye bağlı tünel");
        write("vx7", "ifalias", "baska\n");
        assert_eq!(local_status_in(&root, &switch), Local::Foreign);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
