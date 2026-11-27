[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Installation

The detailed step-by-step guide is [INSTALL.md](../../../INSTALL.md) in the repository root. This page is a summary.

## Requirements

- A 64-bit Linux server (Debian 12 / Ubuntu 22.04 or later) with KVM (`/dev/kvm`).
- `qemu-system-x86`, `qemu-utils`; `ovmf` for UEFI, `swtpm` for a virtual TPM, `lxc` for containers.
- Rust (stable) to build.

## Quick start

```bash
cargo build --release
sudo install -m 755 target/release/redox /usr/local/bin/redox

# First administrator (asks for a password, at least 12 characters)
redox user add admin --users /etc/redox/users.json

# Service
redox serve --addr 127.0.0.1:8006 --users /etc/redox/users.json \
  --driver qemu --data-dir /var/lib/redox --state /var/lib/redox/state.json --lang en
```

The interface is at `http://127.0.0.1:8006`. To expose it, put a TLS-terminating proxy (Caddy) in front and add
`--behind-tls-proxy`; see section 5 of INSTALL.md.

## Common service options

| Option | What it does |
|---|---|
| `--driver qemu` | The real driver. Without it a simulation runs (for trying things out). |
| `--state FILE` | Makes the state persistent. Without it everything stays in memory. |
| `--storage name=/path` | An extra storage. |
| `--offsite FILE`, `--offsite-copy FILE` | Off-site backup targets ([Backups](Backups.md)). |
| `--incremental-backups` | Incremental (chunked) backups. |
| `--peers FILE` | Peer servers ([Multiple servers](Multiple-Servers.md)). |
| `--firewall` | Guest firewall and NIC protection (nftables, CAP_NET_ADMIN). |
| `--four-eyes` | Deletions wait for a second administrator. |
| `--lang en` | English interface for visitors without a language cookie ([Language](Language.md)). |
| `--alert-webhook`, `--alert-email`, `--alert-ntfy`, `--alert-telegram` | Alert channels. |
| `--backup-report 07:30`, `--backup-age-days 7`, `--backup-test-every 7@03:00` | Backup report, unprotected threshold, scheduled boot tests. |
| `--api-rate 600` | API rate limit (per user per minute). |

## The console screen

After an install with `install.sh` or the ISO, the server's own screen (the first virtual console) shows a status
screen instead of a login prompt: the server's name and addresses, where the web interface is, whether the management
service answers, how many guests are running, memory, load and uptime. It refreshes every few seconds and performs no
actions. Press **Enter** to get the ordinary login; when you log out the screen comes back. The other virtual consoles
(Alt+F2 and up) keep the ordinary login prompt.

```
redox tty --once        # print the screen once (also works over SSH)
systemctl disable --now redox-console.service && systemctl enable --now getty@tty1.service   # go back to a plain login prompt
```

The screen has not been tried on a physical console yet; if it misbehaves there, the second command restores the login prompt.
