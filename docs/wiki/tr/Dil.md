[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Arayüz dili

RedoX VE'nin arayüzü Türkçe ve İngilizcedir.

- **Kullanıcı başına:** üst çubuktaki **EN / TR** düğmesi. Seçim tarayıcıda bir çerezde saklanır.
- **Sunucu varsayılanı:** hizmet satırına `--lang en` yazarsanız çerezi olmayan ziyaretçi İngilizce görür
  (varsayılan Türkçedir).
- **API:** hata iletileri `Accept-Language: en` başlığıyla İngilizce döner; `code` alanı dilden bağımsızdır.
- **Komut satırı:** `REDOX_LANG=en redox vm list`. Yardım metni, çıktılar ve hata iletileri İngilizce olur.
  `redox serve --lang en` sunucunun kendi açılış iletilerini, uyarı bildirimlerini ve yedek raporu e-postasını da
  İngilizce yapar.

## Kapsam

İngilizceye çevrilenler: bütün sayfalar, formlar, bildirimler, form hataları, uyarılar, API hata iletileri, komut
satırı ve sürücülerin (QEMU, LXC, dış hedef, parça deposu, e-posta, tek oturum açma) hata iletileri.

Çevrilmeyenler:

- Görev günlüğündeki komut adları dilden bağımsızdır (`vm start 103`).
- Dış programların kendi çıktıları (QEMU'nun, `qemu-img`in, `nft`nin hata satırları) olduğu gibi gösterilir.
- Yedek kopyanın adındaki `-kopya` eki ve `tasindi-` / `kopya-` etiketleri veridir, çevrilmez.

Çevirisi olmayan bir metin sayfayı bozmaz: Türkçesi gösterilir.

## Çeviri eklemek

Kaynak metinler Türkçedir. İngilizce tablo `crates/redox-server/src/i18n/en.rs` dosyasındadır; anahtar Türkçe metnin
kendisidir. Değişkenli iletilerde Türkçe kalıpta `{}`, İngilizce karşılığında sırasıyla `{0}`, `{1}` kullanılır.
