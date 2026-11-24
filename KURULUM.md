# RedoX VE — sunucuya kurulum

Genel tanıtım, ekran görüntüleri ve teknik doküman: [README.tr.md](README.tr.md) (İngilizce: [README.md](README.md)).

Bu kılavuz Debian 12 / Ubuntu 22.04+ (systemd, x86_64) içindir. Komutlardaki
`redox.example.com` ve `kullanici@sunucu` kısımlarını kendinizinkiyle değiştirin.

> **İki sürücü var.** Varsayılan `sim` sürücüsü hiçbir şey çalıştırmaz; örnek bir küme gösterir.
> `qemu` sürücüsü bu sunucuda gerçek KVM sanal makineleri çalıştırır (8. bölüm).
> Aynı sürücü, LXC kuruluysa konteynerleri de çalıştırır (9. bölüm).

## Güvenlik modeli (özet)

| Katman | Ne yapıyor |
|---|---|
| Kimlik | Kullanıcı adı + parola (Argon2id). Varsayılan hesap ya da parola yoktur; kullanıcı yokken sunucu başlamaz. |
| Oturum | 256 bit rastgele çerez; `HttpOnly`, `SameSite=Strict`, TLS arkasında `Secure`. 2 saat hareketsizlikte, en geç 12 saatte biter. Parola değişince ya da kullanıcı silinince düşer. |
| Kaba kuvvet | Aynı adres + kullanıcı için 15 dakikada 5 hatalı deneme → 15 dakika kilit. |
| İkinci adım | İsteğe bağlı TOTP (kimlik doğrulama uygulaması). Hesabım sayfasından açılır; kod bir kez kullanılır. |
| Roller | `admin` her şeyi yapar; `viewer` yalnızca görür (konsolu göremez). |
| Denetim | Girişler, reddedilen denemeler ve durum değiştiren her istek `audit.log` dosyasına ve Denetim Günlüğü sayfasına yazılır. |
| API | `Authorization: Bearer <token>`. Token'lar dosyada yalnızca SHA-256 özetiyle durur. |
| Ağ | Sunucu TLS sonlandırmaz; 127.0.0.1 dışına şifresiz açılmayı reddeder. Dışarıya Caddy (TLS) açar. |
| Tarayıcı | Canlı konsol sayfası dışında hiç JavaScript yok (`script-src 'none'`); o sayfada yalnızca sunucunun kendi betiği çalışır (`script-src 'self'`, satır içi betik ve dış kaynak yasak). Site dışı POST'lar ve site dışından açılan konsol bağlantıları reddedilir, sayfa çerçeveye alınamaz, HSTS. Yazı tipleri sunucuya gömülüdür; sayfa hiçbir dış adrese istek atmaz. |
| Süreç | Yetkisiz `redox` kullanıcısı, systemd sandbox'ı, dosyalar `0600`. |

Kapsam dışı kalanlar en altta: **Bilinen eksikler**.

## 1. Kaynağı sunucuya gönderin (Windows'tan)

PowerShell'de, derleme çıktısı (`target`) hariç arşivleyip kopyalayın:

```powershell
tar --exclude=target --exclude=.claude -czf redox-ve.tar.gz -C D:\Projelerim-Claude Proxmox-Rust
```

```powershell
scp redox-ve.tar.gz kullanici@sunucu:~
```

## 2. Sunucuda derleyin

```bash
sudo apt update && sudo apt install -y build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"

tar -xzf redox-ve.tar.gz && cd Proxmox-Rust
cargo test --workspace          # hepsi geçmeli (Linux'ta 381 test; Node kuruluysa canlı konsol istemcisinin 7 testi de çalışır)
cargo build --release
sudo install -m 0755 target/release/redox /usr/local/bin/redox
```

## 3. Hizmet kullanıcısı ve ilk yönetici

```bash
sudo useradd --system --home-dir /var/lib/redox --shell /usr/sbin/nologin redox
sudo install -d -o redox -g redox -m 0700 /var/lib/redox

# Parola iki kez sorulur, ekrana yazılmaz (en az 12 karakter).
sudo -u redox redox user add admin --users /var/lib/redox/users.json
```

Salt okunur bir hesap eklemek için: `sudo -u redox redox user add izleyici --role viewer --users /var/lib/redox/users.json`

## 4. systemd hizmeti

```bash
sudo cp deploy/redox.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload
sudo systemctl enable --now redox
systemctl status redox --no-pager
curl -s http://127.0.0.1:8006/healthz      # "ok" yazmalı
```

Günlükler (girişler ve başarısız denemeler dahil): `journalctl -u redox -f`

## 5. TLS: Caddy ile dışarı açın

Caddy'yi resmi depodan kurun: <https://caddyserver.com/docs/install#debian-ubuntu-raspbian>

```bash
sudo cp deploy/Caddyfile /etc/caddy/Caddyfile
sudo nano /etc/caddy/Caddyfile             # alan adınızı yazın
sudo systemctl reload caddy
```

Güvenlik duvarı: yalnızca SSH, 80 ve 443 açık olsun; **8006'yı açmayın**.

```bash
sudo ufw allow OpenSSH
sudo ufw allow 80,443/tcp
sudo ufw enable
```

Tarayıcıda `https://redox.example.com` adresini açın ve `admin` ile girin.

### Alan adınız yoksa: SSH tüneli

Caddy'yi atlayın, `deploy/redox.service` içindeki `--behind-tls-proxy` satırını silin ve
kendi bilgisayarınızdan tünel açın; sonra `http://127.0.0.1:8006` adresini kullanın:

```powershell
ssh -L 8006:127.0.0.1:8006 kullanici@sunucu
```

Bu durumda sunucu yalnızca 127.0.0.1'de dinler; trafik SSH ile şifrelenir.

## 6. Komut satırı için API token'ı

```bash
sudo -u redox redox token create admin dizustu --users /var/lib/redox/users.json
```

Token yalnızca bir kez gösterilir. Kendi bilgisayarınızda:

```powershell
$env:REDOX_URL = "https://redox.example.com"; $env:REDOX_TOKEN = "redox_..."; redox cluster status
```

İptal etmek için: `sudo -u redox redox token revoke admin dizustu --users /var/lib/redox/users.json`

## 7. Bakım

- **Parola değiştirme:** `sudo -u redox redox user passwd admin --users /var/lib/redox/users.json`
  Sunucuyu yeniden başlatmak gerekmez; kullanıcının açık oturumları hemen düşer.
- **Yedek:** `/var/lib/redox/users.json` ve `/var/lib/redox/state.json`.
- **Güncelleme:** yeni kaynağı gönderin, `cargo build --release`, `sudo install ...`, `sudo systemctl restart redox`.
- **Kilitlenen giriş:** 15 dakika bekleyin ya da `sudo systemctl restart redox` (kilitler bellekte tutulur).

## 8. Gerçek sanal makineler (QEMU/KVM)

Buraya kadarki kurulum simülasyon sürücüsünü çalıştırır. Gerçek sanal makineler için:

```bash
sudo apt install -y qemu-system-x86 qemu-utils
ls -l /dev/kvm                                  # yoksa BIOS'ta sanallaştırmayı açın
sudo cp deploy/redox-qemu.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload && sudo systemctl restart redox
journalctl -u redox -n 20 --no-pager            # "sürücü: qemu-kvm" yazmalı
```

- **Veri dizini** `/var/lib/redox`: disk imajları `images/`, kurulum kalıpları `iso/`, hazır imajlar `cloud/`,
  yedekler `backups/`, çalışma dosyaları `run/`, konuk kayıtları `state.json`, ölçüm geçmişi `metrics.json`.
- **ISO ekleme:** dosyayı `iso/` klasörüne koyun; oluşturma formunda listelenir.
  `sudo install -o redox -g redox -m 0600 debian-12.iso /var/lib/redox/iso/`
- **Kapat** önce konuktan kapanmasını ister (ACPI); 60 saniyede kapanmazsa zorla kapatır
  (`--shutdown-timeout-secs` ile değişir). İşletim sistemi kurulmamış konuk bu süreyi bekler.
- **Hizmet yeniden başlasa da** sanal makineler çalışmaya devam eder ve yeniden sahiplenilir.
- **`/dev/kvm` yoksa** sunucu uyarı verir ve yazılım öykünmesine (TCG) düşer; çok yavaştır.

Sürücünün sunduğu işlemler (konuk sayfasındaki sekmelerden):

- **Konsol:** konuğun ekranı 2 saniyede bir yenilenen görüntü olarak gösterilir; metin ve özel tuşlar
  (Enter, Esc, oklar, F2/F10/F12, Ctrl+Alt+Del) gönderilebilir. Fare yoktur; metin ABD klavye düzeniyle gider.
  Bu sekme JavaScript'siz çalışır.
- **Kendiliğinden yeniden başlatma:** *Seçenekler* sekmesinde **Kendiliğinden kapanırsa yeniden başlat** işaretli
  konuk çökerse ya da içinden kapatılırsa yaklaşık 15 saniye içinde yeniden açılır (`redox vm set 103 restart=true`).
  Panelden kapatılan, askıya alınan ya da üzerinde işlem süren konuğa dokunulmaz. Açılıp açılıp çöken konuk 10
  dakikada en çok 3 kez denenir; sonra vazgeçilir ve görev günlüğüne `gözcü` kullanıcısıyla hata yazılır. Bu, tek
  sunucudaki bir önlemdir: sunucunun kendisi çökerse konuklar başka sunucuda kendiliğinden açılmaz (aşağıda).
- **Paylaşılan klasörler (sanal makine):** *Donanım* sekmesinde **Paylaşımlar** alanına adları yazın
  (`ortak, veri:ro`; `:ro` salt okunur; en fazla 4) ya da `redox vm set 103 shares=ortak,veri:ro`. Her ad sunucuda
  `/var/lib/redox/shares/<ad>` klasörüdür (ilk açılışta `0700` izinle oluşturulur); aynı adı kullanan konuklar aynı
  klasörü görür. Konukta bağlamak için: `mount -t 9p -o trans=virtio,version=9p2000.L ortak /mnt/ortak`. Dosyalar
  sunucuda `redox` kullanıcısının olur. Paylaşım konukta bağlıyken konuk askıya alınamaz, çalışırken anlık görüntüsü
  alınamaz ve çalışırken taşınamaz (önce konukta `umount`). Windows konuklarında 9p sürücüsü yoktur. Konuk silinince
  paylaşımdaki dosyalar silinmez.
- **Canlı konsol:** konsol sekmesindeki **Canlı Konsolu Aç** düğmesi (`/guests/<kimlik>/console`) konuğun
  ekranını gerçek zamanlı gösterir; ekrana tıklayınca klavye ve fare konuğa gider. Tuşlar fiziksel konumlarıyla
  gönderilir, yani karakteri konuğun kendi klavye düzeni belirler (Türkçe düzen doğru çalışır). Uygulamada
  JavaScript kullanan tek sayfa budur. Nasıl çalışır: QEMU ekranı yalnızca veri dizinindeki bir Unix soketinden
  (`run/<kimlik>.vnc`, parolasız, ağa kapalı) sunar; sunucu, oturumu ve yetkisi denetlenmiş tarayıcı bağlantısını
  (WebSocket) o sokete köprüler. Konsolu yönetici ve kapsamındaki konukta operatör açabilir; her oturum denetim
  günlüğüne `console <kimlik>` olarak yazılır ve en çok 12 saat sürer. Caddy WebSocket'i kendiliğinden geçirir;
  başka bir vekil kullanıyorsanız `Upgrade` başlıklarını iletin. Bu özellikten önce açılmış ve hâlâ çalışan
  konuklarda canlı konsol yoktur: bir kez kapatıp açın (imleç için konuğa bir USB işaretçi aygıtı eklenir).
- **Anlık görüntü ve yedek** sanal makine **çalışırken de** alınır (anlık görüntü belleği de kaydeder;
  geri dönünce konuk o andan devam eder). **Klon ve disk büyütme** konuk kapalıyken yapılır. Yedekler
  `/var/lib/redox/backups/` altında qcow2 dosyalarıdır (artımlı yedek açıksa parça deposu, aşağıda).
- **Zamanlanmış yedek:** konuğun *Seçenekler* sekmesinde saat (`03:30`) ve saklanacak yedek sayısı
  verilir; ya da `redox vm set 103 backup-time=03:30 backup-keep=7`. Saat sunucunun yerel saatidir;
  sayıyı aşan en eski **otomatik** yedekler silinir, elle alınanlara dokunulmaz. Sunucu o dakikada
  kapalıysa o günün yedeği, açıldığında gün bitmeden alınır.
- **Açılışta başlatma ve sıra:** *Seçenekler* sekmesinde **Düğüm açıldığında başlat** işaretli konuklar, sunucu
  yeniden açıldığında (kapalıysalar) başlatılır. **Başlatma sırası** 1 olan ilk başlar; 0 sırasızdır ve
  sıralılardan sonra gelir. **Sonraki konuktan önce bekle** ile araya saniye konur (örn. veritabanından 30 sn
  sonra uygulama). Komut satırından: `redox vm set 103 onboot=true boot-order=1 boot-delay=30`. Hizmetin
  yeniden başlaması (`systemctl restart redox`) açılış sayılmaz: sunucu, çekirdeğin açılış kimliğini
  `/var/lib/redox/boot-id` dosyasında tutar. Tümüyle kapatmak için hizmet satırına `--no-autostart` ekleyin.
  Açılamayan konuk görev günlüğünde `açılış` kullanıcısıyla hata olarak görünür; sıra devam eder.
- **Çalışırken disk ve ağ kartı takma (sanal makine):** *Donanım* sekmesindeki **Disk Ekle** ve **Kart Ekle**
  konuk çalışırken de kullanılır; ya da `redox vm disk add 103 20`, `redox vm nic add 103 vmbr1`. Aygıt hemen
  takılır, konuğun işletim sistemi onu yeni donanım olarak görür. Çıkarmak, büyütmek ve öbür donanım ayarları
  için konuk yine kapatılır; duraklatılmış konuğa takılmaz. Her ek diskin ve ek kartın sanal makinede sabit bir
  bağlantı noktası vardır; bu yüzden **bu sürümden önce açılmış ve hâlâ çalışan** konuklara takılamaz (bir kez
  kapatıp açın), ek diskleri olan konuklarda da disklerin PCI adresi bir kez değişir (diskleri etiket ya da
  UUID ile bağlayan sistemler etkilenmez). Önceki sürümde askıya alınmış konuklar eski yerleşimle döner.
- **Ek depolama alanları (sanal makine diskleri):** ikinci bir diski ya da bağlı bir ağ paylaşımını konuk diskleri
  için kullanmak üzere hizmet satırına `--storage AD=/mutlak/dizin` ekleyin (birden çok kez yazılabilir; ad küçük
  harf, rakam ve tire). Dizin `redox` kullanıcısına ait ve yazılabilir olmalıdır
  (`sudo install -d -o redox -g redox -m 0700 /mnt/nvme2/redox`); ağ paylaşımı hizmetten önce bağlanmış olmalıdır.
  Alan *Depolama* sayfasında ve konuk oluşturma formunda görünür; konuğun bütün diskleri (asıl, ek, klon ve geri
  yükleme) seçilen alanda durur. Yedekler, kurulum kalıpları ve konteynerler her zaman veri dizinindedir
  (`local`). Bir konuğu sonradan başka alana taşıma yoktur. Üzerinde konuk olan bir alanı hizmet satırından
  çıkarırsanız sunucu başlamaz ve hangi konukların orada olduğunu söyler.
- **Kaynak uyarıları:** depolama doluluğuna ek olarak düğümün belleği (%90 uyarı, %97 kritik), düğümün işlemcisi
  (%90) ve çalışan bir konuğun işlemcisi (%95) izlenir. Koşul *Genel Bakış* sayfasında hemen görünür; webhook ve
  e-posta bildirimi ise ancak koşul 5 dakika kesintisiz sürerse gönderilir (anlık sıçramalar bildirim üretmez).
- **VLAN etiketi (sanal makine):** *Donanım* sekmesinde birinci kart için **VLAN etiketi** (1-4094; 0 etiketsiz),
  ek kartlar için ağ adının yanına `köprü:etiket` yazılır (`vmbr1:20`); komut satırından
  `redox vm set 103 vlan=20 nics=vmbr1:30`. Etiketli kart için sunucu, adı `rx<kimlik>n<kart>` olan bir TAP arayüzü
  kurar, köprüye bağlar ve yalnızca o VLAN'ın etiketsiz üyesi yapar; konuk kapanınca arayüz kaldırılır. Gerekenler:
  hizmet biriminde `AmbientCapabilities=CAP_NET_ADMIN`, kurulu `iproute2`, köprüde açık VLAN süzme
  (`ip link set vmbr0 type bridge vlan_filtering 1`) ve dış ağa giden portta o VLAN'ın etiketli üyeliği
  (`bridge vlan add dev eth0 vid 20`). Arayüz kurulamazsa konuk başlatılmaz. Köprü adları büyük/küçük harfe duyarlıdır.
- **Hız sınırları (sanal makine):** *Donanım* sekmesinde disk için MB/sn ve işlem/sn, ağ için Mbit/sn verilir;
  0 sınırsızdır. Komut satırından: `redox vm set 103 disk-mbps=50 disk-iops=2000 net-mbps=100`. Konuk kapalıyken
  değişir, sonraki açılışta geçerli olur. Disk sınırı QEMU'nun kendi kısıtlamasıdır ve her diske ayrı uygulanır.
  Ağ sınırı, köprüye bağlı kartın sunucudaki TAP arayüzüne `tc` ile uygulanır: `sudo apt install iproute2` ve
  hizmet biriminde `AmbientCapabilities=CAP_NET_ADMIN` gerekir (yalnızca `redox-host.service`); NAT kartına
  uygulanamaz. Sınır uygulanamazsa konuk sınırsız çalışmaya bırakılmaz: başlatma hata verir.
- **Zamanlanmış güç işlemleri:** *Seçenekler* sekmesinin altındaki listeye gün ve saat vererek konuğu
  başlatın, kapatın ya da yeniden başlatın (konuk başına en fazla 8). Komut satırından:
  `redox vm set 103 schedules=start@08:00@1-5,stop@20:00` (günler 1 Pazartesi … 7 Pazar; yazılmazsa her gün).
  Konuk zaten istenen durumdaysa işlem atlanır. Sunucu o saatte kapalıysa işlem 15 dakika içinde açılırsa
  çalışır, sonrası o gün atlanır (kaçan bir kapatma ile başlatma saatler sonra art arda uygulanmasın diye).
- **Artımlı yedek:** hizmet satırına `--incremental-backups` ekleyin. Yeni sanal makine yedekleri tek dosya yerine
  ortak bir parça deposuna (`backups/chunks/`) yazılır: disk 4 MiB'lik parçalara bölünür, her parça içeriğinin
  SHA-256 özetiyle adlandırılır ve bir kez saklanır. Bu yüzden ikinci ve sonraki yedekler yalnızca değişen kısım
  kadar yer kaplar; boş (sıfır) bölgeler hiç saklanmaz.
  - **Zincir yoktur.** Her yedek bütün diski tarif eden bir parça listesidir (`backups/<yedek>.rdxi.json`): herhangi
    birini, en eskisini de, silebilirsiniz; diğerleri etkilenmez. Silinen yedeğe özgü parçalar kendiliğinden toplanır.
  - **Bozulma sessiz kalmaz.** Geri yüklemede her parçanın özeti yeniden hesaplanır; tek bir parça bile uyuşmazsa
    geri yükleme reddedilir ve konuğun diski olduğu gibi kalır. İstediğiniz an denetlemek için *Yedekler* sekmesindeki
    **Doğrula** düğmesi ya da `redox backup verify <yedek>`; sonuç görev günlüğüne (ve başarısızsa uyarılara) düşer.
  - **Yer kazanır, zaman kazanmaz.** Her yedekte diskin tamamı okunur. Yedek ve geri yükleme sırasında `backups/`
    (geri yüklemede `images/`) altında geçici olarak diskin dolu kısmı kadar boş yer gerekir.
  - Özelliği açmadan önce alınmış tam yedekler çalışmaya devam eder; iki tür aynı listede durur. Özelliği sonradan
    kapatırsanız artımlı yedekler yine geri yüklenir, yalnızca yenileri tam alınır.
  - **İndir**, artımlı yedeği tek dosyalık sıkıştırılmış qcow2'ye döker. Dış hedefe kopya ise parça parçadır
    (11. bölüm). Konteyner yedekleri her zaman tam arşivdir.
  - Parça deposunu elle düzenlemeyin; `chunks/` altından dosya silmek o parçayı kullanan bütün yedekleri bozar.
- **Hazır imajdan sanal makine (cloud-init):** dağıtımın bulut imajını (`debian-12-genericcloud-amd64.qcow2` gibi)
  `/var/lib/redox/cloud/` klasörüne koyun. Oluşturma formunda *Disk imajı* olarak seçin, kullanıcı adını ve SSH açık
  anahtarınızı yazın: disk imajdan kopyalanır, istediğiniz boyuta büyütülür ve ilk açılışta kullanıcı, anahtar ve
  makine adı kendiliğinden ayarlanır. Parola ile giriş kapalıdır.
- **Başka sanallaştırıcıdan aktarma:** aynı klasöre koyduğunuz `vmdk`, `vdi`, `vhdx` diskleri de imaj olarak seçilir ve
  qcow2'ye çevrilir. OVA dosyası bir tar arşividir: `tar -xf makine.ova` ile açıp içindeki `.vmdk` dosyasını koyun.
- **Dışa aktarma:** *Yedekler* sekmesindeki **İndir** düğmesi yedeği tek başına kullanılabilir bir qcow2 dosyası olarak
  verir. Komut satırından: `curl -H "Authorization: Bearer $REDOX_TOKEN" -o vm.qcow2 $REDOX_URL/api/v1/backups/<yedek>/download`
- **UEFI, güvenli önyükleme, TPM (Windows 11 için):** `sudo apt install ovmf swtpm`, sonra hizmeti yeniden başlatın.
  Oluşturma ve düzenleme formlarında *Donanım yazılımı* ve *Sanal TPM 2.0* seçenekleri vardır. Güvenli önyüklemede
  Microsoft anahtarları kayıtlı gelir. Dosyalar başka yerdeyse: `--ovmf-dir DİZİN`, `--swtpm-bin YOL`.
- **Ek diskler ve ağ kartları:** *Donanım* sekmesinden kapalı konuğa en fazla 8 veri diski ve 3 ek ağ kartı
  eklenir. Ek diskler anlık görüntülere, klonlara ve **yedeklere dahildir** (aşağıda). VLAN etiketi için yukarıdaki
  *VLAN etiketi* maddesine bakın.
- **Kalıp ve imaj yükleme:** *Depolama* sayfasındaki bölümden kurulum kalıbı (`.iso`) ve disk imajı (`.qcow2`, `.img`,
  `.raw`, `.vmdk`, `.vdi`, `.vhdx`, `.vhd`, `.ova`) yükleyin; dosyayı sunucuya elle kopyalamak gerekmez.
  - *Bilgisayardan yükle*: dosya parça parça diske akar (belleğe alınmaz); yükleme bitene kadar sayfa bekler.
    Önünüzde ters vekil varsa gövde sınırını kaldırın (Caddy'de varsayılan sınır yoktur; nginx'te
    `client_max_body_size 0;`).
  - *Adresten indir*: sunucu dosyayı kendisi indirir, sonucu görev günlüğüne yazar. Yalnızca `https://` ve genel
    internet adresleri kabul edilir (sunucunun iç ağına istek yaptırılamasın diye; yönlendirmeler de denetlenir).
    SHA-256 özeti verirseniz dosya onunla doğrulanır, tutmazsa kaydedilmez.
  - Komut satırından: `curl -T debian.iso -H "Authorization: Bearer $REDOX_TOKEN" $REDOX_URL/api/v1/media/debian.iso`
    (isteğe bağlı `-H "X-Sha256: <özet>"`), `redox media list`, `redox media fetch <adres> [özet]`,
    `redox media delete <dosya>`.
  - Var olan dosyanın üzerine yazılmaz; bir konuğa takılı kalıp silinmez. Tek dosya sınırı 64 GB'dir.
- **OVA içe ve dışa aktarma:** VMware ya da VirtualBox'tan aldığınız `.ova` dosyasını yükleyip konuk oluştururken
  *Disk imajı* olarak seçin: arşivdeki disk kendiliğinden çıkarılıp qcow2'ye çevrilir. Depolama sayfası arşivin
  önerdiği çekirdek ve bellek miktarını gösterir (formu siz doldurursunuz). Birden fazla diskli arşivde yalnızca ilk
  disk alınır. Dışa aktarmak için kapalı konuğun *Çoğalt ve Taşı* sekmesinde **OVA Olarak İndir** (ya da
  `curl -H "Authorization: Bearer $REDOX_TOKEN" -o vm.ova $REDOX_URL/api/v1/guests/103/export`): tanım ve bütün diskler
  (streamOptimized vmdk) tek arşivde gelir.
- **Askıya alma:** konuk sayfasının başlığında **Askıya Al** (ya da `redox vm suspend 103`): konuğun bellek ve aygıt
  durumu diske (`images/vm-<kimlik>.state`) yazılır ve konuk kapanır; sunucuyu yeniden başlatabilirsiniz. **Başlat**
  konuğu kaldığı yerden sürdürür (duraklatılmışken askıya alındıysa duraklatılmış döner). Askıdayken disk ve donanım
  değiştirilemez: geri yükleme, anlık görüntüye dönüş, disk ekleme reddedilir; yedek alınabilir (yedek yalnızca diski
  içerir). Vazgeçmek için **Askı Durumunu At** (`redox vm discard-suspend 103`): konuk sıfırdan açılır. Durum dosyası
  konuğun kullandığı bellek kadar yer tutar; QEMU sürümünü yükselttikten sonra eski durumdan dönülemeyebilir.
- **Konuk ajanı (VMware Tools karşılığı):** konuğun içine `qemu-guest-agent` kurun (Debian/Ubuntu:
  `apt install qemu-guest-agent`; Windows: virtio-win paketindeki *QEMU Guest Agent*). Ek ayar gerekmez; kanal her sanal
  makinede hazırdır. Ajan yanıt verdiğinde *Özet* sekmesinde işletim sistemi ve (NAT ağında bile) IP adresi
  görünür. Asıl kazanç yedektedir: konuk çalışırken alınan yedekte dosya sistemleri kopyadan hemen önce dondurulur,
  kopya başlar başlamaz çözülür; yedek listesinde `donduruldu` etiketi çıkar. Ajan yoksa her şey eskisi gibi çalışır
  (yedek, elektriği kesilmiş bir diskin tutarlılığındadır). Veritabanı gibi uygulamalar için ajanın `fsfreeze` kancaları
  konukta ayrıca ayarlanmalıdır; RedoX yalnızca dondurma isteğini gönderir.
- **Şablonlar ve bağlı klonlar:** *Çoğalt ve Taşı* sekmesinde **Şablona Çevir** (ya da `redox vm template 103 on`).
  Şablon başlatılamaz, diski değişmez (geri yükleme, anlık görüntü, disk ekleme reddedilir). Şablondan klonlarken
  *Bağlı klon* işaretliyse (`redox vm clone 103 web-2 --linked`) disk kopyalanmaz: klon bir saniyenin altında hazır olur
  ve yalnızca şablondan farkını tutar. Bağlı klonu olan şablon silinemez ve şablonluktan çıkarılamaz; önce klonları
  silin. Bağlı klonun yedeği tamdır (şablona bağlı değildir) ve tek başına geri yüklenir. Tam klon her zaman bağımsızdır.
- **Değişen blok takibi (hızlı artımlı yedek):** `--incremental-backups` yanına `--changed-block-tracking` ekleyin.
  QEMU, çalışan konuğun hangi bloklara yazdığını kalıcı bir haritada tutar; sonraki yedekte yalnızca o bloklar okunur
  ve yedek listesinde `hızlı` etiketi çıkar. Sonuç yine eksiksiz, kendi başına geri yüklenen bir yedektir.
  - Yalnızca **çalışan** konukta işe yarar; kapalı konuğun yedeği diski baştan sona okur (ve haritayı sıfırlar).
  - Harita güvenilmez olabilecek her durumda kendiliğinden tam okumaya dönülür: geri yükleme, anlık görüntüye dönüş,
    disk büyütme, disk ekleme/çıkarma, konuğun düzgün kapanmaması (çökme, `kill -9`, elektrik kesintisi), temel
    yedeğin silinmiş olması ya da haritayı desteklemeyen eski QEMU.
  - Arka arkaya en çok 6 yedek haritayla alınır; yedincisi diski baştan sona okur. Harita bir nedenle gerçeği
    yansıtmıyorsa hata bununla sınırlı kalır.
  - Konuğun disk dosyasına RedoX dışından (elle `qemu-img`, `qemu-io`, dosyayı değiştirmek) dokunursanız harita bunu
    göremez. Böyle bir şey yaptıysanız `images/vm-<kimlik>.cbt.json` dosyasını silin; sonraki yedek tam okur.
- **Ek disklerin yedeği:** yedek, alındığı anda konuğa takılı bütün diskleri kapsar. Konuk çalışıyorsa kopyalar tek bir
  QEMU işlemiyle (transaction) aynı anda başlar: diskler birbirine göre aynı andaki halleriyle yedeklenir. Tam yedekte
  her ek disk ayrı dosyadır (`<yedek>.disk1.qcow2`); artımlı yedekte hepsi aynı parça deposundadır.
  - **Geri yükleme**, yedekteki ek disklerden konuğa hâlâ takılı olanları da geri getirir. Yedekten sonra kaldırdığınız
    diski geri eklemez; sonradan eklediğiniz diske dokunmaz. Bir diski kaldırıp aynı numaraya yenisini taktıysanız
    o da yeni bir disktir: üzerine yazılmaz. Onay sayfası hangi disklerin geri yükleneceğini numaralarıyla gösterir.
  - Yedekten sonra büyüttüğünüz asıl disk geri yüklenince yedekteki boyutuna döner; kayıt da buna göre düzelir.
  - Diskler önce yanlarına yazılır, hepsi hazır olunca yerlerine konur: biri eksik ya da bozuksa hiçbir diske dokunulmaz.
  - **İndir** asıl diski verir; her ek disk için ayrı düğme vardır (`.../download?disk=1`). Dış hedefe hepsi gider.
  - Bu özellikten önce alınmış yedekler yalnızca asıl diski içerir ve öyle geri yüklenir.
- **Aygıt aktarımı:** *Donanım* sekmesine `lsusb` kimliğini (`046d:c52b`) ya da `lspci -D` adresini (`0000:01:00.0`)
  yazın. USB için hizmet kullanıcısının aygıta erişimi (udev kuralı), PCI için BIOS'ta IOMMU, çekirdekte
  `intel_iommu=on` ve aygıtın `vfio-pci` sürücüsüne bağlanması gerekir. **Gerçek donanımla denenmedi.**
- **Port yönlendirme:** `sunucu-portu:konuk-portu` (TCP). Port sunucunun bütün arayüzlerinde dinler;
  dışarıdan erişim için güvenlik duvarında ayrıca açın: `sudo ufw allow 8080/tcp`.
- **Ölçümler:** `https://redox.example.com/metrics` Prometheus biçimindedir; `Authorization: Bearer <token>` ister
  (salt okunur bir hesabın token'ı yeterlidir).

Şimdilik yapmadıkları:

- **Varsayılan ağ NAT'tır (kullanıcı kipi).** Konuk internete çıkar; dışarıdan yalnızca yönlendirdiğiniz portlardan erişilir. Köprü ve konteynerler için 9. bölüme bakın.
- **Gerçek küme yok:** kesintisiz canlı taşıma ve sunucular arası kendiliğinden devralma yok. Birden çok sunucuyu tek
  sayfada görmek ve çalışan konuğu taşımak için 10. bölüme bakın.
- **Konuk güvenlik duvarı varsayılan olarak kapalıdır;** açmak için 12. bölüme bakın.

## 9. Köprü ağı ve LXC konteynerleri

> **Bu bölüm gerçek bir sunucuda denenmedi.** Kod, sahte araçlarla süreç düzeyinde test edildi; köprü
> yardımcısı ve yetkisiz LXC kök yetkisiyle kurulum istediği için geliştirme ortamında çalıştırılamadı.
> Adımları sırayla uygulayın; takıldığınız yerde `journalctl -u redox -n 50` çıktısına bakın.
> Köprü kurulumu ağ bağlantınızı kesebilir: SSH yerine sunucunun konsolundan yapın.

### 9.1 Köprü (konuklar sunucunun ağından adres alır)

1. Sunucuda bir köprü kurun. Netplan örneği (`eth0` yerine kendi arayüzünüz):

   ```yaml
   # /etc/netplan/60-vmbr0.yaml
   network:
     version: 2
     ethernets:
       eth0: { dhcp4: false }
     bridges:
       vmbr0:
         interfaces: [eth0]
         dhcp4: true
   ```

   ```bash
   sudo netplan try        # 120 saniye içinde onaylamazsanız geri alır
   ```

2. QEMU'nun köprü yardımcısına izin verin:

   ```bash
   echo "allow vmbr0" | sudo tee -a /etc/qemu/bridge.conf
   sudo chmod u+s /usr/lib/qemu/qemu-bridge-helper
   ```

3. Köprü ve konteyner destekli birimi kullanın (sandbox'ı daha gevşektir; nedenleri dosyada):

   ```bash
   sudo cp deploy/redox-host.service /etc/systemd/system/redox.service
   sudo systemctl daemon-reload && sudo systemctl restart redox
   ```

Bundan sonra oluşturma ve düzenleme formlarındaki **Ağ** kutusunda `Köprü: vmbr0` çıkar. Köprüdeki konuğun
adresi, konuk ağda bir paket gönderdikten sonra sunucunun komşu tablosundan bulunup gösterilir. Her konuğun
MAC adresi kimliğinden türetilir (`52:54:00:…`) ve değişmez; DHCP sunucunuzda sabit adres tanımlayabilirsiniz.
Köprüdeki konukta port yönlendirme kullanılmaz.

### 9.2 LXC konteynerleri (yetkisiz)

```bash
sudo apt install -y lxc uidmap acl
# Hizmet kullanıcısına konteyner kimlikleri için aralık ver.
sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 redox
# Konteynerlerin köprülere sanal kart takmasına izin ver (en çok 20 kart).
printf 'redox veth lxcbr0 20
redox veth vmbr0 20
' | sudo tee -a /etc/lxc/lxc-usernet
# Konteynerin içindeki kök (sunucuda 100000 numaralı kimlik) veri dizininden geçebilmeli.
sudo setfacl -m u:100000:x /var/lib/redox
sudo systemctl restart redox
journalctl -u redox -n 20 --no-pager      # "sürücü: qemu-kvm+lxc" yazmalı
```

- **Şablonlar:** alpine-3.20, debian-12, ubuntu-24.04, rockylinux-9. İlk oluşturmada imaj
  `images.linuxcontainers.org` adresinden indirilir (sunucunun internete çıkması gerekir); oluşturma isteği
  indirme bitene kadar bekler.
- **Ağ:** `NAT` seçilirse konteyner LXC'nin kendi köprüsüne (`lxcbr0`, `lxc-net` hizmeti kurar) bağlanır;
  köprü seçilirse doğrudan o köprüye.
- **Konsol sekmesi** konteynerde komut çalıştırır (kök olarak, en çok 20 saniye) ve çıktısını gösterir.
  Etkileşimli programlar çalışmaz. Komut metni günlüğe yazılmaz.
- **Sınırlar:** bellek ve çekirdek sınırı uygulanır; **disk boyutu sınırı uygulanmaz** (kök dosya sistemi
  düz bir dizindir). Klon ve disk büyütme konteynerlerde yok.
- **Anlık görüntü ve yedek** konteyner **kapalıyken** alınır. Anlık görüntü `lxc-snapshot` ile, yedek kök
  dosya sisteminin `tar.gz` arşivi olarak (`/var/lib/redox/backups/ct-….tar.gz`) tutulur; geri yükleme
  kök dosya sistemini boşaltıp arşivi açar, sınırlar ve ağ ayarı değişmez. Zamanlanmış yedek çalışan
  konteyneri atlar ve görev günlüğüne hata yazar.
- LXC'yi kullanmak istemiyorsanız `--no-lxc` ile kapatın. Hizmet kök olarak çalışırsa konteynerler
  kimlik eşlemesiz (yetkili) olur; sunucu bunu başlarken uyarır.

## 10. Uyarılar ve birden çok sunucu

### 10.1 Uyarılar

Sunucu 15 saniyede bir şunları denetler: düğüm ya da depolama doluluğu, çevrimdışı düğüm, başarısız görev,
beklenmedik biçimde duran konuk. Etkin uyarılar ana sayfanın üstünde görünür. Dışarıya bildirim için
hizmet satırına bir webhook ekleyin (Slack, Discord, ntfy gibi `text`/`content` alanı okuyan her servis):

```
ExecStart=/usr/local/bin/redox serve ... --alert-webhook https://hooks.example.com/…
```

Adres hizmet dosyasında düz metin durur; dosyayı yalnızca kökün okuyabildiğinden emin olun.
Gönderim başarısız olursa olay yeniden denenmez, yalnızca günlüğe yazılır.

**E-posta:** bir dosya yazıp hizmet satırına `--alert-email /var/lib/redox/mail.conf` ekleyin:

```
host = smtp.example.com
port = 465
user = redox@example.com
password = ...
from = redox@example.com
to = ben@example.com, nobet@example.com
```

Bağlantı baştan TLS'lidir (465. kapı); STARTTLS (587) desteklenmez. Bir turdaki olaylar tek iletide gönderilir.
Yalnızca sahte bir SMTP sunucusuyla sınandı; gerçek bir sağlayıcıyla (TLS dahil) denenmedi.

**Grafikler:** düğüm ve konuk sayfalarında son 10 dakika, 24 saat (dakikalık ortalama) ve 30 gün (saatlik ortalama)
seçilebilir. Geçmiş `metrics.json` dosyasında tutulur ve yeniden başlatmada korunur; sunucunun kapalı olduğu süreler
grafikte yer almaz.

### 10.2 Birleşik görünüm (birden çok sunucu)

Birkaç RedoX VE sunucunuz varsa birini "merkez" yapıp diğerlerinin düğüm ve konuklarını **Küme**
sayfasında görebilirsiniz. Bu **gerçek bir küme değildir**: görünüm salt okunurdur, her sunucu kendi
konuklarını yönetir; ortak durum ve kendiliğinden devralma yoktur (konuk taşıma aşağıda). Konuğun adına tıklamak
sizi o sunucunun arayüzüne götürür (orada ayrıca giriş yaparsınız).

1. Her **eş** sunucuda salt okunur bir kullanıcı ve token oluşturun:

   ```bash
   sudo -u redox redox user add merkez --role viewer --users /var/lib/redox/users.json
   sudo -u redox redox token create merkez birlesik --users /var/lib/redox/users.json
   ```

2. **Merkez** sunucuda eş dosyasını yazın (her satır `AD ADRES TOKEN`):

   ```bash
   sudo install -o redox -g redox -m 0600 /dev/null /var/lib/redox/peers.conf
   sudo -u redox tee /var/lib/redox/peers.conf >/dev/null <<'EOF'
   # ad     adres                        token
   izmir   https://izmir.example.com    redox_...
   EOF
   ```

3. Hizmet satırına `--peers /var/lib/redox/peers.conf` ekleyip yeniden başlatın. Günlükte
   `Birleşik görünüm: 1 eş sunucu (izmir).` yazmalı. Eşler 15 saniyede bir okunur.

Adres `https://` olmalıdır (token ağdan geçer); şifresiz `http://` yalnızca aynı makinedeki eşlerde kabul
edilir. Eşin sertifikası geçerli olmalıdır (kendinden imzalı sertifika reddedilir). Erişilemeyen ya da
token'ı reddeden eş sayfada nedeniyle birlikte gösterilir; diğerleri etkilenmez. Komut satırından:
`redox peer list`.

**Sanal makine taşıma:** eşteki **kapalı** bir sanal makineyi merkeze kopyalamak için *Küme* sayfasındaki
**Buraya Al** düğmesini ya da `redox peer import izmir 103` komutunu kullanın. Bunun için eş dosyasındaki token
eşte **yönetici** yetkili olmalıdır. Merkez eşte bir yedek aldırır, indirir, yeni bir kimlikle konuğu oluşturur ve
eşteki geçici yedeği siler. Kaynak konuk yerinde kalır: kopyanın çalıştığını görünce eşte siz silersiniz. Köprü
merkezde yoksa ağ NAT'a çevrilir; USB/PCI aktarımı taşınmaz. Konteynerler ve ek diskli sanal makineler taşınmaz.

**Çalışan konuğu taşıma (Çalışırken Al):** eşte çalışan (ya da askıda duran) bir sanal makinede aynı düğme
**Çalışırken Al** adını alır; komut aynıdır. Merkez konuğu eşte askıya aldırır, diskini ve bellek durumunu indirir
ve konuğu burada **kaldığı yerden** sürdürür: konuk yeniden başlamaz, açık programları ve bağlantı durumu korunur.
Bu kesintisiz bir canlı taşıma değildir: konuk, disk ve bellek kopyalanırken (disk boyutuyla orantılı bir süre)
yanıt vermez. Kimlik merkezde boşsa konuk aynı kimlikle gelir (ağ kartının adresi değişmez). Koşullar: iki sunucu
aynı RedoX sürümünü ve benzer işlemciyi kullanmalı, merkezde en az konuk kadar çekirdek ve bellek olmalı, konuğa
sunucu aygıtı (USB/PCI) aktarılmamış ve konukta paylaşılan klasör bağlı olmamalı. Bellek durumu konuğun belleğinin
tamamıdır; yalnızca yönetici token'ıyla indirilir. Taşıma başarısız olursa merkezde yarım kopya bırakılmaz ve konuk
eşte askıda durur: orada başlatırsanız kaldığı yerden devam eder. Başarılıysa eşteki kopya yine askıda kalır;
merkezde çalıştığını görünce eşte **silin, başlatmayın** (iki kopya aynı adresle çalışır).

**Sunucular arası kendiliğinden devralma yoktur.** Bir sunucu çöktüğünde konuklarını başka sunucuda otomatik açmak,
çöken sunucunun gerçekten kapandığından emin olmayı (çitleme) ve ortak ya da eşlenmiş depolamayı gerektirir; bunlar
olmadan aynı diskin iki yerde açılması veriyi bozar. Sunucu geri gelince işaretli konuklar açılış sırasıyla başlar.

## 11. Yedekleri sunucu dışına kopyalama

Yedekler varsayılan olarak aynı diskte durur; disk giderse yedek de gider. Bir dış hedef tanımlayın:

```bash
sudo install -o redox -g redox -m 0600 /dev/null /var/lib/redox/offsite.conf
sudo -u redox tee /var/lib/redox/offsite.conf >/dev/null <<'EOF'
# Bağlı bir dizin (ikinci disk, NFS, SMB):
kind = dir
path = /mnt/yedek
# ya da S3 uyumlu bir depo (AWS, MinIO, Backblaze B2, Wasabi):
# kind = s3
# endpoint = https://s3.eu-central-1.amazonaws.com
# region = eu-central-1
# bucket = yedek-kovam
# prefix = redox/
# access_key = ...
# secret_key = ...
# İsteğe bağlı: hedefe gitmeden önce şifrele (en az 12 karakter). Parolayı kaybederseniz yedekler açılamaz.
passphrase = buraya-uzun-bir-parola
EOF
```

Hizmet satırına `--offsite /var/lib/redox/offsite.conf` ekleyip yeniden başlatın.

- Her yeni yedek (elle ya da zamanlanmış) alınır alınmaz hedefe de kopyalanır; kopya başarısız olursa yerel yedek
  yine geçerlidir ve hata görev günlüğüne (dolayısıyla uyarılara) düşer. Sonradan: `redox backup push <yedek>`.
- Yerel dosya kaybolmuşsa **Geri Yükle** dış kopyayı kendiliğinden indirir. Yedek silinince dış kopyası da silinir.
- Şifreleme istemci tarafındadır (ChaCha20-Poly1305, parola Argon2id ile anahtara çevrilir): hedef yalnızca şifreli
  veriyi görür; değiştirilmiş ya da kesilmiş dosya geri yüklenirken reddedilir.
- Şifreli kopya **yedeğin adına bağlıdır**: hedefe yazabilen biri iki yedeğin dosyalarını yer değiştirirse ya da
  birini öbürünün adıyla kopyalarsa geri yükleme "başka bir yedeğe ait" diye reddedilir ve diske dokunulmaz. Bu
  sürümden önce gönderilmiş kopyalar ada bağlı değildir; okunmaya devam ederler, bağlamak için yedeği yeniden
  gönderin (`redox backup push <yedek>`). Şifresiz hedefte (parola verilmemişse) bu koruma da yoktur.
- **Artımlı yedekler hedefe de artımlı gider** (`--incremental-backups` açıkken): hedefte parçalar `chunks/` altında
  bir kez durur, her yedeğin küçük bir dizin dosyası (`<yedek>.rdxi.json`) vardır. Yeni yedekte yalnızca hedefte
  olmayan parçalar gönderilir; dizin en son gider, yani hedefte dizini görünen yedeğin bütün parçaları oradadır.
  - Geri yüklemede yalnızca yerelde eksik parçalar indirilir ve her biri yerine konmadan önce özetiyle doğrulanır;
    hedefte bozulmuş ya da eksik tek parça geri yüklemeyi durdurur, konuğun diski değişmez.
  - Yedek silinince hedefte yalnızca ona özgü parçalar silinir. Hangi parçaların başka yedeklerce kullanıldığı
    belirlenemezse (ör. bir dizin okunamıyorsa) hiçbir parça silinmez; hedefte artık dosya kalır, veri kaybolmaz.
  - `redox backup push <yedek>` bütün parçaları hedefe sorar ve eksik olanları yeniden gönderir (hedefte elle bir şey
    silindiyse onarmak için). Otomatik kopyada, dış kopyası olan diğer yedeklerin parçaları sorulmadan atlanır.
  - **Şifreleme parolasını değiştirmeyin.** Hedefe ilk göndermede küçük bir işaret dosyası (`redox-store.json`) konur
    ve her göndermede okunur; parola değişmişse gönderme durur ve bunu söyler (yoksa eski parolayla şifrelenmiş
    parçalar "hedefte var" sayılır, yeni yedeğin dış kopyası açılamazdı). Parolayı değiştirmek için boş bir hedef verin.
  - `--offsite` hedefini değiştirirseniz yeni yedekler eksiksiz gider: eski yedeklerin "dış kopyası var" işaretine
    güvenilmez, dizinlerinin yeni hedefte gerçekten durduğu denetlenir.
  - Yerelde bir parça bozulmuşsa geri yükleme önce durur (diske dokunmadan), dış kopyadan o parçayı indirip onarır ve
    yeniden dener.
  - Yedek silinince dış kopyası arka planda silinir; hedefe o an erişilemiyorsa dış kopya kalır ve hata görev
    günlüğüne `backup unpush` olarak düşer (elle silmeniz gerekir).
  - Hedefteki `chunks/` klasörünü elle düzenlemeyin ve aynı hedefi iki ayrı RedoX sunucusuna vermeyin: biri, diğerinin
    kullandığı parçaları gereksiz sanıp silebilir. Her sunucuya ayrı dizin ya da ayrı `prefix` verin.
  - Şifreleme açıksa parça adları (içerik özetleri) şifrelenmez: hedefi gören biri veriyi okuyamaz ama iki yedeğin
    hangi parçaları paylaştığını görebilir.
- **Hedefe ne kadar güvenilir:** şifreleme açıkken hedef veriyi okuyamaz ve değiştiremez (değişen dosya reddedilir).
  Artımlı yedekte her dizin hangi yedeğe ait olduğunu taşır; başka bir yedeğin dizini onun yerine konursa fark edilir.
  Tam yedekler ve konteyner arşivleri de adlarına bağlıdır: başka bir yedeğin dosyası onun adıyla konursa geri yükleme
  reddedilir (bu sürümden önce gönderilmiş kopyalar hariç; yukarıya bakın). Şifresiz hedefte ise dosyalar hem yer
  değiştirilebilir hem de içerikleri değiştirilebilir; geri
  yüklenen tam yedek imajının başka bir dosyaya bağlanmadığı denetlenir, içeriği denetlenemez. Güvenmediğiniz
  hedeflerde şifrelemeyi açın.
- S3'te tek dosya sınırı 5 GB'dir (parçalı yükleme yok). Artımlı yedeklerde parçalar en çok 4 MiB olduğu için bu sınır
  onları etkilemez; tam yedekler ve konteyner arşivleri için geçerlidir. S3'te her yeni parça için bir sorgu ve bir
  yükleme isteği yapılır; çok büyük ilk yedek uzun sürebilir.
- S3 hedefi yalnızca yerel bir sahte sunucuyla sınandı; gerçek bir sağlayıcıyla denenmedi. İmza hesabı AWS'nin
  belgelerindeki örnekle doğrulandı.

## 12. Konuk güvenlik duvarı

```bash
sudo apt install -y nftables
```

Hizmet biriminde `AmbientCapabilities=CAP_NET_ADMIN` satırını ekleyin (yalnızca `redox-host.service`), hizmet satırına
`--firewall` yazın ve yeniden başlatın. Günlükte `Konuk güvenlik duvarı açık` görünmeli.

- Konuğun *Seçenekler* sekmesinde **Gelen trafiği kısıtla** kutusunu işaretleyip izin verilen trafiği yazın:
  `tcp/22 from 10.0.0.0/8`, `tcp/80-443`, `udp/53`, `icmp` (satır başına bir kural).
- Açıkken kurallara uymayan **gelen** bağlantılar düşürülür; konuğun başlattığı bağlantılar ve yanıtları serbesttir.
- Kurallar köprüdeki konuklara ve bütün konteynerlere uygulanır (nftables, `bridge` ailesi, konuğun MAC adresine göre).
  NAT'taki sanal makineye dışarıdan zaten yalnızca yönlendirdiğiniz kapılardan erişilir; orada etkisizdir ve sayfa bunu söyler.
- `CAP_NET_ADMIN` hizmete sunucunun ağ ayarlarını değiştirme yetkisi verir; bu yüzden varsayılan olarak kapalıdır.
- **Gerçek sunucuda denenmedi.** Üretilen kuralları nftables kabul ediyor, ama geliştirme ortamının çekirdeğinde
  `bridge` ailesi olmadığı için kurallar yüklenip trafikle sınanamadı. İlk açışınızda SSH ile değil konsoldan deneyin.

## 13. Roller, tek oturum açma ve oturumlar

| Rol | Yapabildikleri |
|---|---|
| Yönetici | Her şey. |
| Operatör | Kapsamındaki konukları açıp kapatır, konsolunu kullanır, anlık görüntü ve yedek **alır**. Konuk oluşturamaz, silemez, ayar değiştiremez, geri yükleyemez. |
| İzleyici | Yalnızca görüntüler. |

```bash
sudo -u redox redox user add nobet --role operator --users /var/lib/redox/users.json
sudo -u redox redox user scope nobet web,test --users /var/lib/redox/users.json   # yalnızca bu etiketli konuklar
```

Kapsam boşsa operatör bütün konuklarda yetkilidir. Operatör bütün konukları **görür**; kapsam yalnızca işlem yapmayı
kısıtlar. Kaynak kotası yoktur. Rol ve kapsam *Kullanıcılar* sayfasından da değişir.

**Tek oturum açma (OpenID Connect):** kimlik sağlayıcınızda (Keycloak, Authentik, Google, Microsoft Entra) bir istemci
oluşturun; dönüş adresi `https://redox.example.com/login/oidc/callback` olsun.

```bash
sudo install -o redox -g redox -m 0600 /dev/null /var/lib/redox/oidc.conf
sudo -u redox tee /var/lib/redox/oidc.conf >/dev/null <<'EOF'
issuer = https://id.example.com/realms/sirket
client_id = redox
client_secret = ...
redirect_url = https://redox.example.com/login/oidc/callback
# claim = email      (ya da preferred_username, sub)
EOF
sudo -u redox redox user sso ayse ayse@example.com --users /var/lib/redox/users.json
```

Hizmet satırına `--oidc /var/lib/redox/oidc.conf` ekleyin. Giriş sayfasında **Kurumsal hesapla giriş yap** bağlantısı
çıkar. Sağlayıcı yalnızca kimliği doğrular: giriş yapabilmek için o kimliğin `redox user sso` ile yerel bir kullanıcıya
eşlenmiş olması gerekir; rol ve kapsam yereldir. Bu yolla girişte yerel iki adımlı doğrulama sorulmaz (ikinci adımı
sağlayıcıda zorunlu kılın). Yalnızca sahte bir sağlayıcıyla sınandı; gerçek bir sağlayıcıyla denenmedi.

**Oturumlar** artık hizmet yeniden başlayınca düşmez: `/var/lib/redox/sessions.json` dosyasında, token'ın kendisi değil
özeti saklanır. Herkesi çıkarmak için dosyayı silip hizmeti yeniden başlatın.

## Bilinen eksikler

- **İki adımlı doğrulama isteğe bağlıdır.** Her yönetici Hesabım sayfasından açmalıdır; mümkünse paneli VPN ya da IP kısıtıyla da koruyun.
- **Kilit adres bazlıdır.** Çok sayıda adresten dağıtık deneme yapan biri yavaşlatılır ama tümüyle engellenmez; Argon2 her denemeyi pahalı kılar.
- **Giriş kilitleri bellekte.** Hizmet yeniden başlayınca hatalı deneme sayaçları sıfırlanır (oturumlar korunur).
- **Denetim kaydı istek düzeyindedir** (kim, nereden, hangi adres); form içerikleri kaydedilmez. Dosya (`audit.log`) kendiliğinden küçülmez; `logrotate` ile döndürün.
- **Yedekler, dış hedef tanımlamadıkça aynı sunucudadır** (11. bölüm). UEFI değişken deposu ve sanal TPM durumu yedeğe girmez.
- **Artımlı depoda bozulma kendiliğinden fark edilmez:** yeni yedekler, depoda zaten duran parçaları yeniden
  okumadan kullanır. Diskte bozulmuş bir parça, onu paylaşan bütün yedekleri etkiler ve ancak **Doğrula** ya da geri
  yükleme sırasında ortaya çıkar. Önemli yedekleri ara sıra doğrulayın; dış kopya varsa geri yükleme onarır.
- **Çok diskli geri yükleme anlık değildir:** diskler hazırlandıktan sonra tek tek yerine konur; tam o sırada sunucu
  çökerse bazı diskler yeni, bazıları eski kalabilir (yeniden geri yükleyin).
- **Artımlı yedek, değişen blok takibi kapalıyken yerde tasarruf eder, sürede etmez:** her yedekte disk baştan sona
  okunur. `--changed-block-tracking` ile çalışan konukta yalnızca değişen bloklar okunur. Dış hedefe yalnızca yeni
  parçalar gider.
- **Yapılmayanlar:** LVM/ZFS depolama havuzları, kesintisiz (sıfır duraksamalı) canlı taşıma, sunucular arası
  kendiliğinden devralma, kaynak kotaları,
  konteyner klonu, konuğu başka depolama alanına taşıma, canlı konsolda pano ve ses.
- **QEMU birimi** (`redox-qemu.service`) simülasyon birimine göre daha gevşek bir sandbox kullanır; nedenleri dosyada yazılı.
- Bu kılavuzdaki systemd birimleri ve Caddy ayarı gerçek bir sunucuda denenmedi. Kod ve testler Linux'ta (WSL, Ubuntu) çalıştırıldı.
  QEMU sürücüsü NAT ağıyla gerçek KVM üzerinde denendi (çalışan konukta anlık görüntü, geri dönüş ve yedek dahil);
  köprü kipi ve LXC (konteyner yedeği ve anlık görüntüsü dahil) yalnızca sahte araçlarla sınandı.
  Birleşik görünüm ve sanal makine taşıma iki sunucu arasında yerel HTTP üzerinden sınandı; HTTPS'li gerçek eşle denenmedi.
  Gerçek KVM'de ayrıca denenenler: hazır imajdan oluşturma ve cloud-init sürücüsünün takılması (konuğun içinde
  cloud-init'in çalıştığı gerçek bir bulut imajıyla doğrulanmadı), UEFI + güvenli önyükleme + TPM (gerçek OVMF ve swtpm),
  ek disk ve ikinci ağ kartı, vmdk içe aktarma, yedek indirme, artımlı yedek (kapalı ve çalışan konukta; değişen
  kısım kadar büyüme, her yedeğin bayt bayt geri gelmesi, temel yedek silindikten sonra geri yükleme, bozuk parçanın
  yakalanması) ve artımlı yedeğin şifreli bir dizin hedefine parça parça gitmesi (yerel yedek dizini tümüyle silindikten
  sonra hedeften geri yükleme, hedefte bozulan parçanın yakalanması, silmede yalnızca yedeğe özgü parçaların gitmesi).
  Ek diskli konuğun yedeği de gerçek KVM'de (tam ve artımlı, kapalı ve çalışan konukta) denendi; ancak çalışan konuk
  disklere yazarken alınan yedeğin diskler arası tutarlılığı sınanmadı (denemedeki konukta işletim sistemi yoktu).
  Bu turda gerçek KVM'de denenenler: bağlı klon, değişen blok takibi (çalışan konuğa yazıp geri yüklenen diskle
  karşılaştırma; çökme ve anlık görüntüye dönüş sonrası tam okumaya dönüş), OVA dışa/içe aktarma, kalıp yükleme ve
  askıya alma (geri dönen konuğun belleği askıdan önceki haliyle birebir karşılaştırıldı).
  Denenmeyenler: konuk ajanı gerçek bir konukta (yalnızca sahte ajanla sınandı; denemedeki konukta işletim sistemi
  yoktu), adresten indirme gerçek bir HTTPS sunucusuyla, dışa aktarılan OVA'nın VMware ya da VirtualBox'a alınması
  (arşiv sistem `tar`'ıyla açıldı, diskler ve XML doğrulandı).
  Parçalı eşitleme S3 hedefinde yalnızca sahte sunucuyla, tek tek dosya işlemleri düzeyinde sınandı. Artımlı yedek birkaç GB'lik disklerle denendi; yüzlerce GB'lik disklerde süre ve bellek ölçülmedi.
