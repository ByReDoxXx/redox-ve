[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Yedekleme

Yedek, konuğun disklerinin (ek diskler, UEFI değişkenleri ve TPM durumu dahil) sunucudaki yedek dizinine alınan
kopyasıdır. Çalışan sanal makinenin yedeği kesintisiz alınır.

## Almak ve geri yüklemek

Konuğun *Yedekler* sekmesi ya da:

```bash
redox backup create 103
redox backup list
redox backup restore <yedek>            # konuğun üstüne (konuk kapalı olmalı)
redox backup restore-new <yedek> <ad>   # ayrı bir konuk olarak (özgün konuk silinmiş olsa da)
redox backup instant <yedek> <ad>       # anında kurtarma: konuk yedeğin üzerinde hemen açılır
redox vm detach <kimlik>                # anında kurtarılan konuğu yedekten ayırır
```

**Dosya düzeyinde geri yükleme:** *Yedekler* sayfasından yedeğin içine göz atıp tek dosya indirirsiniz (sunucuda
`libguestfs-tools` gerekir; yalnızca yöneticiler, her erişim denetim günlüğüne yazılır). Konuk çalışıyorsa ve içinde
`qemu-guest-agent` kuruluysa, dosyanın ya da klasörün yanındaki *Konuğa yükle* onu doğrudan konuğun içine geri yazar:
hedef yolu siz seçersiniz (varsayılan, özgün yoldur), sonuç *Görevler* sayfasında görünür. Dosya hedefin üzerine yazılır,
sahibi ve izinleri korunmaz; klasör, konukta var olan dizine konuğun kendi `tar` programıyla açılır.
Komut satırından: `redox backup put <yedek> /etc/hostname` ya da `redox backup put-dir <yedek> /etc/ssh`.

## Zamanlama ve saklama

- **Konuk başına:** *Seçenekler* sekmesinde saat ve saklanacak yedek sayısı; ek olarak haftalık/aylık/yıllık (GFS).
- **Yedek işleri:** *Yedekler* sayfası. Bir iş, bir etiketteki (ya da bütün) konukları sırayla yedekler; pencere sonu,
  kilit süresi ve doğrulama ayarlanır. `redox job add gece time=03:00 days=1-5 tags=web`
- **Kilit:** kilitli yedek, süresi dolana kadar kimse tarafından silinemez.
- **Yedek yaşı hedefi (RPO):** `redox vm set 103 rpo=24`. Son yedeği bundan eski konuk "korumasız" görünür.

## Güvendiğinizi doğrulayın

- **Doğrula:** bozulma denetimi.
- **Açarak sına:** yedek, yalıtılmış geçici bir konukta başlatılır. İsterseniz bir TCP kapısından veri gelmesi ve konukta
  bir komutun başarıyla bitmesi de beklenir: `redox vm set 103 test-port=22 test-cmd=/usr/local/bin/saglik.sh`
- **Zamanlanmış sınama:** `--backup-test-every 7@03:00` her konuğun en yeni yedeğini haftada bir sınar.
- **Olağan dışı değişim uyarısı:** artımlı yedekte değişen veri olağanın katlarına çıkarsa yedek işaretlenir (fidye
  yazılımı belirtisi olabilir). Baktıysanız `redox backup ack <yedek>` ile kapatın.
- **Günlük rapor:** `--backup-report 07:30` (e-posta ayarı gerekir) ya da `redox backup report`.
- **Doluluk tahmini:** *Yedekler* sayfası, son iki haftada eklenen yedek verisinden alanın kaç gün sonra dolacağını
  gösterir; 14 gün kala uyarı, 3 gün kala kritik uyarı verilir. Silinen yedeklerin geri verdiği yer sayılmaz, yani
  tahmin temkinlidir.

## Sunucu dışına kopyalama

`--offsite DOSYA` asıl dış hedefi (bir dizin ya da S3 uyumlu depo), `--offsite-copy DOSYA` ek hedefleri tanımlar.
Dosyada `passphrase` varsa yedekler gönderilmeden önce şifrelenir. Kopya hedefinin dosyasına `keep_days = 90`
yazarsanız o hedef **arşiv** olur: yerelde silinen yedek orada 90 gün durur ve *Yedekler → Arşiv* kartından geri
getirilir.

Yerel yedekler şifrelenmez; yedek dizinini şifreli bir birime (LUKS) koyun.

## Artımlı yedek

`--incremental-backups` ile yedekler ortak bir parça deposuna yazılır: her yedek yalnızca yeni veriyi ekler ama tek
başına geri yüklenir; birini silmek diğerlerini bozmaz. Çalışan sanal makinede değişen blok takibi yalnızca değişen
bölümleri okur.

## Yerel yedeklerin şifrelenmesi

```
redox serve … --incremental-backups --backup-passphrase-file /etc/redox/yedek-parola
```

Dosyada tek satır, en az 12 karakterlik bir parola bulunur (`chmod 600`). Bundan sonra alınan sanal makine
yedeklerinin parçaları ve konteyner arşivleri diske ChaCha20-Poly1305 ile şifreli yazılır; anahtar paroladan Argon2id ile türetilir. *Yedekler*
sayfasında **şifreli depo** etiketi görünür.

- **Parolayı kaybederseniz bu yedekler açılamaz.** Parolayı sunucunun dışında da saklayın.
- Şifrelenmiş depo parolasız ya da yanlış parolayla açılmaz; sunucu başlamayı reddeder.
- Şifrelemeyi açmadan önce alınmış yedekler olduğu gibi (şifresiz) kalır ve okunmaya devam eder.
- Şifrelenmeyenler: sanal makinelerin tam (artımlı olmayan) yedekleri ve yedek dizinleri (parça özetleri ile disk
  boyutları). Geri yükleme ve dışa aktarma sırasında geçici açık dosyalar oluşur ve iş bitince silinir.
- Dış hedefe giden parçalar da şifreli gider (dış hedefin kendi parolası varsa iki kat).
- Şifreli depoda artımlı çoğaltma için açık önbellek tutulmaz; bu sunucudan yapılan çoğaltma her turda tam kopyalar.
