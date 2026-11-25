[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Snapshots

A snapshot is the state of the disk (and optionally memory) at a moment; if something goes wrong you return to it. It is
not a backup: it lives on the same disk.

| Kind | When | Contains | Rollback |
|---|---|---|---|
| Stopped | Guest stopped | Disks | While stopped |
| Running (with memory) | Guest running | Disks + memory | Also while running; the guest continues from that moment |
| Production checkpoint | Guest running, **Production (no memory)** ticked | Disks only; file systems frozen if the agent is present | Only while stopped; the guest boots from that disk |

A production checkpoint is small and fast, and consistent for applications such as databases.

```bash
redox snapshot create 103 before-update
redox snapshot create 103 before-update --production
redox snapshot rollback 103 before-update
redox snapshot delete 103 before-update
```
