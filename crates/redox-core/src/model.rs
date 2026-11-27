use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub type VmId = u32;
pub type TaskId = u64;

/// Geçerli konuk kimliği aralığı.
pub const VMID_MIN: VmId = 100;
pub const VMID_MAX: VmId = 999_999_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuestKind {
    Vm,
    Ct,
}

impl GuestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GuestKind::Vm => "vm",
            GuestKind::Ct => "ct",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GuestKind::Vm => "VM",
            GuestKind::Ct => "CT",
        }
    }

    pub fn long_label(self) -> &'static str {
        match self {
            GuestKind::Vm => "Sanal makine",
            GuestKind::Ct => "Konteyner",
        }
    }

    /// Bu türü çalıştıran sanallaştırma teknolojisi.
    pub fn backend(self) -> &'static str {
        match self {
            GuestKind::Vm => "kvm",
            GuestKind::Ct => "lxc",
        }
    }

    /// Oluşturulurken izin verilen en az bellek (MB).
    pub fn min_memory_mb(self) -> u64 {
        match self {
            GuestKind::Vm => 64,
            GuestKind::Ct => 16,
        }
    }
}

impl FromStr for GuestKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "vm" => Ok(GuestKind::Vm),
            "ct" => Ok(GuestKind::Ct),
            _ => Err(()),
        }
    }
}

impl fmt::Display for GuestKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuestStatus {
    Running,
    Stopped,
    Paused,
}

impl GuestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            GuestStatus::Running => "running",
            GuestStatus::Stopped => "stopped",
            GuestStatus::Paused => "paused",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GuestStatus::Running => "Çalışıyor",
            GuestStatus::Stopped => "Durduruldu",
            GuestStatus::Paused => "Duraklatıldı",
        }
    }
}

impl fmt::Display for GuestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GuestStatus::Running => "çalışıyor",
            GuestStatus::Stopped => "durdurulmuş",
            GuestStatus::Paused => "duraklatılmış",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Online,
    Offline,
}

impl NodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Online => "online",
            NodeStatus::Offline => "offline",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NodeStatus::Online => "Çevrimiçi",
            NodeStatus::Offline => "Çevrimdışı",
        }
    }
}

/// Bir konuğun yaşam döngüsü eylemleri.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Start,
    Stop,
    Reboot,
    Pause,
    Resume,
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::Start,
        Action::Stop,
        Action::Reboot,
        Action::Pause,
        Action::Resume,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Reboot => "reboot",
            Action::Pause => "pause",
            Action::Resume => "resume",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Start => "Başlat",
            Action::Stop => "Kapat",
            Action::Reboot => "Yeniden Başlat",
            Action::Pause => "Duraklat",
            Action::Resume => "Sürdür",
        }
    }

    /// Eylem bu durumdan uygulanabiliyorsa varılacak durumu döndürür.
    pub fn target(self, from: GuestStatus) -> Option<GuestStatus> {
        use GuestStatus::*;
        match (self, from) {
            (Action::Start, Stopped) => Some(Running),
            (Action::Stop, Running | Paused) => Some(Stopped),
            (Action::Reboot, Running) => Some(Running),
            (Action::Pause, Running) => Some(Paused),
            (Action::Resume, Paused) => Some(Running),
            _ => None,
        }
    }
}

impl FromStr for Action {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        Action::ALL.into_iter().find(|a| a.as_str() == s).ok_or(())
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageKind {
    #[serde(rename = "zfs")]
    Zfs,
    #[serde(rename = "nfs")]
    Nfs,
    #[serde(rename = "lvm-thin")]
    LvmThin,
    #[serde(rename = "lvm")]
    Lvm,
    #[serde(rename = "dir")]
    Dir,
}

impl StorageKind {
    pub fn label(self) -> &'static str {
        match self {
            StorageKind::Zfs => "ZFS",
            StorageKind::Nfs => "NFS",
            StorageKind::LvmThin => "LVM-thin",
            StorageKind::Lvm => "LVM",
            StorageKind::Dir => "Dizin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub status: NodeStatus,
    pub cores: u32,
    pub cpu_pct: u8,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub uptime_secs: u64,
    pub kernel: String,
    pub ip: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guest {
    pub vmid: VmId,
    pub name: String,
    pub kind: GuestKind,
    pub node: String,
    pub status: GuestStatus,
    pub cores: u32,
    pub cpu_pct: u8,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub disk_gb: u64,
    pub storage: String,
    pub os: String,
    pub ip: Option<String>,
    /// Konuk ajanı yanıt veriyorsa bildirdiği işletim sistemi; ajan yoksa `None`.
    #[serde(default)]
    pub agent: Option<String>,
    pub bridge: String,
    pub onboot: bool,
    /// Son başlatılma anı (epoch saniyesi); çalışmıyorsa `None`.
    pub started_at: Option<u64>,
    /// Üzerinde süren görev; doluysa yeni eylem kabul edilmez.
    #[serde(default)]
    pub lock: Option<TaskId>,
    /// Takılı kurulum kalıbı (ISO dosya adı).
    #[serde(default)]
    pub iso: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Sunucu portundan konuk portuna TCP yönlendirmeleri.
    #[serde(default)]
    pub forwards: Vec<PortForward>,
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
    /// Günlük otomatik yedeğin saati (`SS:DD`, sunucu saatiyle); `None` ise zamanlama yok.
    #[serde(default)]
    pub backup_time: Option<String>,
    /// Saklanacak otomatik yedek sayısı; fazlası en eskiden başlayarak silinir.
    #[serde(default = "default_keep")]
    pub backup_keep: u32,
    /// Son otomatik yedeğin alındığı gün (`YYYY-AA-GG`); aynı gün ikinci kez alınmaz.
    #[serde(default)]
    pub last_auto_backup: Option<String>,
    /// Konuğun kaynak havuzu; havuzun kotası bu konuğun ayrılmış kaynaklarını da sayar.
    #[serde(default)]
    pub pool: Option<String>,
    /// Otomatik yedeklerde "son N"e ek olarak saklanacak haftalık, aylık ve yıllık yedek sayıları.
    #[serde(default)]
    pub backup_gfs: crate::retention::Gfs,
    /// İsteğe bağlı donanım ve ilk kurulum ayarları (JSON'da konuğun kendi alanları olarak durur).
    #[serde(flatten)]
    pub hw: crate::hw::Hardware,
}

pub const DEFAULT_KEEP: u32 = 7;
pub const MAX_KEEP: u32 = 365;

fn default_keep() -> u32 {
    DEFAULT_KEEP
}

/// `SS:DD` biçimindeki saati gece yarısından itibaren dakikaya çevirir.
pub fn parse_time(text: &str) -> Option<u16> {
    let (h, m) = text.split_once(':')?;
    if h.len() != 2 || m.len() != 2 || !text.bytes().all(|b| b.is_ascii_digit() || b == b':') {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub const MAX_TAGS: usize = 8;
pub const MAX_FORWARDS: usize = 16;
pub const MAX_SNAPSHOTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForward {
    pub host_port: u16,
    pub guest_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    pub created: u64,
    /// Üretim denetim noktası: konuk çalışırken yalnızca diskler kaydedildi (bellek durumu yok).
    /// Geri dönünce konuk o andaki diskle **yeniden açılır**; bu yüzden dönüş konuk kapalıyken yapılır.
    #[serde(default)]
    pub disk_only: bool,
    /// Görüntü alınırken dosya sistemleri konuk ajanıyla donduruldu mu (uygulama tutarlı)?
    #[serde(default)]
    pub quiesced: bool,
    /// Görüntü alındığı andaki çekirdek sayısı, bellek (MB) ve çalışırken eklenmiş bellek parçaları. Bellek
    /// durumlu görüntü yalnızca aynı donanımlı çalışan konuğa yüklenebilir. (Eski kayıtlarda 0: bilinmiyor.)
    #[serde(default)]
    pub cores: u32,
    #[serde(default)]
    pub mem_mb: u64,
    #[serde(default)]
    pub dimms: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backup {
    /// Yedek dosyasının adı (uzantısız), örn. `vm-103-1800000000`.
    pub id: String,
    pub vmid: VmId,
    pub guest_name: String,
    pub created: u64,
    pub size_mb: u64,
    /// Zamanlayıcının aldığı yedek; saklama sayısı yalnızca bunlara uygulanır.
    #[serde(default)]
    pub auto: bool,
    /// Sunucu dışındaki hedefte kopyası var mı?
    #[serde(default)]
    pub offsite: bool,
    /// Artımlı yedekte depoya gerçekten eklenen kısım (MB); tam yedekte `None`.
    #[serde(default)]
    pub new_mb: Option<u64>,
    /// Yedeğin kapsadığı ek diskler (alındığı andaki numara ve boyutlarıyla). Asıl disk her zaman dahildir.
    #[serde(default)]
    pub disks: Vec<crate::hw::ExtraDisk>,
    /// Asıl diskin yedek alındığı andaki boyutu (GB); eski kayıtlarda 0 (bilinmiyor).
    #[serde(default)]
    pub disk_gb: u64,
    /// Konuk çalışırken alınan yedekte dosya sistemi konuk ajanıyla donduruldu mu?
    /// `None`: konuk kapalıydı (ya da eski kayıt).
    #[serde(default)]
    pub quiesced: Option<bool>,
    /// Yedek, değişen blok takibiyle (disk baştan sona okunmadan) mı alındı?
    #[serde(default)]
    pub cbt: bool,
    /// Dış kopya, yedeğin adına bağlı biçimde mi gönderildi? Öyleyse geri indirirken ada bağlı olmayan
    /// (eski biçimli) bir dosya kabul edilmez. Eski kayıtlarda `false`.
    #[serde(default)]
    pub offsite_bound: bool,
    /// Yedeği alan yedek işinin adı; konuğun kendi zamanlamasıyla ya da elle alındıysa `None`.
    #[serde(default)]
    pub job: Option<String>,
    /// Yedeğin yanında makine durumu (UEFI değişken deposu, sanal TPM) da var mı?
    #[serde(default)]
    pub machine: bool,
    /// Yedeğin kaç ek dış hedefte (yedek kopyalama hedefleri) kopyası var?
    #[serde(default)]
    pub copies: u8,
    /// Yedeğin açılarak sınanmasının son sonucu; hiç sınanmadıysa `None`.
    #[serde(default)]
    pub tested: Option<BackupTest>,
    /// Olağan dışı değişim uyarısı: yedekte değişen veri önceki yedeklere göre çok fazlaysa nedeni burada
    /// yazar (toplu şifreleme, yani fidye yazılımı belirtisi olabilir). Olağansa `None`.
    #[serde(default)]
    pub suspicious: Option<String>,
    /// Bu ana (epoch saniyesi) kadar silinemez ve budanmaz; `None` ise kilitsiz.
    #[serde(default)]
    pub locked_until: Option<u64>,
    /// Konuğun yedek alındığı andaki tanımı: yedekten yeni bir konuk açmak için (konuk silinmiş olsa da).
    /// Bu özellikten önce alınmış yedeklerde yoktur.
    #[serde(default)]
    pub spec: Option<Box<GuestSpec>>,
}

/// Konuk ayarlarında istenen değişiklikler; `None` alanlar olduğu gibi kalır.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuestPatch {
    pub name: Option<String>,
    pub cores: Option<u32>,
    pub memory_mb: Option<u64>,
    pub os: Option<String>,
    /// Boş dizge kalıbı çıkarır.
    pub iso: Option<String>,
    pub onboot: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub forwards: Option<Vec<PortForward>>,
    /// `nat` ya da sunucudaki bir köprünün adı.
    pub bridge: Option<String>,
    /// `SS:DD`; boş dizge zamanlamayı kapatır.
    pub backup_time: Option<String>,
    pub backup_keep: Option<u32>,
    /// Yedek yaşı hedefi (RPO, saat); 0 sunucunun genel ayarına bırakır.
    pub rpo_hours: Option<u32>,
    /// Yedek sınamasında denetlenecek TCP kapısı (0: yok) ve konukta çalıştırılacak komut (boş: yok).
    pub test_port: Option<u16>,
    pub test_cmd: Option<String>,
    pub firewall: Option<bool>,
    /// Ağ kartı koruması (MAC sahteciliği, DHCP sunucusu ve yönlendirici duyurusu engeli).
    pub nic_guard: Option<bool>,
    pub fw_rules: Option<Vec<crate::hw::FwRule>>,
    pub firmware: Option<crate::hw::Firmware>,
    pub secure_boot: Option<bool>,
    pub tpm: Option<bool>,
    /// Ek ağ kartlarının ağları (`nat` ya da köprü adı).
    pub nics: Option<Vec<String>>,
    pub usb: Option<Vec<String>>,
    pub pci: Option<Vec<String>>,
    /// Açılışta başlatma sırası; 0 sırasız.
    pub boot_order: Option<u32>,
    /// Başlatıldıktan sonra sıradaki konuk için beklenecek saniye.
    pub boot_delay: Option<u32>,
    /// Zamanlanmış güç işlemlerinin tam listesi (verilirse eskisinin yerine geçer).
    pub schedules: Option<Vec<crate::hw::Schedule>>,
    /// Disk hız sınırı (MB/sn); 0 sınırsız.
    pub disk_mbps: Option<u32>,
    /// Disk işlem sınırı (işlem/sn); 0 sınırsız.
    pub disk_iops: Option<u32>,
    /// Ağ hız sınırı (Mbit/sn); 0 sınırsız.
    pub net_mbps: Option<u32>,
    /// Birinci kartın VLAN etiketi; 0 etiketsiz.
    pub vlan: Option<u16>,
    /// Beklenmedik kapanmada yeniden başlat.
    pub restart: Option<bool>,
    /// Paylaşılan klasörlerin tam listesi (verilirse eskisinin yerine geçer).
    pub shares: Option<Vec<crate::hw::Share>>,
    /// cloud-init ayarları; boş metin alanı temizler. Anahtar boşaltılırsa cloud-init kapanır.
    pub ci_user: Option<String>,
    pub ci_ssh_key: Option<String>,
    /// Sabit IPv4 adresi (`10.0.0.5/24`); boşsa DHCP.
    pub ci_ip: Option<String>,
    pub ci_gw: Option<String>,
    /// Ad sunucuları, virgülle.
    pub ci_dns: Option<String>,
    /// İşlemci modeli; boş metin sunucunun kendi işlemcisi.
    pub cpu_type: Option<String>,
    /// Çalışırken çıkılabilecek en fazla çekirdek; 0 kapatır.
    pub max_cores: Option<u32>,
    /// Çalışırken çıkılabilecek en fazla bellek (MB); 0 kapatır.
    pub max_memory_mb: Option<u64>,
    /// İşlemci payı (0: olağan).
    pub cpu_shares: Option<u32>,
    /// Bellek garantisi (MB; 0: yok).
    pub mem_reserve_mb: Option<u64>,
    /// Bellek balonu aygıtı.
    pub balloon: Option<bool>,
    /// İç içe sanallaştırma.
    pub nested: Option<bool>,
    /// Kaynak havuzu; boş metin konuğu havuzdan çıkarır.
    pub pool: Option<String>,
    /// Yedekten önce ve sonra konukta çalıştırılacak komutlar; boş metin kaldırır.
    pub backup_pre: Option<String>,
    pub backup_post: Option<String>,
    /// Saklanacak haftalık, aylık ve yıllık otomatik yedek sayıları (0: kapalı).
    pub backup_weekly: Option<u32>,
    pub backup_monthly: Option<u32>,
    pub backup_yearly: Option<u32>,
}

impl GuestPatch {
    /// cloud-init alanlarını donanım kaydına uygular; bir şey değiştiyse `true`.
    pub fn apply_cloud_init(&self, hw: &mut crate::hw::Hardware) -> bool {
        hw.apply_cloud_init(self.ci_user.as_deref(), self.ci_ssh_key.as_deref(), self.ci_ip.as_deref(), self.ci_gw.as_deref(), self.ci_dns.as_deref())
    }
}

/// Konuğun ağ kipi olarak "köprü yok": kullanıcı kipinde NAT.
pub const NAT: &str = "nat";

/// Ağ ayarı: `nat` ya da bir Linux arayüz adı (en çok 15 karakter).
pub fn valid_bridge(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !name.starts_with('-')
        && name != "."
        && name != ".."
}

/// Konuğun sanal ağ kartı adresi: kimlikten türetilir, yeniden başlatmalarda değişmez.
/// `52:54:00` QEMU/KVM'in yerel yönetimli önekidir.
/// Çoğaltılan konuğun buradaki yedek kopyasının adı (ad sınırını aşmayacak biçimde).
pub fn replica_name(name: &str) -> String {
    const SUFFIX: &str = "-kopya";
    let keep: String = name.chars().take(63usize.saturating_sub(SUFFIX.len())).collect();
    format!("{}{SUFFIX}", keep.trim_end_matches('-'))
}

pub fn mac_for(vmid: VmId) -> String {
    let b = vmid.to_be_bytes();
    format!("52:54:00:{:02x}:{:02x}:{:02x}", b[1], b[2], b[3])
}

/// Ek ağ kartının adresi (`index` 1'den başlar; 0 birinci karttır).
pub fn mac_for_nic(vmid: VmId, index: u8) -> String {
    let b = vmid.to_be_bytes();
    format!("52:54:{index:02x}:{:02x}:{:02x}:{:02x}", b[1], b[2], b[3])
}

/// Yedeğin geçici bir konuk olarak açılıp çalıştığının sınanması.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupTest {
    pub at: u64,
    pub ok: bool,
    /// Neyin doğrulandığı ya da neden başarısız olduğu.
    pub note: String,
}

impl Backup {
    pub fn is_locked(&self, now: u64) -> bool {
        self.locked_until.is_some_and(|until| until > now)
    }
}

/// Yedekten açılan konuğun önerilen adı: `<ad>-yedekten`, ad sınırına sığacak biçimde kısaltılır.
pub fn restored_name(original: &str) -> String {
    const SUFFIX: &str = "-yedekten";
    let keep: String = original.chars().take(63usize.saturating_sub(SUFFIX.len())).collect();
    let name = format!("{}{SUFFIX}", keep.trim_end_matches('-'));
    if valid_name(&name) { name } else { "yedekten".to_string() }
}

pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= 24 && tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn valid_snapshot_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// "prod, web ci" → ["prod", "web", "ci"] (küçük harfe çevrilir; doğrulama ayrıdır).
pub fn parse_tags(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// "8080:80, 2222:22" → yönlendirme listesi (sunucu portu:konuk portu).
pub fn parse_forwards(text: &str) -> Result<Vec<PortForward>, String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|item| {
            let bad = || format!("geçersiz yönlendirme \"{item}\"; biçim sunucu-portu:konuk-portu, örn. 8080:80");
            let (host, guest) = item.split_once(':').ok_or_else(bad)?;
            Ok(PortForward { host_port: host.parse().map_err(|_| bad())?, guest_port: guest.parse().map_err(|_| bad())? })
        })
        .collect()
}

impl Guest {
    pub fn mem_pct(&self) -> u8 {
        crate::fmt::pct(self.mem_used_mb, self.mem_total_mb)
    }

    /// Asıl disk ve ek disklerin toplam boyutu (depolama hesabı bununla yapılır).
    /// Konuğun, aynısını yeniden oluşturmaya yetecek tanımı.
    pub fn to_spec(&self) -> GuestSpec {
        GuestSpec {
            vmid: None,
            name: self.name.clone(),
            kind: self.kind,
            node: self.node.clone(),
            cores: self.cores,
            memory_mb: self.mem_total_mb,
            disk_gb: self.disk_gb,
            storage: self.storage.clone(),
            os: self.os.clone(),
            bridge: self.bridge.clone(),
            onboot: self.onboot,
            iso: None,
            hw: self.hw.clone(),
        }
    }

    pub fn total_disk_gb(&self) -> u64 {
        self.disk_gb + self.hw.disks.iter().map(|d| d.size_gb).sum::<u64>()
    }

    pub fn uptime_secs(&self, now: u64) -> Option<u64> {
        self.started_at.map(|s| now.saturating_sub(s))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Storage {
    pub id: String,
    /// `None` ise depolama bütün düğümlerce paylaşılır.
    pub node: Option<String>,
    pub kind: StorageKind,
    pub content: Vec<String>,
    pub used_gb: u64,
    pub total_gb: u64,
}

impl Storage {
    pub fn free_gb(&self) -> u64 {
        self.total_gb.saturating_sub(self.used_gb)
    }

    pub fn used_pct(&self) -> u8 {
        crate::fmt::pct(self.used_gb, self.total_gb)
    }

    pub fn available_on(&self, node: &str) -> bool {
        self.node.as_deref().is_none_or(|n| n == node)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Ok,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub started: u64,
    pub ended: Option<u64>,
    pub node: String,
    pub user: String,
    /// `redox` komut satırı karşılığı, örn. `vm start 103`.
    pub command: String,
    pub status: TaskStatus,
    pub message: Option<String>,
    pub vmid: Option<VmId>,
    pub action: Option<Action>,
}

fn default_os() -> String {
    "linux".to_string()
}

fn default_bridge() -> String {
    NAT.to_string()
}

/// Yeni konuk isteği.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuestSpec {
    /// Boşsa sıradaki uygun kimlik atanır.
    #[serde(default)]
    pub vmid: Option<VmId>,
    pub name: String,
    pub kind: GuestKind,
    pub node: String,
    pub cores: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    pub storage: String,
    #[serde(default = "default_os")]
    pub os: String,
    #[serde(default = "default_bridge")]
    pub bridge: String,
    #[serde(default)]
    pub onboot: bool,
    /// Kurulum kalıbı (ISO dosya adı); yalnızca sanal makinelerde.
    #[serde(default)]
    pub iso: Option<String>,
    #[serde(flatten)]
    pub hw: crate::hw::Hardware,
}

/// ISO dosya adı kuralı: yol ayırıcısı yok, gizli dosya değil, `.iso` ile biter.
pub fn valid_iso_name(name: &str) -> bool {
    name.len() <= 128
        && name.len() > 4
        && !name.starts_with('.')
        && name.to_ascii_lowercase().ends_with(".iso")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// Ana makine adı kuralı: 1-63 karakter, küçük harf/rakam/tire, uçlarda tire yok.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_table_matches_lifecycle() {
        use GuestStatus::*;
        let expect = [
            (Action::Start, Stopped, Some(Running)),
            (Action::Start, Running, None),
            (Action::Start, Paused, None),
            (Action::Stop, Running, Some(Stopped)),
            (Action::Stop, Paused, Some(Stopped)),
            (Action::Stop, Stopped, None),
            (Action::Reboot, Running, Some(Running)),
            (Action::Reboot, Stopped, None),
            (Action::Reboot, Paused, None),
            (Action::Pause, Running, Some(Paused)),
            (Action::Pause, Paused, None),
            (Action::Pause, Stopped, None),
            (Action::Resume, Paused, Some(Running)),
            (Action::Resume, Running, None),
            (Action::Resume, Stopped, None),
        ];
        for (action, from, to) in expect {
            assert_eq!(action.target(from), to, "{action} / {from:?}");
        }
    }

    #[test]
    fn action_and_kind_parse_round_trip() {
        for a in Action::ALL {
            assert_eq!(a.as_str().parse::<Action>(), Ok(a));
        }
        assert!("destroy".parse::<Action>().is_err());
        assert_eq!("vm".parse::<GuestKind>(), Ok(GuestKind::Vm));
        assert_eq!("ct".parse::<GuestKind>(), Ok(GuestKind::Ct));
        assert!("VM".parse::<GuestKind>().is_err());
    }

    #[test]
    fn name_validation() {
        for ok in ["a", "web-prod-01", "k8s", "0db", &"a".repeat(63)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-web", "web-", "Web", "web_1", "web 1", "çay", &"a".repeat(64)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn iso_name_validation_blocks_path_tricks() {
        for ok in ["debian-12.8.0-amd64-netinst.iso", "a.ISO", "win_2022.iso"] {
            assert!(valid_iso_name(ok), "{ok}");
        }
        for bad in ["", ".iso", "..iso", "../etc/passwd.iso", "a/b.iso", "a\\b.iso", "disk.qcow2", "x.iso,readonly=off", "boşluk var.iso", "-snapshot.iso "] {
            assert!(!valid_iso_name(bad), "{bad}");
        }
    }

    #[test]
    fn storage_availability_and_usage() {
        let local = Storage {
            id: "local-zfs".into(),
            node: Some("n1".into()),
            kind: StorageKind::Zfs,
            content: vec![],
            used_gb: 30,
            total_gb: 120,
        };
        assert!(local.available_on("n1"));
        assert!(!local.available_on("n2"));
        assert_eq!(local.free_gb(), 90);
        assert_eq!(local.used_pct(), 25);
        let shared = Storage { node: None, ..local };
        assert!(shared.available_on("n2"));
    }

    #[test]
    fn storage_kind_serializes_with_dashes() {
        assert_eq!(serde_json::to_string(&StorageKind::LvmThin).unwrap(), "\"lvm-thin\"");
        assert_eq!(serde_json::to_string(&GuestKind::Ct).unwrap(), "\"ct\"");
    }
}
