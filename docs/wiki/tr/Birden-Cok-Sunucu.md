[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Birden çok sunucu

RedoX VE'de ortak küme durumu yoktur: her sunucu kendi konuklarını yönetir. Sunucuları birbirine **eş** olarak
tanıtırsınız (`--peers DOSYA`, her satır `AD ADRES TOKEN`); *Küme* sayfası hepsini birlikte gösterir.

## Taşıma

*Küme* sayfasında eşteki konuğun yanındaki düğmeler:

- **Buraya Al:** kapalı sanal makineyi yedeği üzerinden bu sunucuya kopyalar. Kaynak yerinde kalır; çalıştığını
  görünce orada siz silersiniz.
- **Çalışırken Al:** çalışan konuğu orada askıya alır, diskini ve belleğini kopyalar, burada kaldığı yerden sürdürür.
  Konuk yeniden başlamaz ama kopyalama boyunca yanıt vermez.

- **Konteynerler:** *Buraya Al* konteyneri yedeği üzerinden kopyalar. Bu sunucuda aynı dağıtımın şablonundan boş bir
  konteyner oluşturulur, sonra kök dosya sistemi yedekle değiştirilir. Kaynak çalışmaya devam eder (yedek sırasında
  kısa süre dondurulur); kopya kapalı gelir. Bu sunucuda LXC kurulu olmalıdır. **Sahte LXC araçlarıyla sınandı; gerçek
  LXC ile denenmedi.**

Kesintisiz canlı taşıma ve kendiliğinden devralma yoktur.

## Çoğaltma (yedek kopya)

```bash
redox replica add <eş> <kimlik> 60     # 60 dakikada bir
redox replica list
redox replica run <eş> <kimlik>        # şimdi
redox replica remove <eş> <kimlik>
```

Eşteki sanal makinenin ya da konteynerin kopyası bu sunucuda düzenli tazelenir; kaynak çalışmaya devam eder. Kopya `<ad>-kopya` adıyla
kapalı durur. Kaynak sunucu ölürse kopyayı buradan **siz** başlatırsınız; kayıp, son turdan beri yazılanlardır.
Çalışan bir kopyanın üzerine yazılmaz. **Artımlı:** sanal makinelerde ilk turdan sonra yalnızca değişen 4 MiB'lik bloklar indirilir; değişmeyenler eski
kopyadan alınır ve indirilen her blok SHA-256 özetiyle doğrulanır. *Çoğaltma* kartı son turda kaç bloğun indiğini
gösterir. Kaynak sunucu yine de her turda diskin tamamını okur (özet çıkarmak için); kazanç ağdadır. Artımlı aktarım
bir nedenle başarısız olursa o disk kendiliğinden tam olarak indirilir. Konteynerler her turda tam kopyalanır.

## Bakım kipi

Sunucuyu güncellemeden ya da yeniden başlatmadan önce:

```bash
redox maintenance on --drain   # çalışan konukları sırayla kapatır
# ... güncelleme, yeniden başlatma ...
redox maintenance off          # kapatılanları açılış sırasıyla yeniden başlatır
```

Kipteyken hiçbir konuk başlatılmaz; zamanlanmış yedekler, güç işlemleri ve çoğaltma durur. Kip durum dosyasında
saklanır: sunucu yeniden başlasa da konuklar kendiliğinden açılmaz. Konukları başka sunucuya taşımaz.
