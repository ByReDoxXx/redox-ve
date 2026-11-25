[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# RedoX VE Wiki

RedoX VE, tek bir sunucuda KVM sanal makinelerini ve LXC konteynerlerini yöneten, Rust ile yazılmış bir sanallaştırma
platformudur. Tek bir çalıştırılabilir dosyadır; arayüzü sunucuda üretilir ve (canlı konsol sayfası dışında) JavaScript
kullanmaz.

## Sayfalar

| Konu | Sayfa |
|---|---|
| Sunucuya kurmak, hizmeti başlatmak, ilk giriş | [Kurulum](Kurulum.md) |
| Konuk oluşturmak, başlatmak, konsol, klon, şablon | [Konuklar](Konuklar.md) |
| Disk, ağ kartı, bellek, işlemci, UEFI/TPM, aygıt aktarımı | [Donanım](Donanim.md) |
| Anlık görüntüler ve üretim denetim noktası | [Anlık görüntüler](Anlik-Goruntuler.md) |
| Yedek almak, geri yüklemek, işler, saklama, sınama, dış hedef | [Yedekleme](Yedekleme.md) |
| Birden çok sunucu, taşıma, çoğaltma, bakım kipi | [Birden çok sunucu](Birden-Cok-Sunucu.md) |
| Köprü, VLAN, güvenlik duvarı, ağ kartı koruması | [Ağ ve güvenlik](Ag-ve-Guvenlik.md) |
| Kullanıcılar, roller, token'lar, iki adımlı doğrulama, dört göz ilkesi | [Kullanıcılar ve yetkiler](Kullanicilar-ve-Yetkiler.md) |
| Uyarılar, bildirim kanalları, ölçümler, kaynak ölçümü | [İzleme ve uyarılar](Izleme-ve-Uyarilar.md) |
| `redox` komutu | [Komut satırı](Komut-Satiri.md) |
| REST API | [API](API.md) |
| Arayüz dili | [Dil](Dil.md) |
| Sık karşılaşılan sorunlar | [Sorun giderme](Sorun-Giderme.md) |

## Neyin denendiğini bilin

Her özellik otomatik testlerle sınanır; QEMU sürücüsü ayrıca gerçek QEMU/KVM üzerinde denenmiştir. Bazı parçalar ise
gerçek bir sunucuda henüz **denenmemiştir**: köprü ağı, konuk güvenlik duvarı ve ağ kartı koruması (trafik altında),
LXC konteynerleri, gerçek S3/SMTP/OpenID Connect/ntfy/Telegram, USB/PCI aktarımı, ve konuk işletim sistemi gerektiren
özellikler (dinamik bellek, konuk içi disk ölçümü, sınamadaki hizmet denetimi). İlgili sayfalar bunu ayrıca belirtir.
