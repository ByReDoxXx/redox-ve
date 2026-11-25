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

Kesintisiz canlı taşıma ve kendiliğinden devralma yoktur.

## Çoğaltma (yedek kopya)

```bash
redox replica add <eş> <kimlik> 60     # 60 dakikada bir
redox replica list
redox replica run <eş> <kimlik>        # şimdi
redox replica remove <eş> <kimlik>
```

Eşteki sanal makinenin kopyası bu sunucuda düzenli tazelenir; kaynak çalışmaya devam eder. Kopya `<ad>-kopya` adıyla
kapalı durur. Kaynak sunucu ölürse kopyayı buradan **siz** başlatırsınız; kayıp, son turdan beri yazılanlardır.
Çalışan bir kopyanın üzerine yazılmaz. Her tur diskin tamamını kopyalar (artımlı değildir).

## Bakım kipi

Sunucuyu güncellemeden ya da yeniden başlatmadan önce:

```bash
redox maintenance on --drain   # çalışan konukları sırayla kapatır
# ... güncelleme, yeniden başlatma ...
redox maintenance off          # kapatılanları açılış sırasıyla yeniden başlatır
```

Kipteyken hiçbir konuk başlatılmaz; zamanlanmış yedekler, güç işlemleri ve çoğaltma durur. Kip durum dosyasında
saklanır: sunucu yeniden başlasa da konuklar kendiliğinden açılmaz. Konukları başka sunucuya taşımaz.
