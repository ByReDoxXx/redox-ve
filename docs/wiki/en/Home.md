[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# RedoX VE Wiki

RedoX VE is a virtualization platform written in Rust that manages KVM virtual machines and LXC containers on a single
server. It is one executable; its interface is rendered on the server and uses no JavaScript (except on the console,
terminal and security key pages).

## Pages

| Topic | Page |
|---|---|
| Installing on a server, starting the service, first sign-in | [Installation](Installation.md) |
| Creating and starting guests, console, clones, templates | [Guests](Guests.md) |
| Disks, network cards, memory, CPU, UEFI/TPM, passthrough | [Hardware](Hardware.md) |
| Snapshots and production checkpoints | [Snapshots](Snapshots.md) |
| Taking and restoring backups, jobs, retention, tests, off-site | [Backups](Backups.md) |
| Several servers, moving, replication, maintenance mode | [Multiple servers](Multiple-Servers.md) |
| Bridges, VLANs, the firewall, NIC protection | [Networking and security](Networking-and-Security.md) |
| Users, roles, tokens, two-step verification, four-eyes | [Users and access](Users-and-Access.md) |
| Alerts, notification channels, metrics, metering | [Monitoring and alerts](Monitoring-and-Alerts.md) |
| The `redox` command | [Command line](Command-Line.md) |
| REST API | [API](API.md) |
| Interface language | [Language](Language.md) |
| Common problems | [Troubleshooting](Troubleshooting.md) |

## Know what has been tried

Every feature is covered by automated tests, and the QEMU driver has also been exercised against real QEMU/KVM. Some
parts have **not** been tried on a real server yet: bridged networking, the guest firewall and NIC protection (under
traffic), LXC containers, a real S3/SMTP/OpenID Connect/ntfy/Telegram service, USB/PCI passthrough, and features that
need a guest operating system (dynamic memory, in-guest disk usage, service checks in the boot test). The pages say so
where it applies.
