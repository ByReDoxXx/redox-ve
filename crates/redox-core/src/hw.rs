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
    /// Güvenlik duvarı açık mı? Açıksa yalnızca `fw_rules` içindeki gelen trafiğe izin verilir.
    pub firewall: bool,
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
        match self.days {
            EVERY_DAY => "her gün".to_string(),
            0b001_1111 => "hafta içi".to_string(),
            0b110_0000 => "hafta sonu".to_string(),
            days => (0..7).filter(|d| days & (1 << d) != 0).map(|d| DAY_NAMES[d as usize]).collect::<Vec<_>>().join(", "),
        }
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
        let cloud = self.image.is_some() || self.ci_user.is_some() || self.ci_ssh_key.is_some();
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
pub fn meta_data(vmid: VmId, hostname: &str) -> String {
    format!("instance-id: redox-{vmid}\nlocal-hostname: {hostname}\n")
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
        assert_eq!(meta_data(103, "web-01"), "instance-id: redox-103\nlocal-hostname: web-01\n");
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
