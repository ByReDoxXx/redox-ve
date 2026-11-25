[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Anlık görüntüler

Anlık görüntü, diskin (ve istenirse belleğin) o andaki halidir; sorun çıkarsa o ana dönersiniz. Yedek değildir: aynı
diskte durur.

| Tür | Ne zaman | Ne içerir | Geri dönüş |
|---|---|---|---|
| Kapalıyken | Konuk kapalı | Diskler | Kapalıyken |
| Çalışırken (bellekli) | Konuk çalışıyor | Diskler + bellek | Çalışırken de; konuk o andan sürer |
| Üretim denetim noktası | Konuk çalışıyor, **Üretim (belleksiz)** seçili | Yalnızca diskler; konuk ajanı varsa dosya sistemleri dondurulur | Yalnızca kapalıyken; konuk o diskle yeniden açılır |

Üretim denetim noktası küçük ve hızlıdır, veritabanı gibi uygulamalar için tutarlıdır.

```bash
redox snapshot create 103 guncelleme-oncesi
redox snapshot create 103 guncelleme-oncesi --production
redox snapshot rollback 103 guncelleme-oncesi
redox snapshot delete 103 guncelleme-oncesi
```
