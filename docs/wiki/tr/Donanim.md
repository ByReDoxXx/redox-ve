[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Donanım

Donanım ayarları (*Donanım* sekmesi) konuk **kapalıyken** değişir; aşağıda "çalışırken" yazanlar hariç.

| Ayar | Not |
|---|---|
| Çekirdek, bellek | Kapalıyken. |
| İşlemci türü | Varsayılan `host` (en hızlı). Farklı işlemcili sunucuya taşıyacaksanız ortak bir model seçin. |
| İç içe sanallaştırma | Konuk kendi sanal makinelerini çalıştırır. Sunucuda `kvm_intel`/`kvm_amd` modülünde `nested` açık olmalı. |
| Bellek balonu | Takılıysa bellek hedefi **çalışırken** değişir. |
| Dinamik bellek | Hedefi konuğun kullanımına göre RedoX ayarlar: `redox vm balloon 103 auto 2048`. Konukta balon sürücüsü gerekir. |
| Ek diskler (en çok 8) | **Çalışırken** de eklenir. Disk yalnızca büyütülür. |
| Ek ağ kartları (en çok 3) | **Çalışırken** de eklenir. |
| Hız sınırları | Disk MB/sn ve işlem/sn; ağ Mbit/sn (yalnızca köprüdeki kartlar). |
| Donanım yazılımı | BIOS, UEFI, UEFI + güvenli önyükleme. Windows 11 için UEFI + güvenli önyükleme + TPM. |
| Sanal TPM 2.0 | `swtpm` gerekir. Durumu yedeklere dahildir. |
| Aygıt aktarımı | USB (`lsusb` kimliği) ve PCI (`lspci -D` adresi). **Gerçek donanımla denenmedi.** |
| Paylaşılan klasörler | virtio-9p. Konukta: `mount -t 9p -o trans=virtio,version=9p2000.L ad /mnt/ad` |
| cloud-init | Kullanıcı, SSH anahtarı, sabit adres, ağ geçidi, ad sunucuları. |

## Depolama alanları arasında taşıma

`redox vm move 103 hizli` ya da *Çoğalt ve Taşı* sekmesi. Konuk kapalı olmalıdır; diskler ve anlık görüntüler birlikte
taşınır.
