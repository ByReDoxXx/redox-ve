[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# İzleme ve uyarılar

## Uyarılar

RedoX şunları izler: çevrimdışı düğüm, dolan depolama, yüksek bellek ve işlemci kullanımı, başarısız görevler,
beklenmedik konuk kapanmaları, konuk içinde dolan diskler (%80 uyarı, %90 kritik; konuk ajanı gerekir).

| Kanal | Seçenek |
|---|---|
| Web kancası (Slack, Discord uyumlu) | `--alert-webhook URL` |
| E-posta | `--alert-email DOSYA` |
| ntfy | `--alert-ntfy https://ntfy.sh/konunuz` |
| Telegram | `--alert-telegram DOSYA` (`token = …`, `chat = …`) |

## Ölçümler

- Sayfalardaki grafikler: son 10 dakika, 24 saat, 30 gün.
- `GET /metrics` Prometheus biçimindedir (`Authorization: Bearer <token>`; salt okunur token yeterlidir).

## Kaynak ölçümü

Konuk başına birikimli tüketim: açık kalma süresi, işlemci (çekirdek-saat), bellek (GB-saat), disk okuma/yazma ve
köprüdeki kartların ağ trafiği.

```bash
redox usage
redox vm usage 103
redox vm usage 103 reset   # fatura dönemi kapanınca
redox vm df 103            # konuk içindeki disklerin doluluğu
```

**CSV dışa aktarımı:** kaynak ölçümü `/usage.csv` (API: `GET /api/v1/usage.csv`), denetim günlüğü `/audit.csv`
(yalnızca yönetici; sayfadaki süzgeçle `?q=`). Formül olarak çalışabilecek hücreler tablolama programları için
etkisizleştirilir.
