[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Monitoring and alerts

## Alerts

RedoX watches for: an offline node, storage filling up, high memory and CPU use, failed tasks, unexpected guest
shutdowns, and disks filling up inside guests (80% warning, 90% critical; needs the guest agent).

| Channel | Option |
|---|---|
| Webhook (Slack and Discord compatible) | `--alert-webhook URL` |
| E-mail | `--alert-email FILE` |
| ntfy | `--alert-ntfy https://ntfy.sh/your-topic` |
| Telegram | `--alert-telegram FILE` (`token = …`, `chat = …`) |

## Metrics

- Charts on the pages: last 10 minutes, 24 hours, 30 days.
- `GET /metrics` is in Prometheus format (`Authorization: Bearer <token>`; a read-only token is enough).

## Resource metering

Cumulative consumption per guest: uptime, CPU (core-hours), memory (GB-hours), disk read/write and the network traffic
of bridged cards.

```bash
redox usage
redox vm usage 103
redox vm usage 103 reset   # when a billing period closes
redox vm df 103            # disk usage inside the guest
```
