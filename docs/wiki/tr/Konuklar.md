[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Konuklar

Konuk, bir sanal makine (KVM) ya da konteynerdir (LXC).

## Oluşturmak

*Yeni Konuk* düğmesi ya da API. Sanal makine üç yoldan biriyle kurulur:

- **Kurulum kalıbı (ISO):** kalıbı *Depolama* sayfasından yükleyin, konuğu oluştururken seçin.
- **Hazır bulut imajı:** *Depolama → Hazır bulut imajları* listesinden indirin (Debian, Ubuntu, AlmaLinux, Rocky) ya da
  `redox media get debian-12`. Konuğu oluştururken imajı seçip kullanıcı adı ve SSH anahtarı verin (cloud-init).
- **Şablondan klon:** aşağıya bakın.

Windows kurarken disk ve ağ kartı görünmüyorsa katalogdan **Windows sürücüleri (virtio-win)** kalıbını indirin
(`redox media get virtio-win`) ve kurulum sırasında sürücüleri oradan yükleyin; kurulumdan sonra aynı kalıptan balon
sürücüsünü ve konuk ajanını kurun.

## Güç işlemleri

Başlat, Kapat, Yeniden Başlat, Duraklat, Sürdür. Kapat önce konuğa ACPI ile kapanmasını söyler; süre dolarsa zorla
kapatır. **Askıya Al** belleği diske yazar ve konuğu kapatır; başlatınca kaldığı yerden sürer.

Zamanlanmış güç işlemleri (*Seçenekler* sekmesi): belirli gün ve saatlerde başlat/kapat/yeniden başlat.
*Açılışta başlat* ve başlatma sırası aynı sekmededir. *Kendiliğinden kapanırsa yeniden başlat* çöken konuğu yaklaşık
15 saniyede yeniden açar.

## Konsol

- **Canlı konsol:** gerçek zamanlı ekran, klavye ve fare (JavaScript kullanır). Ses kartı olan makinede **Sesi Aç** ile ses
  de dinlenir; mikrofon taşınmaz.
- **Konteyner terminali:** çalışan konteynerde **Terminali Aç** ile tarayıcıda etkileşimli kök kabuğu (JavaScript
  kullanır). Gerçek `lxc-attach` ile henüz denenmedi.
- **Konsol sekmesi:** JavaScript'siz; ekran görüntüsü 2 saniyede bir yenilenir, metin ve özel tuş gönderilir.
- Konteynerlerde ve konuk ajanı olan makinelerde komut çalıştırma.

## Klon ve şablon

*Çoğalt ve Taşı* sekmesi. Kapalı konuk klonlanır (tam kopya). Bir sanal makineyi **şablona çevirirseniz** başlatılamaz
olur ama ondan saniyeler içinde **bağlı klonlar** üretilir (yalnızca farkı tutarlar).

## Konuk ajanı

Konuğa `qemu-guest-agent` kurarsanız: IP adresi ve işletim sistemi görünür, çalışırken alınan yedekte dosya sistemleri
dondurulur, konuk içi disk doluluğu okunur ve yedek öncesi/sonrası komutları çalışır.

## Kaynak havuzları

*Havuzlar* sayfası. Havuz, bir grup konuğun toplam çekirdek, bellek ve disk ayırmasına kota koyar. Konuğu havuza
*Seçenekler* sekmesinden alırsınız.

## Konsola metin yazdırma

Canlı konsolda pano paylaşımı yoktur. Bunun yerine konsol sayfasının üstündeki kutuya yazdığınız metin **Yazdır** ile
konuğa tuş vuruşları olarak gönderilir (uzun parolalar ve komutlar için). Konukta ajan gerekmez. Yalnızca ABD
klavyesindeki karakterler yazılır ve konuğun klavye düzeni ABD İngilizcesi olmalıdır; başka düzende simgeler farklı
çıkar. İstemci mantığı testlidir; gerçek bir konukta denenmedi.

## Ses, disk şifreleme ve OCI imajları

- **Ses kartı:** ayarlarda *Ses kartı tak* ya da `redox vm set <kimlik> sound=true` (konuk kapalıyken).
- **Disk şifreleme:** sunucu `--disk-key-file` ile çalışıyorsa sanal makine oluştururken *Diskleri şifrele (LUKS)*.
  Anahtar kaybolursa disk açılamaz; şifreli konukta klon, askıya alma ve dışa aktarma gibi işlemler reddedilir.
- **OCI imajı:** konteyner oluştururken *OCI imajı* alanına `nginx:1.29` gibi bir ad; ağ için sabit IP ve ağ geçidi
  verin. Yalnızca herkese açık linux/amd64 imajlar. Gerçek kayıt sunucusu ve gerçek LXC ile henüz denenmedi.
- **Konteynerde Tailscale:** konteyner kapalıyken `redox vm set <kimlik> tun=true` ile tünel aygıtını verin, sonra
  içinde Tailscale'i kurup `tailscale up` çalıştırın. Gerçek LXC konteynerinde henüz denenmedi.

Ayrıntılar ve sınırlar: depodaki `KURULUM.md`, 14. bölüm.
