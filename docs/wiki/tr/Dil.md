[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Arayüz dili

RedoX VE'nin arayüzü Türkçe ve İngilizcedir.

- **Kullanıcı başına:** üst çubuktaki **EN / TR** düğmesi. Seçim tarayıcıda bir çerezde saklanır.
- **Sunucu varsayılanı:** hizmet satırına `--lang en` yazarsanız çerezi olmayan ziyaretçi İngilizce görür
  (varsayılan Türkçedir).
- **API:** hata iletileri `Accept-Language: en` başlığıyla İngilizce döner; `code` alanı dilden bağımsızdır.

## Kapsam

İngilizceye çevrilenler: bütün sayfalar, formlar, bildirimler, form hataları, uyarılar ve API hata iletileri.

Henüz Türkçe kalanlar:

- Komut satırının (`redox`) çıktıları ve yardım metni.
- Sürücülerin içinden gelen hata ayrıntıları (QEMU, LXC, dış hedef, parça deposu). Bunlar görev iletisinin sonunda
  Türkçe görünebilir.
- Görev günlüğündeki komut adları zaten dilden bağımsızdır (`vm start 103`).

Çevirisi olmayan bir metin sayfayı bozmaz: Türkçesi gösterilir.

## Çeviri eklemek

Kaynak metinler Türkçedir. İngilizce tablo `crates/redox-server/src/i18n/en.rs` dosyasındadır; anahtar Türkçe metnin
kendisidir. Değişkenli iletilerde Türkçe kalıpta `{}`, İngilizce karşılığında sırasıyla `{0}`, `{1}` kullanılır.
