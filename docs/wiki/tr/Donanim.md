[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Donanım

Donanım ayarları (*Donanım* sekmesi) konuk **kapalıyken** değişir; aşağıda "çalışırken" yazanlar hariç.

| Ayar | Not |
|---|---|
| Çekirdek, bellek | Kapalıyken. |
| İşlemci türü | Varsayılan `host` (en hızlı). Farklı işlemcili sunucuya taşıyacaksanız ortak bir model seçin. |
| İç içe sanallaştırma | Konuk kendi sanal makinelerini çalıştırır. Sunucuda `kvm_intel`/`kvm_amd` modülünde `nested` açık olmalı. |
| En fazla çekirdek | 0'dan büyükse konuk bu kadar çekirdek yuvasıyla açılır ve **çalışırken** çekirdek eklenir: `redox vm set 103 max-cores=8`, sonra konuk çalışırken `redox vm cores 103 6`. Yalnızca artırılır; azaltmak için konuk kapatılır. Konukta ajan (qemu-guest-agent) varsa yeni çekirdekler kendiliğinden etkinleşir; yoksa konukta `echo 1 > /sys/devices/system/cpu/cpuN/online` gerekebilir. |
| İşlemci payı | Sunucunun işlemcisi yetmediğinde konuğun payı (*Seçenekler* sekmesi ya da `redox vm set 103 cpu-shares=200`). 200 paylı konuk 100 paylının iki katı işlemci alır; 0 olağandır (100), 10 ile 10000 arası verilir. İşlemci boşken kimse kısılmaz. **Çalışırken** de değişir. |
| Bellek garantisi | Sunucunun belleği daraldığında konuktan geri alınmayacak bellek (`redox vm set 103 mem-reserve=4096`). Çalışan konukların garantileri toplamı sunucunun belleğini aşamaz: sığmayan konuk başlatılmaz. Balon, konuğu garantinin altına indiremez. **Çalışırken** de değişir. |
| En fazla bellek | 0'dan büyükse konuk boş bellek yuvalarıyla açılır ve **çalışırken** bellek eklenir: `redox vm set 103 max-memory=16384`, sonra konuk çalışırken `redox vm memory 103 8192`. 128 MB'lık adımlarla ve yalnızca artırılır; bir açılışta en fazla 8 kez eklenir, konuk kapatılıp açılınca yuvalar boşalır. Linux konuklar yeni belleği çoğunlukla kendiliğinden kullanır; kullanmıyorsa konuk ajanı etkinleştirir, ajan da yoksa konukta `echo online > /sys/devices/system/memory/memoryN/state` gerekir. Eklenmemiş bellek sunucuda yer tutmaz. |
| Bellek balonu | Takılıysa bellek hedefi **çalışırken** değişir. |
| Dinamik bellek | Hedefi konuğun kullanımına göre RedoX ayarlar: `redox vm balloon 103 auto 2048`. Konukta balon sürücüsü gerekir. |
| Ek diskler (en çok 8) | **Çalışırken** de eklenir. Disk yalnızca büyütülür. |
| Ek ağ kartları (en çok 3) | **Çalışırken** de eklenir. |
| Hız sınırları | Disk MB/sn ve işlem/sn; ağ Mbit/sn (yalnızca köprüdeki kartlar). |
| Donanım yazılımı | BIOS, UEFI, UEFI + güvenli önyükleme. Windows 11 için UEFI + güvenli önyükleme + TPM. |
| Sanal TPM 2.0 | `swtpm` gerekir. Durumu yedeklere dahildir. |
| Aygıt aktarımı | USB (`lsusb` kimliği) ve PCI (`lspci -D` adresi). **Gerçek donanımla denenmedi.** |
| Ekran kartı paylaşımı (vGPU) | PCI alanına adres yerine aracılı aygıtın kimliği (mdev UUID) yazılır; aynı kart birkaç konuğa bölünür. Aracılı aygıtı sunucuda siz oluşturursunuz (kart ve sürücüsü vGPU desteklemelidir). **Gerçek donanımla denenmedi.** |
| Paylaşılan klasörler | virtio-9p. Konukta: `mount -t 9p -o trans=virtio,version=9p2000.L ad /mnt/ad` |
| cloud-init | Kullanıcı, SSH anahtarı, sabit adres, ağ geçidi, ad sunucuları. |

## ZFS ve LVM üzerinde depolama

Depolama alanı bir dizindir (`--storage ad=/dizin`); diskler orada qcow2 imaj dosyası olarak durur. Dizin bir ZFS
veri kümesi, bir LVM (ya da LVM-thin) biriminin üzerindeki dosya sistemi ya da bir NFS paylaşımı olabilir: RedoX bunu
sunucu başlarken tanır ve *Depolama alanları* tablosundaki **Tür** sütununda gösterir. Sıkıştırma, sağlama toplamı
ve anlık görüntü gibi ZFS özellikleri dosya sisteminin kendisinden gelir.

```
zfs create -o mountpoint=/srv/hizli tank/redox     # ya da: lvcreate -V 500G -T vg0/havuz -n redox && mkfs.ext4 …
redox serve … --storage hizli=/srv/hizli
```

Her diski ayrı bir ZFS birimine (zvol) ya da mantıksal birime koyan yerleşik havuz yönetimi yoktur.

## Depolama alanları arasında taşıma

`redox vm move 103 hizli` ya da *Çoğalt ve Taşı* sekmesi. Kapalı konuk, dosyaları kopyalanarak anlık görüntüleriyle
birlikte taşınır. Çalışan sanal makine **durdurulmadan** taşınır: diskleri konuk çalışırken yeni alana aynalanır, sonra
oraya geçirilir ve eski dosyalar silinir. Yalnızca şimdiki içerik aynalandığı için anlık görüntüsü olan çalışan konuk
taşınmak için kapatılmalıdır.

## Windows ve aktarılan diskler

Konuk virtio sürücüsü bilmiyorsa **Disk denetleyicisi** olarak SATA, **Ağ kartı modeli** olarak Intel seçin. Windows kurarken kurulum kalıbını birinci, `virtio-win` kalıbını ikinci CD sürücüsüne takın.

```bash
redox vm set 103 disk-bus=sata nic-model=e1000e iso2=virtio-win.iso
redox vm media 103 2 baska.iso
```

Çalışırken yalnızca konuk açılırken kalıp takılı olan sürücü değiştirilir. Windows ile denenmedi.

## Performans

`cpu-affinity=0-3` konuğu o çekirdeklere sabitler (çalışırken de değişir). `numa-node=0` belleği o düğüme bağlar. `hugepages=true` için sunucuda önce `vm.nr_hugepages` ayrılmalıdır. KSM için `redox-ksm.service` birimini etkinleştirin; birbirine güvenmeyen kullanıcılar varsa açmayın.
