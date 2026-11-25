//! QEMU sürücüsünün ayarları ve dosya yolları.

use super::*;

#[derive(Debug, Clone)]
pub struct QemuConfig {
    /// `images/`, `iso/` ve `run/` alt dizinlerinin kökü.
    pub data_dir: PathBuf,
    pub qemu_bin: String,
    pub qemu_img_bin: String,
    /// "Kapat" isteğinde konuğun kendi kapanması için beklenen süre; dolunca zorla kapatılır.
    pub shutdown_timeout: Duration,
    /// KVM hızlandırması var mı? Yoksa yazılım öykünmesi (TCG) kullanılır.
    pub kvm: bool,
    /// UEFI donanım yazılımı dosyalarının (OVMF) dizini.
    pub ovmf_dir: PathBuf,
    /// Sanal TPM öykünücüsü.
    pub swtpm_bin: String,
    /// Ağ hız sınırını uygulayan `tc` (iproute2).
    pub tc_bin: String,
    /// VLAN'lı kartların TAP arayüzünü kuran `ip` ve `bridge` (iproute2).
    pub ip_bin: String,
    pub bridge_bin: String,
    /// Ağ arayüzlerinin sysfs dizini (köprünün VLAN süzmesi buradan okunur).
    pub sys_net: PathBuf,
    /// Çekirdek modüllerinin parametre dizini (`/sys/module`); KVM'in iç içe sanallaştırma ayarı buradan okunur.
    pub sys_module: PathBuf,
    /// Ek depolama alanları: `(ad, dizin)`. Konuğun diskleri, kaydındaki depolamanın dizininde durur;
    /// `local` (ve burada tanımlı olmayan her ad) veri dizinindeki `images/` klasörüdür.
    pub storages: Vec<(String, PathBuf)>,
    /// Yeni yedekler parça deposuna mı yazılsın? (Yalnızca değişen kısım yer kaplar.)
    pub incremental_backups: bool,
    /// Yerel yedek deposunun şifreleme parolası (yalnızca artımlı yedekle).
    pub backup_passphrase: Option<String>,
    /// Değişen blok takibi: çalışan konuğun artımlı yedeğinde yalnızca son yedekten beri yazılan bloklar okunur.
    pub cbt: bool,
    /// Yedek kopyalama hız sınırı (MB/sn, disk başına); 0 sınırsız. Yedek, konukların disk hızını tüketmesin diye.
    pub backup_mbps: u64,
}

impl QemuConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        QemuConfig {
            data_dir: data_dir.into(),
            qemu_bin: "qemu-system-x86_64".to_string(),
            qemu_img_bin: "qemu-img".to_string(),
            shutdown_timeout: Duration::from_secs(60),
            kvm: true,
            ovmf_dir: PathBuf::from("/usr/share/OVMF"),
            swtpm_bin: "swtpm".to_string(),
            tc_bin: "tc".to_string(),
            ip_bin: "ip".to_string(),
            bridge_bin: "bridge".to_string(),
            sys_net: PathBuf::from("/sys/class/net"),
            sys_module: PathBuf::from("/sys/module"),
            storages: Vec::new(),
            incremental_backups: false,
            backup_passphrase: None,
            backup_mbps: 0,
            cbt: false,
        }
    }

    /// UEFI kod imajı ve değişken deposu kalıbı. Güvenli önyüklemede Microsoft anahtarları kayıtlı kalıp kullanılır.
    pub fn ovmf_files(&self, secure: bool) -> (PathBuf, PathBuf) {
        let (code, vars) = if secure { ("OVMF_CODE_4M.secboot.fd", "OVMF_VARS_4M.ms.fd") } else { ("OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd") };
        (self.ovmf_dir.join(code), self.ovmf_dir.join(vars))
    }

    /// Konuğun UEFI değişken deposu (önyükleme kayıtları, güvenli önyükleme anahtarları).
    pub fn efivars_path(&self, vmid: VmId, secure: bool) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}-efivars{}.qcow2", if secure { "-secure" } else { "" }))
    }

    /// Sanal TPM'in kalıcı durumu.
    pub fn tpm_dir(&self, vmid: VmId) -> PathBuf {
        self.data_dir.join("tpm").join(vmid.to_string())
    }

    pub fn tpm_sock(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.tpm"))
    }

    pub fn tpm_pid(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.tpm.pid"))
    }

    pub fn images_dir(&self) -> PathBuf {
        self.data_dir.join("images")
    }

    pub fn iso_dir(&self) -> PathBuf {
        self.data_dir.join("iso")
    }

    /// Hazır bulut imajlarının durduğu dizin.
    pub fn cloud_dir(&self) -> PathBuf {
        self.data_dir.join("cloud")
    }

    /// Konuğun cloud-init dosyaları (`user-data`, `meta-data`); konuğa `cidata` etiketli sürücü olarak takılır.
    pub fn seed_dir(&self, vmid: VmId) -> PathBuf {
        self.data_dir.join("cidata").join(vmid.to_string())
    }

    pub fn run_dir(&self) -> PathBuf {
        self.data_dir.join("run")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    /// Bir depolama alanının disk dizini.
    pub fn storage_dir(&self, id: &str) -> PathBuf {
        self.storages.iter().find(|(name, _)| name == id).map(|(_, dir)| dir.clone()).unwrap_or_else(|| self.images_dir())
    }

    /// Konuğun asıl diski, kendi depolama alanında.
    pub fn disk_of(&self, g: &Guest) -> PathBuf {
        self.storage_dir(&g.storage).join(format!("vm-{}-disk-0.qcow2", g.vmid))
    }

    /// Konuğun ek diski, kendi depolama alanında.
    pub fn extra_disk_of(&self, g: &Guest, id: u8) -> PathBuf {
        self.storage_dir(&g.storage).join(format!("vm-{}-disk-{id}.qcow2", g.vmid))
    }

    /// Yerel depolamadaki (`local`) asıl disk yolu.
    pub fn disk_path(&self, vmid: VmId) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}-disk-0.qcow2"))
    }

    /// Ek veri diskinin imajı (`id` 1'den başlar).
    pub fn extra_disk_path(&self, vmid: VmId, id: u8) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}-disk-{id}.qcow2"))
    }

    /// Askıya alınan konuğun bellek durumu.
    pub fn state_path(&self, vmid: VmId) -> PathBuf {
        self.images_dir().join(format!("vm-{vmid}.state"))
    }

    /// Paylaşılan klasörlerin kökü: her paylaşım `shares/<ad>` dizinidir.
    pub fn shares_dir(&self) -> PathBuf {
        self.data_dir.join("shares")
    }

    /// Canlı konsolun bağlandığı VNC soketi (parolasız; yalnızca hizmet kullanıcısının girebildiği dizinde durur).
    pub fn vnc_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.vnc"))
    }

    /// Konuk ajanının (qemu-guest-agent) kanalı.
    pub fn qga_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.qga"))
    }

    pub fn qmp_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.qmp"))
    }

    pub fn pid_path(&self, vmid: VmId) -> PathBuf {
        self.run_dir().join(format!("{vmid}.pid"))
    }
}
