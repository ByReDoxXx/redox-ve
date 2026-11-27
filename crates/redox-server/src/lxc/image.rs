//! OCI imajından konteyner oluşturma. `lxc-create` kullanılmaz: kök dosya sistemi imajın katmanlarından,
//! ayar dosyası imajın çalışma ayarından kurulur.

use std::path::Path;
use std::process::{Command, Stdio};

use redox_core::oci::ImageRef;
use redox_core::Guest;

use super::*;
use crate::oci::{self, Compression, Pulled};

const COMMON_CONFIG: &str = "/usr/share/lxc/config/common.conf";
const USERNS_CONFIG: &str = "/usr/share/lxc/config/userns.conf";

fn set_mode(dir: &Path, bits: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(bits));
    }
    #[cfg(not(unix))]
    let _ = (dir, bits);
}

fn done(command: &mut Command, what: &str) -> Result<Vec<u8>, String> {
    match command.output() {
        Ok(out) if out.status.success() => Ok(out.stdout),
        Ok(out) => Err(format!("{what}: {}", last_line(&out.stderr))),
        Err(e) => Err(format!("{what}: {e}")),
    }
}

/// Yarım indirilen ya da kullanılmış katmanlar her durumda silinir.
struct Cache(std::path::PathBuf);

impl Drop for Cache {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl LxcDriver {
    pub(super) fn create_from_image(&self, g: &Guest, reference: &str) -> Result<(), String> {
        let image = ImageRef::parse(reference)?;
        let name = container_name(g.vmid);
        let dir = self.cfg.lxc_path.join(&name);
        if dir.exists() {
            return Err(format!("{name} adlı konteyner dizini zaten var"));
        }
        // Sınır, konteynere ayrılan disktir: hem indirilen hem açılan veri için.
        let limit = g.disk_gb.saturating_mul(1 << 30);
        let cache = Cache(self.cfg.lxc_path.join(format!(".{name}.oci")));
        let pulled = oci::pull(&image, &cache.0, limit)?;
        std::fs::create_dir(&dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
        // Eşlenmiş kök, kök dosya sistemini bu dizinin içine kurabilsin diye dizin kurulum süresince yazılabilir.
        // Yapışkan bit: sunucudaki başka bir hesap bu sürede içindekileri silip yenisiyle değiştiremez.
        set_mode(&dir, 0o1777);
        let built = self.build_container(g, &dir, &pulled, limit);
        set_mode(&dir, 0o755);
        built.inspect_err(|_| {
            let _ = self.userns().args(["rm", "-rf", "--"]).arg(dir.join("rootfs")).output();
            let _ = std::fs::remove_dir_all(&dir);
        })
    }

    fn unpack_layer(&self, rootfs: &Path, layer: std::fs::File, compression: Compression, budget: u64) -> Result<u64, String> {
        let extractor = match &self.cfg.extractor {
            Some(path) => path.clone(),
            None => std::env::current_exe().map_err(|e| format!("çalışan program bulunamadı: {e}"))?,
        };
        let mut command = self.userns();
        command.arg(extractor).arg("oci-unpack").arg(rootfs).arg(compression.name()).arg(budget.to_string()).stdin(Stdio::from(layer));
        let out = done(&mut command, "imaj katmanı açılamadı")?;
        String::from_utf8_lossy(&out).trim().parse().map_err(|_| "imaj katmanı açılamadı: beklenmeyen çıktı".to_string())
    }

    fn build_container(&self, g: &Guest, dir: &Path, pulled: &Pulled, limit: u64) -> Result<(), String> {
        let rootfs = dir.join("rootfs");
        done(self.userns().args(["mkdir", "--"]).arg(&rootfs), "kök dizin oluşturulamadı")?;
        done(self.userns().args(["chmod", "0755", "--"]).arg(&rootfs), "kök dizin hazırlanamadı")?;
        let mut budget = limit;
        for (path, compression) in &pulled.layers {
            let layer = std::fs::File::open(path).map_err(|e| format!("{} açılamadı: {e}", path.display()))?;
            budget = self.unpack_layer(&rootfs, layer, *compression, budget)?;
        }
        if let Some(gateway) = g.hw.ct_gateway.as_deref().filter(|gw| redox_core::hw::valid_ipv4(gw)) {
            let extra = dir.parent().unwrap_or(dir).join(format!(".{}.resolv", container_name(g.vmid)));
            std::fs::write(&extra, oci::unpack::single_file_layer("etc/resolv.conf", format!("nameserver {gateway}\n").as_bytes())).map_err(|e| format!("ad sunucusu ayarı yazılamadı: {e}"))?;
            let written = std::fs::File::open(&extra).map_err(|e| e.to_string()).and_then(|f| self.unpack_layer(&rootfs, f, Compression::None, budget));
            let _ = std::fs::remove_file(&extra);
            written?;
        }
        let (uid, gid) = pulled.config.ids(&read_in_root(&rootfs, "etc/passwd"), &read_in_root(&rootfs, "etc/group"))?;
        let mut config = format!("# {} imajından\nlxc.include = {COMMON_CONFIG}\n", g.os);
        if self.cfg.idmap.is_some() {
            config.push_str(&format!("lxc.include = {USERNS_CONFIG}\n"));
        }
        config.push_str(&format!("lxc.arch = linux64\n{}lxc.rootfs.path = dir:{}\n{}", idmap_lines(self.cfg.idmap), rootfs.display(), pulled.config.lxc_lines(uid, gid)?));
        super::write_fresh(&dir.join("config"), &config).map_err(|e| format!("konteyner ayarı yazılamadı: {e}"))?;
        self.write_managed(g)
    }
}

fn read_in_root(rootfs: &Path, name: &str) -> String {
    #[cfg(unix)]
    {
        oci::unpack::read_inside(rootfs, name, 1 << 20).unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        let _ = (rootfs, name);
        String::new()
    }
}
