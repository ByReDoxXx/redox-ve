//! Konuğun isteğe bağlı donanım ve ilk kurulum ayarları.
//!
//! Hem oluşturma isteğinde hem konuk kaydında aynı yapı kullanılır; alanların hepsi
//! varsayılanlıdır, böylece eski durum dosyaları ve kısa API istekleri geçerli kalır.

use serde::{Deserialize, Serialize};

use crate::model::{GuestKind, VmId};

mod cpu;
mod devices;
mod firewall;
mod memory;
mod network;
mod schedule;

pub use cpu::*;
pub use devices::*;
pub use firewall::*;
pub use memory::*;
pub use network::*;
pub use schedule::*;

/// cloud-init kullanıcısı verilmezse açılan hesap.
pub const DEFAULT_CI_USER: &str = "redox";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hardware {
    /// Diskin kopyalanacağı hazır bulut imajı (dosya adı); `None` ise boş disk.
    pub image: Option<String>,
    /// cloud-init'in açacağı kullanıcı.
    pub ci_user: Option<String>,
    /// O kullanıcıya yetkili SSH açık anahtarı. Doluysa konuğa `cidata` sürücüsü takılır.
    pub ci_ssh_key: Option<String>,
    /// cloud-init'in birinci karta vereceği sabit IPv4 adresi (`10.0.0.5/24`); `None` ise DHCP.
    pub ci_ip: Option<String>,
    /// Varsayılan ağ geçidi (yalnızca sabit adresle).
    pub ci_gw: Option<String>,
    /// Ad sunucuları (yalnızca sabit adresle; en fazla `MAX_CI_DNS`).
    pub ci_dns: Vec<String>,
    /// cloud-init ayarları her değiştiğinde artar; konuğun kimliğine (`instance-id`) eklenir ki cloud-init
    /// yeni ayarları bir sonraki açılışta yeniden uygulasın.
    pub ci_gen: u32,
    /// Konuk çalışırken alınan yedekten önce ve sonra konuğun içinde çalıştırılacak komutlar (konuk ajanıyla).
    /// Örn. veritabanını tutarlı duruma getirip sonra serbest bırakmak için. Boşsa çalıştırılmaz.
    pub backup_pre: String,
    pub backup_post: String,
    /// Anında kurtarma: konuğun diskleri bu yedeğin üzerine kurulu ince katmanlardır (yedek, diskin arkalığıdır).
    /// Konuk yedekten ayrılana kadar yedek silinemez.
    pub instant: Option<String>,
    /// Ağdan yalıtılmış: bütün kartlar dışarıya kapalı kullanıcı kipi ağına bağlanır (yedek sınama konukları).
    pub isolated: bool,
    /// Konuğa gösterilen işlemci modeli; boşsa sunucunun kendi işlemcisi (`host`). Farklı işlemcili sunucular
    /// arasında taşınacak konuklar için ortak bir model seçilir.
    pub cpu_type: String,
    /// İç içe sanallaştırma: konuğa sanallaştırma uzantıları (VT-x / AMD-V) gösterilir; konuk kendi sanal
    /// makinelerini (KVM, Hyper-V, WSL2, Docker Desktop) çalıştırabilir.
    pub nested: bool,
    /// Çalışırken çıkılabilecek en fazla çekirdek sayısı; 0 ise çalışan konuğa çekirdek eklenemez. Konuk bu
    /// kadar çekirdek yuvasıyla açılır, boş yuvalar çalışırken doldurulur.
    pub max_cores: u32,
    /// İşlemci payı: sunucunun işlemcisi yetmediğinde konuğun ona ortak olma oranı. 0 olağandır (100);
    /// 200 paylı konuk 100 paylının iki katı işlemci alır. İşlemci boşken kimse kısılmaz.
    pub cpu_shares: u32,
    /// Bellek garantisi (MB): sunucunun belleği daraldığında bu konuktan geri alınmayacak bellek. Çalışan
    /// konukların garantileri toplamı sunucunun belleğini aşamaz; balon bu değerin altına indirilmez.
    pub mem_reserve_mb: u64,
    /// Çalışırken çıkılabilecek en fazla bellek (MB); 0 ise çalışan konuğa bellek eklenemez. Konuk boş bellek
    /// yuvalarıyla açılır, eklenen her parça bir yuvaya takılır.
    pub max_mem_mb: u64,
    /// Bu açılışta çalışırken eklenen bellek parçaları (MB), eklenme sırasıyla. Konuğun toplam belleğine
    /// dahildir; askıya alınan konuk aynı parçalarla döner. Konuk sıfırdan açılırken liste boşaltılır:
    /// eklenen bellek artık açılış belleğinin parçasıdır.
    pub dimms: Vec<u64>,
    /// Bellek balonu aygıtı takılı mı? Takılıysa konuğun belleği çalışırken küçültülüp büyütülebilir.
    pub balloon: bool,
    /// Balonla hedeflenen bellek (MB); 0 ise konuk belleğinin tamamını kullanır.
    pub balloon_mb: u64,
    /// Dinamik bellek: 0'dan büyükse bellek hedefi konuğun kullanımına göre bu alt sınır ile belleğin tamamı
    /// arasında kendiliğinden ayarlanır (balon aygıtı ve konukta balon sürücüsü gerekir).
    pub mem_min_mb: u64,
    /// Yedek sınamasında denetlenecek TCP kapısı (0: denetlenmez). Yedekten açılan geçici konukta bu kapıdaki
    /// hizmetin yanıt vermesi beklenir.
    pub test_port: u16,
    /// Yedek sınamasında konuk ajanıyla çalıştırılacak komut (boş: çalıştırılmaz); 0 ile bitmelidir.
    pub test_cmd: String,
    /// Sınama konuğunda `test_port` kapısına açılan sunucu tarafı kapı (yalnızca 127.0.0.1); RedoX atar.
    pub test_fwd: u16,
    /// Yedek yaşı hedefi (RPO, saat): son yedeği bundan eskiyse konuk "korumasız" sayılır. 0 ise sunucunun
    /// genel ayarı (`--backup-age-days`) geçerlidir.
    pub rpo_hours: u32,
    /// Güvenlik duvarı açık mı? Açıksa yalnızca `fw_rules` içindeki gelen trafiğe izin verilir.
    pub firewall: bool,
    /// Ağ kartı koruması: konuk yalnızca kendi MAC adresiyle konuşabilir (sahtecilik engeli), DHCP sunucusu
    /// gibi davranamaz ve yönlendirici duyurusu (RA) gönderemez. Köprüye bağlı kartlarda etkilidir.
    pub nic_guard: bool,
    pub fw_rules: Vec<FwRule>,
    /// Sanal makinenin açılış donanım yazılımı.
    pub firmware: Firmware,
    /// Güvenli önyükleme (yalnızca UEFI ile).
    pub secure_boot: bool,
    /// Sanal TPM 2.0 aygıtı.
    pub tpm: bool,
    /// Asıl diskin yanındaki veri diskleri (yalnızca sanal makinelerde).
    pub disks: Vec<ExtraDisk>,
    /// Askıda: konuk kapalı ama bellek durumu diskte kayıtlı; başlatılınca kaldığı yerden devam eder.
    pub suspended: bool,
    /// Şablon: başlatılamaz ve diski değişmez; ondan klonlar üretilir.
    pub template: bool,
    /// Bağlı klonun dayandığı şablon. Klonun diski yalnızca farkları tutar; şablonun diski arkalığıdır.
    pub base: Option<crate::model::VmId>,
    /// Bu konuğa şimdiye dek eklenen ek disklerin sayacı (`ExtraDisk::serial` buradan verilir).
    #[serde(default)]
    pub disk_seq: u64,
    /// Birinci karta ek ağ kartlarının bağlandığı ağlar (`nat` ya da köprü adı).
    pub nics: Vec<String>,
    /// Konuğa verilen sunucu USB aygıtları (`üretici:ürün`, onaltılık; `lsusb` çıktısındaki kimlik).
    pub usb: Vec<String>,
    /// Konuğa verilen sunucu PCI aygıtları (`0000:01:00.0`; `lspci -D` çıktısındaki adres).
    pub pci: Vec<String>,
    /// Açılışta başlatma sırası (1 ilk başlar); 0 sırasızdır ve sıralılardan sonra başlar.
    pub boot_order: u32,
    /// Bu konuk başlatıldıktan sonra sıradaki konuk için beklenecek süre (saniye).
    pub boot_delay: u32,
    /// Zamanlanmış güç işlemleri (başlat, kapat, yeniden başlat).
    pub schedules: Vec<Schedule>,
    /// Disk hız sınırı (MB/sn, okuma ve yazma toplamı; her diske ayrı uygulanır). 0 sınırsız.
    pub disk_mbps: u32,
    /// Disk işlem sınırı (saniyede giriş/çıkış işlemi; her diske ayrı). 0 sınırsız.
    pub disk_iops: u32,
    /// Ağ hız sınırı (Mbit/sn, her yönde; köprüye bağlı her karta ayrı). 0 sınırsız.
    pub net_mbps: u32,
    /// Beklenmedik biçimde kapanırsa (çökme, konuğun içinden kapatma) kendiliğinden yeniden başlatılsın mı?
    pub restart: bool,
    /// Sunucuyla paylaşılan klasörler (yalnızca sanal makinelerde; konukta 9p ile bağlanır).
    pub shares: Vec<Share>,
    /// Birinci kartın VLAN etiketi (1-4094); 0 etiketsiz. Yalnızca köprüye bağlı kartta.
    /// Ek kartların etiketi ağ tanımında durur: `vmbr1:20`.
    pub vlan: u16,
}

impl Hardware {
    /// Ağ hız sınırı bu ağ ayarıyla uygulanabilir mi? Sınır, sunucudaki TAP arayüzünde uygulanır;
    /// NAT kartında böyle bir arayüz yoktur. `bridge`: birinci kartın ağı.
    pub fn net_limit_applies(&self, bridge: &str) -> bool {
        self.net_mbps == 0 || bridge != crate::model::NAT || self.nics.iter().any(|n| parse_nic(n).is_some_and(|(b, _)| b != crate::model::NAT))
    }
}

/// Disk imajı dosya adı: yol ayırıcısı yok, gizli dosya değil; bulut imajları (`.qcow2`, `.img`, `.raw`)
/// ve başka sanallaştırıcılardan aktarılan diskler (`.vmdk`, `.vdi`, `.vhdx`, `.vhd`).
pub fn valid_image_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.len() <= 128
        && !name.starts_with('.')
        && [".qcow2", ".img", ".raw", ".vmdk", ".vdi", ".vhdx", ".vhd", ".ova"].iter().any(|ext| lower.len() > ext.len() && lower.ends_with(ext))
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub const MAX_CI_DNS: usize = 3;

pub const MAX_HOOK_LEN: usize = 256;

/// Yedek öncesi/sonrası komutu: boş ya da mutlak yolla başlayan, denetim karakteri içermeyen kısa bir satır.
/// Kabuk kullanılmaz: ilk sözcük programın yolu, kalanlar argümanlarıdır (boşlukla ayrılır).
pub fn valid_hook(command: &str) -> bool {
    command.is_empty() || (command.len() <= MAX_HOOK_LEN && !command.chars().any(|c| c.is_control()) && (command.starts_with('/') || command.as_bytes().get(1) == Some(&b':')))
}

/// Komutu program yolu ve argümanlarına böler.
pub fn split_hook(command: &str) -> Option<(String, Vec<String>)> {
    let mut words = command.split_whitespace().map(str::to_string);
    let path = words.next()?;
    Some((path, words.collect()))
}

/// Yedek yaşı hedefinin üst sınırı (saat): bir yıl.
pub const MAX_RPO_HOURS: u32 = 8760;

/// `a.b.c.d/önek` biçiminde IPv4 adresi (önek zorunlu, 1-32).
pub fn valid_ci_address(text: &str) -> bool {
    let Some((ip, prefix)) = text.split_once('/') else { return false };
    valid_ipv4(ip) && prefix.len() <= 2 && prefix.bytes().all(|b| b.is_ascii_digit()) && prefix.parse::<u8>().is_ok_and(|p| (1..=32).contains(&p))
}

/// Noktalı IPv4 adresi; `0.0.0.0` kabul edilmez.
pub fn valid_ipv4(text: &str) -> bool {
    text.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| !ip.is_unspecified()) && text.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Linux kullanıcı adı: küçük harf ya da alt çizgiyle başlar, en çok 32 karakter.
pub fn valid_ci_user(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_lowercase() || b == b'_')
        && name.len() <= 32
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        && name != "root"
}

/// Tek satırlık OpenSSH açık anahtarı: `tür base64 [açıklama]`.
/// YAML'a çift tırnak içinde yazıldığı için tırnak ve ters bölü kabul edilmez.
pub fn valid_ssh_key(key: &str) -> bool {
    const KINDS: [&str; 6] =
        ["ssh-ed25519", "ssh-rsa", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521", "sk-ssh-ed25519@openssh.com"];
    let mut parts = key.split(' ');
    let (Some(kind), Some(data)) = (parts.next(), parts.next()) else { return false };
    key.len() <= 2048
        && KINDS.contains(&kind)
        && data.len() >= 32
        && data.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        && key.bytes().all(|b| (b.is_ascii_graphic() || b == b' ') && b != b'"' && b != b'\\')
}

impl Hardware {
    /// Alanların kendi içinde ve konuk türüyle tutarlılığı.
    pub fn validate(&self, kind: GuestKind) -> Result<(), String> {
        if self.fw_rules.len() > MAX_FW_RULES {
            return Err(format!("en fazla {MAX_FW_RULES} güvenlik duvarı kuralı eklenebilir"));
        }
        if let Some(bad) = self.fw_rules.iter().find(|r| !r.valid()) {
            return Err(format!("geçersiz güvenlik duvarı kuralı: {bad}"));
        }
        if !self.disks.is_empty() && kind != GuestKind::Vm {
            return Err("ek disk yalnızca sanal makinelere eklenebilir".to_string());
        }
        let mut ids: Vec<u8> = self.disks.iter().map(|d| d.id).collect();
        ids.sort_unstable();
        ids.dedup();
        if self.disks.len() > MAX_EXTRA_DISKS || ids.len() != self.disks.len() || self.disks.iter().any(|d| d.id == 0 || d.id as usize > MAX_EXTRA_DISKS || d.size_gb == 0) {
            return Err(format!("geçersiz ek disk listesi (en fazla {MAX_EXTRA_DISKS} disk, her biri en az 1 GB)"));
        }
        if self.disk_mbps > MAX_DISK_MBPS || self.disk_iops > MAX_DISK_IOPS || self.net_mbps > MAX_NET_MBPS {
            return Err(format!("hız sınırı çok büyük: disk en çok {MAX_DISK_MBPS} MB/sn ve {MAX_DISK_IOPS} işlem/sn, ağ en çok {MAX_NET_MBPS} Mbit/sn (0 sınırsız)"));
        }
        if (self.disk_mbps > 0 || self.disk_iops > 0 || self.net_mbps > 0) && kind != GuestKind::Vm {
            return Err("disk ve ağ hız sınırı yalnızca sanal makinelerde kullanılır".to_string());
        }
        if self.boot_order > MAX_BOOT_ORDER {
            return Err(format!("başlatma sırası 0 ile {MAX_BOOT_ORDER} arasında olmalı"));
        }
        if self.boot_delay > MAX_BOOT_DELAY {
            return Err(format!("başlatma gecikmesi en çok {MAX_BOOT_DELAY} saniye olabilir"));
        }
        if self.schedules.len() > MAX_SCHEDULES {
            return Err(format!("en fazla {MAX_SCHEDULES} zamanlanmış işlem eklenebilir"));
        }
        if let Some(bad) = self.schedules.iter().find(|s| !s.valid()) {
            return Err(format!("geçersiz zamanlanmış işlem: {bad} (yalnızca start, stop ve reboot; saat SS:DD)"));
        }
        if self.nics.len() > MAX_EXTRA_NICS {
            return Err(format!("en fazla {MAX_EXTRA_NICS} ek ağ kartı eklenebilir"));
        }
        if let Some(bad) = self.nics.iter().find(|n| parse_nic(n).is_none()) {
            return Err(format!("geçersiz ağ ayarı: {bad} (köprü adı, nat ya da VLAN etiketiyle köprü:etiket; etiket 1-{MAX_VLAN})"));
        }
        if self.shares.len() > MAX_SHARES {
            return Err(format!("en fazla {MAX_SHARES} paylaşılan klasör eklenebilir"));
        }
        if let Some(bad) = self.shares.iter().find(|s| !s.valid()) {
            return Err(format!("geçersiz paylaşım adı \"{}\": küçük harf, rakam ve tire; en çok 24 karakter", bad.name));
        }
        if self.shares.iter().enumerate().any(|(i, s)| self.shares[..i].iter().any(|o| o.name == s.name)) {
            return Err("aynı paylaşım iki kez eklenemez".to_string());
        }
        if !self.shares.is_empty() && kind != GuestKind::Vm {
            return Err("paylaşılan klasör yalnızca sanal makinelerde kullanılır".to_string());
        }
        if self.vlan > MAX_VLAN {
            return Err(format!("VLAN etiketi 1 ile {MAX_VLAN} arasında olmalı (0 etiketsiz)"));
        }
        if (self.vlan > 0 || self.nics.iter().any(|n| n.contains(':'))) && kind != GuestKind::Vm {
            return Err("VLAN etiketi yalnızca sanal makinelerde kullanılır".to_string());
        }
        let cloud = self.image.is_some() || self.ci_user.is_some() || self.ci_ssh_key.is_some() || self.ci_ip.is_some() || self.ci_gw.is_some() || !self.ci_dns.is_empty();
        let machine = self.firmware != Firmware::Bios || self.secure_boot || self.tpm || !self.usb.is_empty() || !self.pci.is_empty();
        if (cloud || machine) && kind != GuestKind::Vm {
            return Err("disk imajı, cloud-init, UEFI, TPM ve aygıt aktarımı yalnızca sanal makinelerde kullanılır".to_string());
        }
        if self.usb.len() > MAX_PASSTHROUGH || self.pci.len() > MAX_PASSTHROUGH {
            return Err(format!("en fazla {MAX_PASSTHROUGH} USB ve {MAX_PASSTHROUGH} PCI aygıtı aktarılabilir"));
        }
        if let Some(bad) = self.usb.iter().find(|u| !valid_usb_id(u)) {
            return Err(format!("geçersiz USB kimliği \"{bad}\"; lsusb çıktısındaki gibi yazın, örn. 046d:c52b"));
        }
        if let Some(bad) = self.pci.iter().find(|p| !valid_passthrough(p)) {
            return Err(format!("geçersiz PCI adresi \"{bad}\"; lspci -D çıktısındaki gibi yazın, örn. 0000:01:00.0"));
        }
        if self.secure_boot && self.firmware != Firmware::Uefi {
            return Err("güvenli önyükleme UEFI donanım yazılımı gerektirir".to_string());
        }
        if let Some(image) = &self.image {
            if !valid_image_name(image) {
                return Err(format!("geçersiz bulut imajı adı: {image}"));
            }
        }
        if let Some(user) = &self.ci_user {
            if !valid_ci_user(user) {
                return Err(format!("geçersiz cloud-init kullanıcı adı: {user} (küçük harf, rakam, tire; root olamaz)"));
            }
            if self.ci_ssh_key.is_none() {
                return Err("cloud-init kullanıcısı için SSH açık anahtarı gerekli".to_string());
            }
        }
        if self.nested {
            if kind != GuestKind::Vm {
                return Err("iç içe sanallaştırma yalnızca sanal makinelerde kullanılır".to_string());
            }
            if matches!(self.cpu_type.as_str(), "qemu64" | "kvm64") {
                return Err(format!("iç içe sanallaştırma {} modeliyle kullanılamaz: sunucunun işlemcisini (host) ya da üreticiye özgü bir model seçin", self.cpu_type));
            }
        }
        if self.nic_guard && kind != GuestKind::Vm {
            return Err("ağ kartı koruması yalnızca sanal makinelerde kullanılır".to_string());
        }
        if (!self.cpu_type.is_empty() || self.balloon) && kind != GuestKind::Vm {
            return Err("işlemci türü ve bellek balonu yalnızca sanal makinelerde kullanılır".to_string());
        }
        if self.test_port > 0 || !self.test_cmd.is_empty() {
            if kind != GuestKind::Vm {
                return Err("yedek sınaması denetimleri yalnızca sanal makinelerde kullanılır".to_string());
            }
            if !valid_hook(&self.test_cmd) {
                return Err(format!("sınama komutu mutlak bir program yoluyla başlamalı, tek satır ve en fazla {MAX_HOOK_LEN} karakter olmalı (örn. /usr/local/bin/saglik.sh)"));
            }
        }
        if !self.backup_pre.is_empty() || !self.backup_post.is_empty() {
            if kind != GuestKind::Vm {
                return Err("yedek öncesi ve sonrası komutları yalnızca sanal makinelerde kullanılır (konuk ajanıyla çalışır)".to_string());
            }
            if !valid_hook(&self.backup_pre) || !valid_hook(&self.backup_post) {
                return Err(format!("yedek komutu mutlak bir program yoluyla başlamalı, tek satır ve en fazla {MAX_HOOK_LEN} karakter olmalı (örn. /usr/local/bin/yedek-oncesi.sh)"));
            }
        }
        if !valid_cpu_type(&self.cpu_type) {
            return Err(format!("bilinmeyen işlemci türü: {} (geçerli olanlar: host, {})", self.cpu_type, CPU_TYPES.join(", ")));
        }
        if let Some(ip) = &self.ci_ip {
            if !valid_ci_address(ip) {
                return Err(format!("geçersiz cloud-init adresi: {ip} (örn. 10.0.0.5/24)"));
            }
            if self.ci_ssh_key.is_none() {
                return Err("cloud-init ağ ayarı için SSH açık anahtarı gerekli".to_string());
            }
        }
        if let Some(gw) = &self.ci_gw {
            if !valid_ipv4(gw) {
                return Err(format!("geçersiz ağ geçidi: {gw}"));
            }
        }
        if let Some(bad) = self.ci_dns.iter().find(|d| !valid_ipv4(d)) {
            return Err(format!("geçersiz ad sunucusu: {bad}"));
        }
        if self.ci_dns.len() > MAX_CI_DNS {
            return Err(format!("en fazla {MAX_CI_DNS} ad sunucusu yazılabilir"));
        }
        if self.ci_ip.is_none() && (self.ci_gw.is_some() || !self.ci_dns.is_empty()) {
            return Err("ağ geçidi ve ad sunucusu yalnızca sabit adresle birlikte verilir (adres boşsa konuk DHCP kullanır)".to_string());
        }
        if let Some(key) = &self.ci_ssh_key {
            if !valid_ssh_key(key) {
                return Err("geçersiz SSH açık anahtarı: tek satır, \"ssh-ed25519 AAAA… açıklama\" biçiminde olmalı".to_string());
            }
        }
        Ok(())
    }

    pub fn firmware_choice(&self) -> &'static str {
        match (self.firmware, self.secure_boot) {
            (Firmware::Bios, _) => "bios",
            (Firmware::Uefi, false) => "uefi",
            (Firmware::Uefi, true) => "uefi-secure",
        }
    }

    pub fn firmware_label(&self) -> &'static str {
        match (self.firmware, self.secure_boot) {
            (Firmware::Bios, _) => "BIOS",
            (Firmware::Uefi, false) => "UEFI",
            (Firmware::Uefi, true) => "UEFI + güvenli önyükleme",
        }
    }

    /// cloud-init kullanılıyor mu (konuğa `cidata` sürücüsü takılacak mı)?
    pub fn cloud_init(&self) -> bool {
        self.ci_ssh_key.is_some()
    }

    /// cloud-init ayarlarını uygular (`None`: dokunma; boş metin: temizle). Anahtar boşaltılırsa cloud-init
    /// kapanır ve öbür ayarlar da silinir. Bir şey değiştiyse `true` döner ve sürüm numarasını artırır.
    pub fn apply_cloud_init(&mut self, user: Option<&str>, key: Option<&str>, ip: Option<&str>, gw: Option<&str>, dns: Option<&str>) -> bool {
        let before = (self.ci_user.clone(), self.ci_ssh_key.clone(), self.ci_ip.clone(), self.ci_gw.clone(), self.ci_dns.clone());
        let text = |v: &str| Some(v.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());
        if let Some(v) = user {
            self.ci_user = text(v);
        }
        if let Some(v) = key {
            self.ci_ssh_key = text(v);
        }
        if let Some(v) = ip {
            self.ci_ip = text(v);
        }
        if let Some(v) = gw {
            self.ci_gw = text(v);
        }
        if let Some(v) = dns {
            self.ci_dns = v.split([',', ' ', '\n']).map(str::trim).filter(|d| !d.is_empty()).map(str::to_string).collect();
        }
        if key.is_some() && self.ci_ssh_key.is_none() {
            (self.ci_user, self.ci_ip, self.ci_gw) = (None, None, None);
            self.ci_dns.clear();
        }
        let changed = before != (self.ci_user.clone(), self.ci_ssh_key.clone(), self.ci_ip.clone(), self.ci_gw.clone(), self.ci_dns.clone());
        if changed {
            self.ci_gen = self.ci_gen.saturating_add(1);
        }
        changed
    }

    /// NoCloud `network-config` dosyası (sürüm 2); sabit adres verilmemişse `None` (konuk DHCP kullanır).
    /// Kart, konuğun birinci kartının değişmeyen MAC adresiyle eşleştirilir.
    pub fn network_config(&self, vmid: VmId) -> Option<String> {
        let ip = self.ci_ip.as_deref().filter(|ip| valid_ci_address(ip))?;
        let mut out = format!(
            "version: 2\nethernets:\n  nic0:\n    match:\n      macaddress: \"{}\"\n    dhcp4: false\n    addresses:\n      - {ip}\n",
            crate::model::mac_for(vmid)
        );
        if let Some(gw) = self.ci_gw.as_deref().filter(|gw| valid_ipv4(gw)) {
            out.push_str(&format!("    gateway4: {gw}\n"));
        }
        let dns: Vec<&str> = self.ci_dns.iter().map(String::as_str).filter(|d| valid_ipv4(d)).collect();
        if !dns.is_empty() {
            out.push_str(&format!("    nameservers:\n      addresses: [{}]\n", dns.join(", ")));
        }
        Some(out)
    }

    /// NoCloud `user-data` dosyası; cloud-init kullanılmıyorsa `None`.
    pub fn user_data(&self, hostname: &str) -> Option<String> {
        let key = self.ci_ssh_key.as_deref().filter(|k| valid_ssh_key(k))?;
        let user = self.ci_user.as_deref().filter(|u| valid_ci_user(u)).unwrap_or(DEFAULT_CI_USER);
        Some(format!(
            "#cloud-config\nhostname: {hostname}\nmanage_etc_hosts: true\nssh_pwauth: false\nusers:\n  - name: {user}\n    \
             sudo: \"ALL=(ALL) NOPASSWD:ALL\"\n    lock_passwd: true\n    ssh_authorized_keys:\n      - \"{key}\"\n"
        ))
    }
}

/// NoCloud `meta-data` dosyası. Kimlik değişirse cloud-init ilk kurulumu yeniden yapar;
/// klonun kimliği farklı olduğu için kendi adını ve anahtarlarını alır.
/// `generation` (ayar sürümü) 0'dan büyükse kimliğe eklenir: ayarlar değişince cloud-init yeniden çalışır.
pub fn meta_data(vmid: VmId, hostname: &str, generation: u32) -> String {
    match generation {
        0 => format!("instance-id: redox-{vmid}\nlocal-hostname: {hostname}\n"),
        n => format!("instance-id: redox-{vmid}-{n}\nlocal-hostname: {hostname}\n"),
    }
}

#[cfg(test)]
mod tests;
