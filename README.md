<p align="center">
  <img src="crates/redox-server/assets/logo.svg" alt="RedoX VE logo" height="96">
  &nbsp;&nbsp;
  <img src="crates/redox-server/assets/wordmark.svg" alt="RedoX VE" height="96">
</p>

<p align="center">
  <b>English</b> · <a href="README.tr.md">Türkçe</a>
</p>

# RedoX VE

RedoX VE is a self-hosted virtualization platform written in Rust, in the spirit of
[Proxmox VE](#redox-ve-and-proxmox-ve). One binary, `redox`, runs KVM virtual
machines and unprivileged LXC containers on a Linux server and gives you a web panel, a REST API and a
command-line client for them.

It is built for a single server or a handful of independent servers: a home lab, a small office, a
development host. It is not a clustered hypervisor (see [Limits](#limits)).

> **Language:** the web panel and the CLI help text are currently in Turkish. The documentation is in both
> languages: this README and [INSTALL.md](INSTALL.md) in English, [README.tr.md](README.tr.md) and
> [KURULUM.md](KURULUM.md) in Turkish. A topic-by-topic wiki is in [docs/wiki/en](docs/wiki/en/Home.md)
> ([Türkçe](docs/wiki/tr/Ana-Sayfa.md)).

![Overview page](docs/img/overview.jpg)

## Contents

- [Features](#features)
- [RedoX VE and Proxmox VE](#redox-ve-and-proxmox-ve)
- [Screenshots](#screenshots)
- [Installation](#installation)
- [Command line and API](#command-line-and-api)
- [Technical documentation](#technical-documentation)
- [Security model](#security-model)
- [Testing](#testing)
- [Limits](#limits)
- [License](#license)

## Features

**Guests**

- KVM virtual machines through QEMU, and unprivileged LXC containers, managed from the same pages.
- Containers can be cloned, and backed up while running (frozen for the duration of the archive).
- BIOS, UEFI, UEFI with secure boot, and a virtual TPM 2.0 (enough for Windows 11).
- Install from an ISO, or create from a cloud image with cloud-init (user, SSH key, host name). The
  cloud-init settings, including a static address, can be edited later from the panel.
- Selectable CPU model per VM, and a memory balloon whose target can be changed while the VM runs.
- Import `vmdk`, `vdi`, `vhdx` disks and OVA archives; export a guest as OVA.
- Templates, full clones and linked clones (a linked clone is ready in under a second).
- Up to 8 extra data disks and 3 extra network cards; both can be hot-plugged into a running VM.
- Suspend to disk and resume, pause, scheduled start/stop/reboot, start order at host boot.
- Automatic restart of a guest that crashed or shut itself down.
- Shared folders between host and VM (virtio-9p), USB and PCI passthrough.
- Dynamic memory (the balloon target follows what the guest uses) and nested virtualization.
- Resource metering per guest: cumulative core-hours, GB-hours and disk I/O for billing and planning.
- NIC protection on bridged cards: no MAC spoofing, no rogue DHCP server or router advertisements.
- Maintenance mode: drain the guests before a host update and start them again afterwards.
- Disk usage inside guests (through the agent) with alerts; alerts by webhook, e-mail, ntfy and Telegram.
- Read-only API tokens and an optional API rate limit.
- Local backup encryption (`--backup-passphrase-file`): incremental backup chunks are stored encrypted on disk.
- Incremental replication: after the first run only the changed 4 MiB blocks cross the network, each verified by hash.
- Containers can be copied and replicated between servers; a forecast of when the backup storage fills up; CSV export
  of resource usage and the audit log; the virtio-win driver media in the image catalog.
- Turkish and English throughout: a switch in the top bar, `--lang en` as the server default, API error messages by
  `Accept-Language`, and the command line with `REDOX_LANG=en`.

**Storage and backup**

- Live snapshots (including memory) and live backups of running VMs.
- Scheduled backups with retention, per guest or as backup jobs that cover many guests one at a time.
- UEFI variables and virtual TPM state are saved with the backup and restored with it.
- Restore a backup as a new guest, or start a guest straight from its backup (instant recovery) and detach it later.
- Boot test: a backup is started in an isolated temporary guest to prove that it boots; optionally a TCP port
  must answer and a command inside the guest must succeed.
- Production checkpoints: disk-only snapshots of a running VM with file systems frozen through the guest agent.
- A warning when an incremental backup changes far more data than usual (a possible sign of ransomware).
- A backup age target (RPO) per guest, and an archive: copy targets can keep backups for a set number of days
  after they were deleted locally.
- Four-eyes principle (optional): deleting a backup, a guest, a disk, a backup job or a user needs a second
  administrator's approval.
- Replication: a standby copy of a peer server's VM, refreshed on an interval and started by hand on failover.
- Scheduled boot tests of the newest backups, and a catalog of ready cloud images.
- File-level restore: browse a backup and download single files (with libguestfs).
- Weekly, monthly and yearly retention (GFS), locked (immutable) backups, backup windows and a copy speed limit.
- Commands inside the guest before and after a live backup, through the guest agent.
- A daily backup report by e-mail and a list of guests that have no recent backup.
- Incremental backups: a content-addressed 4 MiB chunk store with no chains, so any backup can be deleted.
- Changed-block tracking, so an incremental backup of a running VM reads only the blocks that changed.
- Off-site copies to a directory (second disk, NFS, SMB) or any S3-compatible store, with client-side
  encryption. Encrypted copies are bound to the backup's name, so swapped files are rejected on restore.
  Large files go to S3 as multipart uploads. Backups can be copied to several targets (3-2-1).
- Backup verification, and repair of a damaged local chunk from the off-site copy.
- Additional storage directories for VM disks, and moving a stopped VM's disks between them.
- Resource pools with quotas on cores, memory, disk and guest count.

**Network**

- User-mode NAT with port forwarding, or bridged networking.
- VLAN tags per network card, and disk and network rate limits per guest.
- Per-guest inbound firewall (nftables).

**Operations**

- Server-rendered web panel with dark and light themes, usable without JavaScript.
- Live graphical console in the browser, with keyboard and mouse.
- Roles (admin, operator, viewer), tag-based operator scopes, TOTP two-factor login, OpenID Connect.
- Audit log, task log, resource alerts by webhook or e-mail, Prometheus metrics.
- A combined view of several RedoX VE servers, and moving a VM between them while keeping its memory state,
  its extra disks, its UEFI variables and its TPM state.

## RedoX VE and Proxmox VE

RedoX VE is modeled on [Proxmox VE](https://www.proxmox.com/en/proxmox-virtual-environment/overview). If you
have used Proxmox, you will recognize the ideas: numeric guest IDs starting at 100, VMs and containers side
by side, `vmbr0`-style bridges, a task log, snapshots and backups per guest, and the panel on port 8006.

It is an independent project written from scratch in Rust. It shares no code with Proxmox VE, does not read
Proxmox configuration or backup formats, and is not a drop-in replacement.

| | Proxmox VE | RedoX VE |
|---|---|---|
| Form | A Debian-based distribution you install on the machine | One binary you add to an existing Debian or Ubuntu server |
| Guests | KVM virtual machines, LXC containers | KVM virtual machines, LXC containers |
| Clustering | Multi-node cluster with shared state, live migration and high availability | Independent servers with a combined view; a running VM can be moved, with a pause |
| Storage | LVM, ZFS, Ceph, NFS and more | Directories (local disk or a mounted share), qcow2 |
| Backup | vzdump and Proxmox Backup Server | Built-in full and incremental backups, off-site copy to a directory or S3 |
| Web panel | JavaScript application | Server-rendered HTML, no JavaScript outside the live console |
| Maturity | Many years in production | New; several parts not yet tried on a real server (see [Testing](#testing)) |

Choose Proxmox VE if you need a cluster, shared storage or a proven platform. RedoX VE aims at a single
server where a small, auditable service with strict defaults is worth more than breadth.

*Proxmox is a registered trademark of Proxmox Server Solutions GmbH. RedoX VE is not affiliated with or
endorsed by Proxmox Server Solutions GmbH.*

## Screenshots

The screenshots show the panel running with the built-in simulation driver, which displays a sample cluster.

| Guests | Guest summary |
|---|---|
| ![Guest list](docs/img/guests.jpg) | ![Guest summary](docs/img/guest-summary.jpg) |

| Hardware | Options |
|---|---|
| ![Hardware tab](docs/img/hardware.jpg) | ![Options tab](docs/img/options.jpg) |

| Storage | Light theme |
|---|---|
| ![Storage page](docs/img/storage.jpg) | ![Light theme](docs/img/light-theme.jpg) |

Live console, connected to a real QEMU guest:

![Live console](docs/img/live-console.jpg)

## Installation

These steps target Debian 12 or Ubuntu 22.04 and later (systemd, x86_64). Replace `redox.example.com`
with your own name. The full guide, with every option explained, is [INSTALL.md](INSTALL.md).

### 1. Build

Copy the source tree to the server, then:

```bash
sudo apt update && sudo apt install -y build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"

cd Proxmox-Rust
cargo test --workspace
cargo build --release
sudo install -m 0755 target/release/redox /usr/local/bin/redox
```

### 2. Service user and first administrator

There is no default account. The server refuses to start until a user exists.

```bash
sudo useradd --system --home-dir /var/lib/redox --shell /usr/sbin/nologin redox
sudo install -d -o redox -g redox -m 0700 /var/lib/redox
sudo -u redox redox user add admin --users /var/lib/redox/users.json
```

The password is asked twice, is not echoed, and must be at least 12 characters.

### 3. systemd service

```bash
sudo cp deploy/redox.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload
sudo systemctl enable --now redox
curl -s http://127.0.0.1:8006/healthz
```

The last command prints `ok`. At this point the server runs the simulation driver, which starts nothing
real and shows a sample cluster.

### 4. TLS

RedoX VE does not terminate TLS and refuses to listen in plain HTTP on anything but `127.0.0.1`. Put
[Caddy](https://caddyserver.com/docs/install) in front of it:

```bash
sudo cp deploy/Caddyfile /etc/caddy/Caddyfile
sudo nano /etc/caddy/Caddyfile
sudo systemctl reload caddy
sudo ufw allow OpenSSH && sudo ufw allow 80,443/tcp && sudo ufw enable
```

Do not open port 8006. Without a domain name, skip Caddy, remove `--behind-tls-proxy` from the unit file
and use an SSH tunnel instead: `ssh -L 8006:127.0.0.1:8006 user@server`.

### 5. Real virtual machines

```bash
sudo apt install -y qemu-system-x86 qemu-utils
ls -l /dev/kvm
sudo cp deploy/redox-qemu.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload && sudo systemctl restart redox
journalctl -u redox -n 20 --no-pager
```

The log should show the `qemu-kvm` driver. If `/dev/kvm` is missing, enable virtualization in the
firmware; otherwise QEMU falls back to software emulation, which is very slow.

Optional pieces, each described in [INSTALL.md](INSTALL.md):

| Need | Packages | Section |
|---|---|---|
| UEFI, secure boot, TPM | `ovmf`, `swtpm` | 8 |
| Bridged networking, LXC containers | `lxc`, `uidmap`, `acl`; unit `deploy/redox-host.service` | 9 |
| VLAN tags, network rate limits | `iproute2`, `CAP_NET_ADMIN` | 8 |
| Guest firewall | `nftables`, `CAP_NET_ADMIN`, `--firewall` | 12 |
| Alerts, several servers | `--alert-webhook`, `--alert-email`, `--peers` | 10 |
| Off-site backups | `--offsite` | 11 |
| Single sign-on | `--oidc` | 13 |

### Trying it on your own machine

The simulation driver also runs on Windows and macOS, which is enough to look around the panel:

```bash
cargo build
./target/debug/redox user add admin --users users.json
./target/debug/redox serve --users users.json
```

Then open `http://127.0.0.1:8006`.

## Command line and API

The same binary is the client. Create a token on the server (it is shown once):

```bash
sudo -u redox redox token create admin laptop --users /var/lib/redox/users.json
```

Then, from any machine:

```bash
export REDOX_URL=https://redox.example.com
export REDOX_TOKEN=redox_...

redox cluster status
redox vm list
redox vm start 103
redox vm set 103 cores=4 memory=8192 tags=prod,web backup-time=03:30
redox vm disk add 103 20
redox snapshot create 103 before-upgrade
redox backup create 103
redox backup restore vm-103-1795546651
redox task list
```

`redox --help` lists every command.

The REST API lives under `/api/v1` and takes `Authorization: Bearer <token>`. State-changing calls return
a task; poll `/api/v1/tasks/{id}` for the result.

| Method and path | Purpose |
|---|---|
| `GET /cluster`, `GET /nodes`, `GET /nodes/{id}` | Cluster and node status |
| `GET /guests`, `POST /guests` | List and create guests |
| `GET`, `PATCH`, `DELETE /guests/{vmid}` | Show, change, destroy |
| `POST /guests/{vmid}/{start,stop,reboot,pause,resume}` | Power actions |
| `POST /guests/{vmid}/suspend`, `/discard-suspend`, `/detach` | Suspend to disk, detach from backup |
| `POST /guests/{vmid}/snapshots`, `.../{name}/rollback` | Snapshots |
| `POST /guests/{vmid}/clone`, `/template`, `/resize`, `/disks`, `/nics`, `/balloon`, `/move` | Clones, hardware, storage move |
| `GET /guests/{vmid}/export` | OVA export |
| `POST /guests/{vmid}/backups`, `GET /backups` | Backups |
| `POST /backups/{id}/restore`, `/push`, `/verify`, `GET .../download` | Restore, off-site, verify, download |
| `POST /backups/{id}/restore-new`, `/instant`, `/test`, `/lock` | New guest, instant recovery, boot test, lock |
| `GET /backups/{id}/files`, `/file`, `GET /backups/report` | Files inside a backup, backup report |
| `GET /jobs`, `POST /jobs`, `DELETE /jobs/{name}`, `POST /jobs/{name}/run` | Backup jobs |
| `GET /pools`, `POST /pools`, `DELETE /pools/{name}` | Resource pools and quotas |
| `GET /storage`, `GET /media`, `PUT /media/{name}`, `POST /media/fetch` | Storage, ISO and image upload |
| `GET /peers`, `POST /peers/{name}/import/{vmid}` | Other servers, VM import |
| `GET /tasks`, `GET /tasks/{id}` | Task log |

`GET /metrics` (outside `/api/v1`) serves Prometheus metrics with the same token.

## Technical documentation

### Layout

```
crates/
  redox-core/     Domain model and rules. No I/O, no async.
  redox-server/   The `redox` binary: HTTP server, drivers, CLI.
deploy/           systemd units and a Caddyfile.
docs/img/         Screenshots used in this README.
INSTALL.md        Detailed installation and operations guide (KURULUM.md is the Turkish version).
```

About 28,500 lines of Rust in the two crates.

### `redox-core`

| Module | Contents |
|---|---|
| `model`, `hw` | Guests, nodes, storage, snapshots, backups, tasks; the hardware description of a guest |
| `cluster`, `ops` | The state machine: which operation is allowed in which state, guest locking, task bookkeeping |
| `jobs` | Backup jobs: selection by tag, days, one guest at a time, per-job retention |
| `pools` | Resource pools: quotas on allocated cores, memory, disk and guest count |
| `retention` | Weekly, monthly and yearly retention, backup locks, calendar arithmetic |
| `driver` | The `Hypervisor` trait that every backend implements, and the `Remote` trait for off-site targets |
| `command` | The command grammar shared by the CLI and the panel's command bar |
| `seed` | The sample cluster shown by the simulation driver |

Every rule lives here and is tested without a hypervisor. The server only decides *how* an operation is
carried out; `redox-core` decides *whether* it may happen.

### `redox-server`

| Area | Modules |
|---|---|
| HTTP | `lib` (router, security headers), `api` (REST), `ui`, `ui_ops`, `ui_admin` (page handlers), `view*` (HTML with [maud](https://maud.lambda.xyz)) |
| State | `state` (application state, task runner, schedulers), `history` and `metrics` (graphs, Prometheus) |
| Drivers | `qemu` (QEMU/KVM over QMP), `lxc`, `hostdriver` (combines the two), `qga` (guest agent) |
| Backup | `chunkstore` (incremental store), `offsite` (directory and S3 targets, encryption), `ova`, `guestfs` (files inside a backup), `report` |
| Network | `net`, `netlimit` (`tc`), `firewall` (nftables) |
| Identity | `auth` (users, sessions, tokens), `totp`, `oidc`, `audit` |
| Operations | `alerts`, `mail`, `ha` (restart watchdog), `peers` (other servers), `media` (ISO and image upload) |
| Client | `cli`, `main` |

### How an operation runs

1. A request arrives from the panel (form POST), the API or the CLI (which calls the API).
2. `redox-core` checks the rule and, if allowed, locks the guest and records a task.
3. The task runs on a background thread against the driver.
4. The result is written to the task log and the guest is unlocked. A failed task leaves the guest as it was.

State is a single JSON file (`state.json`), written atomically. Guests keep running when the service
restarts and are adopted again on start.

### QEMU driver

Each VM is one QEMU process started with a fixed, versioned device layout:

- Control over a QMP Unix socket in the data directory.
- Display over a VNC Unix socket (no password, never on the network).
- PCIe root ports reserved for hot-plug: eight for disks, three for network cards.
- The layout version is recorded with a suspended guest, so it resumes with the layout it was saved with.

Data directory (`/var/lib/redox`):

| Path | Contents |
|---|---|
| `images/` | Guest disks (qcow2), suspend state, changed-block maps |
| `iso/`, `cloud/` | Installation media and cloud images |
| `backups/`, `backups/chunks/` | Full backups and the incremental chunk store |
| `shares/` | Shared folders |
| `run/` | QMP and VNC sockets, pid files |
| `state.json`, `users.json`, `sessions.json`, `audit.log`, `metrics.json` | Records |

### Backups

- **Full:** one compressed qcow2 per disk. A running VM is backed up with a QEMU backup job; with several
  disks the jobs start in one transaction.
- **Incremental** (`--incremental-backups`): the disk is cut into 4 MiB chunks named by their SHA-256.
  Each backup is a complete list of chunks, so there is no chain to break. Zero regions are not stored.
- **Changed-block tracking** (`--changed-block-tracking`): a persistent QEMU dirty bitmap. It is dropped,
  and the next backup reads the whole disk, whenever it might be wrong (restore, rollback, resize, crash).
- **Machine state:** the UEFI variable store and the virtual TPM state go into a small archive next to the
  backup (`<backup>.machine.tar`) and follow it off-site.
- **Off-site:** ChaCha20-Poly1305 in 64 KiB chunks, key derived with Argon2id. The file's name is
  authenticated with every chunk.

### Web panel and live console

Pages are rendered on the server and work without JavaScript; the content security policy is
`script-src 'none'`. Fonts and icons are embedded in the binary, so a page makes no external request.

The live console is the one exception. That page loads a single script served by RedoX VE itself: an RFB
3.8 (VNC) client of about 300 lines. The server checks the session and the permission, then bridges the
browser's WebSocket to the guest's VNC Unix socket. Keys are sent as scancodes, so the guest's own keyboard
layout decides the character.

## Security model

| Layer | What it does |
|---|---|
| Identity | User name and password (Argon2id). No default account. |
| Session | 256-bit random cookie; `HttpOnly`, `SameSite=Strict`, `Secure` behind TLS. Ends after 2 hours idle or 12 hours. |
| Brute force | 5 failed attempts in 15 minutes for the same address and user locks for 15 minutes. |
| Second factor | Optional TOTP; a code is accepted once. |
| Roles | `admin` does everything, `operator` runs guests within its scope, `viewer` only reads. |
| Audit | Logins, refused attempts and every state-changing request go to `audit.log`. |
| API | Bearer tokens, stored only as SHA-256 digests. |
| Browser | Cross-site POSTs and cross-site console connections are refused; no framing; HSTS. |
| Process | Unprivileged `redox` user, systemd sandbox, files `0600`. |

## Testing

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

505 tests pass on Linux and 435 on Windows (the QEMU and LXC driver tests are Linux-only). The driver
tests run against a fake QEMU that speaks QMP. If Node.js is installed, the live console client's own
7 tests run as well.

Beyond the test suite, the QEMU driver has been exercised against real QEMU 10.2 with KVM: snapshots,
full and incremental backups, changed-block tracking, off-site copies, suspend and resume, hot-plug,
VLAN and rate limits, the live console, moving a running VM between two servers, backup jobs, CPU models,
the balloon device, backup and restore of UEFI variables and TPM state, moving disks between storages,
pool quotas, moving a VM with extra disks, UEFI and TPM between two servers, restore as a new guest, instant
recovery, the boot test, locks, copies to two off-site targets, production checkpoints, nested virtualization,
resource metering, the boot test's port forwarding, the copy-target archive, the four-eyes approval flow,
replication of a running VM between two servers, maintenance mode across a server restart, read-only tokens and
the API rate limit.

Not yet verified on a real server: the systemd units and the Caddy configuration, bridged networking,
LXC containers, the guest firewall and NIC protection under traffic, dynamic memory and the boot-test service
checks and in-guest disk usage with a real guest operating system, network metering on a real bridge, real ntfy
and Telegram, a real S3 provider, a real SMTP or OpenID Connect provider, and
USB/PCI passthrough. [INSTALL.md](INSTALL.md) marks each of these where it is described.

## Limits

- No shared cluster state. Several servers can be viewed together, but each one manages its own guests.
- Moving a running VM keeps its memory but pauses it while disk and memory are copied. It is not a
  zero-downtime live migration.
- No automatic failover between servers. A crashed guest is restarted on the same server only.
- No LVM or ZFS storage pools.
- CPU and memory cannot be added to a running VM (the balloon only lowers and restores memory).
- Containers cannot be moved to another storage or to another server.
- The live console has no clipboard or audio.
- The interface is Turkish only.

## License

RedoX VE is source-available under the [PolyForm Noncommercial License 1.0.0](LICENSE.md).

- **Allowed:** using, studying, changing and sharing it for any noncommercial purpose. That covers personal
  use, home labs, hobby projects, research, and use by schools, charities and government institutions.
- **Not allowed:** commercial use. That includes running it as part of a business, offering it as a paid
  service, and selling it or products built on it.
- Copies and modified versions must carry the license and the `Required Notice` line in `LICENSE.md`.

Because commercial use is excluded, this is not an open-source license in the OSI sense. For a commercial
license, open an issue on the repository.

The embedded fonts keep their own licenses (SIL Open Font License; see
`crates/redox-server/assets/fonts/`).
