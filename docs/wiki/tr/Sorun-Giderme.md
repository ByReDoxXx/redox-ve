[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Sorun giderme

**Konuk başlamıyor.** *Görevler* sayfasındaki başarısız görevin iletisine bakın. Sık nedenler: `/dev/kvm` yok ya da
hizmet kullanıcısının `kvm` grubunda olmaması; köprünün `/etc/qemu/bridge.conf` içinde izinli olmaması; UEFI için
`ovmf`, TPM için `swtpm` kurulu olmaması; iç içe sanallaştırma isteniyor ama sunucuda kapalı.

**"Sunucu bakım kipinde" hatası.** `redox maintenance off`.

**Konuk kendiliğinden açılmıyor.** Bakım kipi açık olabilir; ya da konuk 10 dakikada çok kez çöktüğü için yeniden
başlatmaktan vazgeçilmiştir (görev günlüğünde yazar).

**Yedek alınmıyor.** *Yedekler* sayfasındaki "Korumasız konuklar" kartına ve *Görevler* sayfasına bakın. Aynı konuğun
aynı saniyede ikinci yedeği alınamaz. Yedek öncesi komut başarısızsa yedek bilerek alınmaz.

**Yedek silinemiyor.** Kilitli olabilir (süresi dolana kadar kimse silemez), üzerinde anında kurtarılmış bir konuk
çalışıyor olabilir, ya da dört göz ilkesi açıktır ve ikinci yöneticinin onayı bekleniyordur (`redox approval list`).

**Dinamik bellek hedefi değişmiyor.** Konukta virtio balon sürücüsü yoksa ya da "kullanılabilir bellek" ölçümünü
bildirmiyorsa hedefe dokunulmaz.

**Konuk içi diskler görünmüyor.** Konukta `qemu-guest-agent` çalışmalıdır; ölçüm dakikada bir okunur.

**Ağ kartı koruması açıkken konuk ağa çıkamıyor.** Konuk kendi MAC adresinden başka bir adresle (ör. içinde köprü ya da
iç içe sanal makine) konuşuyorsa çerçeveleri düşürülür; o konukta korumayı kapatın.

**Çoğaltma "devralınmış" diyor.** Yedek kopya çalışıyor; üzerine yazılmaz. Kopyayı kapatın ya da çoğaltmayı kaldırın.

**API 429 döndürüyor.** `--api-rate` sınırı aşıldı; `Retry-After` kadar bekleyin.

**Arayüz yanlış dilde.** Üst çubuktaki EN / TR düğmesi; ya da tarayıcıdaki `lang` çerezini silin ([Dil](Dil.md)).

Çözülmeyen sorunlar için hizmet günlüğü: `journalctl -u redox -e`.

## Destek paketi

Sorun bildirirken bir destek paketi ekleyin: `redox support > destek.json` ya da *Sunucular* sayfasında *Destek paketi*
altındaki *İndir* (yalnızca yönetici). Tek bir JSON dosyasıdır: sürüm, sürücünün bu sunucuda yapabildikleri, ayarlar,
düğümler ve depolama, her konuğun donanımı, son 200 görev ve sunucuya dair birkaç ölçüm (çekirdek, yük, bellek,
adresler).

İçinde kullanıcılar, parolalar, token'lar, eş sunucuların token'ları ve denetim günlüğü **yoktur**; yedek öncesi ve
sonrası çalışan komutlar ile konukların SSH anahtarları maskelenir. Konuk adları, ağ adresleri ve görev iletileri ise
vardır: göndermeden önce okuyun. Yönetim hizmetinin kendi günlüğü pakette yoktur; onu `journalctl -u redox` ile alın.
