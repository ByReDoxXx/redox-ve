//! Hipervizör sürücü arayüzü.
//!
//! Küme kuralları (`Cluster`) bir eylemin *yapılabilir* olup olmadığına karar verir;
//! sürücü ise onu gerçekten uygular. İki sürücü vardır: hiçbir şey çalıştırmayan
//! `SimDriver` (burada) ve `redox-server` içindeki QEMU/KVM sürücüsü.

use crate::model::{Action, Guest, GuestKind, GuestStatus, VmId};

/// Sürücünün bir konuk hakkında gözlediği gerçek durum.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub status: GuestStatus,
    /// Konuğa ayrılan çekirdeklere göre yüzde; ölçülemediyse `None`.
    pub cpu_pct: Option<u8>,
    pub mem_used_mb: Option<u64>,
}

/// Konuğun içinden (konuk ajanıyla) okunan bir dosya sisteminin doluluğu.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FsUsage {
    pub mount: String,
    pub used_mb: u64,
    pub total_mb: u64,
}

impl FsUsage {
    pub fn pct(&self) -> u8 {
        crate::fmt::pct(self.used_mb, self.total_mb)
    }
}

/// Konuğun balon sürücüsünün bildirdiği bellek durumu (MB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BalloonStats {
    /// Konuğun o an gördüğü toplam bellek (balon şişikse ayrılandan azdır).
    pub total_mb: u64,
    /// Takasa başvurmadan kullanılabilecek bellek (boş + geri alınabilir önbellek).
    pub available_mb: u64,
}

/// Sunucu dışındaki yedek hedefi: dosyaları adlarıyla koyar, alır ve siler.
/// Ad en çok üç bileşenlidir (`chunks/ab/<özet>` gibi) ve yalnızca güvenli karakterler içerir.
pub trait Remote: Send + Sync {
    fn exists(&self, name: &str) -> Result<bool, String>;
    /// Yerel dosyayı hedefe kopyalar; gönderilen bayt sayısını döndürür.
    fn put(&self, name: &str, local: &std::path::Path) -> Result<u64, String>;
    /// Hedefteki dosyayı `local` yoluna indirir.
    fn get(&self, name: &str, local: &std::path::Path) -> Result<(), String>;
    /// Hedefteki dosyayı siler; zaten yoksa hata değildir.
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// Bir konuğun disk yansıtmasının o anki hali.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MirrorProbe {
    /// Konuğun disk sayısı ve bunlardan yansıtma işi süren.
    pub disks: usize,
    pub jobs: usize,
    /// Bütün işler yetişmiş ve her yazma iki yere birden gidiyor.
    pub synced: bool,
    /// İşlerin toplam ilerlemesi (bayt). `done < total` iken uzun süre değişmemesi karşı tarafın yanıt
    /// vermediğini gösterir; o sürede konuğun disk yazmaları da bekler.
    pub done: u64,
    pub total: u64,
}

pub trait Hypervisor: Send + Sync {
    fn name(&self) -> &'static str;

    /// Gerçek süreçler yönetiyor mu? Öyleyse durum ve ölçümler `probe` ile okunur.
    fn is_real(&self) -> bool {
        false
    }

    fn supports(&self, _kind: GuestKind) -> bool {
        true
    }

    /// Kullanılabilir kurulum kalıpları (ISO dosya adları).
    fn isos(&self) -> Vec<String> {
        Vec::new()
    }

    /// Hazır bulut imajları (yeni sanal makinenin diski bunlardan kopyalanır).
    fn images(&self) -> Vec<String> {
        Vec::new()
    }

    /// Yedeğin sunucudaki dosyası (var olsun ya da olmasın); dosya tutmayan sürücüde `None`.
    fn backup_file(&self, _id: &str) -> Option<std::path::PathBuf> {
        None
    }

    /// Yedeğin yanındaki makine durumu arşivi (UEFI değişkenleri, sanal TPM); var olsun ya da olmasın.
    /// Makine durumu yedeklemeyen sürücüde `None`.
    fn backup_machine_file(&self, _id: &str) -> Option<std::path::PathBuf> {
        None
    }

    /// Konuğa boş bir ek disk oluşturur.
    fn add_disk(&self, _guest: &Guest, _id: u8, _size_gb: u64) -> Result<(), String> {
        Ok(())
    }

    /// Ek diski ve içindeki verileri siler.
    fn remove_disk(&self, _guest: &Guest, _id: u8) -> Result<(), String> {
        Ok(())
    }

    /// Çalışan konuğun canlı ekranına (VNC) bağlanılacak Unix soketi; yoksa `None`.
    fn vnc_socket(&self, _guest: &Guest) -> Option<std::path::PathBuf> {
        None
    }

    /// Askıdaki konuğun bellek durumu: `(dosya, çalışırken mi askıya alındı, donanım yerleşimi)`.
    fn suspend_state(&self, _guest: &Guest) -> Option<(std::path::PathBuf, bool, u8)> {
        None
    }

    /// Başka bir sunucudan getirilen bellek durumunu bu konuğun askı kaydı yapar (dosya taşınır).
    fn adopt_suspend(&self, _guest: &Guest, _file: &std::path::Path, _running: bool, _layout: u8) -> Result<(), String> {
        Err("bu sürücü bellek durumu devralamaz".to_string())
    }

    /// Sanal makinelere paylaşılan klasör takılabiliyor mu?
    fn shared_folders(&self) -> bool {
        true
    }

    /// Paylaşılan klasörün sunucudaki dizini (gerçek sürücüde).
    fn share_dir(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }

    /// Bu türden konuk bu depolama alanında oluşturulabilir mi?
    fn storage_supports(&self, _storage: &str, _kind: GuestKind) -> bool {
        true
    }

    /// Anında kurtarma: konuğun disklerini, yedeğin dosyalarını arkalık alan ince katmanlar olarak kurar
    /// (kopyalama yok; konuk hemen başlatılabilir). Yalnızca tek dosyalı (tam) yedeklerle çalışır.
    fn instant_from_backup(&self, _guest: &Guest, _backup: &str, _disks: &[u8]) -> Result<(), String> {
        Ok(())
    }

    /// Anında kurtarılmış konuğun disklerini yedekten bağımsızlaştırır (yedekteki veri disklere kopyalanır).
    /// Konuk çalışırken de yapılır.
    fn detach_backing(&self, _guest: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Kapalı konuğun bütün disklerini `target` depolama alanına taşır. Yarıda kalırsa kaynak diskler yerinde kalır.
    fn move_storage(&self, _guest: &Guest, _target: &str) -> Result<(), String> {
        Ok(())
    }

    /// Başka sunucudan indirilen disk dosyasını (qcow2) konuğun `id` numaralı ek diski yapar.
    fn import_disk(&self, _guest: &Guest, _id: u8, _source: &std::path::Path) -> Result<(), String> {
        Ok(())
    }

    /// Makine durumu arşivini (UEFI değişkenleri, sanal TPM) konuğa uygular; konuk kapalı olmalıdır.
    fn restore_machine(&self, _guest: &Guest, _archive: &std::path::Path) -> Result<(), String> {
        Ok(())
    }

    /// Sunucunun işlemci üreticisi (`intel` / `amd`); bilinmiyorsa `None`.
    fn host_cpu_vendor(&self) -> Option<&'static str> {
        None
    }

    /// Çalışan konuğun bellek balonunu `target_mb` hedefine ayarlar (konukta balon aygıtı olmalıdır).
    fn set_balloon(&self, _guest: &Guest, _target_mb: u64) -> Result<(), String> {
        Ok(())
    }

    /// Çalışan konukta konuk ajanıyla bir program çalıştırır (kabuk yok: mutlak yol ve argümanlar). Program
    /// 0 ile bitmezse ya da ajan yoksa hata döner.
    fn guest_run(&self, _guest: &Guest, _command: &str) -> Result<(), String> {
        Err("bu sürücüde konuk ajanıyla komut çalıştırma yok".to_string())
    }

    /// Sunucu iç içe sanallaştırmayı destekliyor mu (KVM modülünde açık mı)?
    fn nested_virt(&self) -> bool {
        true
    }

    /// Çalışan konuğun disklerinden açılıştan beri (okunan, yazılan) bayt; ölçülemiyorsa `None`.
    fn io_counters(&self, _guest: &Guest) -> Option<(u64, u64)> {
        None
    }

    /// Yedekler diskte şifreli mi saklanıyor?
    fn backups_sealed(&self) -> bool {
        false
    }

    /// Yedekteki bir diskin ham içeriğini `dest` dosyasına yazar (artımlı çoğaltma blokları bundan okunur).
    fn backup_raw(&self, _id: &str, _disk: u8, _dest: &std::path::Path) -> Result<(), String> {
        Err("bu sürücü yedeğin ham içeriğini veremiyor".to_string())
    }

    /// Kapalı konuğun bir diskinin ham içeriğini `dest` dosyasına yazar.
    fn disk_raw(&self, _guest: &Guest, _disk: u8, _dest: &std::path::Path) -> Result<(), String> {
        Err("bu sürücü diskin ham içeriğini veremiyor".to_string())
    }

    /// Ham içeriği, içe aktarılabilir bir disk imajına çevirir (biçim tahmine bırakılmaz).
    fn raw_to_image(&self, _raw: &std::path::Path, _dest: &std::path::Path) -> Result<(), String> {
        Err("bu sürücü ham içerikten imaj üretemiyor".to_string())
    }

    /// Çalışan konuğun köprüdeki kartlarından açılıştan beri (aldığı, gönderdiği) bayt; ölçülemiyorsa `None`.
    fn net_counters(&self, _guest: &Guest) -> Option<(u64, u64)> {
        None
    }

    /// Çalışan konuğun dosya sistemleri (konuk ajanı gerekir); okunamıyorsa `None`.
    fn guest_filesystems(&self, _guest: &Guest) -> Option<Vec<FsUsage>> {
        None
    }

    /// Çalışan konuğun balon sürücüsünden bellek ölçümü; sürücü yoksa ya da henüz bildirmediyse `None`.
    fn balloon_stats(&self, _guest: &Guest) -> Option<BalloonStats> {
        None
    }

    /// Çalışan konuğa disk ve ağ kartı takılabiliyor mu?
    fn hotplug(&self, _kind: GuestKind) -> bool {
        false
    }

    /// `add_disk` ile oluşturulmuş diski çalışan konuğa takar.
    fn hot_add_disk(&self, _guest: &Guest, _id: u8) -> Result<(), String> {
        Err("bu sürücü çalışan konuğa disk takamaz".to_string())
    }

    /// Çalışan konuğa ek ağ kartı takar. `index`: kartın sırası (1 ilk ek kart); `network`: `nat` ya da köprü adı.
    fn hot_add_nic(&self, _guest: &Guest, _index: u8, _network: &str) -> Result<(), String> {
        Err("bu sürücü çalışan konuğa ağ kartı takamaz".to_string())
    }

    /// Çalışan konuğa bir çekirdek ekler (konuk boş çekirdek yuvalarıyla açılmış olmalıdır).
    fn hot_add_core(&self, _guest: &Guest) -> Result<(), String> {
        Err("bu sürücü çalışan konuğa çekirdek ekleyemez".to_string())
    }

    /// Sonradan eklenen çekirdekleri konuk ajanıyla etkinleştirir. Ajan yoksa ya da yapamadıysa `false`:
    /// çekirdek takılıdır, ama konuğun içinde elle etkinleştirilmesi gerekebilir.
    fn online_cores(&self, _guest: &Guest) -> bool {
        false
    }

    /// Çalışan konuğa `mb` MB bellek ekler (konuk boş bellek yuvalarıyla açılmış olmalıdır). Parçanın sırası
    /// konuğun kaydındaki parça sayısıdır.
    fn hot_add_memory(&self, _guest: &Guest, _mb: u64) -> Result<(), String> {
        Err("bu sürücü çalışan konuğa bellek ekleyemez".to_string())
    }

    /// Sonradan eklenen belleği konuk ajanıyla etkinleştirir. Ajan yoksa ya da yapamadıysa `false`: bellek
    /// takılıdır, konuk onu kendiliğinden kullanmıyorsa içinde elle etkinleştirilmesi gerekir.
    fn online_memory(&self, _guest: &Guest) -> bool {
        false
    }

    /// Akıştaki veriyi çalışan konuğun içine yazar (konuk ajanıyla); yazılan bayt sayısını döndürür. `tar`
    /// yanlışsa veri `target` dosyasına yazılır (varsa üzerine); doğruysa veri bir tar arşividir ve konukta var
    /// olan `target` dizinine açılır.
    fn guest_put(&self, _guest: &Guest, _target: &str, _tar: bool, _data: &mut dyn std::io::Read) -> Result<u64, String> {
        Err("bu sürücü çalışan konuğun içine dosya yazamaz".to_string())
    }

    /// Bu sürücü çalışan konuğu başka bir sunucuya (ya da başka bir sunucudan) canlı taşıyabilir mi?
    fn live_migration(&self) -> bool {
        false
    }

    /// Çalışan konuğun açıldığı donanım yerleşimi (hedef aynı yerleşimle açılır); konuk çalışmıyorsa `None`.
    fn migrate_layout(&self, _guest: &Guest) -> Option<u8> {
        None
    }

    /// Canlı taşımanın hedefi: konuğu taşıma bekler halde (duraklatılmış) açar. Diskleri `nbd_port`'tan
    /// yazılabilir sunulur, bellek `ram_port`'tan beklenir; bağlantılar `key` ile şifrelenir.
    fn migrate_listen(&self, _guest: &Guest, _layout: u8, _bind: &str, _nbd_port: u16, _ram_port: u16, _key: &str) -> Result<(), String> {
        Err("bu sürücü canlı taşımayı desteklemiyor".to_string())
    }

    /// Canlı taşımanın kaynağı: diskleri hedefe aynalar, sonra belleği taşır. Başarıda konuk burada
    /// kapanmıştır ve duraklama süresi (ms) döner; bellek hedefe geçmeden hata olursa konuk çalışmayı sürdürür.
    fn migrate_send(&self, _guest: &Guest, _host: &str, _nbd_port: u16, _ram_port: u16, _key: &str) -> Result<u64, String> {
        Err("bu sürücü canlı taşımayı desteklemiyor".to_string())
    }

    /// Hedef: bellek geldi ve kaynak kapandı; konuk sürdürülür.
    fn migrate_finish(&self, _guest: &Guest) -> Result<(), String> {
        Err("bu sürücü canlı taşımayı desteklemiyor".to_string())
    }

    /// Hedef: taşıma olmadı; bekleyen konuk kapatılır.
    fn migrate_abort(&self, _guest: &Guest) {}

    /// Çalışan konuğun disklerini, konuğu durdurmadan `target` depolama alanına taşır.
    fn move_storage_live(&self, _guest: &Guest, _target: &str) -> Result<(), String> {
        Err("bu sürücü çalışan konuğun disklerini taşıyamaz".to_string())
    }

    /// Bu sürücü çalışan konuğun disklerini eşzamanlı yansıtabilir ve başkasının kopyasını tutabilir mi?
    fn disk_mirroring(&self) -> bool {
        false
    }

    /// Çalışan konuğun disklerinin sanal boyutları (bayt), disk sırasıyla.
    fn mirror_disk_sizes(&self, _guest: &Guest) -> Result<Vec<u64>, String> {
        Err("bu sürücü disk yansıtmayı desteklemiyor".to_string())
    }

    /// Kaynak: konuğun disklerini `host:port`taki kopyaya yansıtmaya başlar. Eşitlendikten sonra her yazma iki
    /// yere birden gider; bağlantı `key` ile şifrelenir. İş kendiliğinden bitmez.
    fn mirror_start(&self, _guest: &Guest, _host: &str, _port: u16, _key: &str) -> Result<(), String> {
        Err("bu sürücü disk yansıtmayı desteklemiyor".to_string())
    }

    /// Kaynak: yansıtmanın o anki hali; konuk çalışmıyorsa `None`. Yalnızca yerel hipervizöre sorar, ağa dokunmaz.
    fn mirror_probe(&self, _guest: &Guest) -> Option<MirrorProbe> {
        None
    }

    /// Kaynak: yansıtmayı bırakır. Karşı taraf yanıt vermiyorsa bağlantı zorla kesilir; konuğun bekleyen disk
    /// yazmaları tamamlanır.
    fn mirror_stop(&self, _guest: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Hedef: `id` adlı kopya için boş diskler açar ve `bind:port` üzerinden şifreli, yazılabilir sunar.
    fn replica_serve(&self, _id: &str, _sizes: &[u64], _bind: &str, _port: u16, _key: &str) -> Result<(), String> {
        Err("bu sürücü disk kopyası tutamaz".to_string())
    }

    /// Hedef: kopyanın sunumunu kapatır (yazılanlar diske iner); dosyalar kalır.
    fn replica_release(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }

    /// Hedef: kopyanın dosyalarını siler.
    fn replica_delete(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }

    /// Hedef: kopyanın disklerini yeni oluşturulmuş konuğun diskleri yapar.
    fn replica_adopt(&self, _id: &str, _guest: &Guest) -> Result<(), String> {
        Err("bu sürücü disk kopyası tutamaz".to_string())
    }

    /// UEFI donanım yazılımı kurulu mu?
    fn uefi(&self) -> bool {
        false
    }

    /// Sanal TPM (swtpm) kurulu mu?
    fn tpm(&self) -> bool {
        false
    }

    /// Konuk güvenlik duvarı kurallarını uygulayabiliyor mu?
    fn firewall(&self) -> bool {
        false
    }

    /// Bütün konukların güvenlik duvarı kurallarını tek seferde (atomik) yeniden yükler.
    fn apply_firewall(&self, _guests: &[Guest]) -> Result<(), String> {
        Ok(())
    }

    /// Ağ kartı korumalarını günceller: konukların sunucudaki arayüzleri açılıp kapandıkça değişir. Kurallar
    /// yalnızca değiştiyse yeniden yüklenir; düzenli aralıkla çağrılır.
    fn refresh_guards(&self, _guests: &[Guest]) -> Result<(), String> {
        Ok(())
    }

    /// Kapalı konuğun disklerini dışa aktarma için (VMware'in okuduğu) vmdk dosyalarına çevirir.
    /// Dönen değer: geçici dizin ve diskler (dosya adı, yol, sanal kapasite bayt). Çağıran, işi bitince dizini siler.
    #[allow(clippy::type_complexity)]
    fn export_disks(&self, _guest: &Guest) -> Result<Option<(std::path::PathBuf, Vec<(String, std::path::PathBuf, u64)>)>, String> {
        Ok(None)
    }

    /// Verilen adlı kurulum kalıbının (ISO) sunucudaki yolu (var olsun ya da olmasın); kalıp tutmayan sürücüde `None`.
    fn iso_file(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }

    /// Verilen adlı disk imajının sunucudaki yolu (var olsun ya da olmasın); imaj tutmayan sürücüde `None`.
    fn image_file(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }

    /// Sunucudaki ağ köprüleri (konuğun bağlanabileceği arayüzler).
    fn bridges(&self) -> Vec<String> {
        Vec::new()
    }

    /// Konteyner şablonları (dağıtım-sürüm). Boşsa işletim sistemi serbest metindir.
    fn templates(&self) -> Vec<String> {
        Vec::new()
    }

    /// Konuğun içindeki ajan yanıt veriyorsa bildirdiği işletim sistemi; ajan yoksa `None`.
    fn guest_agent(&self, _guest: &Guest) -> Option<String> {
        None
    }

    /// Çalışan konuğun ağ adresi; bulunamadıysa `None`.
    fn guest_ip(&self, _guest: &Guest) -> Option<String> {
        None
    }

    /// Konuğun içinde bir kabuk komutu çalıştırıp çıktısını döndürür (konteynerler).
    fn exec(&self, _guest: &Guest, _command: &str) -> Result<String, String> {
        Err("bu konuk türünde komut çalıştırma yok".to_string())
    }

    /// Yeni konuğun kaynaklarını (disk imajı vb.) hazırlar.
    fn create(&self, _guest: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Ayarlar değişince sürücünün kendi yapılandırmasını günceller (ör. LXC yapılandırma dosyası).
    fn configure(&self, _guest: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Konuğun kaynaklarını siler. Konuk durdurulmuş olmalıdır.
    fn destroy(&self, _guest: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Eylemi konuğa uygular. Hata metni görev günlüğünde kullanıcıya gösterilir.
    /// Uzun sürebilir (ör. konuğun kapanmasını beklemek); kilit tutulmadan çağrılır.
    fn apply(&self, guest: &Guest, action: Action) -> Result<(), String>;

    /// Konuğun gerçek durumunu okur. Simülasyon sürücüsü `None` döner.
    fn probe(&self, _guest: &Guest) -> Option<Probe> {
        None
    }

    /// Anlık görüntü ve yedek, konuk çalışırken de alınabiliyor mu?
    fn live_disk_ops(&self, _kind: GuestKind) -> bool {
        false
    }

    /// Üretim denetim noktası: çalışan konuğun yalnızca disklerinin, bellek durumu olmadan alınan anlık görüntüsü.
    /// Konuk ajanı varsa dosya sistemleri dondurulur; dönen değer dondurmanın yapılıp yapılmadığıdır.
    fn snapshot_disk_only(&self, _guest: &Guest, _name: &str) -> Result<bool, String> {
        Ok(false)
    }

    /// Yedek, konuk çalışırken de alınabiliyor mu? Varsayılan: anlık görüntüyle aynı.
    /// (Konteynerde anlık görüntü kapalıyken alınır ama yedek, konteyner dondurularak çalışırken de alınır.)
    fn live_backup(&self, kind: GuestKind) -> bool {
        self.live_disk_ops(kind)
    }

    /// Disk üzerinde anlık görüntü işlemi. `live_disk_ops` yoksa konuk durdurulmuş olmalıdır.
    fn snapshot(&self, _guest: &Guest, _op: SnapshotOp<'_>) -> Result<(), String> {
        Ok(())
    }

    /// Diski `new_gb` boyutuna büyütür.
    fn resize_disk(&self, _guest: &Guest, _new_gb: u64) -> Result<(), String> {
        Ok(())
    }

    /// `source` diskini `target` konuğu için kopyalar.
    fn clone_disk(&self, _source: &Guest, _target: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Çalışan konuğun bellek durumunu diske yazıp konuğu kapatır (askıya alma). Sonraki başlatma kaldığı yerden devam eder.
    fn suspend(&self, _guest: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Kayıtlı bellek durumunu siler: konuk bir sonraki başlatmada sıfırdan açılır.
    fn discard_suspend(&self, _guest: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// `target` için `source` şablonunun disklerine dayanan (yalnızca farkları tutan) diskler oluşturur.
    fn link_disk(&self, _source: &Guest, _target: &Guest) -> Result<(), String> {
        Ok(())
    }

    /// Konuğun asıl diskinin ve bütün ek disklerinin yedeğini `id` adıyla alır; yedeğin boyutunu döndürür.
    fn backup(&self, _guest: &Guest, _id: &str) -> Result<BackupSize, String> {
        Ok(BackupSize::default())
    }

    /// Yedek bu sunucuda duruyor mu? (Yalnızca dış kopyası kalmış olabilir.)
    fn backup_present(&self, id: &str) -> bool {
        self.backup_file(id).is_some_and(|path| path.exists())
    }

    /// Yedeğin tek dosyalık kopyasının yolu (indirmek ya da dış hedefe göndermek için).
    /// İkinci değer `true` ise dosya bu çağrı için üretilmiştir: işi biten çağıran, dosyayı ve
    /// içinde durduğu dizini siler.
    /// `disk`: 0 asıl disk, diğerleri ek disk numarası.
    fn export_backup(&self, id: &str, disk: u8) -> Result<Option<(std::path::PathBuf, bool)>, String> {
        Ok(self.backup_disk_file(id, disk).map(|path| (path, false)))
    }

    /// Tam yedeğin `disk` numaralı diskinin dosyası (0: asıl disk). Ek disk tutmayan sürücüde yalnızca 0.
    fn backup_disk_file(&self, id: &str, disk: u8) -> Option<std::path::PathBuf> {
        if disk == 0 { self.backup_file(id) } else { None }
    }

    /// Yedeği dış hedefe parça parça gönderir (yalnızca hedefte olmayan kısım). `known`: dış kopyası
    /// olduğu bilinen diğer yedekler; onların parçaları yeniden sorulmaz. Yedek parçalı değilse `None`:
    /// çağıran, tek dosyalık kopyayı (`export_backup`) gönderir.
    fn push_backup(&self, _id: &str, _known: &[String], _remote: &dyn Remote) -> Result<Option<u64>, String> {
        Ok(None)
    }

    /// Dış hedefteki parçalı kopyayı yerel depoya indirir. Hedefte parçalı kopya yoksa `false`.
    fn pull_backup(&self, _id: &str, _remote: &dyn Remote) -> Result<bool, String> {
        Ok(false)
    }

    /// Dış hedefteki parçalı kopyayı siler; `others` (dış kopyası olan diğer yedekler) tarafından
    /// kullanılan parçalara dokunmaz. Hedefte parçalı kopya yoksa `false`.
    fn unpush_backup(&self, _id: &str, _others: &[String], _remote: &dyn Remote) -> Result<bool, String> {
        Ok(false)
    }

    /// Yedeğin okunabilir ve bozulmamış olduğunu denetler; kısa bir özet döndürür.
    /// `disks`: yedekte bulunması gereken ek diskler.
    fn verify_backup(&self, _id: &str, _disks: &[u8]) -> Result<String, String> {
        Err("bu sürücüde yedek doğrulama yok".to_string())
    }

    /// `id` yedeğini konuğun asıl diskinin ve `disks` içindeki ek disklerinin yerine koyar.
    /// İstenen bir disk yedekte yoksa hiçbir diske dokunulmadan hata verilir.
    fn restore(&self, _guest: &Guest, _id: &str, _disks: &[u8]) -> Result<(), String> {
        Ok(())
    }

    fn delete_backup(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }

    /// Konuğun ekran görüntüsü (PNG).
    fn screenshot(&self, _guest: &Guest) -> Result<Vec<u8>, String> {
        Err("bu sürücüde konsol yok".to_string())
    }

    /// Tuş vuruşları gönderir; her öge aynı anda basılan QEMU tuş kodlarıdır (ör. ["shift", "a"]).
    fn send_keys(&self, _guest: &Guest, _chords: &[Vec<String>]) -> Result<(), String> {
        Err("bu sürücüde konsol yok".to_string())
    }
}

/// Alınan yedeğin kapladığı yer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackupSize {
    /// Yedeğin tek başına kapladığı yer (MB).
    pub size_mb: u64,
    /// Artımlı yedekte depoya gerçekten eklenen kısım (MB); tam yedekte `None`.
    pub new_mb: Option<u64>,
    /// Konuk çalışıyorsa: dosya sistemi yedekten önce donduruldu mu? Kapalı konukta `None`.
    pub quiesced: Option<bool>,
    /// Yalnızca değişen bloklar mı okundu (değişen blok takibi)? Değilse disk baştan sona okunmuştur.
    pub cbt: bool,
}

impl BackupSize {
    pub fn full(size_mb: u64) -> Self {
        BackupSize { size_mb, new_mb: None, quiesced: None, cbt: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOp<'a> {
    Create(&'a str),
    Rollback(&'a str),
    Delete(&'a str),
}

/// Hiçbir şeyi gerçekten çalıştırmayan sürücü; geliştirme ve testler içindir.
#[derive(Debug, Default, Clone)]
pub struct SimDriver {
    /// Bu kimlikteki konuk için her eylem başarısız olur (hata yollarını denemek için).
    pub fail_on: Option<VmId>,
}

impl SimDriver {
    pub fn failing_on(vmid: VmId) -> Self {
        SimDriver { fail_on: Some(vmid) }
    }
}

impl Hypervisor for SimDriver {
    fn name(&self) -> &'static str {
        "sim"
    }

    fn apply(&self, guest: &Guest, action: Action) -> Result<(), String> {
        if self.fail_on == Some(guest.vmid) {
            return Err(format!("simülasyon: {} {action} başarısız oldu", guest.vmid));
        }
        Ok(())
    }
}
