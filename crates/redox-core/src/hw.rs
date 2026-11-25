//! Konuğun isteğe bağlı donanım ve ilk kurulum ayarları.
//!
//! Hem oluşturma isteğinde hem konuk kaydında aynı yapı kullanılır; alanların hepsi
//! varsayılanlıdır, böylece eski durum dosyaları ve kısa API istekleri geçerli kalır.

use serde::{Deserialize, Serialize};

use crate::model::{GuestKind, VmId};

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

pub const MAX_VLAN: u16 = 4094;
pub const MAX_SHARES: usize = 4;

/// Paylaşılan klasör. Ad hem sunucudaki dizinin (`shares/<ad>`) hem konuktaki bağlama etiketinin adıdır;
/// aynı adı kullanan konuklar aynı klasörü görür.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub name: String,
    /// Konuk yalnızca okuyabilir.
    #[serde(default)]
    pub readonly: bool,
}

impl Share {
    /// Ad: küçük harf, rakam ve tire; 1-24 karakter; tireyle başlamaz. Yol ayırıcı ve nokta içeremez.
    pub fn valid(&self) -> bool {
        let n = &self.name;
        !n.is_empty() && n.len() <= 24 && !n.starts_with('-') && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }
}

/// Form ve komut satırı biçimi: `ortak` ya da salt okunur için `veri:ro`.
impl std::fmt::Display for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.name, if self.readonly { ":ro" } else { "" })
    }
}

/// Virgülle ya da boşlukla ayrılmış paylaşım listesi: `ortak, veri:ro`.
pub fn parse_shares(text: &str) -> Result<Vec<Share>, String> {
    text.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (name, readonly) = match item.split_once(':') {
                Some((name, "ro")) => (name, true),
                Some((name, "rw")) => (name, false),
                Some(_) => return Err(format!("geçersiz paylaşım \"{item}\": ad ya da salt okunur için ad:ro yazın")),
                None => (item, false),
            };
            let share = Share { name: name.to_string(), readonly };
            if share.valid() { Ok(share) } else { Err(format!("geçersiz paylaşım adı \"{name}\": küçük harf, rakam ve tire; en çok 24 karakter")) }
        })
        .collect()
}

/// Kullanıcının yazdığı ağ adını düzenler: `NAT` her yazımıyla `nat` olur; köprü adlarına dokunulmaz
/// (Linux'ta arayüz adları büyük/küçük harfe duyarlıdır).
pub fn normalize_network(spec: &str) -> String {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case(crate::model::NAT) { crate::model::NAT.to_string() } else { spec.to_string() }
}

/// Ek kart tanımını köprü (ya da `nat`) ve VLAN etiketine ayırır: `vmbr1` → `("vmbr1", 0)`, `vmbr1:20` → `("vmbr1", 20)`.
/// Geçersiz tanımda `None`. Arayüz adlarında `:` bulunmadığı için ayrım belirsiz değildir.
pub fn parse_nic(spec: &str) -> Option<(&str, u16)> {
    match spec.split_once(':') {
        None => crate::model::valid_bridge(spec).then_some((spec, 0)),
        Some((bridge, tag)) => {
            let vlan: u16 = tag.parse().ok().filter(|v| (1..=MAX_VLAN).contains(v) && tag.bytes().all(|b| b.is_ascii_digit()))?;
            (crate::model::valid_bridge(bridge) && bridge != crate::model::NAT).then_some((bridge, vlan))
        }
    }
}

pub const MAX_DISK_MBPS: u32 = 10_000;
pub const MAX_DISK_IOPS: u32 = 1_000_000;
pub const MAX_NET_MBPS: u32 = 100_000;

impl Hardware {
    /// Ağ hız sınırı bu ağ ayarıyla uygulanabilir mi? Sınır, sunucudaki TAP arayüzünde uygulanır;
    /// NAT kartında böyle bir arayüz yoktur. `bridge`: birinci kartın ağı.
    pub fn net_limit_applies(&self, bridge: &str) -> bool {
        self.net_mbps == 0 || bridge != crate::model::NAT || self.nics.iter().any(|n| parse_nic(n).is_some_and(|(b, _)| b != crate::model::NAT))
    }
}

pub const MAX_BOOT_ORDER: u32 = 999;
pub const MAX_BOOT_DELAY: u32 = 600;
pub const MAX_SCHEDULES: usize = 8;
/// Saati kaçan zamanlanmış işlem bu kadar dakika içinde yine çalıştırılır; sonrası o gün atlanır.
pub const SCHEDULE_GRACE_MIN: u16 = 15;
pub const EVERY_DAY: u8 = 0b111_1111;
const DAY_NAMES: [&str; 7] = ["Pzt", "Sal", "Çar", "Per", "Cum", "Cmt", "Paz"];

fn every_day() -> u8 {
    EVERY_DAY
}

/// Zamanlanmış güç işlemi: belirli günlerde, belirli saatte konuğu başlatır, kapatır ya da yeniden başlatır.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub action: crate::model::Action,
    /// `SS:DD`, sunucu saatiyle.
    pub time: String,
    /// Günler: bit 0 Pazartesi … bit 6 Pazar.
    #[serde(default = "every_day")]
    pub days: u8,
    /// Son çalıştırıldığı gün (`YYYY-AA-GG`); aynı gün ikinci kez çalışmaz.
    #[serde(default)]
    pub last_run: Option<String>,
}

impl Schedule {
    pub fn valid(&self) -> bool {
        use crate::model::Action;
        matches!(self.action, Action::Start | Action::Stop | Action::Reboot)
            && crate::model::parse_time(&self.time).is_some()
            && self.days != 0
            && self.days <= EVERY_DAY
    }

    /// Bu gün (0 Pazartesi … 6 Pazar) çalışır mı?
    pub fn on_day(&self, weekday: u8) -> bool {
        weekday < 7 && self.days & (1 << weekday) != 0
    }

    /// Günlerin okunur hali: `her gün`, `hafta içi`, `Pzt, Çar`.
    pub fn days_label(&self) -> String {
        days_label(self.days)
    }

    /// Günlerin komut satırı biçimi: `1-5`, `67`, `135`.
    fn days_text(&self) -> String {
        (0..7u8).filter(|d| self.days & (1 << d) != 0).map(|d| (d + 1).to_string()).collect()
    }
}

/// Komut satırı ve form biçimi: `start@08:00` ya da günlerle `stop@20:00@12345`.
impl std::fmt::Display for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.action, self.time)?;
        if self.days != EVERY_DAY {
            write!(f, "@{}", self.days_text())?;
        }
        Ok(())
    }
}

/// Gün maskesinin okunur hali: `her gün`, `hafta içi`, `Pzt, Çar`.
pub fn days_label(days: u8) -> String {
    match days {
        EVERY_DAY => "her gün".to_string(),
        0b001_1111 => "hafta içi".to_string(),
        0b110_0000 => "hafta sonu".to_string(),
        days => (0..7).filter(|d| days & (1 << d) != 0).map(|d| DAY_NAMES[d as usize]).collect::<Vec<_>>().join(", "),
    }
}

/// Gün listesi: `1`-`7` rakamları (1 Pazartesi) ve `1-5` gibi aralıklar; `135`, `1-5`, `1-57`.
pub fn parse_days(text: &str) -> Option<u8> {
    let bytes = text.as_bytes();
    let digit = |b: u8| (b'1'..=b'7').contains(&b).then(|| b - b'1');
    let (mut days, mut i) = (0u8, 0);
    while i < bytes.len() {
        let from = digit(bytes[i])?;
        if bytes.get(i + 1) == Some(&b'-') {
            let to = digit(*bytes.get(i + 2)?)?;
            if to < from {
                return None;
            }
            (from..=to).for_each(|d| days |= 1 << d);
            i += 3;
        } else {
            days |= 1 << from;
            i += 1;
        }
    }
    (days != 0).then_some(days)
}

/// Virgülle ya da satırla ayrılmış zamanlama listesi: `start@08:00@1-5, stop@20:00`.
pub fn parse_schedules(text: &str) -> Result<Vec<Schedule>, String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let bad = || format!("geçersiz zamanlama \"{item}\"; eylem@SS:DD ya da eylem@SS:DD@günler biçiminde yazın, örn. start@08:00@1-5");
            let mut parts = item.split('@');
            let action: crate::model::Action = parts.next().unwrap_or("").trim().parse().map_err(|_| bad())?;
            let time = parts.next().ok_or_else(bad)?.trim().to_string();
            let days = match parts.next() {
                Some(d) => parse_days(d.trim()).ok_or_else(bad)?,
                None => EVERY_DAY,
            };
            let schedule = Schedule { action, time, days, last_run: None };
            if parts.next().is_some() || !schedule.valid() {
                return Err(bad());
            }
            Ok(schedule)
        })
        .collect()
}

pub const MAX_PASSTHROUGH: usize = 4;

fn hex_fields(text: &str, separators: &[char], widths: &[usize]) -> bool {
    let parts: Vec<&str> = text.split(separators).collect();
    parts.len() == widths.len()
        && text.chars().filter(|c| separators.contains(c)).count() == widths.len() - 1
        && parts.iter().zip(widths).all(|(p, w)| p.len() == *w && p.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// USB aygıt kimliği: `046d:c52b`.
pub fn valid_usb_id(id: &str) -> bool {
    hex_fields(id, &[':'], &[4, 4])
}

/// PCI adresi: `0000:01:00.0` (alan:veri yolu:aygıt.işlev).
pub fn valid_pci_addr(addr: &str) -> bool {
    match addr.rsplit_once('.') {
        Some((rest, function)) => hex_fields(rest, &[':'], &[4, 2, 2]) && matches!(function, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7"),
        None => false,
    }
}

/// Ek veri diski. Numara kalıcıdır: aradaki bir disk silinse de diğerlerinin dosya adı değişmez.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraDisk {
    pub id: u8,
    pub size_gb: u64,
    /// Diskin kimliği: konuğa eklenen her diske artan bir sıra numarası verilir. Aynı numaraya (`id`)
    /// sonradan takılan disk başka bir disktir. (Eski kayıtlarda 0.)
    #[serde(default)]
    pub serial: u64,
}

pub const MAX_EXTRA_DISKS: usize = 8;
pub const MAX_EXTRA_NICS: usize = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Firmware {
    #[default]
    Bios,
    Uefi,
}

/// Formlardaki tek seçim kutusunun değeri: `bios`, `uefi` ya da `uefi-secure`.
pub fn parse_firmware_choice(choice: &str) -> Option<(Firmware, bool)> {
    match choice {
        "bios" => Some((Firmware::Bios, false)),
        "uefi" => Some((Firmware::Uefi, false)),
        "uefi-secure" => Some((Firmware::Uefi, true)),
        _ => None,
    }
}

/// Güvenlik duvarında izin verilen gelen trafik türü.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FwProto {
    Tcp,
    Udp,
    Icmp,
}

impl FwProto {
    pub fn as_str(self) -> &'static str {
        match self {
            FwProto::Tcp => "tcp",
            FwProto::Udp => "udp",
            FwProto::Icmp => "icmp",
        }
    }
}

/// Konuğa gelen trafiğe izin veren kural. Güvenlik duvarı açıkken kurallara uymayan gelen
/// bağlantılar düşürülür; konuğun başlattığı bağlantılar ve yanıtları serbesttir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FwRule {
    pub proto: FwProto,
    /// Kapı aralığı (dahil); `None` ise bütün kapılar. ICMP'de kullanılmaz.
    #[serde(default)]
    pub ports: Option<(u16, u16)>,
    /// Yalnızca bu IPv4 adresinden ya da ağından (`10.0.0.0/8`); `None` ise her yerden.
    #[serde(default)]
    pub source: Option<String>,
}

pub const MAX_FW_RULES: usize = 32;

impl std::fmt::Display for FwRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.proto.as_str())?;
        match self.ports {
            Some((a, b)) if a == b => write!(f, "/{a}")?,
            Some((a, b)) => write!(f, "/{a}-{b}")?,
            None => {}
        }
        match &self.source {
            Some(source) => write!(f, " from {source}"),
            None => Ok(()),
        }
    }
}

/// IPv4 adresi ya da ağı: `192.168.1.10`, `10.0.0.0/8`.
pub fn valid_cidr(text: &str) -> bool {
    let (addr, prefix) = text.split_once('/').unwrap_or((text, "32"));
    let octets: Vec<&str> = addr.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|o| !o.is_empty() && o.len() <= 3 && o.bytes().all(|b| b.is_ascii_digit()) && o.parse::<u16>().is_ok_and(|v| v <= 255))
        && !prefix.is_empty()
        && prefix.len() <= 2
        && prefix.bytes().all(|b| b.is_ascii_digit())
        && prefix.parse::<u8>().is_ok_and(|p| p <= 32)
}

impl FwRule {
    pub fn valid(&self) -> bool {
        let ports_ok = match (self.proto, self.ports) {
            (FwProto::Icmp, ports) => ports.is_none(),
            (_, Some((a, b))) => a >= 1 && a <= b,
            (_, None) => true,
        };
        ports_ok && self.source.as_deref().is_none_or(valid_cidr)
    }
}

/// "tcp/22, tcp/80-443 from 10.0.0.0/8, icmp" → kural listesi. Kurallar virgül ya da satırla ayrılır.
pub fn parse_fw_rules(text: &str) -> Result<Vec<FwRule>, String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let bad = || format!("geçersiz güvenlik duvarı kuralı \"{item}\"; örnekler: tcp/22, udp/53, tcp/8000-8100 from 10.0.0.0/8, icmp");
            let (what, source) = match item.split_once(" from ") {
                Some((what, source)) => (what.trim(), Some(source.trim().to_string())),
                None => (item, None),
            };
            let (proto, ports) = what.split_once('/').map_or((what, None), |(p, range)| (p, Some(range)));
            let proto = match proto {
                "tcp" => FwProto::Tcp,
                "udp" => FwProto::Udp,
                "icmp" => FwProto::Icmp,
                _ => return Err(bad()),
            };
            let ports = match ports {
                None => None,
                Some(range) => {
                    let (a, b) = range.split_once('-').unwrap_or((range, range));
                    Some((a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?))
                }
            };
            let rule = FwRule { proto, ports, source };
            if rule.valid() { Ok(rule) } else { Err(bad()) }
        })
        .collect()
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

/// Seçilebilen işlemci modelleri (QEMU adlarıyla), eskiden yeniye. Boş değer sunucunun kendi işlemcisidir.
pub const CPU_TYPES: [&str; 17] = [
    "qemu64", "kvm64", "Nehalem", "Westmere", "SandyBridge", "IvyBridge", "Haswell", "Broadwell", "Skylake-Client", "Skylake-Server",
    "Cascadelake-Server", "Icelake-Server", "EPYC", "EPYC-Rome", "EPYC-Milan", "EPYC-Genoa", "max",
];

/// Modelin üreticisi: `intel`, `amd` ya da üreticiden bağımsız modellerde (`qemu64`, `kvm64`, `max`, boş) `None`.
pub fn cpu_vendor_of(model: &str) -> Option<&'static str> {
    match model {
        "" | "qemu64" | "kvm64" | "max" => None,
        m if m.starts_with("EPYC") => Some("amd"),
        m if CPU_TYPES.contains(&m) => Some("intel"),
        _ => None,
    }
}

/// `/proc/cpuinfo` içeriğinden sunucunun işlemci üreticisi.
pub fn parse_cpu_vendor(cpuinfo: &str) -> Option<&'static str> {
    let vendor = cpuinfo.lines().find_map(|l| l.strip_prefix("vendor_id")).and_then(|v| v.split_once(':')).map(|(_, v)| v.trim())?;
    match vendor {
        "GenuineIntel" => Some("intel"),
        "AuthenticAMD" | "HygonGenuine" => Some("amd"),
        _ => None,
    }
}

/// QEMU'nun `-cpu` değeri. `model` çözülmüş addır (`host`, `max` ya da bir model). İç içe sanallaştırmada
/// adı belli modellere sanallaştırma uzantısı eklenir; `host` ve `max` onu zaten taşır.
pub fn cpu_arg(model: &str, nested: bool) -> String {
    match cpu_vendor_of(model).filter(|_| nested) {
        Some("amd") => format!("{model},+svm"),
        Some(_) => format!("{model},+vmx"),
        None => model.to_string(),
    }
}

/// Seçilen model bu sunucunun işlemcisinde çalıştırılabilir mi? Üretici bilinmiyorsa engellenmez.
pub fn check_cpu_vendor(model: &str, host: Option<&str>) -> Result<(), String> {
    match (cpu_vendor_of(model), host) {
        (Some(wanted), Some(have)) if wanted != have => Err(format!(
            "{model} bir {} modeli, bu sunucunun işlemcisi {}: konuk eksik özelliklerle açılır ya da hiç açılmaz. Sunucunun kendi üreticisinden bir model ya da host seçin",
            vendor_label(wanted),
            vendor_label(have)
        )),
        _ => Ok(()),
    }
}

fn vendor_label(vendor: &str) -> &'static str {
    if vendor == "amd" { "AMD" } else { "Intel" }
}

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

pub fn valid_cpu_type(name: &str) -> bool {
    name.is_empty() || CPU_TYPES.contains(&name)
}

/// Balonla inilebilecek en küçük bellek (MB).
pub const MIN_BALLOON_MB: u64 = 256;
/// Yedek yaşı hedefinin üst sınırı (saat): bir yıl.
pub const MAX_RPO_HOURS: u32 = 8760;

/// Dinamik bellekte konuğun kullandığının üstüne bırakılan pay (yüzde) ve bu payın alt sınırı (MB).
pub const DYNAMIC_BUFFER_PCT: u64 = 20;
pub const DYNAMIC_BUFFER_MIN_MB: u64 = 256;
/// Hedefler bu adıma yuvarlanır; daha küçük değişiklikler uygulanmaz.
pub const DYNAMIC_STEP_MB: u64 = 64;

/// Dinamik bellek için yeni balon hedefi (MB); değişiklik gerekmiyorsa `None`.
///
/// Hedef, konuğun kullandığı bellek + paydır; `min_mb` ile `total_mb` arasında tutulur. Büyütme hemen yapılır
/// (konuk bellek darlığı çekmesin), küçültme ise yalnızca fark belirginse: hedef sürekli oynamasın.
/// `current_mb` kayıtlı hedeftir (0: belleğin tamamı).
pub fn dynamic_memory_target(total_mb: u64, min_mb: u64, current_mb: u64, stats: crate::driver::BalloonStats) -> Option<u64> {
    if min_mb == 0 || min_mb > total_mb || stats.total_mb == 0 {
        return None;
    }
    let current = if current_mb == 0 { total_mb } else { current_mb.min(total_mb) };
    let used = stats.total_mb.saturating_sub(stats.available_mb);
    let wanted = used + (used * DYNAMIC_BUFFER_PCT / 100).max(DYNAMIC_BUFFER_MIN_MB);
    let wanted = wanted.div_ceil(DYNAMIC_STEP_MB) * DYNAMIC_STEP_MB;
    let target = wanted.clamp(min_mb, total_mb);
    if target > current {
        return Some(target);
    }
    let slack = (total_mb / 20).max(2 * DYNAMIC_STEP_MB);
    (current - target >= slack).then_some(target)
}

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
        if let Some(bad) = self.pci.iter().find(|p| !valid_pci_addr(p)) {
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
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGxvcmVtaXBzdW1kb2xvcnNpdGFtZXRjb25zZWN0 ben@dizustu";

    fn hw(image: Option<&str>, user: Option<&str>, key: Option<&str>) -> Hardware {
        Hardware { image: image.map(Into::into), ci_user: user.map(Into::into), ci_ssh_key: key.map(Into::into), ..Default::default() }
    }

    #[test]
    fn names_and_keys_are_validated() {
        for ok in ["debian-12-genericcloud-amd64.qcow2", "noble-server-cloudimg-amd64.img", "a.raw"] {
            assert!(valid_image_name(ok), "{ok}");
        }
        for ok in ["eski-sunucu.vmdk", "win.vhdx", "kutu.vdi", "a.ova"] {
            assert!(valid_image_name(ok), "{ok}");
        }
        for bad in ["", ".qcow2", ".gizli.qcow2", "../a.qcow2", "a/b.img", "a.iso", "a b.img", "a.qcow2;rm", "a.ovf", ".ova"] {
            assert!(!valid_image_name(bad), "{bad}");
        }
        for ok in ["redox", "_svc", "ali-2", "a"] {
            assert!(valid_ci_user(ok), "{ok}");
        }
        for bad in ["", "root", "Ali", "2ali", "a b", "a:b", &"a".repeat(33)] {
            assert!(!valid_ci_user(bad), "{bad}");
        }
        assert!(valid_ssh_key(KEY));
        assert!(valid_ssh_key(KEY.rsplit_once(' ').unwrap().0), "açıklama isteğe bağlı");
        for bad in [
            "",
            "ssh-ed25519",
            "ssh-dss AAAAB3NzaC1kc3MAAACBAJ0000000000000000000000000000 eski",
            "ssh-ed25519 kısa x",
            &format!("{KEY}\nssh-ed25519 AAAA ikinci"),
            &format!("{KEY} \"tırnak"),
            &format!("{KEY} ters\\bolu"),
            "-----BEGIN OPENSSH PRIVATE KEY-----",
        ] {
            assert!(!valid_ssh_key(bad), "{bad}");
        }
    }

    #[test]
    fn hardware_rules_depend_on_kind_and_each_other() {
        assert!(Hardware::default().validate(GuestKind::Ct).is_ok());
        assert!(hw(Some("d.qcow2"), Some("ali"), Some(KEY)).validate(GuestKind::Vm).is_ok());
        assert!(hw(None, None, Some(KEY)).validate(GuestKind::Vm).is_ok(), "ISO kurulumunda da kullanılabilir");
        for (h, kind, needle) in [
            (hw(None, None, Some(KEY)), GuestKind::Ct, "yalnızca sanal makinelerde"),
            (hw(None, Some("ali"), None), GuestKind::Vm, "SSH açık anahtarı gerekli"),
            (hw(Some("../d.qcow2"), None, Some(KEY)), GuestKind::Vm, "geçersiz bulut imajı"),
            (hw(None, Some("root"), Some(KEY)), GuestKind::Vm, "geçersiz cloud-init kullanıcı"),
            (hw(None, None, Some("parola123")), GuestKind::Vm, "geçersiz SSH açık anahtarı"),
        ] {
            let err = h.validate(kind).unwrap_err();
            assert!(err.contains(needle), "{err}");
        }
    }

    #[test]
    fn renders_nocloud_files() {
        assert_eq!(Hardware::default().user_data("web"), None);
        let data = hw(None, Some("ali"), Some(KEY)).user_data("web-01").unwrap();
        assert!(data.starts_with("#cloud-config\nhostname: web-01\n"), "{data}");
        assert!(data.contains("  - name: ali\n") && data.contains(&format!("      - \"{KEY}\"\n")) && data.contains("ssh_pwauth: false"));
        assert!(hw(None, None, Some(KEY)).user_data("x").unwrap().contains("  - name: redox\n"));
        // Doğrulamadan kaçmış bozuk anahtar YAML'a yazılmaz.
        assert_eq!(hw(None, None, Some("ssh-ed25519 AAAA\"\nruncmd: [reboot]")).user_data("x"), None);
        assert_eq!(meta_data(103, "web-01", 0), "instance-id: redox-103\nlocal-hostname: web-01\n");
        assert_eq!(meta_data(103, "web-01", 2), "instance-id: redox-103-2\nlocal-hostname: web-01\n");
    }

    #[test]
    fn backup_hooks_are_validated_and_split() {
        for ok in ["", "/usr/local/bin/once.sh", "/usr/bin/pg_ctl -D /var/lib/pg checkpoint", "C:\\betik\\once.cmd /sessiz"] {
            assert!(valid_hook(ok), "{ok}");
        }
        for bad in ["once.sh", "sh -c reboot", "/bin/x\nrm -rf /", "; reboot"] {
            assert!(!valid_hook(bad), "{bad:?}");
        }
        assert!(!valid_hook(&format!("/{}", "a".repeat(300))));
        assert_eq!(split_hook("/usr/bin/pg_ctl  -D /veri checkpoint"), Some(("/usr/bin/pg_ctl".to_string(), vec!["-D".to_string(), "/veri".to_string(), "checkpoint".to_string()])));
        assert_eq!(split_hook("   "), None);
        let with = Hardware { backup_pre: "/usr/local/bin/once.sh".into(), ..Default::default() };
        with.validate(GuestKind::Vm).unwrap();
        assert!(with.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        assert!(Hardware { backup_post: "sonra.sh".into(), ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("mutlak bir program yoluyla"));
    }

    #[test]
    fn nested_virtualization_adds_the_vendor_extension() {
        assert_eq!(cpu_arg("host", true), "host");
        assert_eq!(cpu_arg("max", true), "max");
        assert_eq!(cpu_arg("Haswell", true), "Haswell,+vmx");
        assert_eq!(cpu_arg("EPYC-Milan", true), "EPYC-Milan,+svm");
        assert_eq!(cpu_arg("Haswell", false), "Haswell");
        let hw = |cpu: &str| Hardware { cpu_type: cpu.into(), nested: true, ..Default::default() };
        hw("").validate(GuestKind::Vm).unwrap();
        hw("EPYC").validate(GuestKind::Vm).unwrap();
        assert!(hw("kvm64").validate(GuestKind::Vm).unwrap_err().contains("kvm64 modeliyle kullanılamaz"));
        assert!(hw("").validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
    }

    #[test]
    fn dynamic_memory_follows_guest_usage_within_bounds() {
        use crate::driver::BalloonStats;
        let stats = |total_mb, available_mb| BalloonStats { total_mb, available_mb };
        // 8 GB'lık konuk 1 GB kullanıyor: hedef kullanım + pay (en az 256 MB), 64'e yuvarlanır.
        assert_eq!(dynamic_memory_target(8192, 1024, 0, stats(8192, 7168)), Some(1280));
        // Alt sınırın altına inilmez.
        assert_eq!(dynamic_memory_target(8192, 2048, 0, stats(8192, 7168)), Some(2048));
        // Kullanım artınca hedef hemen büyür (pay %20: 3000 + 600 = 3600 → 3648).
        assert_eq!(dynamic_memory_target(8192, 1024, 1280, stats(1280, 0)), Some(1536));
        assert_eq!(dynamic_memory_target(8192, 1024, 3072, stats(3072, 72)), Some(3648));
        // Belleğin tamamı aşılmaz.
        assert_eq!(dynamic_memory_target(4096, 1024, 2048, stats(4096, 96)), Some(4096));
        // Küçük farklar için küçültme yapılmaz; hedef zaten yerindeyse değişmez.
        assert_eq!(dynamic_memory_target(8192, 1024, 1536, stats(1536, 400)), None);
        assert_eq!(dynamic_memory_target(8192, 1024, 1280, stats(1280, 256)), None);
        // Kapalıyken, tutarsız sınırla ya da ölçüm yokken dokunulmaz.
        assert_eq!(dynamic_memory_target(8192, 0, 0, stats(8192, 7168)), None);
        assert_eq!(dynamic_memory_target(1024, 2048, 0, stats(1024, 900)), None);
        assert_eq!(dynamic_memory_target(8192, 1024, 0, stats(0, 0)), None);
    }

    #[test]
    fn cpu_type_and_balloon_are_validated() {
        let ok = Hardware { cpu_type: "Haswell".into(), balloon: true, ..Default::default() };
        ok.validate(GuestKind::Vm).unwrap();
        assert!(ok.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        Hardware::default().validate(GuestKind::Ct).unwrap();
        for bad in ["haswell", "Pentium9", "host,+vmx", "Haswell -device x"] {
            assert!(Hardware { cpu_type: bad.into(), ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("bilinmeyen işlemci türü"), "{bad}");
        }
        assert!(valid_cpu_type("") && valid_cpu_type("max") && !valid_cpu_type("host"), "host boş değerle seçilir");
        // Üretici: başka üreticinin modeli reddedilir; bağımsız modeller ve bilinmeyen sunucu engellenmez.
        assert_eq!((cpu_vendor_of("Haswell"), cpu_vendor_of("EPYC-Milan"), cpu_vendor_of("kvm64"), cpu_vendor_of("")), (Some("intel"), Some("amd"), None, None));
        assert_eq!(parse_cpu_vendor("processor\t: 0\nvendor_id\t: GenuineIntel\ncpu family\t: 6\n"), Some("intel"));
        assert_eq!((parse_cpu_vendor("vendor_id\t: AuthenticAMD\n"), parse_cpu_vendor("vendor_id\t: Baska\n"), parse_cpu_vendor("")), (Some("amd"), None, None));
        assert!(check_cpu_vendor("EPYC", Some("intel")).unwrap_err().contains("bir AMD modeli, bu sunucunun işlemcisi Intel"));
        assert!(check_cpu_vendor("Haswell", Some("amd")).is_err());
        for (model, host) in [("Haswell", Some("intel")), ("EPYC", Some("amd")), ("max", Some("intel")), ("", Some("amd")), ("EPYC", None)] {
            check_cpu_vendor(model, host).unwrap();
        }
    }

    #[test]
    fn cloud_init_settings_are_edited_validated_and_versioned() {
        let mut h = Hardware::default();
        assert!(!h.apply_cloud_init(Some(""), Some(""), Some(""), Some(""), Some("")), "boş alanlar değişiklik sayılmaz");
        assert_eq!(h.ci_gen, 0);
        assert!(h.apply_cloud_init(Some("ali"), Some(&format!("  {KEY}\n")), Some("10.0.0.5/24"), Some("10.0.0.1"), Some("1.1.1.1, 9.9.9.9")));
        assert_eq!((h.ci_user.as_deref(), h.ci_ssh_key.as_deref(), h.ci_ip.as_deref(), h.ci_gw.as_deref(), h.ci_dns.len(), h.ci_gen), (Some("ali"), Some(KEY), Some("10.0.0.5/24"), Some("10.0.0.1"), 2, 1));
        h.validate(GuestKind::Vm).unwrap();
        assert!(!h.apply_cloud_init(Some("ali"), None, Some("10.0.0.5/24"), None, None), "aynı değerler sürümü artırmaz");
        assert!(h.apply_cloud_init(None, None, Some("10.0.0.6/24"), None, None));
        assert_eq!(h.ci_gen, 2);
        assert_eq!(
            h.network_config(103).unwrap(),
            "version: 2\nethernets:\n  nic0:\n    match:\n      macaddress: \"52:54:00:00:00:67\"\n    dhcp4: false\n    addresses:\n      - 10.0.0.6/24\n    gateway4: 10.0.0.1\n    nameservers:\n      addresses: [1.1.1.1, 9.9.9.9]\n"
        );
        // Geçersiz değerler.
        for (ip, gw, dns, want) in [
            ("10.0.0.5", "", "", "geçersiz cloud-init adresi"),
            ("10.0.0.5/33", "", "", "geçersiz cloud-init adresi"),
            ("10.0.0.5/24\n    x: y", "", "", "geçersiz cloud-init adresi"),
            ("10.0.0.5/24", "10.0.0", "", "geçersiz ağ geçidi"),
            ("10.0.0.5/24", "", "1.1.1.1, dns.example", "geçersiz ad sunucusu"),
            ("10.0.0.5/24", "", "1.1.1.1 2.2.2.2 3.3.3.3 4.4.4.4", "en fazla 3"),
            ("", "10.0.0.1", "", "yalnızca sabit adresle"),
        ] {
            let mut bad = h.clone();
            bad.apply_cloud_init(None, None, Some(ip), Some(gw), Some(dns));
            assert!(bad.validate(GuestKind::Vm).unwrap_err().contains(want), "{ip} {gw} {dns}");
        }
        let mut no_key = Hardware::default();
        no_key.apply_cloud_init(None, None, Some("10.0.0.5/24"), None, None);
        assert!(no_key.validate(GuestKind::Vm).unwrap_err().contains("SSH açık anahtarı gerekli"));
        assert!(no_key.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        // Adres boşaltılınca DHCP'ye dönülür; anahtar boşaltılınca cloud-init tümüyle kapanır.
        let mut dhcp = h.clone();
        dhcp.apply_cloud_init(None, None, Some(""), Some(""), Some(""));
        assert_eq!((dhcp.network_config(103), dhcp.cloud_init()), (None, true));
        assert!(h.apply_cloud_init(None, Some(""), None, None, None));
        assert_eq!((h.cloud_init(), h.ci_user.clone(), h.ci_ip.clone(), h.ci_dns.len(), h.ci_gen), (false, None, None, 0, 3));
        assert_eq!(h.user_data("x"), None);
    }

    #[test]
    fn firewall_rules_parse_print_and_validate() {
        let rules = parse_fw_rules("tcp/22, tcp/80-443 from 10.0.0.0/8\nudp/53 from 192.168.1.10 ,icmp,, udp").unwrap();
        let text: Vec<String> = rules.iter().map(ToString::to_string).collect();
        assert_eq!(text, ["tcp/22", "tcp/80-443 from 10.0.0.0/8", "udp/53 from 192.168.1.10", "icmp", "udp"]);
        assert_eq!(parse_fw_rules(&text.join(", ")).unwrap(), rules, "yazdırılan biçim yeniden okunur");
        assert!(parse_fw_rules("  \n ").unwrap().is_empty());
        for bad in ["sctp/22", "tcp/0", "tcp/443-80", "tcp/70000", "tcp/22 from evim", "tcp/22 from 10.0.0.0/33", "tcp/22 from 300.1.1.1",
            "icmp/8", "tcp/22; drop", "tcp/22 from 1.2.3.4 }", "tcp/a-b"] {
            assert!(parse_fw_rules(bad).unwrap_err().contains("geçersiz güvenlik duvarı kuralı"), "{bad}");
        }
        for ok in ["0.0.0.0/0", "10.1.2.3", "192.168.0.0/16"] {
            assert!(valid_cidr(ok), "{ok}");
        }
        for bad in ["", "1.2.3", "1.2.3.4.5", "1.2.3.4/", "1.2.3.4/x", "a.b.c.d", "1.2.3.4 ", "::1"] {
            assert!(!valid_cidr(bad), "{bad}");
        }

        // Güvenlik duvarı konteynerlerde de geçerlidir; doğrulamadan kaçan bozuk kural yakalanır.
        let fw = Hardware { firewall: true, fw_rules: rules, ..Default::default() };
        assert!(fw.validate(GuestKind::Ct).is_ok() && fw.validate(GuestKind::Vm).is_ok());
        let forged = Hardware { fw_rules: vec![FwRule { proto: FwProto::Tcp, ports: Some((22, 22)), source: Some("1.2.3.4 } ; flush ruleset".into()) }], ..Default::default() };
        assert!(forged.validate(GuestKind::Vm).unwrap_err().contains("geçersiz güvenlik duvarı kuralı"));
        let many = Hardware { fw_rules: vec![FwRule { proto: FwProto::Icmp, ports: None, source: None }; MAX_FW_RULES + 1], ..Default::default() };
        assert!(many.validate(GuestKind::Vm).unwrap_err().contains("en fazla"));
    }

    #[test]
    fn firmware_rules() {
        let uefi = Hardware { firmware: Firmware::Uefi, secure_boot: true, tpm: true, ..Default::default() };
        assert!(uefi.validate(GuestKind::Vm).is_ok());
        assert_eq!((uefi.firmware_choice(), uefi.firmware_label()), ("uefi-secure", "UEFI + güvenli önyükleme"));
        assert!(uefi.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        let tpm_only = Hardware { tpm: true, ..Default::default() };
        assert!(tpm_only.validate(GuestKind::Vm).is_ok(), "TPM BIOS ile de kullanılabilir");
        assert!(tpm_only.validate(GuestKind::Ct).is_err());
        let bad = Hardware { secure_boot: true, ..Default::default() };
        assert!(bad.validate(GuestKind::Vm).unwrap_err().contains("UEFI donanım yazılımı gerektirir"));
        for choice in ["bios", "uefi", "uefi-secure"] {
            let (firmware, secure_boot) = parse_firmware_choice(choice).unwrap();
            assert_eq!(Hardware { firmware, secure_boot, ..Default::default() }.firmware_choice(), choice);
        }
        assert_eq!(parse_firmware_choice("coreboot"), None);
        assert_eq!(serde_json::to_value(Firmware::Uefi).unwrap(), "uefi");
    }

    #[test]
    fn extra_disks_and_nics_are_bounded() {
        let d = |id, size_gb| ExtraDisk { id, size_gb, serial: 0 };
        let ok = Hardware { disks: vec![d(1, 10), d(3, 500)], nics: vec!["vmbr1".into(), "nat".into()], ..Default::default() };
        assert!(ok.validate(GuestKind::Vm).is_ok());
        assert!(ok.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelere"));
        assert!(Hardware { nics: vec!["vmbr1".into()], ..Default::default() }.validate(GuestKind::Ct).is_ok(), "konteynere ek kart takılabilir");
        for disks in [vec![d(1, 10), d(1, 20)], vec![d(0, 10)], vec![d(9, 10)], vec![d(1, 0)]] {
            assert!(Hardware { disks, ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("geçersiz ek disk"));
        }
        assert!(Hardware { nics: vec!["nat".into(); 4], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("en fazla 3"));
        assert!(Hardware { nics: vec!["br0,script=/x".into()], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("geçersiz ağ"));
    }

    #[test]
    fn passthrough_ids_are_strict() {
        for ok in ["046d:c52b", "0000:ffff"] {
            assert!(valid_usb_id(ok), "{ok}");
        }
        for bad in ["", "046d", "046D:C52B", "46d:c52b", "046d:c52b:1", "046d-c52b", "046g:c52b", "046d:c52b,hostbus=1"] {
            assert!(!valid_usb_id(bad), "{bad}");
        }
        for ok in ["0000:01:00.0", "0000:3b:1f.7"] {
            assert!(valid_pci_addr(ok), "{ok}");
        }
        for bad in ["", "01:00.0", "0000:01:00", "0000:01:00.8", "0000:01:00.0,x-vga=on", "0000:1:00.0", "0000.01.00.0"] {
            assert!(!valid_pci_addr(bad), "{bad}");
        }
        let hw = Hardware { usb: vec!["046d:c52b".into()], pci: vec!["0000:01:00.0".into()], ..Default::default() };
        assert!(hw.validate(GuestKind::Vm).is_ok());
        assert!(hw.validate(GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        assert!(Hardware { usb: vec!["046d:c52b".into(); 5], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("en fazla 4"));
        assert!(Hardware { pci: vec!["gpu".into()], ..Default::default() }.validate(GuestKind::Vm).unwrap_err().contains("geçersiz PCI adresi"));
        // Bulut imajı artık anahtarsız da seçilebilir: başka sanallaştırıcıdan aktarılan diskte cloud-init olmayabilir.
        assert!(Hardware { image: Some("eski.vmdk".into()), ..Default::default() }.validate(GuestKind::Vm).is_ok());
    }

    #[test]
    fn old_records_without_hardware_still_load() {
        let h: Hardware = serde_json::from_str("{}").unwrap();
        assert_eq!(h, Hardware::default());
    }

    #[test]
    fn schedules_parse_format_and_validate() {
        use crate::model::Action;
        let list = parse_schedules("start@08:00@1-5, stop@20:00\nreboot@03:15@67").unwrap();
        assert_eq!(list.iter().map(|s| (s.action, s.time.as_str(), s.days)).collect::<Vec<_>>(), [(Action::Start, "08:00", 0b001_1111), (Action::Stop, "20:00", EVERY_DAY), (Action::Reboot, "03:15", 0b110_0000)]);
        assert_eq!(list.iter().map(ToString::to_string).collect::<Vec<_>>(), ["start@08:00@12345", "stop@20:00", "reboot@03:15@67"]);
        assert_eq!(list.iter().map(Schedule::days_label).collect::<Vec<_>>(), ["hafta içi", "her gün", "hafta sonu"]);
        assert_eq!(parse_schedules("start@08:00@135").unwrap()[0].days_label(), "Pzt, Çar, Cum");
        assert!(list[0].on_day(0) && list[0].on_day(4) && !list[0].on_day(5) && !list[0].on_day(9));
        assert_eq!(parse_schedules(" , ").unwrap(), []);
        // Biçimi kendi çıktısından geri okunur.
        assert_eq!(parse_schedules(&list.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).unwrap(), list);
        for bad in ["start", "start@8:00", "start@25:00", "pause@08:00", "uç@08:00", "start@08:00@", "start@08:00@8", "start@08:00@5-1", "start@08:00@1-5@x", "start@08:00@0"] {
            assert!(parse_schedules(bad).is_err(), "{bad}");
        }
        assert_eq!((parse_days("1-57"), parse_days("7"), parse_days("1-"), parse_days("")), (Some(0b101_1111), Some(0b100_0000), None, None));

        let hw = |f: &dyn Fn(&mut Hardware)| {
            let mut hw = Hardware::default();
            f(&mut hw);
            hw.validate(GuestKind::Ct)
        };
        assert!(hw(&|h| h.schedules = list.clone()).is_ok(), "konteynerde de zamanlanır");
        assert!(hw(&|h| h.schedules = vec![list[0].clone(); MAX_SCHEDULES + 1]).unwrap_err().contains("en fazla"));
        assert!(hw(&|h| h.schedules = vec![Schedule { action: Action::Pause, time: "08:00".into(), days: EVERY_DAY, last_run: None }]).is_err());
        assert!(hw(&|h| h.schedules = vec![Schedule { action: Action::Stop, time: "08:00".into(), days: 0, last_run: None }]).is_err());
        assert!(hw(&|h| h.boot_order = MAX_BOOT_ORDER + 1).is_err() && hw(&|h| h.boot_delay = MAX_BOOT_DELAY + 1).is_err());
        assert!(hw(&|h| (h.boot_order, h.boot_delay) = (MAX_BOOT_ORDER, MAX_BOOT_DELAY)).is_ok());
    }

    #[test]
    fn rate_limits_are_bounded_and_vm_only() {
        let hw = |f: &dyn Fn(&mut Hardware), kind| {
            let mut hw = Hardware::default();
            f(&mut hw);
            hw.validate(kind)
        };
        assert!(hw(&|h| (h.disk_mbps, h.disk_iops, h.net_mbps) = (MAX_DISK_MBPS, MAX_DISK_IOPS, MAX_NET_MBPS), GuestKind::Vm).is_ok());
        assert!(hw(&|h| h.disk_mbps = MAX_DISK_MBPS + 1, GuestKind::Vm).is_err());
        assert!(hw(&|h| h.disk_iops = MAX_DISK_IOPS + 1, GuestKind::Vm).is_err());
        assert!(hw(&|h| h.net_mbps = MAX_NET_MBPS + 1, GuestKind::Vm).is_err());
        for set in [(|h: &mut Hardware| h.disk_mbps = 1) as fn(&mut Hardware), |h| h.disk_iops = 1, |h| h.net_mbps = 1] {
            assert!(hw(&set, GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        }
        let mut limited = Hardware { net_mbps: 100, ..Default::default() };
        assert!(limited.net_limit_applies("vmbr0") && !limited.net_limit_applies("nat"));
        limited.nics = vec!["nat".into(), "vmbr1".into()];
        assert!(limited.net_limit_applies("nat"), "köprüdeki ek kart yeter");
        assert!(Hardware::default().net_limit_applies("nat"), "sınır yoksa koşul da yok");
    }

    #[test]
    fn nic_specs_carry_an_optional_vlan_tag() {
        assert_eq!(parse_nic("vmbr1"), Some(("vmbr1", 0)));
        assert_eq!([normalize_network(" NAT "), normalize_network("Nat"), normalize_network("vmbrT"), normalize_network("vmbrT:20")], ["nat", "nat", "vmbrT", "vmbrT:20"]);
        assert_eq!(parse_nic("nat"), Some(("nat", 0)));
        assert_eq!(parse_nic("vmbr1:20"), Some(("vmbr1", 20)));
        assert_eq!(parse_nic("br.lan:4094"), Some(("br.lan", 4094)));
        for bad in ["", "vmbr1:", "vmbr1:0", "vmbr1:4095", "vmbr1:+5", "vmbr1:2 0", "nat:20", ":20", "vmbr1:20:30", "br0,script=/x:20", "vmbr1:yirmi"] {
            assert_eq!(parse_nic(bad), None, "{bad}");
        }
        let hw = |f: &dyn Fn(&mut Hardware), kind| {
            let mut hw = Hardware::default();
            f(&mut hw);
            hw.validate(kind)
        };
        assert!(hw(&|h| (h.vlan, h.nics) = (4094, vec!["vmbr1:20".into(), "nat".into()]), GuestKind::Vm).is_ok());
        assert!(hw(&|h| h.vlan = 4095, GuestKind::Vm).is_err());
        assert!(hw(&|h| h.nics = vec!["nat:5".into()], GuestKind::Vm).unwrap_err().contains("geçersiz ağ ayarı"));
        assert!(hw(&|h| h.vlan = 5, GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        assert!(hw(&|h| h.nics = vec!["vmbr1:5".into()], GuestKind::Ct).is_err());
        assert!(hw(&|h| h.nics = vec!["vmbr1".into()], GuestKind::Ct).is_ok(), "etiketsiz ek kart konteynerde de olur");
        // Hız sınırı için köprüdeki kart sayılır; etiketli tanım da köprüdür.
        let limited = Hardware { net_mbps: 10, nics: vec!["vmbr1:20".into()], ..Default::default() };
        assert!(limited.net_limit_applies("nat"));
    }

    #[test]
    fn shares_parse_format_and_validate() {
        let list = parse_shares("ortak, veri:ro\nyedek-2:rw").unwrap();
        assert_eq!(list.iter().map(|s| (s.name.as_str(), s.readonly)).collect::<Vec<_>>(), [("ortak", false), ("veri", true), ("yedek-2", false)]);
        assert_eq!(list.iter().map(ToString::to_string).collect::<Vec<_>>(), ["ortak", "veri:ro", "yedek-2"]);
        assert_eq!(parse_shares(" , ").unwrap(), []);
        for bad in ["../etc", "a/b", "Ortak", "-x", "a.b", "veri:rx", "a:ro:ro", "çok", &"a".repeat(25)] {
            assert!(parse_shares(bad).is_err(), "{bad}");
        }
        let hw = |shares: Vec<Share>, kind| Hardware { shares, ..Default::default() }.validate(kind);
        assert!(hw(list.clone(), GuestKind::Vm).is_ok());
        assert!(hw(list.clone(), GuestKind::Ct).unwrap_err().contains("yalnızca sanal makinelerde"));
        assert!(hw(vec![list[0].clone(), list[0].clone()], GuestKind::Vm).unwrap_err().contains("iki kez"));
        assert!(hw(vec![Share { name: "..".into(), readonly: false }], GuestKind::Vm).is_err(), "API'den gelen geçersiz ad da reddedilir");
        let many: Vec<Share> = (0..=MAX_SHARES).map(|i| Share { name: format!("p{i}"), readonly: false }).collect();
        assert!(hw(many, GuestKind::Vm).unwrap_err().contains("en fazla"));
    }
}
