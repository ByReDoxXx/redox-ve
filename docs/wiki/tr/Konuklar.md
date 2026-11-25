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

## Güç işlemleri

Başlat, Kapat, Yeniden Başlat, Duraklat, Sürdür. Kapat önce konuğa ACPI ile kapanmasını söyler; süre dolarsa zorla
kapatır. **Askıya Al** belleği diske yazar ve konuğu kapatır; başlatınca kaldığı yerden sürer.

Zamanlanmış güç işlemleri (*Seçenekler* sekmesi): belirli gün ve saatlerde başlat/kapat/yeniden başlat.
*Açılışta başlat* ve başlatma sırası aynı sekmededir. *Kendiliğinden kapanırsa yeniden başlat* çöken konuğu yaklaşık
15 saniyede yeniden açar.

## Konsol

- **Canlı konsol:** gerçek zamanlı ekran, klavye ve fare (JavaScript kullanan tek sayfa).
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
