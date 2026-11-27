//! `redox` komut satırı istemcisi: çalışan sunucunun REST API'sine bağlanır.

use redox_core::fmt::{gb, mb_as_gb};
use serde_json::{json, Map, Value};

pub const DEFAULT_ADDR: &str = "127.0.0.1:8006";

/// İngilizce yardım metni (`REDOX_LANG=en`).
pub const USAGE_EN: &str = include_str!("usage_en.txt");

/// Komut satırının diline göre yardım metni.
pub fn usage() -> &'static str {
    match crate::i18n::cli_lang() {
        crate::i18n::Lang::En => USAGE_EN,
        crate::i18n::Lang::Tr => USAGE,
    }
}

pub const USAGE: &str = "\
redox — RedoX VE

Sunucu:
  redox serve [--addr 127.0.0.1:8006] [--users DOSYA] [--state DOSYA]
              [--driver sim|qemu] [--data-dir DİZİN] [--qemu-bin YOL] [--ovmf-dir DİZİN] [--swtpm-bin YOL]
              [--shutdown-timeout-secs 60] [--no-lxc] [--no-autostart] [--storage AD=DİZİN]...
              [--behind-tls-proxy] [--allow-plain-http] [--task-delay-ms 1500]
              [--alert-webhook URL] [--alert-email DOSYA] [--peers DOSYA] [--offsite DOSYA] [--offsite-copy DOSYA]... [--oidc DOSYA] [--guestfish-bin YOL] [--migrate-ports 49152-49215] [--migrate-host ADRES] [--migrate-bind 0.0.0.0] [--backup-test-secs 60] [--backup-report SS:DD] [--backup-age-days 7] [--backup-mbps 0] [--firewall] [--incremental-backups [--changed-block-tracking] [--backup-passphrase-file DOSYA]] [--move-precopy]

Kullanıcılar (kullanıcı dosyasını doğrudan düzenler; --users DOSYA ya da REDOX_USERS):
  redox user add <ad> [--role admin|operator|viewer]
  redox user role <ad> admin|operator|viewer
  redox user scope <ad> [etiket,etiket]   (operatörün yetkili olduğu konuk etiketleri; boşsa hepsi)
  redox user sso <ad> <kimlik>            (tek oturum açmada eşleşen e-posta ya da kimlik; boş değer kaldırır)
  redox user passwd <ad>
  redox user remove <ad>
  redox user totp-reset <ad>      (iki adımlı doğrulamayı kapatır)
  redox user permits <ad> <power,console,snapshot,backup | all>   (operatörün yetki türleri)
  redox user keys-reset <ad>      (güvenlik anahtarlarını kaldırır)
  redox user list
  redox token create <kullanıcı> <etiket> [--read-only] [--days <gün>]   (süre verilmezse süresiz)
  redox token revoke <kullanıcı> <etiket>

İstemci (çalışan sunucuya bağlanır; REDOX_URL adresi, REDOX_TOKEN API token'ı):
  redox cluster status
  redox node list | node status <düğüm>
  redox vm|ct list | show <kimlik>
  redox vm|ct start|stop|reboot|pause|resume <kimlik>
  redox vm|ct set <kimlik> anahtar=değer ...
        anahtarlar: name cores memory os iso bridge onboot restart boot-order boot-delay tags forwards backup-time backup-keep
                    schedules=start@08:00@1-5,stop@20:00   (eylem@SS:DD@günler; günler 1 Pazartesi … 7 Pazar, yazılmazsa her gün)
                    disk-mbps disk-iops net-mbps   (hız sınırları; 0 sınırsız. Ağ sınırı yalnızca köprüdeki kartlarda)
                    vlan=20   (birinci kartın VLAN etiketi; ek kartlarda nics=vmbr1:20)
                    shares=ortak,veri:ro   (paylaşılan klasörler; :ro salt okunur)
                    backup-pre='/usr/local/bin/once.sh' backup-post='/usr/local/bin/sonra.sh'
                                           (çalışan konuğun yedeğinden önce ve sonra konukta çalışır; konuk ajanı gerekir)
                    nic-guard=true   (ağ kartı koruması: sahte MAC, DHCP sunucusu ve yönlendirici duyurusu engellenir)
                    test-port=22 test-cmd=/usr/local/bin/saglik.sh   (yedek sınamasında denetlenecek TCP kapısı ve konukta çalıştırılacak komut)
                    rpo=24   (yedek yaşı hedefi, saat: son yedek bundan eskiyse konuk korumasız sayılır; 0 genel ayar)
                    nested=true   (iç içe sanallaştırma: konuğa VT-x / AMD-V gösterilir)
                    sound=true    (ses kartı; ses canlı konsoldan dinlenir)
                    cpu=Haswell   (işlemci modeli; cpu=host sunucunun kendi işlemcisi)   balloon=true   (bellek balonu aygıtı)
                    ci-user=ali ci-key='ssh-ed25519 AAAA…' ci-ip=10.0.0.5/24 ci-gw=10.0.0.1 ci-dns=1.1.1.1,9.9.9.9
                                           (cloud-init; ci-key= boş verilirse kapanır, ci-ip= boşsa DHCP)
                    cpu-affinity=0-3,8 numa-node=0 hugepages=true   (çekirdek sabitleme, NUMA bağı, büyük sayfalar; boş değer kaldırır)
                    disk-bus=virtio|virtio-scsi|sata nic-model=virtio|e1000e|e1000 iso2=virtio-win.iso   (Windows ve aktarılan diskler için)
                    nics=vmbr1,nat usb=046d:c52b pci=0000:01:00.0 firmware=bios|uefi|uefi-secure tpm firewall fw-rules   (örn. firewall=true fw-rules='tcp/22,tcp/443 from 10.0.0.0/8')
        örnek: redox vm set 103 cores=4 memory=8192 tags=prod,web backup-time=03:30
  redox vm|ct clone <kimlik> <yeni-ad>
  redox vm disk add <kimlik> <GB> | vm nic add <kimlik> <nat|köprü>   (çalışan sanal makineye de takılır)
  redox vm resize <kimlik> <GB>
  redox vm move <kimlik> <depolama>      (sanal makinenin disklerini başka depolama alanına taşır; çalışırken de olur)
  redox vm media <kimlik> <1|2> [kalıp]  (çalışan konuğun CD sürücüsündeki kalıbı değiştirir; kalıp verilmezse boşaltır)
  redox vm balloon <kimlik> <MB>         (bellek hedefi; çalışırken de değişir, 0: belleğin tamamı)
  redox vm cores <kimlik> <sayı>         (çalışan konuğu <sayı> çekirdeğe çıkarır; önce kapalıyken max-cores=<en fazla> verin)
  redox vm memory <kimlik> <MB>          (çalışan konuğun belleğini <MB> değerine çıkarır; önce kapalıyken max-memory=<en fazla> verin)
  redox replica list | add <eş> <kimlik> [dakika] | run <eş> <kimlik> | remove <eş> <kimlik>
  redox replica failover <eş> <kimlik> <dakika|off>    # kaynak ölünce yedek kopyayı hakem onayıyla başlat
  redox net vxlan <köprü> <vni> <yerel-adres> <eş-adres>[,...] [--unit]   # sunucular arası ortak ağın kurulum komutlarını (ya da kalıcı systemd birimini) yazar
        (eş sunucudaki sanal makinenin buradaki yedek kopyasını düzenli tazeler; varsayılan 60 dakikada bir)
  redox maintenance status | on [--drain] | off   (bakım kipi: konuk başlatılmaz, zamanlanmış işler durur;
        --drain çalışan konukları sırayla kapatır, off onları yeniden başlatır)
  redox maintenance on --evacuate <eş> [--as <ad>] [--drain]   (çalışan sanal makineleri kapatmadan eşe taşır;
        --as bu sunucunun eşteki adı, --drain taşınamayanları kapatır)
  redox lockdown status | on | off   (kilit kipi: açıkken kimse değişiklik yapamaz; konuklar ve zamanlanmış işler sürer)
  redox approval list | approve <no> | reject <no>   (dört göz ilkesi: bekleyen silme istekleri; kendi isteğinizi onaylayamazsınız)
  redox backup ack <yedek>               (olağan dışı değişim uyarısını kapatır: baktınız, sorun yok)
  redox backup archive | recover <yedek>   (yerelde silinmiş ama kopya hedefinde saklanan yedekler; recover listeye geri getirir)
  redox media catalog | get <anahtar>    (hazır bulut imajları: Debian, Ubuntu, AlmaLinux, Rocky; get indirir)
  redox vm df <kimlik>                   (konuğun içindeki disklerin doluluğu; konukta qemu-guest-agent gerekir)
  redox usage | vm usage <kimlik> [reset]   (birikimli kaynak tüketimi: çekirdek-saat, GB-saat, disk G/Ç; reset sayaçları sıfırlar)
  redox vm balloon <kimlik> auto <MB>    (dinamik bellek: hedef kullanıma göre <MB> ile belleğin tamamı arasında; 0 kapatır)
  redox vm disk add <kimlik> <GB> | disk remove <kimlik> <numara>
  redox vm suspend <kimlik> | discard-suspend <kimlik>   (askıya alır; start kaldığı yerden sürdürür)
  redox vm template <kimlik> on|off      (şablona çevirir / çıkarır)
  redox vm clone <kimlik> <ad> --linked  (şablondan bağlı klon: disk kopyalanmaz)
  redox ct exec <kimlik> <komut ...>
  redox vm|ct destroy <kimlik>
  redox snapshot list <kimlik> | create|rollback|delete <kimlik> <ad>
  redox snapshot create <kimlik> <ad> --production   (çalışan konukta belleksiz, dosya sistemi dondurulmuş denetim noktası)
  redox backup report                     (son 24 saatin yedek raporu ve korumasız konuklar)
  redox backup list | create <kimlik> | restore <yedek> | delete <yedek>
  redox backup restore-new <yedek> <ad>   (yedekten yeni bir sanal makine açar; özgün konuğa dokunmaz)
  redox backup ls <yedek> [yol]           (yedeğin içindeki dosyaları listeler; sunucuda guestfish gerekir)
  redox backup put <yedek> <yol> [hedef]  (yedekteki dosyayı çalışan konuğun içine yazar; konukta qemu-guest-agent gerekir)
  redox backup put-dir <yedek> <yol> [hedef]   (yedekteki klasörü konukta var olan hedef dizine açar)
        dosya indirmek: curl -H 'Authorization: Bearer TOKEN' -o dosya 'REDOX_URL/api/v1/backups/YEDEK/file?path=/etc/hostname'
  redox backup instant <yedek> <ad>       (anında kurtarma: konuk kopyalama beklemeden yedeğin üzerinde açılır)
  redox vm detach <kimlik>                (anında kurtarılmış konuğu yedekten ayırır; çalışırken de yapılır)
  redox backup test <yedek>               (yedeği yalıtılmış geçici bir konuk olarak açıp sınar; sonuç: redox task list)
  redox backup lock <yedek> <gün>         (yedeği o kadar gün silinemez yapar; kilit yalnızca uzatılır)
  redox backup push <yedek>       (dış hedefe yeniden gönderir)
  redox backup verify <yedek>     (bozulma denetimi; sonuç: redox task list)
  redox pool list | remove <ad>                (kaynak havuzları: konuk grubuna toplam kota)
  redox pool add <ad> [cores=32] [memory=65536] [disk=500] [guests=10]   (memory MB, disk GB; yazılmayan sınırsız)
        konuğu havuza almak: redox vm set 103 pool=web   (pool= boş verilirse havuzdan çıkar)
  redox job list | run <ad> | remove <ad>      (yedek işleri: birden çok konuk, tek zamanlama, sırayla)
  redox job add <ad> time=03:00 [days=1-5] [tags=web,db] [keep=7] [weekly=4] [monthly=12] [yearly=1] [lock=14]
        weekly/monthly/yearly: son N'e ek olarak saklanacak haftalık, aylık, yıllık yedek; lock: yedekler kaç gün silinemez
        until=06:00: yedek penceresinin sonu (sonrasında sıradaki konuklara başlanmaz); verify=true: her yedek doğrulanır
        tags verilmezse bütün konuklar; days verilmezse her gün; aynı adla yeniden yazmak işi günceller
  redox storage list
  redox media list | delete <dosya>            (kurulum kalıpları ve disk imajları)
  redox media fetch <https-adresi> [sha256]    (sunucu dosyayı kendisi indirir)
        OVA dışa aktarma: curl -H 'Authorization: Bearer TOKEN' -o vm.ova REDOX_URL/api/v1/guests/KIMLIK/export
        yüklemek için: curl -T debian.iso -H 'Authorization: Bearer TOKEN' REDOX_URL/api/v1/media/debian.iso
  redox profile export > profil.json | diff <dosya> | apply <dosya>   (sunucu profili: ayarları dışa alır, başka
        sunucununkiyle karşılaştırır, havuzları ve yedek işlerini uygular)
  redox support > destek.json     (destek paketi: sorun bildirimi için durum dökümü; parola ve token içermez)
  redox tty [--once]              (sunucunun konsolundaki durum ekranı: adres, hizmet, konuklar; Enter ile oturum açılır)
  redox peer list                 (birleşik görünümdeki eş sunucular)
  redox peer import <eş> <kimlik> (eşteki kapalı sanal makineyi bu sunucuya kopyalar)
  redox peer migrate <eş> <kimlik> (eşteki çalışan sanal makineyi kapatmadan bu sunucuya taşır)
  redox peer send <eş> <kimlik>   (bu sunucudaki çalışan sanal makineyi kapatmadan eşe taşır)
  redox balance                   (yük dengeleme önerisi: sunucu sıkışmışsa hangi konuk hangi eşe taşınmalı)
  redox ft list | on <kimlik> <ikiz-eş> <hakem-eş> | off <kimlik> | twins   (hata toleransı)
  redox mirror list | add <kimlik> <eş> | off <kimlik> [--delete-copy]   (eşzamanlı disk yansıtma)
  redox mirror copies | promote <kaynak> <kimlik> | delete-copy <kaynak> <kimlik>   (burada tutulan kopyalar)
  redox user ldap <ad> on|off --users <dosya>   (parola LDAP/AD'de doğrulanır; sunucuya: serve --ldap <ayar-dosyası>)
  redox ldap test <ayar-dosyası> <kullanıcı>    (LDAP ayarını dener)
  redox node perf | health | update      (performans donanımı; disk sağlığı ve bekleyen güncellemeler; güncellemeleri kurma isteği)
  redox acme issue --domain <ad> --dir <dizin> --agree-tos [--email <adres>] [--staging]   (Let's Encrypt sertifikası; 80 numaralı kapı açık olmalı)
  redox acme renew --dir <dizin> | status --dir <dizin>            (30 günden az kalınca yeniler; zamanlayıcıyla çalıştırın)
  redox tls self-signed <ad> --dir <dizin>   (kendinden imzalı sertifika; sunucuya: serve --tls-cert … --tls-key …)
  redox hostnet list | remove <ad>                                  (sunucu ağları)
  redox hostnet bond <ad> <active-backup|802.3ad|balance-xor> <a,b> | vlan <arayüz> <etiket> | bridge <ad> [ports=a,b] [vlan-aware=true]
  redox hostnet guest <ad> <adres/ön> [dhcp=baş-son] [nat=true]     (DHCP/DNS'li konuk ağı)
  serve --syslog <udp|tcp|tls://ad:kapı> [--syslog-ca <pem>]   (denetim kayıtlarının kopyası dış sunucuya)
  serve --disk-key-file <dosya>   (şifreli diskli konuklar: oluştururken API'de encrypted: true ya da panelde Diskleri şifrele kutusu)
  serve --webauthn-origin <adres> (güvenlik anahtarıyla ikinci adım: panelin tarayıcıdaki https:// adresi; IP adresi olamaz)
  redox shared init <dizin>                                         (bir dizini ortak depo olarak hazırlar; sunucuda: serve --storage ad=<dizin> --shared-storage ad)
  redox shared list | take <eş> <kimlik> | adopt <depo> <kimlik>    (ortak depodaki konuklar; disk kopyalamadan alma ve devralma)
  redox firewall set list | save <ad> <adres,adres> | remove <ad>             (kurallarda @ad olarak kullanılan adres kümeleri)
  redox firewall group list | save <ad> in='<kurallar>' out='<kurallar>' | remove <ad>   (güvenlik grupları; konuğa: vm set <kimlik> fw-groups=ad)
  redox switch list | set <vds-ad> <vni> <adres,adres> [--mtu <n>] | push <ad> | remove <ad> [--everywhere]   (dağıtık anahtar)
  redox task list
";

/// Ayrıştırılmış istemci komutu: API çağrısı ve yanıtın nasıl gösterileceği.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
    pub view: View,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum View {
    Cluster,
    Nodes,
    Node,
    Guests(&'static str),
    Guest,
    Storage,
    Peers,
    Tasks,
    Snapshots,
    Backups,
    Media,
    Jobs,
    Pools,
    /// Onay bekleyen istekler.
    Approvals,
    /// `{ "done": ... }` döndüren onay.
    Approved,
    /// Çoğaltmalar.
    Replicas,
    /// Bakım kipi durumu ya da değişikliği.
    Maintenance,
    Lockdown,
    Profile,
    Balance,
    Switches,
    SwitchPeers,
    Protections,
    Standbys,
    Mirrors,
    MirrorStores,
    Promoted,
    /// Yanıt olduğu gibi (girintili JSON) yazılır.
    Json,
    /// Hazır imaj kataloğu.
    Catalog,
    /// Konuğun içindeki dosya sistemleri.
    Filesystems,
    /// Kaynak ölçümü: bir konuk (nesne) ya da hepsi (dizi).
    Usage,
    /// Yedeğin içindeki dizin listesi.
    Files,
    /// Düz metin yedek raporu.
    Report,
    /// `{ "queued": ... }` döndüren iş başlatma.
    JobStarted,
    /// Görev döndüren işlemler.
    Task,
    /// `{ "id": ... }` döndüren yedek alma.
    BackupStarted,
    /// `{ "vmid": ... }` döndüren klonlama.
    Cloned,
    /// `{ "output": ... }` döndüren komut çalıştırma.
    Output,
    /// Gövdesiz başarı yanıtı.
    Done(&'static str),
}

fn vmid(raw: &str) -> Result<u32, String> {
    raw.parse().map_err(|_| format!("kimlik bir sayı olmalı: {raw}"))
}

/// `anahtar=değer` çiftlerini API'nin beklediği düzenleme gövdesine çevirir.
pub fn patch_body(pairs: &[&str]) -> Result<Value, String> {
    if pairs.is_empty() {
        return Err("en az bir anahtar=değer verin, örn. cores=4".to_string());
    }
    let mut body = Map::new();
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| format!("\"{pair}\" anahtar=değer biçiminde değil"))?;
        let number = || value.parse::<u64>().map(Value::from).map_err(|_| format!("{key} bir sayı olmalı: {value}"));
        let (field, parsed) = match key {
            "name" | "os" | "iso" | "iso2" | "bridge" | "pool" => (key, Value::from(value)),
            "disk-bus" => ("disk_bus", Value::from(redox_core::hw::DiskBus::parse(value).ok_or_else(|| format!("disk-bus virtio, virtio-scsi ya da sata olmalı: {value}"))?.as_str())),
            "nic-model" => ("nic_model", Value::from(redox_core::hw::NicModel::parse(value).ok_or_else(|| format!("nic-model virtio, e1000e ya da e1000 olmalı: {value}"))?.as_str())),
            "cpu-affinity" => ("cpu_affinity", Value::from(value)),
            "numa-node" => ("numa_node", Value::from(value)),
            "hugepages" => ("hugepages", Value::from(match value {
                "true" | "1" | "evet" => true,
                "false" | "0" | "hayır" | "hayir" => false,
                _ => return Err(format!("{key} true ya da false olmalı: {value}")),
            })),
            "cpu" => ("cpu_type", Value::from(if value == "host" { "" } else { value })),
            "ci-user" => ("ci_user", Value::from(value)),
            "ci-key" => ("ci_ssh_key", Value::from(value)),
            "ci-ip" => ("ci_ip", Value::from(value)),
            "ci-gw" => ("ci_gw", Value::from(value)),
            "ci-dns" => ("ci_dns", Value::from(value)),
            "cores" => ("cores", number()?),
            "max-cores" => ("max_cores", number()?),
            "max-memory" => ("max_memory_mb", number()?),
            "memory" => ("memory_mb", number()?),
            "backup-keep" => ("backup_keep", number()?),
            "cpu-shares" => ("cpu_shares", number()?),
            "mem-reserve" => ("mem_reserve_mb", number()?),
            "rpo" => ("rpo_hours", number()?),
            "backup-weekly" => ("backup_weekly", number()?),
            "backup-monthly" => ("backup_monthly", number()?),
            "backup-yearly" => ("backup_yearly", number()?),
            "boot-order" => ("boot_order", number()?),
            "disk-mbps" => ("disk_mbps", number()?),
            "disk-iops" => ("disk_iops", number()?),
            "net-mbps" => ("net_mbps", number()?),
            "vlan" => ("vlan", number()?),
            "shares" => {
                let list = redox_core::hw::parse_shares(value)?;
                ("shares", Value::from(list.iter().map(|s| json!({ "name": s.name, "readonly": s.readonly })).collect::<Vec<_>>()))
            }
            "boot-delay" => ("boot_delay", number()?),
            "schedules" => {
                let list = redox_core::hw::parse_schedules(value)?;
                let items: Vec<Value> = list.iter().map(|s| json!({ "action": s.action.as_str(), "time": s.time, "days": s.days })).collect();
                ("schedules", Value::from(items))
            }
            "backup-time" => ("backup_time", Value::from(value)),
            "backup-pre" => ("backup_pre", Value::from(value)),
            "test-port" => ("test_port", number()?),
            "test-cmd" => ("test_cmd", Value::from(value)),
            "backup-post" => ("backup_post", Value::from(value)),
            "onboot" | "firewall" | "tpm" | "restart" | "balloon" | "sound" | "nic-guard" | "nested" => (
                if key == "nic-guard" { "nic_guard" } else { key },
                Value::from(match value {
                    "true" | "1" | "evet" => true,
                    "false" | "0" | "hayır" | "hayir" => false,
                    _ => return Err(format!("{key} true ya da false olmalı: {value}")),
                }),
            ),
            "firmware" => {
                let (firmware, secure) = redox_core::hw::parse_firmware_choice(value).ok_or_else(|| format!("firmware bios, uefi ya da uefi-secure olmalı: {value}"))?;
                body.insert("secure_boot".to_string(), Value::from(secure));
                ("firmware", serde_json::to_value(firmware).map_err(|e| e.to_string())?)
            }
            "fw-rules" => ("fw_rules", serde_json::to_value(redox_core::hw::parse_fw_rules(value)?).map_err(|e| e.to_string())?),
            "fw-out" => ("fw_out", serde_json::to_value(redox_core::hw::parse_fw_out_rules(value)?).map_err(|e| e.to_string())?),
            "fw-out-restrict" => ("fw_out_restrict", Value::from(match value {
                "true" | "1" | "evet" => true,
                "false" | "0" | "hayır" | "hayir" => false,
                _ => return Err(format!("{key} true ya da false olmalı: {value}")),
            })),
            "fw-groups" => ("fw_groups", Value::from(value.split(',').map(str::trim).filter(|n| !n.is_empty()).collect::<Vec<_>>())),
            "tags" => ("tags", Value::from(redox_core::parse_tags(value))),
            "nics" | "usb" | "pci" => (key, Value::from(value.split(',').map(str::trim).filter(|n| !n.is_empty()).collect::<Vec<_>>())),
            "forwards" => {
                let list = redox_core::parse_forwards(value)?;
                let items: Vec<Value> = list.iter().map(|f| json!({ "host_port": f.host_port, "guest_port": f.guest_port })).collect();
                ("forwards", Value::from(items))
            }
            _ => {
                return Err(format!(
                    "bilinmeyen anahtar \"{key}\"; geçerli olanlar: name cores max-cores cpu-shares cpu-affinity numa-node hugepages memory max-memory mem-reserve os iso iso2 disk-bus nic-model bridge onboot restart boot-order boot-delay schedules disk-mbps disk-iops net-mbps vlan shares tags forwards backup-time backup-keep firewall fw-rules fw-out fw-out-restrict fw-groups firmware tpm nics usb pci ci-user ci-key ci-ip ci-gw ci-dns cpu balloon pool backup-weekly backup-monthly backup-yearly backup-pre backup-post"
                ))
            }
        };
        body.insert(field.to_string(), parsed);
    }
    Ok(Value::Object(body))
}

/// Yolu sorgu dizgisine konacak biçimde kodlar (harf, rakam ve `-_.~/` dışındakiler `%XX`).
fn redox_server_path(path: &str) -> String {
    path.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// `redox pool add` çiftlerini API gövdesine çevirir.
fn pool_body(name: &str, pairs: &[&str]) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert("name".to_string(), Value::from(name));
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| format!("\"{pair}\" anahtar=değer biçiminde değil"))?;
        let field = match key {
            "cores" => "cores",
            "memory" => "memory_mb",
            "disk" => "disk_gb",
            "guests" => "guests",
            _ => return Err(format!("bilinmeyen anahtar \"{key}\"; geçerli olanlar: cores memory disk guests")),
        };
        body.insert(field.to_string(), Value::from(value.parse::<u64>().map_err(|_| format!("{key} bir sayı olmalı: {value}"))?));
    }
    Ok(Value::Object(body))
}

/// `redox job add` çiftlerini API gövdesine çevirir.
fn job_body(name: &str, pairs: &[&str]) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert("name".to_string(), Value::from(name));
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| format!("\"{pair}\" anahtar=değer biçiminde değil"))?;
        let parsed = match key {
            "time" | "days" | "until" => Value::from(value),
            "verify" => Value::from(matches!(value, "true" | "1" | "evet")),
            "tags" => Value::from(value.split(',').map(str::trim).filter(|t| !t.is_empty()).collect::<Vec<_>>()),
            "keep" | "weekly" | "monthly" | "yearly" | "lock" => Value::from(value.parse::<u32>().map_err(|_| format!("{key} bir sayı olmalı: {value}"))?),
            _ => return Err(format!("bilinmeyen anahtar \"{key}\"; geçerli olanlar: time days tags keep weekly monthly yearly lock until verify")),
        };
        body.insert(key.to_string(), parsed);
    }
    if !body.contains_key("time") {
        return Err("saat verin, örn. time=03:00".to_string());
    }
    Ok(Value::Object(body))
}

pub fn parse(args: &[String]) -> Result<Request, String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let req = |method: &'static str, path: String, body: Option<Value>, view: View| Ok(Request { method, path, body, view });
    let get = |path: &str, view: View| req("GET", path.to_string(), None, view);
    match a.as_slice() {
        ["cluster", "status"] => get("/cluster", View::Cluster),
        ["node", "list"] => get("/nodes", View::Nodes),
        ["node", "perf"] => get("/host/performance", View::Json),
        ["node", "health"] => get("/host/health", View::Json),
        ["node", "update"] => req("POST", "/host/update".to_string(), None, View::Done("güncelleme istendi; sonuç: redox node health")),
        ["node", "status", id] => get(&format!("/nodes/{id}"), View::Node),
        ["vm", "list"] => get("/guests", View::Guests("vm")),
        ["ct", "list"] => get("/guests", View::Guests("ct")),
        ["vm" | "ct", "show", id] => get(&format!("/guests/{}", vmid(id)?), View::Guest),
        ["vm" | "ct", "destroy", id] => req("DELETE", format!("/guests/{}", vmid(id)?), None, View::Done("silindi")),
        ["vm" | "ct", action @ ("start" | "stop" | "reboot" | "pause" | "resume"), id] => {
            req("POST", format!("/guests/{}/{action}", vmid(id)?), None, View::Task)
        }
        ["vm" | "ct", "set", id, pairs @ ..] => req("PATCH", format!("/guests/{}", vmid(id)?), Some(patch_body(pairs)?), View::Guest),
        ["vm" | "ct", "clone", id, name] => req("POST", format!("/guests/{}/clone", vmid(id)?), Some(json!({ "name": name })), View::Cloned),
        ["vm", "clone", id, name, "--linked"] => req("POST", format!("/guests/{}/clone", vmid(id)?), Some(json!({ "name": name, "linked": true })), View::Cloned),
        ["vm", "suspend", id] => req("POST", format!("/guests/{}/suspend", vmid(id)?), None, View::Task),
        ["vm", "discard-suspend", id] => req("POST", format!("/guests/{}/discard-suspend", vmid(id)?), None, View::Done("askı durumu atıldı")),
        ["vm", "template", id, state @ ("on" | "off")] => req("POST", format!("/guests/{}/template", vmid(id)?), Some(json!({ "on": *state == "on" })), View::Done("tamam")),
        ["vm", "resize", id, size] => {
            let disk_gb: u64 = size.trim_end_matches(['G', 'g']).parse().map_err(|_| format!("boyut GB cinsinden bir sayı olmalı: {size}"))?;
            req("POST", format!("/guests/{}/resize", vmid(id)?), Some(json!({ "disk_gb": disk_gb })), View::Task)
        }
        ["ct", "exec", id, command @ ..] if !command.is_empty() => {
            req("POST", format!("/guests/{}/exec", vmid(id)?), Some(json!({ "command": command.join(" ") })), View::Output)
        }
        ["snapshot", "list", id] => get(&format!("/guests/{}", vmid(id)?), View::Snapshots),
        ["snapshot", "create", id, name, "--production"] => req("POST", format!("/guests/{}/snapshots", vmid(id)?), Some(json!({ "name": name, "production": true })), View::Task),
        ["snapshot", "create", id, name] => req("POST", format!("/guests/{}/snapshots", vmid(id)?), Some(json!({ "name": name })), View::Task),
        ["snapshot", "rollback", id, name] => req("POST", format!("/guests/{}/snapshots/{name}/rollback", vmid(id)?), None, View::Task),
        ["snapshot", "delete", id, name] => req("DELETE", format!("/guests/{}/snapshots/{name}", vmid(id)?), None, View::Task),
        ["vm", "disk", "add", id, size] => {
            let size_gb: u64 = size.trim_end_matches(['G', 'g']).parse().map_err(|_| format!("boyut GB olarak bir sayı olmalı: {size}"))?;
            req("POST", format!("/guests/{}/disks", vmid(id)?), Some(json!({ "size_gb": size_gb })), View::Done("disk eklendi"))
        }
        ["vm", "nic", "add", id, network] => req("POST", format!("/guests/{}/nics", vmid(id)?), Some(json!({ "network": network })), View::Done("ağ kartı eklendi")),
        ["vm", "disk", "remove", id, n] => {
            let n: u8 = n.parse().map_err(|_| format!("disk numarası bir sayı olmalı: {n}"))?;
            req("DELETE", format!("/guests/{}/disks/{n}", vmid(id)?), None, View::Done("disk silindi"))
        }
        ["media", "list"] => get("/media", View::Media),
        ["media", "delete", name] => req("DELETE", format!("/media/{name}"), None, View::Done("silindi")),
        ["media", "catalog"] => get("/media/catalog", View::Catalog),
        ["media", "get", key] => req("POST", format!("/media/catalog/{key}"), None, View::Done("indiriliyor; sonuç: redox task list")),
        ["media", "fetch", url] => req("POST", "/media/fetch".to_string(), Some(json!({ "url": url })), View::Done("indiriliyor; sonuç: redox task list")),
        ["media", "fetch", url, sha] => req("POST", "/media/fetch".to_string(), Some(json!({ "url": url, "sha256": sha })), View::Done("indiriliyor; sonuç: redox task list")),
        ["backup", "list"] => get("/backups", View::Backups),
        ["backup", "report"] => get("/backups/report", View::Report),
        ["backup", "create", id] => req("POST", format!("/guests/{}/backups", vmid(id)?), None, View::BackupStarted),
        ["backup", "restore", id] => req("POST", format!("/backups/{id}/restore"), None, View::Task),
        ["backup", "instant", id, name] => req("POST", format!("/backups/{id}/instant"), Some(json!({ "name": name })), View::Cloned),
        ["vm", "detach", id] => req("POST", format!("/guests/{}/detach", vmid(id)?), None, View::Task),
        ["replica", "list"] => get("/replicas", View::Replicas),
        ["replica", "add", peer, id] => req("POST", "/replicas".to_string(), Some(json!({ "peer": peer, "vmid": vmid(id)? })), View::Done("çoğaltma eklendi; ilk tur bir dakika içinde başlar")),
        ["replica", "add", peer, id, minutes] => req(
            "POST",
            "/replicas".to_string(),
            Some(json!({ "peer": peer, "vmid": vmid(id)?, "every_min": minutes.parse::<u32>().map_err(|_| format!("aralık dakika olarak bir sayı olmalı: {minutes}"))? })),
            View::Done("çoğaltma eklendi; ilk tur bir dakika içinde başlar"),
        ),
        ["replica", "failover", peer, id, minutes] => {
            let minutes = match *minutes {
                "off" | "kapat" | "0" => None,
                text => Some(text.parse::<u32>().map_err(|_| "devralma süresi dakika olarak bir tam sayı ya da off olmalı".to_string())?),
            };
            req("POST", format!("/replicas/{peer}/{}/failover", vmid(id)?), Some(json!({ "minutes": minutes })), View::Done("devralma ayarı kaydedildi"))
        }
        ["replica", "run", peer, id] => req("POST", format!("/replicas/{peer}/{}/run", vmid(id)?), None, View::Done("çoğaltma başladı; sonuç: redox task list")),
        ["replica", "remove", peer, id] => req("DELETE", format!("/replicas/{peer}/{}", vmid(id)?), None, View::Done("çoğaltma kaldırıldı; yedek kopya olağan bir konuk olarak duruyor")),
        ["maintenance", "status"] => get("/maintenance", View::Maintenance),
        ["maintenance", "on"] => req("POST", "/maintenance".to_string(), Some(json!({ "on": true })), View::Maintenance),
        ["maintenance", "on", "--drain"] => req("POST", "/maintenance".to_string(), Some(json!({ "on": true, "drain": true })), View::Maintenance),
        ["maintenance", "on", "--evacuate", peer, rest @ ..] => {
            let mut body = json!({ "on": true, "evacuate": peer });
            let mut rest = rest.iter();
            while let Some(flag) = rest.next() {
                match *flag {
                    "--drain" => body["drain"] = json!(true),
                    "--as" => body["as"] = json!(rest.next().ok_or("--as bu sunucunun eşteki adını ister")?),
                    other => return Err(format!("bilinmeyen seçenek: {other}")),
                }
            }
            req("POST", "/maintenance".to_string(), Some(body), View::Maintenance)
        }
        ["maintenance", "off"] => req("POST", "/maintenance".to_string(), Some(json!({ "on": false })), View::Maintenance),
        ["lockdown", "status"] => get("/lockdown", View::Lockdown),
        ["lockdown", "on"] => req("POST", "/lockdown".to_string(), Some(json!({ "on": true })), View::Lockdown),
        ["lockdown", "off"] => req("POST", "/lockdown".to_string(), Some(json!({ "on": false })), View::Lockdown),
        ["approval", "list"] => get("/approvals", View::Approvals),
        ["approval", "approve", id] => req("POST", format!("/approvals/{}/approve", id.parse::<u64>().map_err(|_| format!("istek numarası bir sayı olmalı: {id}"))?), None, View::Approved),
        ["approval", "reject", id] => req("DELETE", format!("/approvals/{}", id.parse::<u64>().map_err(|_| format!("istek numarası bir sayı olmalı: {id}"))?), None, View::Done("istek reddedildi")),
        ["backup", "ack", id] => req("POST", format!("/backups/{id}/ack"), None, View::Done("uyarı kapatıldı")),
        ["backup", "archive"] => get("/backups/archive", View::Backups),
        ["backup", "recover", id] => req("POST", format!("/backups/{id}/recover"), None, View::Done("yedek arşivden geri getirildi; geri yüklenirken kopya hedefinden iner")),
        ["backup", "test", id] => req("POST", format!("/backups/{id}/test"), None, View::Done("sınama başladı; sonuç: redox task list")),
        ["backup", "lock", id, days] => req(
            "POST",
            format!("/backups/{id}/lock"),
            Some(json!({ "days": days.parse::<u32>().map_err(|_| format!("gün sayısı bir sayı olmalı: {days}"))? })),
            View::Done("yedek kilitlendi"),
        ),
        ["backup", "ls", id] => get(&format!("/backups/{id}/files?path=/"), View::Files),
        ["backup", "ls", id, path] => get(&format!("/backups/{id}/files?path={}", redox_server_path(path)), View::Files),
        ["backup", verb @ ("put" | "put-dir"), id, path, rest @ ..] if rest.len() <= 1 => req(
            "POST",
            format!("/backups/{id}/file"),
            Some(json!({ "path": path, "dir": *verb == "put-dir", "target": rest.first().copied().unwrap_or("") })),
            View::Done("geri yükleme başladı; sonuç: redox task list"),
        ),
        ["backup", "restore-new", id, name] => req("POST", format!("/backups/{id}/restore-new"), Some(json!({ "name": name })), View::Cloned),
        ["backup", "push", id] => req("POST", format!("/backups/{id}/push"), None, View::Done("dış hedefe gönderiliyor; sonuç: redox task list")),
        ["backup", "verify", id] => req("POST", format!("/backups/{id}/verify"), None, View::Done("doğrulanıyor; sonuç: redox task list")),
        ["backup", "delete", id] => req("DELETE", format!("/backups/{id}"), None, View::Done("yedek silindi")),
        ["vm", "move", id, storage] => req("POST", format!("/guests/{}/move", vmid(id)?), Some(json!({ "storage": storage })), View::Task),
        ["vm", "df", id] => get(&format!("/guests/{}/filesystems", vmid(id)?), View::Filesystems),
        ["usage"] => get("/usage", View::Usage),
        ["vm" | "ct", "usage", id] => get(&format!("/guests/{}/usage", vmid(id)?), View::Usage),
        ["vm" | "ct", "usage", id, "reset"] => req("DELETE", format!("/guests/{}/usage", vmid(id)?), None, View::Done("ölçüm sayaçları sıfırlandı")),
        ["vm", "cores", id, cores] => req(
            "POST",
            format!("/guests/{}/cores", vmid(id)?),
            Some(json!({ "cores": cores.parse::<u32>().map_err(|_| format!("çekirdek sayısı bir sayı olmalı: {cores}"))? })),
            View::Done("çekirdek eklendi"),
        ),
        ["vm", "memory", id, mb] => req(
            "POST",
            format!("/guests/{}/memory", vmid(id)?),
            Some(json!({ "memory_mb": mb.parse::<u64>().map_err(|_| format!("bellek bir sayı olmalı (MB): {mb}"))? })),
            View::Done("bellek eklendi"),
        ),
        ["vm", "media", id, drive, rest @ ..] if rest.len() <= 1 => req(
            "POST",
            format!("/guests/{}/media", vmid(id)?),
            Some(json!({ "drive": drive.parse::<u8>().map_err(|_| format!("CD sürücüsü 1 ya da 2 olmalı: {drive}"))?, "iso": rest.first().copied().unwrap_or("") })),
            View::Done("kalıp değiştirildi"),
        ),
        ["vm", "balloon", id, "auto", min] => req(
            "POST",
            format!("/guests/{}/balloon", vmid(id)?),
            Some(json!({ "min": min.parse::<u64>().map_err(|_| format!("en az bellek MB olarak bir sayı olmalı: {min}"))? })),
            View::Done("dinamik bellek ayarlandı"),
        ),
        ["vm", "balloon", id, mb] => req(
            "POST",
            format!("/guests/{}/balloon", vmid(id)?),
            Some(json!({ "mb": mb.parse::<u64>().map_err(|_| format!("bellek hedefi MB olarak bir sayı olmalı: {mb}"))? })),
            View::Done("bellek hedefi ayarlandı"),
        ),
        ["pool", "list"] => get("/pools", View::Pools),
        ["pool", "add", name, pairs @ ..] => req("POST", "/pools".to_string(), Some(pool_body(name, pairs)?), View::Done("havuz kaydedildi")),
        ["pool", "remove", name] => req("DELETE", format!("/pools/{name}"), None, View::Done("havuz silindi")),
        ["job", "list"] => get("/jobs", View::Jobs),
        ["job", "add", name, pairs @ ..] => req("POST", "/jobs".to_string(), Some(job_body(name, pairs)?), View::Done("yedek işi kaydedildi")),
        ["job", "run", name] => req("POST", format!("/jobs/{name}/run"), None, View::JobStarted),
        ["job", "remove", name] => req("DELETE", format!("/jobs/{name}"), None, View::Done("yedek işi silindi")),
        ["storage", "list"] => get("/storage", View::Storage),
        ["peer", "list"] => get("/peers", View::Peers),
        ["balance"] => get("/balance", View::Balance),
        ["ft", "list"] => get("/ft/protections", View::Protections),
        ["ft", "twins"] => get("/ft/standby", View::Standbys),
        ["ft", "on", id, peer, witness] => req("POST", "/ft/protections".to_string(), Some(json!({ "vmid": vmid(id)?, "peer": peer, "witness": witness })), View::Done("hata toleransı açıldı; konuğu başlatınca ikizi kurulur. Durum: redox ft list")),
        ["ft", "off", id] => req("DELETE", format!("/ft/protections/{}", vmid(id)?), None, View::Done("hata toleransı kapatıldı")),
        ["mirror", "list"] => get("/mirrors", View::Mirrors),
        ["mirror", "add", id, peer] => req("POST", "/mirrors".to_string(), Some(json!({ "vmid": vmid(id)?, "peer": peer })), View::Done("yansıtma açıldı; durumu: redox mirror list")),
        ["mirror", "off", id] => req("DELETE", format!("/mirrors/{}", vmid(id)?), None, View::Done("yansıtma kapatıldı; eşteki kopya duruyor")),
        ["mirror", "off", id, "--delete-copy"] => req("DELETE", format!("/mirrors/{}?delete_copy=true", vmid(id)?), None, View::Done("yansıtma kapatıldı ve eşteki kopya silindi")),
        ["mirror", "copies"] => get("/mirror-stores", View::MirrorStores),
        ["mirror", "promote", origin, id] => req("POST", format!("/mirror-stores/{origin}/{}/promote", vmid(id)?), None, View::Promoted),
        ["mirror", "delete-copy", origin, id] => req("DELETE", format!("/mirror-stores/{origin}/{}", vmid(id)?), None, View::Done("kopya silindi")),
        ["firewall", "set", "list"] => get("/firewall/sets", View::Json),
        ["firewall", "set", "save", name, entries] => req("POST", "/firewall/sets".to_string(), Some(json!({ "name": name, "entries": entries.split(',').map(str::trim).filter(|e| !e.is_empty()).collect::<Vec<_>>() })), View::Done("adres kümesi kaydedildi")),
        ["firewall", "set", "remove", name] => req("DELETE", format!("/firewall/sets/{name}"), None, View::Done("adres kümesi silindi")),
        ["firewall", "group", "list"] => get("/firewall/groups", View::Json),
        ["firewall", "group", "save", name, pairs @ ..] => req("POST", "/firewall/groups".to_string(), Some(fw_group_body(name, pairs)?), View::Done("güvenlik grubu kaydedildi")),
        ["firewall", "group", "remove", name] => req("DELETE", format!("/firewall/groups/{name}"), None, View::Done("güvenlik grubu silindi")),
        ["hostnet", "list"] => get("/host/networks", View::Json),
        ["hostnet", "remove", name] => req("DELETE", format!("/host/networks/{name}"), None, View::Done("sunucu ağı silindi")),
        ["hostnet", kind @ ("bridge" | "bond" | "vlan" | "guest"), rest @ ..] => req("POST", "/host/networks".to_string(), Some(hostnet_body(kind, rest)?), View::Done("sunucu ağı kaydedildi; ağı kök yardımcısı kurar")),
        ["shared", "list"] => get("/shared", View::Json),
        ["shared", "take", peer, id] => req("POST", format!("/shared/take/{peer}/{}", vmid(id)?), None, View::Done("konuk disk kopyalanmadan alındı")),
        ["shared", "adopt", storage, id] => req("POST", format!("/shared/adopt/{storage}/{}", vmid(id)?), None, View::Done("konuk devralındı")),
        ["switch", "list"] => get("/vswitches", View::Switches),
        ["switch", "set", name, vni, members, rest @ ..] => req("POST", "/vswitches".to_string(), Some(switch_body(name, vni, members, rest)?), View::Done("anahtar kaydedildi; eşlere göndermek için: redox switch push <ad>")),
        ["switch", "push", name] => req("POST", format!("/vswitches/{name}/push"), None, View::SwitchPeers),
        ["switch", "remove", name] => req("DELETE", format!("/vswitches/{name}"), None, View::Done("anahtar silindi")),
        ["switch", "remove", name, "--everywhere"] => req("DELETE", format!("/vswitches/{name}?everywhere=true"), None, View::SwitchPeers),
        ["peer", "send", peer, id] => req("POST", format!("/peers/{peer}/send/{}", vmid(id)?), None, View::Done("canlı taşıma başladı; sonuç: redox task list")),
        ["support"] => get("/support", View::Json),
        ["profile", "export"] => get("/profile", View::Json),
        ["profile", verb @ ("diff" | "apply"), file] => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file} okunamadı: {e}"))?;
            let profile: Value = serde_json::from_str(&text).map_err(|e| format!("{file} bir profil dosyası değil: {e}"))?;
            req("POST", format!("/profile/{verb}"), Some(profile), View::Profile)
        }
        ["peer", "migrate", peer, id] => req("POST", format!("/peers/{peer}/migrate/{}", vmid(id)?), None, View::Done("canlı taşıma başladı; sonuç: redox task list")),
        ["peer", "import", peer, id] => req("POST", format!("/peers/{peer}/import/{}", vmid(id)?), None, View::Done("kopyalama başladı; sonuç: redox task list")),
        ["task", "list"] => get("/tasks", View::Tasks),
        _ => Err(format!("anlaşılmayan komut: {}\n\n{USAGE}", a.join(" "))),
    }
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("-")
}

/// Anahtarın bu sunucudaki ve eşteki durumunun okunur adı.
fn switch_local(state: Option<&str>) -> &'static str {
    use crate::vswitch::status::Local;
    state.and_then(|s| [Local::Ready, Local::Missing, Local::Foreign, Local::Differs].into_iter().find(|l| serde_json::to_value(l).is_ok_and(|v| v == s))).map_or("benzetim (ağ kurulmaz)", Local::label)
}

fn switch_peer_state(state: &str) -> &'static str {
    use crate::state::vswitch::PeerState;
    [PeerState::Same, PeerState::Different, PeerState::Missing].into_iter().find(|p| serde_json::to_value(p).is_ok_and(|v| v == state)).unwrap_or(PeerState::Unknown).label()
}

/// `redox firewall group save` değerlerini API gövdesine çevirir: `in=…` ve `out=…`.
fn fw_group_body(name: &str, pairs: &[&str]) -> Result<Value, String> {
    let (mut rules, mut out) = (Vec::new(), Vec::new());
    for pair in pairs {
        match pair.split_once('=') {
            Some(("in", value)) => rules = redox_core::hw::parse_fw_rules(value)?,
            Some(("out", value)) => out = redox_core::hw::parse_fw_out_rules(value)?,
            _ => return Err(format!("\"{pair}\" anlaşılmadı; in='tcp/443' ya da out='udp/53' yazın")),
        }
    }
    if rules.is_empty() && out.is_empty() {
        return Err("en az bir kural verin: in='tcp/443' ya da out='udp/53'".to_string());
    }
    serde_json::to_value(redox_core::fwsets::FwGroup { name: name.to_string(), rules, out }).map_err(|e| e.to_string())
}

/// `redox hostnet …` değerlerini API gövdesine çevirir ve tanımı göndermeden önce doğrular.
fn hostnet_body(kind: &str, rest: &[&str]) -> Result<Value, String> {
    use redox_core::hostnet::{BondMode, HostNet};
    let usage = || "kullanım: redox hostnet bridge <ad> [ports=a,b] [vlan-aware=true] | bond <ad> <kip> <a,b> | vlan <arayüz> <etiket> | guest <ad> <adres/ön> [dhcp=baş-son] [nat=true]".to_string();
    let list = |text: &str| text.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let net = match (kind, rest) {
        ("bond", [name, mode, ports]) => HostNet::Bond { name: name.to_string(), mode: BondMode::parse(mode).ok_or_else(|| format!("bond kipi active-backup, 802.3ad ya da balance-xor olmalı: {mode}"))?, ports: list(ports) },
        ("vlan", [parent, id]) => HostNet::Vlan { name: format!("{parent}.{id}"), parent: parent.to_string(), id: id.parse().map_err(|_| format!("VLAN etiketi bir sayı olmalı: {id}"))? },
        ("bridge", [name, options @ ..]) => {
            let (mut ports, mut vlan_aware) = (Vec::new(), false);
            for option in options {
                match option.split_once('=') {
                    Some(("ports", v)) => ports = list(v),
                    Some(("vlan-aware", v @ ("true" | "false"))) => vlan_aware = v == "true",
                    _ => return Err(usage()),
                }
            }
            HostNet::Bridge { name: name.to_string(), ports, vlan_aware }
        }
        ("guest", [name, address, options @ ..]) => {
            let (mut dhcp_start, mut dhcp_end, mut nat) = (None, None, false);
            for option in options {
                match option.split_once('=') {
                    Some(("dhcp", v)) => {
                        let (a, b) = v.split_once('-').ok_or_else(usage)?;
                        let address = |t: &str| t.parse().map_err(|_| format!("geçersiz adres: {t}"));
                        (dhcp_start, dhcp_end) = (Some(address(a)?), Some(address(b)?));
                    }
                    Some(("nat", v @ ("true" | "false"))) => nat = v == "true",
                    _ => return Err(usage()),
                }
            }
            HostNet::Guest { name: name.to_string(), address: address.to_string(), dhcp_start, dhcp_end, nat }
        }
        _ => return Err(usage()),
    };
    net.check()?;
    serde_json::to_value(net).map_err(|e| e.to_string())
}

/// `redox switch set` değerlerini API gövdesine çevirir.
fn switch_body(name: &str, vni: &str, members: &str, rest: &[&str]) -> Result<Value, String> {
    let vni: u32 = vni.parse().map_err(|_| format!("VNI bir sayı olmalı: {vni}"))?;
    let mtu: u32 = match rest {
        [] => redox_core::vswitch::DEFAULT_MTU,
        ["--mtu", value] => value.parse().map_err(|_| format!("MTU bir sayı olmalı: {value}"))?,
        _ => return Err(format!("anlaşılmayan seçenek: {}", rest.join(" "))),
    };
    let members: Vec<&str> = members.split(',').map(str::trim).filter(|m| !m.is_empty()).collect();
    Ok(json!({ "name": name, "vni": vni, "mtu": mtu, "members": members }))
}

fn n(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

/// API yanıtını terminal için düz metne çevirir.
pub fn render(view: View, v: &Value) -> String {
    let rows = |f: &dyn Fn(&Value) -> String| -> String {
        v.as_array().map(|a| a.iter().map(f).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    };
    let guest = |g: &Value| {
        format!(
            "{:<6} {:<16} {:<3} {:<12} {:<8} cpu {:>3}%  bellek {}/{} GB",
            n(g, "vmid"), s(g, "name"), s(g, "kind"), s(g, "node"), s(g, "status"),
            n(g, "cpu_pct"), mb_as_gb(n(g, "mem_used_mb")), mb_as_gb(n(g, "mem_total_mb")),
        )
    };
    let node = |x: &Value| {
        format!(
            "{:<12} {:<8} cpu {:>3}%  bellek {}/{} GB  {} çekirdek  {}",
            s(x, "id"), s(x, "status"), n(x, "cpu_pct"),
            mb_as_gb(n(x, "mem_used_mb")), mb_as_gb(n(x, "mem_total_mb")), n(x, "cores"), s(x, "ip"),
        )
    };
    let or_none = |text: String, empty: &str| if text.is_empty() { empty.to_string() } else { text };
    match view {
        View::Cluster => format!(
            "küme      {}\ndüğümler  {}/{} çevrimiçi{}\ncpu       {}% ({} çekirdek)\nbellek    {}% ({}/{} GB)\ndepolama  {}% ({} / {})\nkonuklar  {}/{} çalışıyor",
            s(v, "name"), n(v, "nodes_online"), n(v, "nodes_total"),
            if v["quorate"].as_bool() == Some(true) { ", çoğunluk var" } else { ", ÇOĞUNLUK YOK" },
            n(v, "cpu_pct"), n(v, "cores"),
            n(v, "mem_pct"), mb_as_gb(n(v, "mem_used_mb")), mb_as_gb(n(v, "mem_total_mb")),
            n(v, "storage_pct"), gb(n(v, "storage_used_gb")), gb(n(v, "storage_total_gb")),
            n(v, "guests_running"), n(v, "guests_total"),
        ),
        View::Nodes => rows(&node),
        View::Node => node(v),
        View::Guests(kind) => v
            .as_array()
            .map(|a| a.iter().filter(|g| s(g, "kind") == kind).map(guest).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default(),
        View::Guest => {
            let list = |key: &str| v[key].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            let forwards = v["forwards"]
                .as_array()
                .map(|a| a.iter().map(|f| format!("{}:{}", n(f, "host_port"), n(f, "guest_port"))).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let schedule = match v["backup_time"].as_str() {
                Some(t) => format!("her gün {t}, son {} yedek", n(v, "backup_keep")),
                None => "kapalı".to_string(),
            };
            format!(
                "{}\nişletim sistemi  {}\nçekirdek         {}\ndisk             {} GB ({})\nağ               {}  ip {}\netiketler        {}\nport yönlendirme {}\notomatik yedek   {}",
                guest(v), s(v, "os"), n(v, "cores"), n(v, "disk_gb"), s(v, "storage"), s(v, "bridge"), s(v, "ip"),
                or_none(list("tags"), "-"), or_none(forwards, "-"), schedule,
            )
        }
        View::Storage => rows(&|x| {
            format!(
                "{:<10} {:<12} {:<9} {} / {}",
                s(x, "id"), x["node"].as_str().unwrap_or("paylaşımlı"), s(x, "kind"),
                gb(n(x, "used_gb")), gb(n(x, "total_gb")),
            )
        }),
        View::Peers => or_none(
            rows(&|x| {
                let state = match (x["error"].as_str(), x["checked"].is_null()) {
                    (Some(e), _) => format!("HATA: {e}"),
                    (None, true) => "bekleniyor".to_string(),
                    (None, false) => format!("{} düğüm, {} konuk", x["nodes"].as_array().map_or(0, Vec::len), x["guests"].as_array().map_or(0, Vec::len)),
                };
                format!("{:<14} {:<36} {state}", s(x, "name"), s(x, "url"))
            }),
            "eş sunucu tanımlı değil (redox serve --peers DOSYA)",
        ),
        View::Tasks => rows(&|t| {
            format!(
                "#{:<4} {:<7} {:<12} {:<10} redox {}{}",
                n(t, "id"), s(t, "status"), s(t, "node"), s(t, "user"), s(t, "command"),
                t["message"].as_str().map(|m| format!("  ({m})")).unwrap_or_default()
            )
        }),
        View::Snapshots => or_none(
            v["snapshots"]
                .as_array()
                .map(|a| a.iter().map(|x| format!("{:<32} {}", s(x, "name"), n(x, "created"))).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default(),
            "anlık görüntü yok",
        ),
        View::Backups => or_none(
            rows(&|b| {
                let kind = if b["auto"].as_bool() == Some(true) { "otomatik" } else { "elle" };
                let offsite = if b["offsite"].as_bool() == Some(true) { "  dış kopya var" } else { "" };
                let added = b["new_mb"].as_u64().map(|mb| format!("  artımlı (+{mb} MB yeni)")).unwrap_or_default();
                let frozen = if b["quiesced"].as_bool() == Some(true) { "  donduruldu" } else { "" };
                let frozen = format!("{frozen}{}", if b["cbt"].as_bool() == Some(true) { "  hızlı" } else { "" });
                let disks = b["disks"].as_array().filter(|d| !d.is_empty()).map(|d| format!("  +{} ek disk", d.len())).unwrap_or_default();
                let odd = if b["suspicious"].is_string() { "  OLAĞAN DIŞI DEĞİŞİM" } else { "" };
                format!("{:<28} konuk {:<6} {:>7} MB  {kind}{added}{disks}{frozen}{offsite}{odd}", s(b, "id"), n(b, "vmid"), n(b, "size_mb"))
            }),
            "yedek yok",
        ),
        View::Media => or_none(
            rows(&|m| format!("{:<44} {:<8} {:>8} MB", s(m, "name"), if m["iso"].as_bool() == Some(true) { "kalıp" } else { "imaj" }, m["bytes"].as_u64().unwrap_or(0).div_ceil(1024 * 1024))),
            "dosya yok",
        ),
        View::Report => s(v, "body").trim_end().to_string(),
        View::Files => or_none(
            v["entries"].as_array().map(|list| list.iter().map(|e| format!("{} {}", if s(e, "kind") == "d" { "d" } else { "-" }, s(e, "name"))).collect::<Vec<_>>().join("\n")).unwrap_or_default(),
            "dizin boş",
        ),
        View::Usage => {
            let line = |u: &Value| {
                format!(
                    "{:<6} {:<20} açık {:>7} sa   işlemci {:>9} çekirdek-sa   bellek {:>9} GB-sa   disk okuma {:>7} MB   yazma {:>7} MB   ağ alınan {:>7} MB   gönderilen {:>7} MB",
                    n(u, "vmid"),
                    s(u, "name"),
                    n(u, "uptime_secs") / 3600,
                    u["cpu_core_hours"].as_f64().unwrap_or(0.0),
                    u["mem_gb_hours"].as_f64().unwrap_or(0.0),
                    n(u, "disk_read_mb"),
                    n(u, "disk_write_mb"),
                    n(u, "net_rx_mb"),
                    n(u, "net_tx_mb")
                )
            };
            match v.as_array() {
                Some(all) if all.is_empty() => "konuk yok".to_string(),
                Some(all) => all.iter().map(line).collect::<Vec<_>>().join("\n"),
                None => line(v),
            }
        }
        View::Replicas => or_none(
            rows(&|r| {
                let state = match (r["error"].as_str(), r["last_ok"].as_u64()) {
                    (Some(e), _) => format!("HATA: {e}"),
                    (None, Some(_)) => match r["delta"].as_array().filter(|d| d.len() == 2) {
                        Some(d) => format!("yedek kopya: {}  (son tur: {}/{} blok indirildi)", n(r, "standby"), d[0], d[1]),
                        None => format!("yedek kopya: {}", n(r, "standby")),
                    },
                    (None, None) => "henüz çoğaltılmadı".to_string(),
                };
                let failover = match (r["failed_over"].as_u64(), r["failover_min"].as_u64(), r["failover_note"].as_str()) {
                    (Some(_), _, _) => "   DEVRALINDI".to_string(),
                    (None, Some(m), Some(note)) => format!("   devralma: {m} dk (engellendi: {note})"),
                    (None, Some(m), None) => format!("   devralma: {m} dk"),
                    (None, None, _) => String::new(),
                };
                format!("{:<14} {:<6} her {:>5} dk   {state}{failover}", s(r, "peer"), n(r, "source"), n(r, "every_min"))
            }),
            "çoğaltma yok",
        ),
        View::Json => serde_json::to_string_pretty(v).unwrap_or_default(),
        View::Switches => or_none(
            rows(&|x| {
                let list = |key: &str| x[key].as_array().map(|a| a.iter().map(|i| i.as_str().map_or_else(|| i.to_string(), str::to_string)).collect::<Vec<_>>().join(",")).unwrap_or_default();
                let peers: String = x["peers"].as_array().into_iter().flatten().map(|p| format!("\n  eş {}: {}", p[0].as_str().unwrap_or("?"), switch_peer_state(p[1].as_str().unwrap_or("")))).collect();
                format!("{:<15} VNI {:<8} MTU {}\n  bu sunucu: {}\n  üyeler: {}\n  konuklar: {}{peers}", s(x, "name"), n(x, "vni"), n(x, "mtu"), switch_local(x["local"].as_str()), list("members"), list("guests"))
            }),
            "dağıtık anahtar yok",
        ),
        View::SwitchPeers => v["peers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| if p["ok"] == true { format!("{}: tamam", s(p, "peer")) } else { format!("{}: {}", s(p, "peer"), s(p, "error")) })
            .collect::<Vec<_>>()
            .join("\n"),
        View::Mirrors => or_none(
            rows(&|m| {
                let state = serde_json::from_value::<redox_core::MirrorState>(m["state"].clone()).map_or("bilinmiyor", redox_core::MirrorState::label);
                let why = m["error"].as_str().map(|e| format!("\n  neden: {e}")).unwrap_or_default();
                format!("{:<6} -> {:<12} {state}{why}", n(m, "vmid"), s(m, "peer"))
            }),
            "yansıtılan disk yok",
        ),
        View::MirrorStores => or_none(
            rows(&|x| {
                let state = match (x["complete"].is_null(), x["in_sync"] == true) {
                    (true, _) => "ilk kopya tamamlanmadı",
                    (false, true) => "kaynakla eşit",
                    (false, false) => "kaynak eşit bildirmiyor",
                };
                format!("{:<14} {:<6} {:<16} {state}", s(x, "origin"), n(x, "vmid"), s(x, "name"))
            }),
            "tutulan kopya yok",
        ),
        View::Promoted => format!("kopyadan {} numaralı konuk oluşturuldu; başlatmak için: redox vm start {}", n(v, "vmid"), n(v, "vmid")),
        View::Protections => or_none(
            rows(&|p| {
                let state = serde_json::from_value::<redox_core::FtState>(p["state"].clone()).map_or("bilinmiyor", redox_core::FtState::label);
                let why = p["error"].as_str().map(|e| format!("\n  neden: {e}")).unwrap_or_default();
                format!("{:<6} {state}\n  ikiz sunucusu: {}\n  hakem: {}{why}", n(p, "vmid"), s(p, "peer"), s(p, "witness"))
            }),
            "korunan konuk yok",
        ),
        View::Standbys => or_none(rows(&|x| format!("{:<6} <- {}/{}\n  hakem: {}", n(x, "vmid"), s(x, "origin"), n(x, "source"), s(x, "witness"))), "burada ikiz yok"),
        View::Balance => {
            let head = format!("işlemci %{}  bellek %{}", n(v, "cpu_pct"), n(v, "mem_pct"));
            let moves: Vec<String> = v["moves"].as_array().into_iter().flatten().map(|m| format!("  {:<6} {:<16} -> {:<12} ({}; sonra burada %{}, eşte %{})   redox peer send {} {}", n(m, "vmid"), s(m, "name"), s(m, "peer"), s(m, "reason"), n(m, "here_mem_pct"), n(m, "peer_mem_pct"), s(m, "peer"), n(m, "vmid"))).collect();
            let broken: String = v["violations"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|v| format!("\n  kural bozuk: {v}")).collect();
            let busy = v["busy"].as_bool() == Some(true);
            if !busy && !broken.is_empty() {
                let advice = if moves.is_empty() { "taşınacak uygun eş yok".to_string() } else { format!("öneriler:\n{}", moves.join("\n")) };
                return format!("{head}:{broken}\n{advice}");
            }
            let advice = match (busy, moves.is_empty()) {
                (false, _) => format!("{head}: sunucu sıkışmış değil, öneri yok"),
                (true, true) => format!("{head}: sunucu sıkışmış, ama öneri üretilemedi: {}", s(v, "note")),
                (true, false) => format!("{head}: sunucu sıkışmış. Öneriler:\n{}", moves.join("\n")),
            };
            format!("{advice}{broken}")
        }
        View::Profile => {
            let lines = |key: &str| v[key].as_array().map(|all| all.iter().filter_map(Value::as_str).map(|l| format!("  {l}")).collect::<Vec<_>>()).unwrap_or_default();
            if v["differences"].is_array() {
                let all = lines("differences");
                if all.is_empty() { "fark yok: bu sunucu profille aynı".to_string() } else { format!("{} fark:\n{}", all.len(), all.join("\n")) }
            } else {
                let (applied, manual) = (lines("applied"), lines("manual"));
                let mut out = if applied.is_empty() { "uygulanacak değişiklik yoktu".to_string() } else { format!("uygulandı:\n{}", applied.join("\n")) };
                if !manual.is_empty() {
                    out.push_str(&format!("\nelle düzeltilecek (çalışırken değişmez ya da profilde yok):\n{}", manual.join("\n")));
                }
                out
            }
        }
        View::Lockdown => if v["on"].as_bool() == Some(true) { "kilit kipi açık: değişiklik yapılamaz" } else { "kilit kipi kapalı" }.to_string(),
        View::Maintenance => match (v["on"].as_bool() == Some(true), v["guests"].as_u64(), v["resume"].as_array().map(Vec::len)) {
            (true, _, _) if v["evacuating"].is_u64() => format!("bakım kipi açık; {} sanal makine kapatılmadan {} sunucusuna taşınıyor (sonuç: redox task list)", v["evacuating"], s(v, "peer")),
            (true, Some(stopped), _) => format!("bakım kipi açık; {stopped} konuk kapatılıyor"),
            (false, Some(started), _) => format!("bakım kipi kapalı; {started} konuk yeniden başlatılıyor"),
            (true, None, waiting) => format!("bakım kipi açık; bakım bitince başlatılacak konuk: {}", waiting.unwrap_or(0)),
            (false, None, _) => "bakım kipi kapalı".to_string(),
        },
        View::Catalog => rows(&|e| format!("{:<14} {:<28} {}", s(e, "key"), s(e, "label"), s(e, "file"))),
        View::Filesystems => or_none(
            rows(&|f| {
                let (used, total) = (n(f, "used_mb"), n(f, "total_mb"));
                format!("{:<28} {:>8} / {:>8} MB   %{}", s(f, "mount"), used, total, redox_core::fmt::pct(used, total))
            }),
            "ölçüm yok (konuk çalışmıyor ya da konuk ajanı kurulu değil)",
        ),
        View::Approvals => or_none(rows(&|a| format!("#{:<4} {:<14} {:<24} isteyen {}", n(a, "id"), s(a, "action"), s(a, "target"), s(a, "by"))), "onay bekleyen istek yok"),
        View::Approved => s(v, "done").to_string(),
        View::Pools => or_none(
            rows(&|p| {
                let limit = |key: &str, unit: &str| match n(p, key) {
                    0 => format!("{}{unit} / sınırsız", n(&p["used"], key)),
                    max => format!("{}{unit} / {max}{unit}", n(&p["used"], key)),
                };
                format!("{:<16} konuk {:<14} çekirdek {:<14} bellek {:<26} disk {}", s(p, "name"), limit("guests", ""), limit("cores", ""), limit("memory_mb", " MB"), limit("disk_gb", " GB"))
            }),
            "havuz yok",
        ),
        View::Jobs => or_none(
            rows(&|j| {
                let tags: Vec<&str> = j["tags"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let scope = if tags.is_empty() { "bütün konuklar".to_string() } else { format!("etiket: {}", tags.join(",")) };
                let queue = j["queue"].as_array().map_or(0, Vec::len) + usize::from(!j["running"].is_null());
                let state = if queue > 0 { format!("sürüyor ({queue} konuk kaldı)") } else { format!("son: {}", s(j, "last_run")) };
                format!("{:<16} {}  {:<12} son {:<3} {:<28} {state}", s(j, "name"), s(j, "time"), redox_core::hw::days_label(n(j, "days") as u8), n(j, "keep"), scope)
            }),
            "yedek işi yok",
        ),
        View::JobStarted => format!("iş başlatıldı: {} konuk sıraya alındı; sonuç: redox task list", n(v, "queued")),
        View::Task => format!("görev #{} başlatıldı: redox {}", n(v, "id"), s(v, "command")),
        View::BackupStarted => format!("yedek alınıyor: {}", s(v, "id")),
        View::Cloned => format!("klonlanıyor: yeni konuk {}", n(v, "vmid")),
        View::Output => v["output"].as_str().unwrap_or_default().trim_end().to_string(),
        View::Done(text) => text.to_string(),
    }
}

fn base_url() -> String {
    std::env::var("REDOX_URL").unwrap_or_else(|_| format!("http://{DEFAULT_ADDR}"))
}

fn api_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => "hata (401): kimlik doğrulanamadı. REDOX_TOKEN değişkenine geçerli bir API token'ı verin (üretmek için: redox token create <kullanıcı> <etiket>)".to_string(),
        ureq::Error::Status(code, res) => {
            let body: Value = res.into_json().unwrap_or(Value::Null);
            format!("hata ({code}): {}", body["error"].as_str().unwrap_or("sunucu isteği reddetti"))
        }
        ureq::Error::Transport(t) => format!(
            "sunucuya bağlanılamadı ({}): {t}\nSunucu çalışıyor mu? Başlatmak için: redox serve",
            base_url()
        ),
    }
}

/// İstemci komutunu çalıştırır ve yazdırılacak metni döndürür.
pub fn run(args: &[String]) -> Result<String, String> {
    let request = parse(args)?;
    let url = format!("{}/api/v1{}", base_url().trim_end_matches('/'), request.path);
    let token = std::env::var("REDOX_TOKEN").unwrap_or_default();
    let call = ureq::request(request.method, &url).set("Authorization", &format!("Bearer {token}"));
    let response = match &request.body {
        Some(body) => call.send_json(body),
        None => call.call(),
    }
    .map_err(api_error)?;
    if let View::Done(_) = request.view {
        // Dört göz ilkesi: silme yapılmadı, ikinci yöneticinin onayını bekliyor.
        if response.status() == 202 {
            if let Some(id) = response.into_json::<Value>().ok().and_then(|v| v["approval"].as_u64()) {
                return Ok(format!("yapılmadı: ikinci bir yöneticinin onayı bekleniyor (istek #{id}). Onay: redox approval approve {id}"));
            }
            return Ok(render(request.view, &Value::Null));
        }
        return Ok(render(request.view, &Value::Null));
    }
    let value: Value = response.into_json().map_err(|e| format!("sunucu yanıtı okunamadı: {e}"))?;
    Ok(render(request.view, &value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn parsed(s: &str) -> (&'static str, String, Option<Value>, View) {
        let r = parse(&args(s)).unwrap_or_else(|e| panic!("{s}: {e}"));
        (r.method, r.path, r.body, r.view)
    }

    #[test]
    fn parses_commands_to_api_calls() {
        assert_eq!(parsed("cluster status"), ("GET", "/cluster".into(), None, View::Cluster));
        assert_eq!(parsed("ct list"), ("GET", "/guests".into(), None, View::Guests("ct")));
        assert_eq!(parsed("vm show 102"), ("GET", "/guests/102".into(), None, View::Guest));
        assert_eq!(parsed("ct reboot 201"), ("POST", "/guests/201/reboot".into(), None, View::Task));
        assert_eq!(parsed("vm destroy 103"), ("DELETE", "/guests/103".into(), None, View::Done("silindi")));
        assert_eq!(parsed("node status pve-ist-01"), ("GET", "/nodes/pve-ist-01".into(), None, View::Node));
    }

    #[test]
    fn parses_operations_with_bodies() {
        assert_eq!(parsed("vm clone 103 kopya"), ("POST", "/guests/103/clone".into(), Some(json!({"name": "kopya"})), View::Cloned));
        assert_eq!(parsed("vm resize 103 300G"), ("POST", "/guests/103/resize".into(), Some(json!({"disk_gb": 300})), View::Task));
        assert_eq!(parsed("ct exec 200 ls -la /etc"), ("POST", "/guests/200/exec".into(), Some(json!({"command": "ls -la /etc"})), View::Output));
        assert_eq!(parsed("snapshot create 103 temiz"), ("POST", "/guests/103/snapshots".into(), Some(json!({"name": "temiz"})), View::Task));
        assert_eq!(parsed("snapshot rollback 103 temiz"), ("POST", "/guests/103/snapshots/temiz/rollback".into(), None, View::Task));
        assert_eq!(parsed("snapshot delete 103 temiz"), ("DELETE", "/guests/103/snapshots/temiz".into(), None, View::Task));
        assert_eq!(parsed("snapshot list 103"), ("GET", "/guests/103".into(), None, View::Snapshots));
        assert_eq!(parsed("backup create 103"), ("POST", "/guests/103/backups".into(), None, View::BackupStarted));
        assert_eq!(parsed("backup restore vm-103-1"), ("POST", "/backups/vm-103-1/restore".into(), None, View::Task));
        assert_eq!(parsed("backup delete vm-103-1"), ("DELETE", "/backups/vm-103-1".into(), None, View::Done("yedek silindi")));
        assert_eq!(parsed("backup list"), ("GET", "/backups".into(), None, View::Backups));
        assert_eq!(parsed("vm nic add 103 vmbr1"), ("POST", "/guests/103/nics".into(), Some(json!({ "network": "vmbr1" })), View::Done("ağ kartı eklendi")));
        assert_eq!(parsed("backup verify vm-103-1").1, "/backups/vm-103-1/verify");
    }

    #[test]
    fn set_builds_a_typed_patch() {
        let (method, path, body, _) = parsed("vm set 103 cores=4 memory=8192 name=derleme tags=Prod,web onboot=false forwards=8080:80,2222:22 backup-time=03:30 backup-keep=5 bridge=nat iso=");
        assert_eq!((method, path.as_str()), ("PATCH", "/guests/103"));
        assert_eq!(
            body.unwrap(),
            json!({
                "cores": 4, "memory_mb": 8192, "name": "derleme", "tags": ["prod", "web"], "onboot": false,
                "forwards": [{"host_port": 8080, "guest_port": 80}, {"host_port": 2222, "guest_port": 22}],
                "backup_time": "03:30", "backup_keep": 5, "bridge": "nat", "iso": ""
            })
        );
        let (_, _, body, _) = parsed("vm set 103 boot-order=2 boot-delay=30 schedules=start@08:00@1-5,stop@20:00");
        assert_eq!(
            body.unwrap(),
            json!({ "boot_order": 2, "boot_delay": 30, "schedules": [{"action": "start", "time": "08:00", "days": 31}, {"action": "stop", "time": "20:00", "days": 127}] })
        );
        assert_eq!(parsed("vm set 103 schedules=").2.unwrap(), json!({ "schedules": [] }), "boş liste zamanlamayı kaldırır");
        assert_eq!(parsed("vm set 103 restart=true").2.unwrap(), json!({ "restart": true }));
        assert_eq!(parsed("vm set 103 disk-mbps=50 disk-iops=2000 net-mbps=100").2.unwrap(), json!({ "disk_mbps": 50, "disk_iops": 2000, "net_mbps": 100 }));
        assert_eq!(parsed("vm set 103 vlan=20 nics=vmbr1:30,nat").2.unwrap(), json!({ "vlan": 20, "nics": ["vmbr1:30", "nat"] }));
        assert_eq!(parsed("vm set 103 shares=ortak,veri:ro").2.unwrap(), json!({ "shares": [{"name": "ortak", "readonly": false}, {"name": "veri", "readonly": true}] }));
        for (bad, needle) in [
            ("vm set 103 schedules=pause@08:00", "geçersiz zamanlama"),
            ("vm set 103 shares=../x", "geçersiz paylaşım"),
            ("vm set 103 schedules=start@8", "geçersiz zamanlama"),
            ("vm set 103 boot-order=ilk", "bir sayı olmalı"),
            ("vm set 103", "en az bir"),
            ("vm set 103 cores", "anahtar=değer biçiminde değil"),
            ("vm set 103 cores=dört", "cores bir sayı olmalı"),
            ("vm set 103 renk=mavi", "bilinmeyen anahtar"),
            ("vm set 103 onboot=belki", "true ya da false"),
            ("vm set 103 forwards=8080", "geçersiz yönlendirme"),
        ] {
            let err = parse(&args(bad)).unwrap_err();
            assert!(err.contains(needle), "{bad}: {err}");
        }
    }

    #[test]
    fn rejects_bad_input_with_usage() {
        assert!(parse(&args("vm start abc")).unwrap_err().contains("kimlik bir sayı olmalı"));
        assert!(parse(&args("vm explode 100")).unwrap_err().contains("redox serve"));
        assert!(parse(&[]).unwrap_err().contains("anlaşılmayan komut"));
        assert!(parse(&args("ct exec 200")).unwrap_err().contains("anlaşılmayan komut"), "komutsuz exec");
        assert!(parse(&args("vm resize 103 büyük")).unwrap_err().contains("boyut GB"));
        assert!(parse(&args("ct resize 200 10")).is_err(), "konteynerde disk büyütme yok");
    }

    #[test]
    fn renders_guest_list_filtered_by_kind() {
        let v = json!([
            {"vmid": 100, "name": "web", "kind": "vm", "node": "n1", "status": "running", "cpu_pct": 23, "mem_used_mb": 5325, "mem_total_mb": 8192},
            {"vmid": 200, "name": "dns", "kind": "ct", "node": "n1", "status": "stopped", "cpu_pct": 0, "mem_used_mb": 0, "mem_total_mb": 512},
        ]);
        let out = render(View::Guests("ct"), &v);
        assert!(out.contains("200") && out.contains("dns") && out.contains("0/0.5 GB"), "{out}");
        assert!(!out.contains("web"));
    }

    #[test]
    fn renders_cluster_summary() {
        let v = json!({"name": "ist", "nodes_online": 2, "nodes_total": 3, "quorate": true, "cpu_pct": 40, "cores": 80,
            "mem_pct": 63, "mem_used_mb": 205824, "mem_total_mb": 327680, "storage_pct": 77,
            "storage_used_gb": 9612, "storage_total_gb": 12500, "guests_running": 5, "guests_total": 8});
        let out = render(View::Cluster, &v);
        assert!(out.contains("2/3 çevrimiçi, çoğunluk var"), "{out}");
        assert!(out.contains("77% (9.6 TB / 12.5 TB)"), "{out}");
    }

    #[test]
    fn renders_operation_results() {
        assert_eq!(render(View::Task, &json!({"id": 7, "command": "vm snapshot 103 temiz"})), "görev #7 başlatıldı: redox vm snapshot 103 temiz");
        assert_eq!(render(View::Cloned, &json!({"vmid": 105})), "klonlanıyor: yeni konuk 105");
        assert_eq!(render(View::BackupStarted, &json!({"id": "vm-103-1"})), "yedek alınıyor: vm-103-1");
        assert_eq!(render(View::Output, &json!({"output": "merhaba\n\n"})), "merhaba");
        assert_eq!(render(View::Snapshots, &json!({"snapshots": []})), "anlık görüntü yok");
        assert!(render(View::Snapshots, &json!({"snapshots": [{"name": "temiz", "created": 5}]})).starts_with("temiz"));
        assert_eq!(render(View::Backups, &json!([])), "yedek yok");
        let b = render(View::Backups, &json!([{"id": "vm-103-1", "vmid": 103, "size_mb": 42, "auto": true}]));
        assert!(b.contains("vm-103-1") && b.contains("42 MB") && b.contains("otomatik") && !b.contains("artımlı"), "{b}");
        let b = render(View::Backups, &json!([{"id": "vm-103-2", "vmid": 103, "size_mb": 900, "auto": false, "new_mb": 12}]));
        assert!(b.contains("900 MB") && b.contains("artımlı (+12 MB yeni)") && !b.contains("ek disk"), "{b}");
        let b = render(View::Backups, &json!([{"id": "vm-103-3", "vmid": 103, "size_mb": 5, "auto": false, "disks": [{"id": 1, "size_gb": 10}, {"id": 2, "size_gb": 20}]}]));
        assert!(b.contains("+2 ek disk"), "{b}");
        let peers = render(View::Peers, &json!([{"name": "ankara", "url": "https://a.example.com", "checked": 5, "error": null, "nodes": [{}], "guests": [{}, {}]},
            {"name": "izmir", "url": "https://i.example.com", "checked": 5, "error": "token reddedildi (401)"}, {"name": "yeni", "url": "https://y.example.com", "checked": null}]));
        assert!(peers.contains("1 düğüm, 2 konuk") && peers.contains("HATA: token reddedildi") && peers.contains("bekleniyor"), "{peers}");
        assert!(render(View::Peers, &json!([])).contains("tanımlı değil"));
        assert_eq!(parse(&["peer".to_string(), "list".to_string()]).unwrap().path, "/peers");
        assert_eq!(parse(&args("support")).unwrap().path, "/support");
        assert_eq!(parse(&args("balance")).unwrap().path, "/balance");
        assert_eq!(parse(&args("peer send ankara 103")).unwrap().path, "/peers/ankara/send/103");
        assert_eq!(render(View::Balance, &json!({"cpu_pct": 20, "mem_pct": 30, "busy": false, "moves": []})), "işlemci %20  bellek %30: sunucu sıkışmış değil, öneri yok");
        let advice = render(View::Balance, &json!({"cpu_pct": 20, "mem_pct": 90, "busy": true, "moves": [{"vmid": 103, "name": "db", "peer": "ankara", "reason": "6000 MB bellek kullanıyor", "here_mem_pct": 75, "peer_mem_pct": 40}]}));
        assert!(advice.contains("sunucu sıkışmış. Öneriler:") && advice.contains("redox peer send ankara 103"), "{advice}");
        let english = crate::i18n::lines_to_english(&advice);
        assert!(english.contains("the server is busy. Advice:") && english.contains("(uses 6000 MB of memory; afterwards 75% here, 40% on the peer)   redox peer send ankara 103"), "{english}");
        let set = parse(&args("switch set vds-web 100 10.0.0.1,10.0.0.2 --mtu 1400")).unwrap();
        assert_eq!((set.path.as_str(), set.body.clone().unwrap()), ("/vswitches", json!({"name": "vds-web", "vni": 100, "mtu": 1400, "members": ["10.0.0.1", "10.0.0.2"]})));
        assert_eq!(parse(&args("switch push vds-web")).unwrap().path, "/vswitches/vds-web/push");
        assert_eq!(parse(&args("switch remove vds-web --everywhere")).unwrap().path, "/vswitches/vds-web?everywhere=true");
        assert!(parse(&args("switch set vds-web yuz 10.0.0.1,10.0.0.2")).is_err() && parse(&args("switch set vds-web 1 10.0.0.1 --hiz 5")).is_err());
        let listed = render(View::Switches, &json!([{"name": "vds-web", "vni": 100, "mtu": 1450, "members": ["10.0.0.1", "10.0.0.2"], "local": "missing", "guests": [100, 101], "peers": [["ankara", "same"], ["izmir", "different"]]}]));
        for part in ["vds-web         VNI 100      MTU 1450", "bu sunucu: kurulmadı", "üyeler: 10.0.0.1,10.0.0.2", "konuklar: 100,101", "eş ankara: aynı", "eş izmir: tanımı farklı"] {
            assert!(listed.contains(part), "{part}\n{listed}");
        }
        let english = crate::i18n::lines_to_english(&listed);
        for part in ["this server: not set up", "members: 10.0.0.1,10.0.0.2", "guests: 100,101", "peer ankara: same", "peer izmir: definition differs"] {
            assert!(english.contains(part), "{part}\n{english}");
        }
        assert_eq!(render(View::SwitchPeers, &json!({"peers": [{"peer": "ankara", "ok": true}, {"peer": "izmir", "ok": false, "error": "erişilemiyor"}]})), "ankara: tamam\nizmir: erişilemiyor");
        let on = parse(&args("mirror add 103 ankara")).unwrap();
        assert_eq!((on.path.as_str(), on.body.clone().unwrap()), ("/mirrors", json!({"vmid": 103, "peer": "ankara"})));
        assert_eq!(parse(&args("mirror off 103 --delete-copy")).unwrap().path, "/mirrors/103?delete_copy=true");
        assert_eq!(parse(&args("mirror promote pve-ist-01 103")).unwrap().path, "/mirror-stores/pve-ist-01/103/promote");
        assert!(parse(&args("mirror add yuz ankara")).is_err());
        let listed = render(View::Mirrors, &json!([{"vmid": 103, "peer": "ankara", "state": "in_sync", "error": null}, {"vmid": 104, "peer": "izmir", "state": "broken", "error": "yansıtma koptu"}]));
        assert!(listed.contains("103    -> ankara       eşit") && listed.contains("koptu\n  neden: yansıtma koptu"), "{listed}");
        let held = render(View::MirrorStores, &json!([{"origin": "pve-ist-01", "vmid": 103, "name": "db", "complete": 2, "in_sync": false}, {"origin": "pve-ist-01", "vmid": 104, "name": "web", "complete": null, "in_sync": false}]));
        assert!(held.contains("kaynak eşit bildirmiyor") && held.contains("ilk kopya tamamlanmadı"), "{held}");
        assert!(render(View::Promoted, &json!({"vmid": 120})).contains("redox vm start 120"));
        let english = crate::i18n::lines_to_english(&listed);
        assert!(english.contains("in sync") && english.contains("reason: "), "{english}");
        let on = parse(&args("ft on 103 ankara izmir")).unwrap();
        assert_eq!((on.path.as_str(), on.body.clone().unwrap()), ("/ft/protections", json!({"vmid": 103, "peer": "ankara", "witness": "izmir"})));
        assert_eq!(parse(&args("ft off 103")).unwrap().path, "/ft/protections/103");
        assert!(parse(&args("ft on x ankara izmir")).is_err() && parse(&args("ft on 103 ankara")).is_err());
        let listed = render(View::Protections, &json!([{"vmid": 103, "peer": "ankara", "witness": "izmir", "state": "unprotected", "error": "ikiz kayboldu"}]));
        assert!(listed.contains("korumasız") && listed.contains("ikiz sunucusu: ankara") && listed.contains("hakem: izmir") && listed.contains("neden: ikiz kayboldu"), "{listed}");
        let english = crate::i18n::lines_to_english(&listed);
        assert!(english.contains("unprotected") && english.contains("twin server: ankara") && english.contains("witness: izmir"), "{english}");
        assert!(render(View::Standbys, &json!([{"vmid": 103, "origin": "pve-a", "source": 103, "witness": "izmir"}])).contains("<- pve-a/103"));
        assert_eq!(parse(&args("profile export")).unwrap().path, "/profile");
        let file = std::env::temp_dir().join(format!("redox-profil-{}.json", std::process::id()));
        std::fs::write(&file, r#"{"kind":"redox-profile","options":{}}"#).unwrap();
        let diff = parse(&["profile".to_string(), "diff".to_string(), file.display().to_string()]).unwrap();
        assert_eq!((diff.method, diff.path.as_str(), diff.body.unwrap()["kind"].as_str().map(str::to_string)), ("POST", "/profile/diff", Some("redox-profile".to_string())));
        std::fs::write(&file, "bozuk").unwrap();
        assert!(parse(&["profile".to_string(), "apply".to_string(), file.display().to_string()]).unwrap_err().contains("profil dosyası değil"));
        std::fs::remove_file(&file).unwrap();
        assert!(parse(&args("profile diff /yok/dosya.json")).unwrap_err().contains("okunamadı"));
        assert_eq!(render(View::Profile, &json!({"differences": []})), "fark yok: bu sunucu profille aynı");
        assert_eq!(render(View::Profile, &json!({"differences": ["a", "b"]})), "2 fark:\n  a\n  b");
        assert!(render(View::Profile, &json!({"applied": ["havuz web"], "manual": ["seçenek x"]})).contains("uygulandı:\n  havuz web\nelle düzeltilecek"));
        assert_eq!(render(View::Json, &json!({"a": 1})), "{\n  \"a\": 1\n}");
        let migrate = parse(&args("peer migrate ankara 103")).unwrap();
        assert_eq!((migrate.method, migrate.path.as_str(), migrate.body), ("POST", "/peers/ankara/migrate/103", None));
        assert!(parse(&args("peer migrate ankara yuz")).is_err());
        let args = |text: &str| text.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(patch_body(&["cpu=Haswell", "balloon=true"]).unwrap(), json!({"cpu_type": "Haswell", "balloon": true}));
        assert_eq!(patch_body(&["nic-guard=true"]).unwrap(), json!({"nic_guard": true}));
        assert_eq!(patch_body(&["nested=true"]).unwrap(), json!({"nested": true}));
        assert_eq!(patch_body(&["cores=2", "max-cores=8"]).unwrap(), json!({"cores": 2, "max_cores": 8}));
        assert_eq!(patch_body(&["cpu-affinity=0-3", "numa-node=", "hugepages=true"]).unwrap(), json!({"cpu_affinity": "0-3", "numa_node": "", "hugepages": true}));
        assert_eq!(parse(&args("node perf")).unwrap().path, "/host/performance");
        let guest = parse(&args("hostnet guest vnet1 10.10.0.1/24 dhcp=10.10.0.50-10.10.0.99 nat=true")).unwrap();
        assert_eq!((guest.path.as_str(), guest.body.unwrap()), ("/host/networks", json!({"kind": "guest", "name": "vnet1", "address": "10.10.0.1/24", "dhcp_start": "10.10.0.50", "dhcp_end": "10.10.0.99", "nat": true})));
        assert_eq!(parse(&args("hostnet vlan bond0 20")).unwrap().body.unwrap()["name"], json!("bond0.20"));
        assert!(parse(&args("hostnet bond bond0 yuvarlak e1")).unwrap_err().contains("bond kipi"));
        assert!(parse(&args("hostnet guest vnet1 8.8.8.8/24")).unwrap_err().contains("özel bir IPv4"));
        assert_eq!(parse(&args("shared take ankara 103")).unwrap().path, "/shared/take/ankara/103");
        assert_eq!(parse(&args("shared adopt ortak 103")).unwrap().path, "/shared/adopt/ortak/103");
        let set = parse(&args("firewall set save ofis 10.0.0.0/24,192.168.1.5")).unwrap();
        assert_eq!((set.path.as_str(), set.body.unwrap()), ("/firewall/sets", json!({"name": "ofis", "entries": ["10.0.0.0/24", "192.168.1.5"]})));
        let group = parse(&["firewall", "group", "save", "web", "in=tcp/443", "out=udp/53 to @ofis"].map(String::from)).unwrap();
        assert_eq!(group.body.unwrap()["out"][0]["source"], json!("@ofis"));
        assert!(parse(&args("firewall group save web")).unwrap_err().contains("en az bir kural"));
        assert_eq!(patch_body(&["fw-out-restrict=true", "fw-groups=web,db"]).unwrap(), json!({"fw_out_restrict": true, "fw_groups": ["web", "db"]}));
        let media = parse(&args("vm media 103 2 virtio-win.iso")).unwrap();
        assert_eq!((media.path.as_str(), media.body.unwrap()), ("/guests/103/media", json!({"drive": 2, "iso": "virtio-win.iso"})));
        assert!(parse(&args("vm media 103 ikinci")).unwrap_err().contains("1 ya da 2"));
        assert_eq!(patch_body(&["disk-bus=sata", "nic-model=e1000e", "iso2=a.iso"]).unwrap(), json!({"disk_bus": "sata", "nic_model": "e1000e", "iso2": "a.iso"}));
        assert!(patch_body(&["disk-bus=ide"]).unwrap_err().contains("virtio-scsi ya da sata"));
        let cores = parse(&args("vm cores 103 6")).unwrap();
        assert_eq!((cores.method, cores.path.as_str(), cores.body.unwrap()), ("POST", "/guests/103/cores", json!({"cores": 6})));
        assert!(parse(&args("vm cores 103 cok")).unwrap_err().contains("bir sayı olmalı"));
        assert_eq!(patch_body(&["memory=2048", "max-memory=8192"]).unwrap(), json!({"memory_mb": 2048, "max_memory_mb": 8192}));
        assert_eq!(patch_body(&["cpu-shares=200", "mem-reserve=4096"]).unwrap(), json!({"cpu_shares": 200, "mem_reserve_mb": 4096}));
        let memory = parse(&args("vm memory 103 4096")).unwrap();
        assert_eq!((memory.method, memory.path.as_str(), memory.body.unwrap()), ("POST", "/guests/103/memory", json!({"memory_mb": 4096})));
        assert!(parse(&args("vm memory 103 cok")).unwrap_err().contains("bir sayı olmalı"));
        assert_eq!(patch_body(&["rpo=24"]).unwrap(), json!({"rpo_hours": 24}));
        assert_eq!(patch_body(&["test-port=22", "test-cmd=/bin/true"]).unwrap(), json!({"test_port": 22, "test_cmd": "/bin/true"}));
        assert_eq!(patch_body(&["cpu=host"]).unwrap(), json!({"cpu_type": ""}), "host boş değerle gider");
        let balloon = parse(&args("vm balloon 103 4096")).unwrap();
        assert_eq!((balloon.path.as_str(), balloon.body.unwrap()), ("/guests/103/balloon", json!({"mb": 4096})));
        assert!(parse(&args("vm balloon 103 cok")).unwrap_err().contains("bir sayı olmalı"));
        assert_eq!(parsed("usage"), ("GET", "/usage".into(), None, View::Usage));
        assert_eq!(parsed("vm df 103"), ("GET", "/guests/103/filesystems".into(), None, View::Filesystems));
        assert_eq!(parsed("media catalog"), ("GET", "/media/catalog".into(), None, View::Catalog));
        assert_eq!(parsed("media get debian-12").1, "/media/catalog/debian-12");
        assert_eq!(render(View::Filesystems, &json!([{"mount": "/", "used_mb": 900, "total_mb": 1000}])), "/                                 900 /     1000 MB   %90");
        assert!(render(View::Filesystems, &json!([])).contains("ölçüm yok"));
        assert_eq!(parsed("backup archive"), ("GET", "/backups/archive".into(), None, View::Backups));
        assert_eq!(parsed("backup ack vm-103-1").1, "/backups/vm-103-1/ack");
        assert_eq!(parsed("approval list"), ("GET", "/approvals".into(), None, View::Approvals));
        assert_eq!(parsed("replica add ankara 500 30").2, Some(json!({"peer": "ankara", "vmid": 500, "every_min": 30})));
        assert_eq!(parsed("replica run ankara 500").1, "/replicas/ankara/500/run");
        assert_eq!(parsed("replica failover ankara 500 10").2, Some(json!({"minutes": 10})));
        assert_eq!(parsed("replica failover ankara 500 off").2, Some(json!({"minutes": null})));
        assert_eq!(parsed("replica remove ankara 500"), ("DELETE", "/replicas/ankara/500".into(), None, View::Done("çoğaltma kaldırıldı; yedek kopya olağan bir konuk olarak duruyor")));
        assert_eq!(render(View::Replicas, &json!([{"peer": "ankara", "source": 500, "every_min": 60, "standby": 103, "last_ok": 5, "error": null}])), "ankara         500    her    60 dk   yedek kopya: 103");
        assert_eq!(parsed("maintenance on --drain"), ("POST", "/maintenance".into(), Some(json!({"on": true, "drain": true})), View::Maintenance));
        assert_eq!(parsed("maintenance off").2, Some(json!({"on": false})));
        assert_eq!(parsed("lockdown on"), ("POST", "/lockdown".into(), Some(json!({"on": true})), View::Lockdown));
        assert_eq!(render(View::Lockdown, &json!({"on": true})), "kilit kipi açık: değişiklik yapılamaz");
        assert_eq!(parsed("maintenance on --evacuate ankara").2, Some(json!({"on": true, "evacuate": "ankara"})));
        assert_eq!(parsed("maintenance on --evacuate ankara --as istanbul --drain").2, Some(json!({"on": true, "evacuate": "ankara", "as": "istanbul", "drain": true})));
        assert!(parse(&args("maintenance on --evacuate ankara --as")).is_err() && parse(&args("maintenance on --evacuate ankara --hizli")).is_err());
        assert_eq!(render(View::Maintenance, &json!({"on": true, "guests": 3})), "bakım kipi açık; 3 konuk kapatılıyor");
        assert_eq!(render(View::Maintenance, &json!({"on": false, "resume": []})), "bakım kipi kapalı");
        assert_eq!(parsed("approval approve 7"), ("POST", "/approvals/7/approve".into(), None, View::Approved));
        assert_eq!(parsed("approval reject 7").1, "/approvals/7");
        assert!(parse(&args("approval approve yedi")).is_err());
        assert_eq!(render(View::Approvals, &json!([{"id": 3, "action": "backup-delete", "target": "vm-103-1", "by": "ayse"}])), "#3    backup-delete  vm-103-1                 isteyen ayse");
        assert_eq!(parsed("backup recover vm-103-1").1, "/backups/vm-103-1/recover");
        assert_eq!(parsed("vm usage 103"), ("GET", "/guests/103/usage".into(), None, View::Usage));
        assert_eq!(parsed("ct usage 104 reset"), ("DELETE", "/guests/104/usage".into(), None, View::Done("ölçüm sayaçları sıfırlandı")));
        let auto = parse(&args("vm balloon 103 auto 2048")).unwrap();
        assert_eq!((auto.path.as_str(), auto.body.unwrap()), ("/guests/103/balloon", json!({"min": 2048})));
        assert!(parse(&args("vm balloon 103 auto az")).unwrap_err().contains("bir sayı olmalı"));
        assert_eq!(parse(&args("backup report")).unwrap().path, "/backups/report");
        assert_eq!(render(View::Report, &json!({"subject": "x", "body": "satır 1\nsatır 2\n"})), "satır 1\nsatır 2");
        let checkpoint = parse(&args("snapshot create 103 once --production")).unwrap();
        assert_eq!(checkpoint.body.unwrap(), json!({"name": "once", "production": true}));
        let instant = parse(&args("backup instant vm-103-1 acil")).unwrap();
        assert_eq!((instant.path.as_str(), instant.body.unwrap()), ("/backups/vm-103-1/instant", json!({"name": "acil"})));
        assert_eq!(parse(&args("vm detach 104")).unwrap().path, "/guests/104/detach");
        assert_eq!(parse(&args("backup test vm-103-1")).unwrap().path, "/backups/vm-103-1/test");
        assert_eq!(patch_body(&["backup-pre=/usr/local/bin/once.sh -q", "backup-post="]).unwrap(), json!({"backup_pre": "/usr/local/bin/once.sh -q", "backup_post": ""}));
        let lock = parse(&args("backup lock vm-103-1 30")).unwrap();
        assert_eq!((lock.path.as_str(), lock.body.unwrap()), ("/backups/vm-103-1/lock", json!({"days": 30})));
        assert!(parse(&args("backup lock vm-103-1 cok")).unwrap_err().contains("bir sayı olmalı"));
        let windowed = parse(&args("job add gece time=22:00 until=06:00 verify=true")).unwrap();
        assert_eq!(windowed.body.unwrap(), json!({"name": "gece", "time": "22:00", "until": "06:00", "verify": true}));
        let gfs = parse(&args("job add gece time=03:00 weekly=4 monthly=12 yearly=1 lock=14")).unwrap();
        assert_eq!(gfs.body.unwrap(), json!({"name": "gece", "time": "03:00", "weekly": 4, "monthly": 12, "yearly": 1, "lock": 14}));
        assert_eq!(patch_body(&["backup-weekly=4", "backup-monthly=6", "backup-yearly=1"]).unwrap(), json!({"backup_weekly": 4, "backup_monthly": 6, "backup_yearly": 1}));
        assert_eq!(parse(&args("backup ls vm-103-1")).unwrap().path, "/backups/vm-103-1/files?path=/");
        let put = parse(&args("backup put vm-103-1 /etc/hostname")).unwrap();
        assert_eq!((put.method, put.path.as_str(), put.body.unwrap()), ("POST", "/backups/vm-103-1/file", json!({"path": "/etc/hostname", "dir": false, "target": ""})));
        assert_eq!(parse(&args("backup put-dir vm-103-1 /etc/ssh /tmp/ssh")).unwrap().body.unwrap(), json!({"path": "/etc/ssh", "dir": true, "target": "/tmp/ssh"}));
        assert!(parse(&args("backup put vm-103-1 /a /b /c")).is_err());
        assert_eq!(parse(&["backup".to_string(), "ls".to_string(), "vm-103-1".to_string(), "/home/ali/Yeni Klasör".to_string()]).unwrap().path, "/backups/vm-103-1/files?path=/home/ali/Yeni%20Klas%C3%B6r");
        let files = render(View::Files, &json!({"path": "/etc", "entries": [{"name": "ssh", "kind": "d"}, {"name": "hostname", "kind": "r"}]}));
        assert_eq!(files, "d ssh\n- hostname");
        assert_eq!(render(View::Files, &json!({"path": "/bos", "entries": []})), "dizin boş");
        let fresh = parse(&args("backup restore-new vm-103-1 kurtarilan")).unwrap();
        assert_eq!((fresh.path.as_str(), fresh.body.unwrap()), ("/backups/vm-103-1/restore-new", json!({"name": "kurtarilan"})));
        let moved = parse(&args("vm move 103 hizli")).unwrap();
        assert_eq!((moved.method, moved.path.as_str(), moved.body.unwrap()), ("POST", "/guests/103/move", json!({"storage": "hizli"})));
        let pool = parse(&args("pool add web cores=32 memory=65536 disk=500 guests=10")).unwrap();
        assert_eq!((pool.method, pool.path.as_str(), pool.body.unwrap()), ("POST", "/pools", json!({"name": "web", "cores": 32, "memory_mb": 65536, "disk_gb": 500, "guests": 10})));
        assert_eq!(parse(&args("pool add bos")).unwrap().body.unwrap(), json!({"name": "bos"}));
        assert!(parse(&args("pool add web cpu=4")).unwrap_err().contains("bilinmeyen anahtar"));
        assert_eq!(parse(&args("pool remove web")).unwrap().path, "/pools/web");
        assert_eq!(patch_body(&["pool=web"]).unwrap(), json!({"pool": "web"}));
        let listed = render(View::Pools, &json!([{"name": "web", "cores": 32, "memory_mb": 0, "disk_gb": 500, "guests": 0, "used": {"guests": 2, "cores": 12, "memory_mb": 16384, "disk_gb": 160}}]));
        assert!(listed.contains("çekirdek 12 / 32") && listed.contains("16384 MB / sınırsız") && listed.contains("disk 160 GB / 500 GB") && listed.contains("konuk 2 / sınırsız"), "{listed}");
        assert_eq!(render(View::Pools, &json!([])), "havuz yok");
        let ci = patch_body(&["ci-user=ali", "ci-key=ssh-ed25519 AAAA ali@x", "ci-ip=10.0.0.5/24", "ci-gw=10.0.0.1", "ci-dns=1.1.1.1,9.9.9.9"]).unwrap();
        assert_eq!(ci, json!({"ci_user": "ali", "ci_ssh_key": "ssh-ed25519 AAAA ali@x", "ci_ip": "10.0.0.5/24", "ci_gw": "10.0.0.1", "ci_dns": "1.1.1.1,9.9.9.9"}));
        assert_eq!(patch_body(&["ci-key="]).unwrap(), json!({"ci_ssh_key": ""}), "boş anahtar cloud-init'i kapatır");
        let add = parse(&args("job add gece time=03:00 days=1-5 tags=web,db keep=4")).unwrap();
        assert_eq!((add.method, add.path.as_str()), ("POST", "/jobs"));
        assert_eq!(add.body.unwrap(), json!({"name": "gece", "time": "03:00", "days": "1-5", "tags": ["web", "db"], "keep": 4}));
        assert!(parse(&args("job add gece days=1-5")).unwrap_err().contains("saat verin"));
        assert!(parse(&args("job add gece time=03:00 kep=4")).unwrap_err().contains("bilinmeyen anahtar"));
        assert_eq!(parse(&args("job run gece")).unwrap().path, "/jobs/gece/run");
        assert_eq!(parse(&args("job remove gece")).unwrap().method, "DELETE");
        let list = render(View::Jobs, &json!([{"name": "gece", "time": "03:00", "days": 31, "keep": 4, "tags": ["web"], "queue": [100, 101], "running": 103, "last_run": "2026-11-25"},
            {"name": "hepsi", "time": "01:00", "days": 127, "keep": 7, "tags": [], "queue": [], "running": null, "last_run": null}]));
        assert!(list.contains("hafta içi") && list.contains("etiket: web") && list.contains("sürüyor (3 konuk kaldı)"), "{list}");
        assert!(list.contains("bütün konuklar") && list.contains("her gün") && list.contains("son: -"), "{list}");
        assert_eq!(render(View::Jobs, &json!([])), "yedek işi yok");
        let g = render(View::Guest, &json!({"vmid": 103, "name": "x", "kind": "vm", "node": "n", "status": "stopped", "os": "debian", "cores": 2,
            "disk_gb": 10, "storage": "local", "bridge": "nat", "tags": ["a", "b"], "forwards": [{"host_port": 8080, "guest_port": 80}], "backup_time": "03:30", "backup_keep": 5}));
        assert!(g.contains("etiketler        a, b") && g.contains("8080:80") && g.contains("her gün 03:30, son 5 yedek"), "{g}");
    }
}
