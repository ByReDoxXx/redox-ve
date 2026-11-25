[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# API

All endpoints live under `/api/v1` and need `Authorization: Bearer <token>`. Responses are JSON.

```bash
curl -H "Authorization: Bearer $REDOX_TOKEN" https://redox.example.com/api/v1/guests
curl -X POST -H "Authorization: Bearer $REDOX_TOKEN" https://redox.example.com/api/v1/guests/103/start
curl -X PATCH -H "Authorization: Bearer $REDOX_TOKEN" -H "Content-Type: application/json" \
  -d '{"cores": 4}' https://redox.example.com/api/v1/guests/103
```

| Endpoint | Purpose |
|---|---|
| `GET /cluster`, `/nodes`, `/storage`, `/tasks` | Status |
| `GET/POST /guests`, `GET/PATCH/DELETE /guests/{id}` | Guests |
| `POST /guests/{id}/{start,stop,reboot,pause,resume}` | Power |
| `POST /guests/{id}/snapshots`, `…/snapshots/{name}/rollback` | Snapshots (`{"production": true}`) |
| `POST /guests/{id}/backups`, `GET /backups`, `POST /backups/{id}/restore` … | Backups |
| `GET /usage`, `/guests/{id}/usage`, `/guests/{id}/filesystems` | Metering |
| `GET/POST /replicas`, `GET/POST /maintenance`, `GET /approvals` | Replication, maintenance, approvals |
| `GET /media/catalog`, `POST /media/catalog/{key}` | Ready images |

Long-running operations return a task; follow it with `GET /tasks`.

**Errors** have the form `{"error": "...", "code": "..."}`. `code` does not depend on the language (use it in scripts);
the `error` message is English when you send `Accept-Language: en`.

**Limits:** with `--api-rate N`, requests beyond the limit get 429 and `Retry-After`. With the four-eyes principle on,
delete requests return 202 and `{"approval": <n>}`.

## OpenAPI document

`GET /api/v1/openapi.json` returns an OpenAPI 3 description of the API: every endpoint, its methods, path
parameters and a one-line summary. It does **not** contain request or response body schemas; see the examples on
this page for those. The document is compared with the server's real routes in a test, so no endpoint can be missing.

```
curl -H "Authorization: Bearer $TOKEN" https://redox.example/api/v1/openapi.json
```
