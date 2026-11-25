//! Değişen blok takibi: harita durumu ve geçersiz kılma.

use super::*;

impl QemuDriver {
    pub(super) fn cbt_path(&self, vmid: VmId) -> PathBuf {
        self.cfg.images_dir().join(format!("vm-{vmid}.cbt.json"))
    }

    pub(super) fn cbt_state(&self, vmid: VmId) -> Option<CbtState> {
        serde_json::from_slice(&std::fs::read(self.cbt_path(vmid)).ok()?).ok()
    }

    /// Diskin içeriği haritaya işlenmeden değişecekse (geri yükleme, geri dönüş, boyut değişikliği) çağrılır.
    pub(super) fn cbt_invalidate(&self, vmid: VmId) {
        let _ = std::fs::remove_file(self.cbt_path(vmid));
    }

    pub(super) fn cbt_record(&self, vmid: VmId, state: &CbtState) {
        if let Ok(text) = serde_json::to_vec(state) {
            let _ = std::fs::write(self.cbt_path(vmid), text);
        }
    }

    /// Kapalı konuğun imajında haritayı sıfırlar (yoksa oluşturur). Başarısızsa (eski qemu-img) `false`.
    pub(super) fn arm_offline(&self, disk: &Path) -> bool {
        let run = |flag: &str| self.img(&["bitmap".as_ref(), flag.as_ref(), disk.as_os_str(), CBT_BITMAP.as_ref()], "harita").is_ok();
        run("--add") || run("--clear") || (run("--remove") && run("--add"))
    }
}
