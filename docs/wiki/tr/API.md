[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# API

Bütün uçlar `/api/v1` altındadır ve `Authorization: Bearer <token>` ister. Yanıtlar JSON'dır.

```bash
curl -H "Authorization: Bearer $REDOX_TOKEN" https://redox.example.com/api/v1/guests
curl -X POST -H "Authorization: Bearer $REDOX_TOKEN" https://redox.example.com/api/v1/guests/103/start
curl -X PATCH -H "Authorization: Bearer $REDOX_TOKEN" -H "Content-Type: application/json" \
  -d '{"cores": 4}' https://redox.example.com/api/v1/guests/103
```

| Uç | İşlev |
|---|---|
| `GET /cluster`, `/nodes`, `/storage`, `/tasks` | Durum |
| `GET/POST /guests`, `GET/PATCH/DELETE /guests/{id}` | Konuklar |
| `POST /guests/{id}/{start,stop,reboot,pause,resume}` | Güç |
| `POST /guests/{id}/snapshots`, `…/snapshots/{ad}/rollback` | Anlık görüntüler (`{"production": true}`) |
| `POST /guests/{id}/backups`, `GET /backups`, `POST /backups/{id}/restore` … | Yedekler |
| `GET /usage`, `/guests/{id}/usage`, `/guests/{id}/filesystems` | Ölçüm |
| `GET/POST /replicas`, `GET/POST /maintenance`, `GET /approvals` | Çoğaltma, bakım, onaylar |
| `GET /media/catalog`, `POST /media/catalog/{anahtar}` | Hazır imajlar |

Uzun süren işlemler bir görev döndürür; sonucu `GET /tasks` ile izlenir.

**Hatalar** `{"error": "...", "code": "..."}` biçimindedir. `code` dilden bağımsızdır (betiklerde onu kullanın); `error`
iletisi `Accept-Language: en` başlığıyla İngilizce döner.

**Sınırlar:** `--api-rate N` açıksa aşan istek 429 ve `Retry-After` alır. Dört göz ilkesi açıkken silme istekleri 202 ve
`{"approval": <no>}` döndürür.

## OpenAPI belgesi

`GET /api/v1/openapi.json`, API'nin OpenAPI 3 tarifini verir: bütün uçlar, yöntemleri, yol değişkenleri ve birer
satırlık (İngilizce) açıklamaları. İstek ve yanıt gövdelerinin şemaları **yoktur**; onlar için bu sayfadaki
örneklere bakın. Belge, sunucudaki gerçek yönlendirmelerle bir testte karşılaştırılır, yani eksik uç olamaz.

```
curl -H "Authorization: Bearer $TOKEN" https://redox.example/api/v1/openapi.json
```
