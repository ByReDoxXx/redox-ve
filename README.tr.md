<p align="center">
  <img src="crates/redox-server/assets/logo.svg" alt="RedoX VE simgesi" height="96">
  &nbsp;&nbsp;
  <img src="crates/redox-server/assets/wordmark.svg" alt="RedoX VE" height="96">
</p>

<p align="center">
  <a href="README.md">English</a> · <b>Türkçe</b>
</p>

# RedoX VE

RedoX VE, Rust ile yazılmış, kendi sunucunuza kurduğunuz, [Proxmox VE](#redox-ve-ve-proxmox-ve) benzeri
bir sanallaştırma platformudur. Tek bir program
(`redox`) bir Linux sunucuda KVM sanal makinelerini ve yetkisiz LXC konteynerlerini çalıştırır; bunları
yönetmek için bir web paneli, bir REST API ve bir komut satırı istemcisi sunar.

Tek bir sunucu ya da birbirinden bağımsız birkaç sunucu için tasarlandı: ev laboratuvarı, küçük ofis,
geliştirme sunucusu. Kümelenmiş bir sanallaştırıcı değildir ([Sınırlar](#sınırlar) bölümüne bakın).

![Genel Bakış sayfası](docs/img/overview.jpg)

## İçindekiler

- [Özellikler](#özellikler)
- [RedoX VE ve Proxmox VE](#redox-ve-ve-proxmox-ve)
- [Ekran görüntüleri](#ekran-görüntüleri)
- [Kurulum](#kurulum)
- [Komut satırı ve API](#komut-satırı-ve-api)
- [Teknik doküman](#teknik-doküman)
- [Güvenlik modeli](#güvenlik-modeli)
- [Testler](#testler)
- [Sınırlar](#sınırlar)
- [Lisans](#lisans)

## Özellikler

**Konuklar**

- QEMU üzerinden KVM sanal makineleri ve yetkisiz LXC konteynerleri; ikisi de aynı sayfalardan yönetilir.
- Konteynerler klonlanır ve çalışırken yedeklenir (arşiv süresince dondurulur).
- BIOS, UEFI, güvenli önyüklemeli UEFI ve sanal TPM 2.0 (Windows 11 için yeterli).
- ISO'dan kurulum ya da hazır bulut imajından cloud-init ile oluşturma (kullanıcı, SSH anahtarı, makine adı).
  cloud-init ayarları, sabit adres dahil, sonradan panelden düzenlenir.
- Sanal makine başına seçilebilen işlemci modeli ve hedefi çalışırken değişen bellek balonu.
- Çalışan sanal makineye çekirdek ve bellek ekleme (`redox vm cores 103 6`, `redox vm memory 103 8192`); üst sınır konuk başına verilir.
- Çalışan sanal makinenin diskleri, konuk durdurulmadan başka depolama alanına taşınır (`redox vm move 103 hizli`).
- Bakım kipi sunucuyu boşaltabilir: çalışan sanal makineler eşe canlı taşınır (`redox maintenance on --evacuate <eş>`).
- Sanal makine başına işlemci payı ve bellek garantisi (`redox vm set 103 cpu-shares=200 mem-reserve=4096`): sunucu sıkışınca işlemciyi kimin alacağı ve geri alınmayacak bellek; ikisi de konuk çalışırken değişir.
- Kilit kipi (`redox lockdown on`): açıkken yöneticiler dahil hiç kimsenin arayüzden ya da API'den değişiklik yapamadığı bir değişiklik dondurma; konuklar ve zamanlanmış işler sürer.
- Hata toleransı (`redox ft`, QEMU COLO): çalışan sanal makinenin ikinci sunucudaki ikizi, konuk yeniden başlamadan devralır; üçüncü sunucu hakemdir. Desteklenen donanım dardır, korunurken yedek alınamaz.
- Eşzamanlı disk yansıtma (`redox mirror`): çalışan sanal makinenin her yazması eşteki bir kopyaya da gider; eş donarsa bekçi konuğu kurtarır, eş kopyadan (elle) devralabilir.
- Dağıtık anahtarlar (`redox switch`): bir kez tanımlanıp bütün eşlere gönderilen ortak VXLAN konuk ağı; ağı küçük bir root yardımcısı kurar ve yalnızca kendi aygıtlarına dokunur.
- Yük dengeleme önerisi (`redox balance`): sunucu sıkıştığında çalışan hangi sanal makinenin hangi eşe canlı taşınacağını söyler; kendiliğinden bir şey taşımaz, taşımayı `redox peer send` yapar.
- Sunucu profili (`redox profile export | diff | apply`): bir sunucunun ayarlarını başkasıyla karşılaştırır, havuzları ve yedek işlerini eşitler.
- Destek paketi (`redox support > destek.json` ya da *Sunucular* sayfası): sürüm, yetenekler, ayarlar, konukların donanımı ve son görevler tek dosyada; parola, token ve denetim günlüğü içermez.
- Sunucunun kendi konsolunda durum ekranı: ad, adresler, panelin adresi, hizmetin yanıt verip vermediği ve çalışan konuk sayısı; Enter ile oturum açılır.
- `vmdk`, `vdi`, `vhdx` disklerini ve OVA arşivlerini içe aktarma; konuğu OVA olarak dışa aktarma.
- Şablonlar, tam klonlar ve bağlı klonlar (bağlı klon bir saniyenin altında hazır olur).
- En fazla 8 ek veri diski ve 3 ek ağ kartı; ikisi de çalışan sanal makineye takılabilir.
- Diske askıya alma ve kaldığı yerden sürdürme, duraklatma, zamanlanmış başlatma/kapatma/yeniden başlatma,
  sunucu açılışında başlatma sırası.
- Çöken ya da içinden kapanan konuğun kendiliğinden yeniden başlatılması.
- Sunucu ile sanal makine arasında paylaşılan klasörler (virtio-9p), USB ve PCI aygıt aktarımı.
- Dinamik bellek (balon hedefi konuğun kullanımını izler) ve iç içe sanallaştırma.
- Konuk başına kaynak ölçümü: faturalama ve planlama için birikimli çekirdek-saat, GB-saat ve disk G/Ç.
- Köprüdeki kartlarda ağ kartı koruması: sahte MAC, sahte DHCP sunucusu ve yönlendirici duyurusu engellenir.
- Bakım kipi: sunucu güncellemesinden önce konukları kapatır, sonra yeniden başlatır.
- Konuk içi disk doluluğu (ajanla) ve uyarısı; uyarılar web kancası, e-posta, ntfy ve Telegram ile gider.
- Salt okunur API token'ları ve isteğe bağlı API hız sınırı.
- Yerel yedek şifrelemesi (`--backup-passphrase-file`): artımlı yedek parçaları diskte şifreli durur.
- Sunucular arası ortak konuk ağı için VXLAN kurulum komutları (`redox net vxlan`), aracılı aygıtla ekran kartı paylaşımı (vGPU) ve depolamanın altındaki ZFS, LVM ve NFS'in tanınması.
- API'nin OpenAPI belgesi (`/api/v1/openapi.json`), canlı konsola metni tuş vuruşu olarak yazdırma, şifreli konteyner yedekleri ve VXLAN için kalıcı systemd birimi (`redox net vxlan … --unit`).
- Proxmox gibi kurulum: `deploy/iso/build-iso.sh` önyüklenebilir bir kurulum ISO'su üretir, `deploy/install.sh` yeni bir Debian'ı tek komutla RedoX VE sunucusuna çevirir (ISO üretildi ve bir sanal makinede baştan sona kuruldu; fiziksel makinede henüz denenmedi).
- Hakemli kendiliğinden devralma: kaynak sunucu ölünce ve üçüncü bir sunucu onaylayınca yedek kopya başlatılır.
- Artımlı çoğaltma: ilk turdan sonra ağdan yalnızca değişen 4 MiB'lik bloklar geçer; her blok özetiyle doğrulanır.
- Konteynerler sunucular arasında kopyalanır ve çoğaltılır; yedek alanının ne zaman dolacağı tahmin edilir; kaynak
  ölçümü ve denetim günlüğü CSV olarak indirilir; imaj kataloğunda virtio-win sürücü kalıbı vardır.
- Baştan sona Türkçe ve İngilizce: üst çubukta dil düğmesi, sunucu varsayılanı için `--lang en`, API hata iletileri
  `Accept-Language` başlığına göre, komut satırı `REDOX_LANG=en` ile.

- Windows ve aktarılan diskler için uyumluluk: SATA ya da virtio-scsi disk, Intel ağ kartı modelleri, ikinci CD sürücüsü, çalışırken kalıp değiştirme (`redox vm media`); disklerde TRIM açık.
- Performans: çekirdek sabitleme (çalışırken de), NUMA düğümüne bellek bağlama, büyük sayfalar; düğüm sayfasında KSM durumu.
- Yerleştirme etiketleri (`ayri-<grup>`, `birlikte-<grup>`, `sabit`) ve isteğe bağlı kendiliğinden yük dengeleme (`--auto-balance`).

**Sunucu ve kimlik**

- Panelden sunucu ağı (`redox hostnet`): bond, VLAN, köprü ve DHCP/DNS'li, NAT'lı ya da yalıtılmış konuk ağı. Adres ya da varsayılan yol taşıyan arayüze dokunulmaz.
- Disk sağlığı (SMART), IPMI algılayıcıları, bekleyen güncellemeler ve isteğe bağlı panelden güncelleme.
- Yerleşik TLS (`--tls-cert`, `--tls-key`, `redox tls self-signed`); Let's Encrypt için certbot yenileme kancası.
- LDAP / Active Directory ile giriş (`--ldap`, `redox user ldap`); roller yerel kalır.

**Depolama ve yedek**

- Çalışan sanal makinede anlık görüntü (bellekle birlikte) ve yedek.
- Saklama sayısıyla zamanlanmış yedek: konuk başına ya da birçok konuğu sırayla yedekleyen yedek işleriyle.
- UEFI değişkenleri ve sanal TPM durumu yedekle birlikte saklanır ve geri yüklenir.
- Yedeği yeni bir konuk olarak açma ya da konuğu doğrudan yedeğin üzerinde başlatma (anında kurtarma) ve sonradan ayırma.
- Açarak sınama: yedek, yalıtılmış geçici bir konukta başlatılarak gerçekten açıldığı kanıtlanır; istenirse bir
  TCP kapısının yanıt vermesi ve konuk içinde bir komutun başarıyla bitmesi de beklenir.
- Üretim denetim noktası: çalışan sanal makinenin, dosya sistemleri konuk ajanıyla dondurularak alınan belleksiz
  görüntüsü.
- Artımlı yedekte olağandan çok fazla veri değiştiğinde uyarı (fidye yazılımı belirtisi olabilir).
- Konuk başına yedek yaşı hedefi (RPO) ve arşiv: kopya hedefleri, yerelde silinen yedekleri belirlenen gün kadar
  saklayabilir.
- Dört göz ilkesi (isteğe bağlı): yedek, konuk, disk, yedek işi ya da kullanıcı silmek ikinci bir yöneticinin
  onayını ister.
- Çoğaltma: eş sunucudaki sanal makinenin, aralıkla tazelenen ve devralmada elle başlatılan yedek kopyası.
- En yeni yedeklerin zamanlanmış açarak sınanması ve hazır bulut imajı kataloğu.
- Dosya düzeyinde geri yükleme: yedeğin içinde gezme, tek dosya indirme ya da bir dosyayı/klasörü konuk ajanıyla doğrudan çalışan konuğun içine geri yazma (libguestfs ile).
- Haftalık, aylık, yıllık saklama (GFS), kilitli (değiştirilemez) yedekler, yedek penceresi ve kopyalama hız sınırı.
- Canlı yedekten önce ve sonra konuk ajanıyla konuğun içinde komut çalıştırma.
- E-postayla günlük yedek raporu ve yakın zamanda yedeği alınmamış konukların listesi.
- Artımlı yedek: içeriğe göre adlandırılan 4 MiB'lik parça deposu. Zincir yoktur; herhangi bir yedek silinebilir.
- Değişen blok takibi: çalışan sanal makinenin artımlı yedeğinde yalnızca değişen bloklar okunur.
- Bir dizine (ikinci disk, NFS, SMB) ya da S3 uyumlu bir depoya dış kopya; şifreleme istemci tarafındadır.
  Şifreli kopya yedeğin adına bağlıdır: hedefte yer değiştirilen dosyalar geri yüklemede reddedilir.
  Büyük dosyalar S3'e parça parça yüklenir. Yedekler birden çok hedefe kopyalanabilir (3-2-1).
- Yedek doğrulama ve yerelde bozulan parçanın dış kopyadan onarılması.
- Sanal makine diskleri için ek depolama dizinleri ve kapalı konuğun disklerini aralarında taşıma.
- Çekirdek, bellek, disk ve konuk sayısına kota koyan kaynak havuzları.

**Ağ**

- Port yönlendirmeli kullanıcı kipi NAT ya da köprü ağı.
- Ağ kartı başına VLAN etiketi; konuk başına disk ve ağ hız sınırı.
- Konuk başına gelen ve giden trafik güvenlik duvarı (nftables), yeniden kullanılan güvenlik grupları ve adres kümeleri; konuğun bütün ağ kartları süzülür.

**İşletim**

- Sunucuda üretilen web paneli; koyu ve açık tema; JavaScript olmadan kullanılır (canlı konsol, terminal ve güvenlik anahtarı adımı dışında).
- Ortak depoda (ör. NFS) disk kopyalamadan taşıma ve devralma; LUKS ile disk şifreleme.
- Güvenlik anahtarı (WebAuthn) ile ikinci adım.
- Tarayıcıda konteyner terminali; sanal makinede ses kartı (canlı konsoldan dinlenir).
- OCI / Docker imajından konteyner. Ayrıntılar ve sınanmayanlar: [KURULUM.md](KURULUM.md), 14. bölüm.
- Süreli API token'ları, açık oturum yönetimi, operatöre kısmi yetki, denetim günlüğünü syslog'a gönderme, yerleşik
  Let's Encrypt istemcisi ve çalışan sanal makineden çekirdek / bellek çıkarma ([KURULUM.md](KURULUM.md), 15. bölüm).
- Tarayıcıda klavye ve fareyle canlı grafik konsol.
- Roller (yönetici, operatör, izleyici), etikete dayalı operatör kapsamı, TOTP ile iki adımlı giriş,
  OpenID Connect.
- Denetim günlüğü, görev günlüğü, webhook ya da e-posta ile kaynak uyarıları, Prometheus ölçümleri.
- Birkaç RedoX VE sunucusunun birleşik görünümü ve bir sanal makineyi bellek durumu, ek diskleri, UEFI
  değişkenleri ve TPM durumuyla birlikte aralarında taşıma.

## RedoX VE ve Proxmox VE

RedoX VE, [Proxmox VE](https://www.proxmox.com/en/proxmox-virtual-environment/overview) örnek alınarak
tasarlandı. Proxmox kullandıysanız fikirleri tanırsınız: 100'den başlayan sayısal konuk kimlikleri, yan yana
sanal makineler ve konteynerler, `vmbr0` biçimli köprüler, görev günlüğü, konuk başına anlık görüntü ve
yedek, 8006 numaralı kapıdaki panel.

Rust ile sıfırdan yazılmış bağımsız bir projedir. Proxmox VE ile ortak kodu yoktur, Proxmox'un ayar ve
yedek biçimlerini okumaz ve onun yerine doğrudan konabilecek bir ürün değildir.

| | Proxmox VE | RedoX VE |
|---|---|---|
| Biçim | Makineye kurduğunuz, Debian tabanlı bir dağıtım | Var olan Debian ya da Ubuntu sunucuya eklediğiniz tek bir program |
| Konuklar | KVM sanal makineleri, LXC konteynerleri | KVM sanal makineleri, LXC konteynerleri |
| Küme | Ortak durumlu çok düğümlü küme, canlı taşıma ve yüksek erişilebilirlik | Birleşik görünümlü bağımsız sunucular; çalışan sanal makine aralarında canlı taşınır (ortak depoda disk kopyalanmadan), ama ortak küme durumu yoktur |
| Depolama | LVM, ZFS, Ceph, NFS ve fazlası | Dizinler (yerel disk ya da bağlı bir paylaşım), qcow2, isteğe bağlı LUKS |
| Yedek | vzdump ve Proxmox Backup Server | Gömülü tam ve artımlı yedek, dizine ya da S3'e dış kopya |
| Web paneli | JavaScript uygulaması | Sunucuda üretilen HTML; JavaScript yalnızca konsol, terminal ve güvenlik anahtarı sayfalarında |
| Olgunluk | Yıllardır üretimde | Yeni; bazı parçaları gerçek sunucuda henüz denenmedi ([Testler](#testler)) |

Kümeye, Ceph gibi dağıtık depolamaya ya da kendini kanıtlamış bir platforma ihtiyacınız varsa Proxmox VE'yi seçin.
RedoX VE, küçük, denetlenebilir ve sıkı varsayılanlı bir hizmetin geniş özellik listesinden daha değerli
olduğu tek sunucuyu hedefler.

*Proxmox, Proxmox Server Solutions GmbH'nin tescilli markasıdır. RedoX VE'nin Proxmox Server Solutions GmbH
ile bir bağı yoktur ve onun tarafından desteklenmez.*

## Ekran görüntüleri

Görüntüler, panelin gömülü simülasyon sürücüsüyle çalışırken alındı; bu sürücü örnek bir küme gösterir.

| Konuklar | Konuk özeti |
|---|---|
| ![Konuk listesi](docs/img/guests.jpg) | ![Konuk özeti](docs/img/guest-summary.jpg) |

| Donanım | Seçenekler |
|---|---|
| ![Donanım sekmesi](docs/img/hardware.jpg) | ![Seçenekler sekmesi](docs/img/options.jpg) |

| Depolama | Açık tema |
|---|---|
| ![Depolama sayfası](docs/img/storage.jpg) | ![Açık tema](docs/img/light-theme.jpg) |

Gerçek bir QEMU konuğuna bağlı canlı konsol:

![Canlı konsol](docs/img/live-console.jpg)

## Kurulum

Adımlar Debian 12 ya da Ubuntu 22.04 ve sonrası (systemd, x86_64) içindir. `redox.example.com` yerine
kendi adınızı yazın. Her seçeneğin açıklandığı eksiksiz kılavuz: [KURULUM.md](KURULUM.md).
> Konu başına ayrı sayfalardan oluşan wiki: [docs/wiki/tr](docs/wiki/tr/Ana-Sayfa.md) ([English](docs/wiki/en/Home.md)).

### 1. Derleme

Kaynak ağacını sunucuya kopyalayın, sonra:

```bash
sudo apt update && sudo apt install -y build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"

cd Proxmox-Rust
cargo test --workspace
cargo build --release
sudo install -m 0755 target/release/redox /usr/local/bin/redox
```

### 2. Hizmet kullanıcısı ve ilk yönetici

Varsayılan hesap yoktur. Bir kullanıcı oluşturulmadan sunucu başlamaz.

```bash
sudo useradd --system --home-dir /var/lib/redox --shell /usr/sbin/nologin redox
sudo install -d -o redox -g redox -m 0700 /var/lib/redox
sudo -u redox redox user add admin --users /var/lib/redox/users.json
```

Parola iki kez sorulur, ekrana yazılmaz ve en az 12 karakter olmalıdır.

### 3. systemd hizmeti

```bash
sudo cp deploy/redox.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload
sudo systemctl enable --now redox
curl -s http://127.0.0.1:8006/healthz
```

Son komut `ok` yazar. Bu aşamada sunucu simülasyon sürücüsünü çalıştırır: gerçek hiçbir şey başlatmaz,
örnek bir küme gösterir.

### 4. TLS

RedoX VE burada TLS sonlandırmaz ve `127.0.0.1` dışında şifresiz HTTP dinlemeyi reddeder. Dışarıya
[Tailscale](https://tailscale.com/download/linux) ile açın: panel yalnızca kendi Tailscale ağınızdan,
`https://<makine>.<ağ>.ts.net` adresinde açılır.

```bash
sudo tailscale up
sudo tailscale serve --bg --https=443 http://127.0.0.1:8006
```

Önce Tailscale yönetim panelinde MagicDNS ve HTTPS sertifikaları açık olmalı. Hizmet satırındaki
`--behind-tls-proxy` yanına `--proxy-uid 0` ekleyin (`install.sh` kendisi ekler). `tailscale funnel` kullanmayın:
paneli internete açar. 8006 numaralı kapıyı açmayın. Tailscale kullanmayacaksanız hizmet dosyasından
`--behind-tls-proxy` satırını silin ve SSH tüneli kullanın: `ssh -L 8006:127.0.0.1:8006 kullanici@sunucu`.
Ayrıntı: [KURULUM.md](KURULUM.md) 5. bölüm.

### 5. Gerçek sanal makineler

```bash
sudo apt install -y qemu-system-x86 qemu-utils
ls -l /dev/kvm
sudo cp deploy/redox-qemu.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload && sudo systemctl restart redox
journalctl -u redox -n 20 --no-pager
```

Günlükte `qemu-kvm` sürücüsü görünmelidir. `/dev/kvm` yoksa donanım yazılımında sanallaştırmayı açın;
yoksa QEMU yazılım öykünmesine düşer ve çok yavaş çalışır.

İsteğe bağlı parçalar; her biri [KURULUM.md](KURULUM.md) içinde anlatılır:

| İhtiyaç | Gerekenler | Bölüm |
|---|---|---|
| UEFI, güvenli önyükleme, TPM | `ovmf`, `swtpm` | 8 |
| Köprü ağı, LXC konteynerleri | `lxc`, `uidmap`, `acl`; `deploy/redox-host.service` birimi | 9 |
| VLAN etiketi, ağ hız sınırı | `iproute2`, `CAP_NET_ADMIN` | 8 |
| Konuk güvenlik duvarı | `nftables`, `CAP_NET_ADMIN`, `--firewall` | 12 |
| Uyarılar, birden çok sunucu | `--alert-webhook`, `--alert-email`, `--peers` | 10 |
| Dış hedefe yedek | `--offsite` | 11 |
| Tek oturum açma | `--oidc` | 13 |

### Kendi bilgisayarınızda denemek

Simülasyon sürücüsü Windows ve macOS'ta da çalışır; paneli gezmek için yeterlidir:

```bash
cargo build
./target/debug/redox user add admin --users users.json
./target/debug/redox serve --users users.json
```

Sonra `http://127.0.0.1:8006` adresini açın.

## Komut satırı ve API

Aynı program istemcidir. Sunucuda bir token oluşturun (yalnızca bir kez gösterilir):

```bash
sudo -u redox redox token create admin dizustu --users /var/lib/redox/users.json
```

Sonra herhangi bir makineden:

```bash
export REDOX_URL=https://redox.example.com
export REDOX_TOKEN=redox_...

redox cluster status
redox vm list
redox vm start 103
redox vm set 103 cores=4 memory=8192 tags=prod,web backup-time=03:30
redox vm disk add 103 20
redox snapshot create 103 guncelleme-oncesi
redox backup create 103
redox backup restore vm-103-1795546651
redox task list
```

Bütün komutlar: `redox --help`.

REST API `/api/v1` altındadır ve `Authorization: Bearer <token>` ister. Durum değiştiren çağrılar bir görev
döndürür; sonucu `/api/v1/tasks/{id}` adresinden okursunuz.

| Yöntem ve yol | Ne işe yarar |
|---|---|
| `GET /cluster`, `GET /nodes`, `GET /nodes/{id}` | Küme ve düğüm durumu |
| `GET /guests`, `POST /guests` | Konukları listeleme ve oluşturma |
| `GET`, `PATCH`, `DELETE /guests/{vmid}` | Gösterme, değiştirme, silme |
| `POST /guests/{vmid}/{start,stop,reboot,pause,resume}` | Güç işlemleri |
| `POST /guests/{vmid}/suspend`, `/discard-suspend`, `/detach` | Diske askıya alma, yedekten ayırma |
| `POST /guests/{vmid}/snapshots`, `.../{name}/rollback` | Anlık görüntüler |
| `POST /guests/{vmid}/clone`, `/template`, `/resize`, `/disks`, `/nics`, `/balloon`, `/cores`, `/memory`, `/move` | Klonlar, donanım, depolar arası taşıma |
| `GET /guests/{vmid}/export` | OVA dışa aktarma |
| `POST /guests/{vmid}/backups`, `GET /backups` | Yedekler |
| `POST /backups/{id}/restore`, `/push`, `/verify`, `GET .../download` | Geri yükleme, dış kopya, doğrulama, indirme |
| `POST /backups/{id}/restore-new`, `/instant`, `/test`, `/lock` | Yeni konuk, anında kurtarma, açarak sınama, kilit |
| `GET /backups/{id}/files`, `/file`, `POST /backups/{id}/file`, `GET /backups/report` | Yedeğin içindeki dosyalar, bir dosyayı çalışan konuğa geri yükleme, yedek raporu |
| `GET /jobs`, `POST /jobs`, `DELETE /jobs/{name}`, `POST /jobs/{name}/run` | Yedek işleri |
| `GET /pools`, `POST /pools`, `DELETE /pools/{name}` | Kaynak havuzları ve kotalar |
| `GET /storage`, `GET /media`, `PUT /media/{name}`, `POST /media/fetch` | Depolama, kalıp ve imaj yükleme |
| `GET /peers`, `POST /peers/{name}/import/{vmid}`, `POST /peers/{name}/migrate/{vmid}` | Öbür sunucular, sanal makine alma, canlı taşıma |
| `GET /support` | Destek paketi (yalnızca yönetici) |
| `GET`/`POST /lockdown` | Kilit kipi (değişiklik dondurma) |
| `GET /profile`, `POST /profile/diff`, `/profile/apply` | Sunucu profili |
| `GET`/`POST /ft/protections`, `DELETE /ft/protections/{vmid}`, `GET /ft/standby` | Hata toleransı |
| `GET`/`POST /mirrors`, `DELETE /mirrors/{vmid}` | Eşzamanlı disk yansıtma (kaynak) |
| `GET /mirror-stores`, `POST /mirror-stores/{origin}/{vmid}/promote`, `DELETE …` | Eşler için tutulan kopyalar, devralma |
| `GET`/`POST /vswitches`, `DELETE /vswitches/{name}`, `POST /vswitches/{name}/push` | Dağıtık anahtarlar |
| `POST /guests/{vmid}/media` | Çalışan konuğun CD sürücüsündeki kalıbı değiştirme |
| `GET`/`POST /firewall/sets`, `/firewall/groups`, `DELETE …/{name}` | Adres kümeleri ve güvenlik grupları |
| `GET /host/performance`, `GET /host/health`, `POST /host/update` | Performans donanımı, donanım sağlığı, güncelleme isteği |
| `GET`/`POST /host/networks`, `DELETE /host/networks/{name}` | Sunucu ağları |
| `GET /balance`, `POST /peers/{name}/send/{vmid}` | Yük dengeleme önerisi; sanal makineyi eşe canlı taşıma |
| `GET /tasks`, `GET /tasks/{id}` | Görev günlüğü |

`GET /metrics` (`/api/v1` dışında) aynı token ile Prometheus ölçümlerini verir.

## Teknik doküman

### Dizin yapısı

```
crates/
  redox-core/     Alan modeli ve kurallar. Girdi/çıktı yok, async yok.
  redox-server/   `redox` programı: HTTP sunucusu, sürücüler, komut satırı.
deploy/           systemd birimleri ve kurulum betiği.
docs/img/         Bu belgedeki ekran görüntüleri.
KURULUM.md        Ayrıntılı kurulum ve işletim kılavuzu.
```

İki pakette yaklaşık 28.500 satır Rust vardır.

### `redox-core`

| Modül | İçerik |
|---|---|
| `model`, `hw` | Konuklar, düğümler, depolama, anlık görüntüler, yedekler, görevler; konuğun donanım tanımı |
| `cluster`, `ops` | Durum makinesi: hangi işlem hangi durumda yapılabilir, konuk kilitleme, görev kayıtları |
| `jobs` | Yedek işleri: etiketle seçim, günler, konukların tek tek yedeklenmesi, işe özgü saklama sayısı |
| `pools` | Kaynak havuzları: ayrılan çekirdek, bellek, disk ve konuk sayısına kota |
| `retention` | Haftalık, aylık, yıllık saklama, yedek kilitleri, takvim hesabı |
| `driver` | Her arka ucun uyguladığı `Hypervisor` arayüzü ve dış hedefler için `Remote` arayüzü |
| `command` | Komut satırının ve paneldeki komut çubuğunun ortak komut dili |
| `seed` | Simülasyon sürücüsünün gösterdiği örnek küme |

Bütün kurallar buradadır ve sanallaştırıcı olmadan test edilir. Sunucu bir işlemin *nasıl* yapılacağına
karar verir; *yapılıp yapılamayacağına* `redox-core` karar verir.

### `redox-server`

| Alan | Modüller |
|---|---|
| HTTP | `lib` (yönlendirici, güvenlik başlıkları), `api` (REST), `ui`, `ui_ops`, `ui_admin` (sayfa işleyicileri), `view*` ([maud](https://maud.lambda.xyz) ile HTML) |
| Durum | `state` (uygulama durumu, görev yürütücü, zamanlayıcılar), `history` ve `metrics` (grafikler, Prometheus) |
| Sürücüler | `qemu` (QMP üzerinden QEMU/KVM), `lxc`, `hostdriver` (ikisini birleştirir), `qga` (konuk ajanı) |
| Yedek | `chunkstore` (artımlı depo), `offsite` (dizin ve S3 hedefleri, şifreleme), `ova`, `guestfs` (yedeğin içindeki dosyalar), `report` |
| Ağ | `net`, `netlimit` (`tc`), `firewall` (nftables) |
| Kimlik | `auth` (kullanıcılar, oturumlar, token'lar), `totp`, `oidc`, `audit` |
| İşletim | `alerts`, `mail`, `ha` (yeniden başlatma gözcüsü), `peers` (öbür sunucular), `media` (kalıp ve imaj yükleme) |
| İstemci | `cli`, `main` |

### Bir işlem nasıl yürür

1. İstek panelden (form gönderimi), API'den ya da komut satırından (o da API'yi çağırır) gelir.
2. `redox-core` kuralı denetler; izin varsa konuğu kilitler ve bir görev kaydeder.
3. Görev arka planda sürücü üzerinde çalışır.
4. Sonuç görev günlüğüne yazılır ve konuğun kilidi açılır. Başarısız görev konuğu olduğu gibi bırakır.

Durum tek bir JSON dosyasıdır (`state.json`). Hizmet yeniden başlasa da konuklar çalışmaya devam eder ve
açılışta yeniden sahiplenilir.

### QEMU sürücüsü

Her sanal makine, sabit ve sürüm numaralı bir aygıt yerleşimiyle başlatılan tek bir QEMU sürecidir:

- Denetim, veri dizinindeki bir QMP Unix soketi üzerinden yapılır.
- Ekran bir VNC Unix soketinden sunulur (parolasız, ağa hiç açılmaz).
- Çalışırken takma için PCIe kök portları ayrılmıştır: sekizi disklere, üçü ağ kartlarına.
- Yerleşim sürümü askıya alınan konukla birlikte kaydedilir; konuk kaydedildiği yerleşimle döner.

Veri dizini (`/var/lib/redox`):

| Yol | İçerik |
|---|---|
| `images/` | Konuk diskleri (qcow2), askı durumu, değişen blok haritaları |
| `iso/`, `cloud/` | Kurulum kalıpları ve bulut imajları |
| `backups/`, `backups/chunks/` | Tam yedekler ve artımlı parça deposu |
| `shares/` | Paylaşılan klasörler |
| `run/` | QMP ve VNC soketleri, süreç kimliği dosyaları |
| `state.json`, `users.json`, `sessions.json`, `audit.log`, `metrics.json` | Kayıtlar |

### Yedekler

- **Tam:** disk başına bir sıkıştırılmış qcow2. Çalışan sanal makine QEMU yedek işiyle yedeklenir; birden
  çok disk varsa işler tek bir işlemde (transaction) başlar.
- **Artımlı** (`--incremental-backups`): disk, SHA-256 özetiyle adlandırılan 4 MiB'lik parçalara bölünür.
  Her yedek eksiksiz bir parça listesidir, yani kopabilecek bir zincir yoktur. Sıfır bölgeler saklanmaz.
- **Değişen blok takibi** (`--changed-block-tracking`): kalıcı bir QEMU değişiklik haritası. Yanlış
  olabileceği her durumda (geri yükleme, anlık görüntüye dönüş, büyütme, çökme) harita atılır ve sonraki
  yedek diski baştan sona okur.
- **Makine durumu:** UEFI değişken deposu ve sanal TPM durumu yedeğin yanında küçük bir arşive
  (`<yedek>.machine.tar`) yazılır ve dış hedefe onunla birlikte gider.
- **Dış hedef:** 64 KiB'lik parçalarla ChaCha20-Poly1305; anahtar Argon2id ile türetilir. Dosyanın adı her
  parçayla birlikte doğrulanır.

### Web paneli ve canlı konsol

Sayfalar sunucuda üretilir ve JavaScript olmadan çalışır; içerik güvenlik politikası `script-src 'none'`
olarak ayarlıdır. Yazı tipleri ve simgeler programa gömülüdür; sayfa dışarıya hiçbir istek atmaz.

Üç sayfa istisnadır ve her biri RedoX VE'nin kendi sunduğu tek bir betiği yükler: canlı konsol, konteyner
terminali ve güvenlik anahtarı adımı (tarayıcının anahtar arayüzü yalnızca JavaScript'le çağrılır). Canlı
konsolun betiği bir RFB 3.8 (VNC) istemcisidir. Sunucu oturumu ve yetkiyi denetler, sonra tarayıcının WebSocket bağlantısını konuğun VNC Unix
soketine köprüler. Tuşlar tarama koduyla gider; karakteri konuğun kendi klavye düzeni belirler (Türkçe
düzen doğru çalışır).

## Güvenlik modeli

| Katman | Ne yapıyor |
|---|---|
| Kimlik | Kullanıcı adı ve parola (Argon2id). Varsayılan hesap yoktur. |
| Oturum | 256 bit rastgele çerez; `HttpOnly`, `SameSite=Strict`, TLS arkasında `Secure`. 2 saat hareketsizlikte ya da 12 saatte biter. |
| Kaba kuvvet | Aynı adres ve kullanıcı için 15 dakikada 5 hatalı deneme 15 dakika kilitler. |
| İkinci adım | İsteğe bağlı TOTP; bir kod bir kez kabul edilir. |
| Roller | `admin` her şeyi yapar, `operator` kapsamındaki konukları işletir, `viewer` yalnızca görür. |
| Denetim | Girişler, reddedilen denemeler ve durum değiştiren her istek `audit.log` dosyasına yazılır. |
| API | Bearer token'lar; dosyada yalnızca SHA-256 özetiyle durur. |
| Tarayıcı | Site dışı POST'lar ve site dışından açılan konsol bağlantıları reddedilir; sayfa çerçeveye alınamaz; HSTS. |
| Süreç | Yetkisiz `redox` kullanıcısı, systemd sandbox'ı, dosyalar `0600`. |

## Testler

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

Linux'ta 713, Windows'ta 598 test geçer (QEMU ve LXC sürücü testleri yalnızca Linux'ta çalışır). Sürücü
testleri, QMP konuşan sahte bir QEMU'ya karşı çalışır. Node.js kuruluysa tarayıcı betiklerinin (konsol,
terminal, güvenlik anahtarı) kendi testleri de çalışır.

Adı `real_` ile başlayan sürücü testleri **gerçek QEMU** ile, konuk olarak bir Linux çekirdeğiyle çalışır ve sonucu konuğun
içinden doğrular. İkililer gösterilmedikçe atlanırlar:
`REDOX_REAL_QEMU`, `REDOX_REAL_QEMU_IMG`, `REDOX_REAL_KERNEL`, `REDOX_REAL_INITRD` (başlangıç dosya sisteminde
`/bin/sh` kabuğu bulunan herhangi bir çekirdek; örneğin bir Debian kurucusunun `vmlinuz` ve `initrd.gz` dosyaları).
Konuk ajanı testi ayrıca `REDOX_REAL_INITRD_AGENT` ister: içine `qemu-ga` ve onu başlatan bir `/ga/start` betiği
eklenmiş aynı başlangıç dosya sistemi.

Test takımının dışında QEMU sürücüsü, KVM'li gerçek QEMU 10.2 üzerinde denendi: anlık görüntüler, tam ve
artımlı yedekler, değişen blok takibi, dış kopyalar, askıya alma ve dönüş, çalışırken takma, VLAN ve hız
sınırları, canlı konsol, çalışan bir sanal makinenin iki sunucu arasında taşınması, yedek işleri, işlemci
modelleri, balon aygıtı, UEFI değişkenleriyle TPM durumunun yedeklenip geri yüklenmesi, disklerin depolar
arasında taşınması, havuz kotaları, ek diskli, UEFI'li ve TPM'li bir konuğun iki sunucu arasında taşınması, yedekten
yeni konuk, anında kurtarma, açarak sınama, kilit, iki dış hedefe kopya, üretim denetim noktası, iç içe
sanallaştırma, kaynak ölçümü, sınama kapısı yönlendirmesi, kopya hedefi arşivi, dört göz onay akışı, çalışan bir
sanal makinenin iki sunucu arasında çoğaltılması, sunucu yeniden başlatılırken bakım kipi, salt okunur token ve
API hız sınırı.

Gerçek bir sunucuda henüz doğrulanmayanlar: systemd birimleri ve Tailscale ayarı, köprü ağı, LXC
konteynerleri, konuk güvenlik duvarı ve ağ kartı korumasının trafik altındaki davranışı, dinamik belleğin ve
sınama denetimlerinin ve konuk içi disk ölçümünün gerçek bir konuk işletim sistemiyle çalışması, gerçek bir
köprüde ağ ölçümü, gerçek ntfy ve Telegram, gerçek bir S3 sağlayıcısı, gerçek bir SMTP ya da OpenID Connect
sağlayıcısı ve USB/PCI aktarımı. [KURULUM.md](KURULUM.md) bunları anlattığı yerde ayrıca belirtir.

## Sınırlar

- İşlemci payı ve bellek garantisi Linux denetim gruplarını kullanır (cgroup v2: `cpu.weight`, `memory.min`); bu yüzden hizmetin alt grup açma yetkisi olmalıdır (birimde `Delegate=cpu memory`; `deploy/redox-qemu.service` içinde var). Yetki yoksa pay ya da garanti isteyen konuk başlatılmaz. Değerlerin yazılması gerçek çekirdekte sınandı; zamanlayıcının dolu bir işlemciyi sonra nasıl böldüğü ölçülmedi ve hizmetin önce kendini bir alt gruba taşıması gereken yol denenmedi. Garanti yalnızca bir kabul kuralı ile `memory.min` değeridir: sunucu belleği önceden ayrılmaz.
- Konsol durum ekranı (`redox tty`), `install.sh` ya da ISO ile kurulumdan sonra ilk sanal konsolda oturum açma isteminin yerini alır. Metni ve kurulum betiğinin yazdığı birim dosyası sınandı; gerçek bir konsolda henüz görülmedi (geliştirme makinesinde yok), bu yüzden oradaki yanlış bir terminal ayarı ancak gerçek donanımda ortaya çıkar. Öbür konsollarda (Alt+F2) olağan oturum açma durur.
- Ortak küme durumu yoktur. Birkaç sunucu birlikte görüntülenebilir, ama her biri kendi konuklarını yönetir.
- Canlı taşıma, çalışan sanal makineyi onlarca milisaniyelik bir duraklamayla taşır, ama yalnızca sade konukları: UEFI ya da sanal TPM, takılı kurulum kalıbı, cloud-init sürücüsü, paylaşılan klasör, aktarılmış aygıt ya da bir şablona/yedeğe bağlı disk varsa taşımaz. İki QEMU birbirine doğrudan bağlanır (tek kullanımlık ön paylaşımlı anahtarlı TLS); hedefin taşıma kapıları kaynaktan erişilebilir olmalıdır. Gerçek QEMU'da bir Linux konukla, tek makinedeki iki sunucu arasında sınandı; iki fiziksel makine arasında, Windows konukla ya da ağır yük altında denenmedi. Eski "çalışırken al" yolu (askıya al, kopyala, sürdür) konuğu kopyalama süresince duraklatır ve canlı taşınamayan konuklar için durmaktadır.
- Kendiliğinden devralma yalnızca çoğaltılan konuklar içindir ve bir hakem sunucu ister; kaynağı zorla kapatma (fencing) ve ortak durumlu küme yoktur. Gerçek bir kesintide denenmedi.
- Depolama her zaman imaj dosyalarının durduğu bir dizindir. Dizin ZFS, LVM ya da NFS üzerinde olabilir (tanınır ve gösterilir), ama yerleşik zvol ya da mantıksal birim havuzu yönetimi yoktur.
- Çalışan konuğa dosya geri yükleme konuk ajanı üzerinden yapılır: konukta `qemu-guest-agent` çalışmalı ve ajan dosya komutlarına izin vermelidir (RHEL gibi bazı dağıtımlar bunları varsayılan olarak engeller). Tek dosya ajanın kullanıcısıyla yazılır, sahibi ve izinleri korunmaz; klasör konuğun kendi `tar` programıyla açılır ve hedef dizin konukta var olmalıdır. Konteynerler kapsam dışıdır. Konuğun içine yazma (tek dosya ve klasör), sürücünün kendi koduyla gerçek bir konuk ajanına karşı sınanır (Linux konukta qemu-ga 10.2); dosyanın yedekten okunması hâlâ yalnızca sahte `guestfish` ile sınanıyor ve yolun tamamı Windows konukla denenmedi.
- Çalışan sanal makineye çekirdek ve bellek eklenir ve çıkarılır (bellekte yalnızca sonradan eklenen parçalar; Linux konukta güvenilir çıkarma için bellek "taşınabilir" açılmalıdır). Eklenebilecek miktar, konuk kapalıyken verilen "en fazla çekirdek" ve "en fazla bellek" değerleriyle sınırlıdır. Bellek 128 MB'lık adımlarla ve bir açılışta en fazla 8 kez eklenir (konuk kapatılıp açılınca eklenen bellek olağan belleğe katılır, yuvalar boşalır); açılış belleğini çalışırken düşürmenin yolu yalnızca balondur. Çekirdek ya da bellek eklenmeden önce alınmış bellek durumlu anlık görüntüye ancak konuk kapalıyken dönülür. İkisi de sürücünün kendi koduyla gerçek QEMU'da bir Linux konukla, ardından askıya alıp geri dönerek sınanır; yeni çekirdeklerin ve belleğin konuk ajanıyla etkinleştirilmesi de gerçek bir ajana (qemu-ga 10.2) karşı sınanır. İkisi de Windows konukla denenmedi.
- Çalışan sanal makineyi başka depolama alanına taşımak yalnızca disklerin şimdiki içeriğini aynalar; bu yüzden anlık görüntüsü (ya da yedeğe bağlı diski) olan konuk önce kapatılmalıdır. Kapalı konuklar dosyaları kopyalanarak, görüntüleriyle birlikte taşınır. Çok diskli konukta geçiş yarıda kalırsa hata bunu söyler ve taşıma, konuk kapatılmadan yeniden denenmelidir. Gerçek QEMU'da, yazmayı sürdüren bir konukla sınandı; büyük disklerle ya da yavaş bir hedefle denenmedi.
- Konteynerler başka depolama alanına taşınamaz. Eş sunucudan yedek üzerinden **kopyalanır** (kopya kapalı gelir, kaynak yerinde kalır); gerçek LXC ile denenmedi.
- Canlı konsolda pano ve mikrofon yoktur (metin tuş vuruşu olarak yazdırılabilir).
- Arayüz yalnızca Türkçe ve İngilizcedir.
- Ceph, iSCSI, yerleşik ZFS/LVM-thin yönetimi, ortak küme durumu, SPICE, Terraform sağlayıcısı, Ansible modülleri ve ESXi/vCenter'dan doğrudan aktarma yoktur.
- Konteyner terminali ve OCI imajından konteyner gerçek LXC ile; OCI gerçek bir kayıt sunucusuyla; güvenlik anahtarı fiziksel bir anahtarla denenmedi.
- Windows konukla hiçbir şey denenmedi; uyumluluk aygıtları Linux konukla ve BIOS açılışıyla sınandı. Büyük sayfalar yalnızca açıklayıcı ret yoluyla sınandı.
- LDAP yalnızca testlerdeki küçük dizin sunucusuna karşı sınandı; OpenLDAP ya da Active Directory ile denenmedi. SMART ve IPMI okuyucuları elle yazılmış örnek çıktılarla sınandı; gerçek donanımda denenmedi. `redox-hostnet`, `redox-health` ve `redox-update` birimleri gerçek systemd altında çalıştırılmadı.

## Lisans

RedoX VE'nin kaynak kodu [PolyForm Noncommercial License 1.0.0](LICENSE.md) ile açıktır. Bağlayıcı olan
lisansın İngilizce metnidir; aşağısı özettir.

- **Serbest:** ticari olmayan her amaçla kullanmak, incelemek, değiştirmek ve paylaşmak. Kişisel kullanım, ev
  laboratuvarı, hobi projeleri, araştırma ile okulların, hayır kurumlarının ve kamu kurumlarının kullanımı
  buna girer.
- **Yasak:** ticari kullanım. Bir işletmenin parçası olarak çalıştırmak, ücretli hizmet olarak sunmak, kendisini
  ya da üzerine kurulmuş ürünleri satmak buna girer.
- Kopyalar ve değiştirilmiş sürümler lisansı ve `LICENSE.md` içindeki `Required Notice` satırını taşımalıdır.

Ticari kullanım dışarıda bırakıldığı için bu, OSI tanımıyla bir açık kaynak lisansı değildir. Ticari lisans
için depoda bir konu (issue) açın.

Gömülü yazı tipleri kendi lisanslarını korur (SIL Open Font License; `crates/redox-server/assets/fonts/`).
