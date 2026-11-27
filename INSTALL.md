# RedoX VE — installing on a server

This is the English version of [KURULUM.md](KURULUM.md). For an overview, screenshots and the technical
design, see [README.md](README.md).

RedoX VE is an independent virtualization platform written from scratch in Rust and modeled on Proxmox VE.

The guide targets Debian 12 or Ubuntu 22.04 and later (systemd, x86_64). Replace `redox.example.com` and
`user@server` with your own values.

> **There are two drivers.** The default `sim` driver runs nothing; it shows a sample cluster. The `qemu`
> driver runs real KVM virtual machines on this server (section 8). The same driver also runs containers
> when LXC is installed (section 9).

> **Language.** The web panel and the CLI help text are in Turkish. Page and button names in this guide are
> given in Turkish, as they appear on screen, with a translation the first time.

## Installing like Proxmox (installer ISO and one command)

RedoX VE does not run inside a container or another panel; it runs directly on the server's operating system as a
systemd service. There are two ways to install it:

### A. Installer ISO (for an empty machine)

`deploy/iso/build-iso.sh` builds a RedoX VE installer ISO from Debian's network-install image. Write the ISO to a USB
stick and boot it. The installer asks only for language and keyboard, the host name, the root password and the disk
to install to (it asks for confirmation before erasing it); everything else is automatic: a minimal Debian, SSH,
QEMU/KVM, LXC and RedoX VE. The first network card is attached to the `vmbr0` bridge, so guests get addresses
straight from your network.

```
# On Linux, once:
rustup target add x86_64-unknown-linux-musl && sudo apt install musl-tools xorriso cpio
cargo build --release --target x86_64-unknown-linux-musl
sh deploy/iso/build-iso.sh --debian-iso debian-13.x.y-amd64-netinst.iso \
    --binary target/x86_64-unknown-linux-musl/release/redox --out redox-ve.iso
sudo dd if=redox-ve.iso of=/dev/sdX bs=4M status=progress conv=fsync      # sdX: the USB stick
```

When the server boots for the first time the console shows the panel's address (`https://<address>:8006`,
self-signed certificate). The first administrator `admin` is created with a random password, stored only in
`/root/redox-ilk-parola.txt`, readable by root. Change the password after the first login and delete the file. The
machine needs internet access during installation (packages come from the Debian repository).

### B. One command (on an installed Debian or Ubuntu)

```
sudo sh deploy/install.sh --binary target/release/redox --bridge eth0 --lang en
```

It does the same setup and asks you for the administrator password. Without `--bridge` no bridge is created (guests
use NAT); the bridge is only set up on systems that use `/etc/network/interfaces` and comes up after a reboot.
`--no-tls` leaves the panel on `127.0.0.1:8006` only. `--dry-run` prints what would be done without changing anything.

### What was tested and what was not

**Tested (Debian 13.8, in a virtual machine with KVM):** building the ISO, booting it with BIOS and installing
without questions (about 7 minutes), package installation, creation of the administrator on first boot, the console
banner, the `vmbr0` bridge getting an address, the panel answering on `https://<address>:8006` with a self-signed
certificate. The whole life cycle of an Alpine container was also tried with real LXC on the same installation:
create, start, getting an address, running a command, the memory limit, backup while running, restart, stop,
snapshot and rollback, restore from backup, clone (with its own name and address) and delete.

Also tried: the BIOS and UEFI boot menus (the first entry is *Install RedoX VE*; after Enter the installer's
language screen appears), starting bridged (`vmbr0`) and NAT virtual machines with nested KVM on the installed
system, and a bridged Debian 13 container.

**Not tested yet:** installation on a physical machine, booting from a USB stick, the installer's interactive flow
beyond the language screen (the unattended variant was installed end to end), a complete installation under UEFI
(only the menu and first screen), and `install.sh` on Ubuntu. No operating system was installed inside the virtual
machines; the Ubuntu and Rocky Linux container images were not tried. Containers
need systemd 254 or later (Debian 13, Ubuntu 24.04).

## Security model (summary)

| Layer | What it does |
|---|---|
| Identity | User name and password (Argon2id). There is no default account or password; the server does not start without a user. |
| Session | 256-bit random cookie; `HttpOnly`, `SameSite=Strict`, `Secure` behind TLS. Ends after 2 hours idle and after 12 hours at the latest. Dropped when the password changes or the user is removed. |
| Brute force | 5 failed attempts in 15 minutes for the same address and user lock for 15 minutes. |
| Second factor | Optional TOTP (authenticator app), turned on from the account page; a code is accepted once. |
| Roles | `admin` does everything; `operator` runs the guests in its scope; `viewer` only reads (and cannot see the console). |
| Audit | Logins, refused attempts and every state-changing request are written to `audit.log` and the audit page. |
| API | `Authorization: Bearer <token>`. Tokens are stored only as SHA-256 digests. |
| Network | The server does not terminate TLS and refuses to listen in plain HTTP outside 127.0.0.1. Caddy exposes it with TLS. |
| Browser | JavaScript only on three pages: the live console, the container terminal and the security key step; every other page has `script-src 'none'`. On those pages only the server's own script runs (`script-src 'self'`, no inline script, no external source). Cross-site POSTs and cross-site console connections are refused, pages cannot be framed, HSTS. Fonts are embedded; a page makes no external request. |
| Process | Unprivileged `redox` user, systemd sandbox, files `0600`. |

What is out of scope is listed at the end: **Known gaps**.

## 1. Copy the source to the server

From Windows PowerShell, archive without the build output (`target`) and copy:

```powershell
tar --exclude=target --exclude=.claude -czf redox-ve.tar.gz -C D:\Projelerim-Claude Proxmox-Rust
```

```powershell
scp redox-ve.tar.gz user@server:~
```

Or clone the repository on the server: `git clone https://github.com/ByReDoxXx/redox-ve.git Proxmox-Rust`.

## 2. Build on the server

```bash
sudo apt update && sudo apt install -y build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"

tar -xzf redox-ve.tar.gz && cd Proxmox-Rust
cargo test --workspace          # all must pass (713 tests on Linux; with Node installed the browser scripts' tests run too)
cargo build --release
sudo install -m 0755 target/release/redox /usr/local/bin/redox
```

## 3. Service user and first administrator

```bash
sudo useradd --system --home-dir /var/lib/redox --shell /usr/sbin/nologin redox
sudo install -d -o redox -g redox -m 0700 /var/lib/redox

# The password is asked twice and not echoed (at least 12 characters).
sudo -u redox redox user add admin --users /var/lib/redox/users.json
```

A read-only account: `sudo -u redox redox user add watcher --role viewer --users /var/lib/redox/users.json`

## 4. systemd service

```bash
sudo cp deploy/redox.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload
sudo systemctl enable --now redox
systemctl status redox --no-pager
curl -s http://127.0.0.1:8006/healthz      # prints "ok"
```

Logs, including logins and failed attempts: `journalctl -u redox -f`

## 5. TLS: expose it with Caddy

Install Caddy from its official repository: <https://caddyserver.com/docs/install#debian-ubuntu-raspbian>

```bash
sudo cp deploy/Caddyfile /etc/caddy/Caddyfile
sudo nano /etc/caddy/Caddyfile             # write your domain name
sudo systemctl reload caddy
```

Firewall: leave only SSH, 80 and 443 open; **do not open 8006**.

```bash
sudo ufw allow OpenSSH
sudo ufw allow 80,443/tcp
sudo ufw enable
```

Open `https://redox.example.com` in a browser and sign in as `admin`.

### Without a domain name: SSH tunnel

Skip Caddy, delete the `--behind-tls-proxy` line in `deploy/redox.service`, and open a tunnel from your own
computer; then use `http://127.0.0.1:8006`:

```powershell
ssh -L 8006:127.0.0.1:8006 user@server
```

The server then listens on 127.0.0.1 only and SSH encrypts the traffic.

## 6. API token for the command line

```bash
sudo -u redox redox token create admin laptop --users /var/lib/redox/users.json
```

The token is shown once. On your own computer:

```powershell
$env:REDOX_URL = "https://redox.example.com"; $env:REDOX_TOKEN = "redox_..."; redox cluster status
```

To revoke: `sudo -u redox redox token revoke admin laptop --users /var/lib/redox/users.json`

## 7. Maintenance

- **Change a password:** `sudo -u redox redox user passwd admin --users /var/lib/redox/users.json`.
  No restart is needed; the user's open sessions end at once.
- **Back up:** `/var/lib/redox/users.json` and `/var/lib/redox/state.json`.
- **Update:** copy the new source, `cargo build --release`, `sudo install ...`, `sudo systemctl restart redox`.
- **Locked login:** wait 15 minutes or `sudo systemctl restart redox` (locks are kept in memory).

## 8. Real virtual machines (QEMU/KVM)

Everything so far runs the simulation driver. For real virtual machines:

```bash
sudo apt install -y qemu-system-x86 qemu-utils
ls -l /dev/kvm                                  # if missing, enable virtualization in the firmware
sudo cp deploy/redox-qemu.service /etc/systemd/system/redox.service
sudo systemctl daemon-reload && sudo systemctl restart redox
journalctl -u redox -n 20 --no-pager            # shows the qemu-kvm driver
```

- **Data directory** `/var/lib/redox`: disk images in `images/`, installation media in `iso/`, cloud images in
  `cloud/`, backups in `backups/`, runtime files in `run/`, guest records in `state.json`, metric history in
  `metrics.json`.
- **Adding an ISO:** put the file in `iso/`; it then appears in the create form.
  `sudo install -o redox -g redox -m 0600 debian-12.iso /var/lib/redox/iso/`
- **Stop** first asks the guest to shut down (ACPI); after 60 seconds it forces it off
  (`--shutdown-timeout-secs`). A guest with no operating system waits out that time.
- **Guests keep running when the service restarts** and are adopted again.
- **Without `/dev/kvm`** the server warns and falls back to software emulation (TCG), which is very slow.

What the driver offers, from the tabs of a guest page:

### Console

- **Console tab (Konsol):** the guest's screen as an image refreshed every 2 seconds; text and special keys
  (Enter, Esc, arrows, F2/F10/F12, Ctrl+Alt+Del) can be sent. No mouse; text goes with a US layout. This tab
  works without JavaScript.
- **Live console (Canlı Konsolu Aç):** `/guests/<id>/console` shows the screen in real time; click it and the
  keyboard and mouse go to the guest. Keys are sent by physical position, so the guest's own keyboard layout
  decides the character. This page uses JavaScript. QEMU offers the display only on a Unix
  socket in the data directory (`run/<id>.vnc`, no password, never on the network); the server bridges the
  browser's WebSocket to it after checking the session and the permission. Admins, and operators for guests in
  their scope, can open it; each session is written to the audit log as `console <id>` and lasts at most 12
  hours. Caddy passes WebSockets by itself; with another proxy, forward the `Upgrade` headers. Guests started
  before this feature have no live console until they are stopped and started once.

### Power and scheduling

- **Automatic restart:** with *Kendiliğinden kapanırsa yeniden başlat* ticked in the options tab, a guest that
  crashes or shuts itself down is started again in about 15 seconds (`redox vm set 103 restart=true`). Guests
  stopped from the panel, suspended, or busy with a task are left alone. A guest that keeps crashing is tried at
  most 3 times in 10 minutes, then given up, with an error from the `gözcü` user in the task log. This is a
  single-server measure: if the server itself fails, guests are not started elsewhere.
- **Start at boot, with an order:** guests with *Düğüm açıldığında başlat* ticked are started when the server
  boots. Order 1 starts first; 0 is unordered and comes after the ordered ones. A delay in seconds can be put
  before the next guest. `redox vm set 103 onboot=true boot-order=1 boot-delay=30`. A service restart is not a
  boot: the server keeps the kernel's boot ID in `/var/lib/redox/boot-id`. `--no-autostart` turns it off.
- **Scheduled power actions:** in the options tab, start, stop or reboot a guest on given days and at a given
  time (at most 8 per guest). `redox vm set 103 schedules=start@08:00@1-5,stop@20:00` (days 1 Monday … 7 Sunday;
  every day if omitted). An action is skipped if the guest is already in that state. If the server is down at
  that time, the action runs if it comes back within 15 minutes; otherwise it is skipped for the day.
- **Suspend (Askıya Al):** `redox vm suspend 103` writes the guest's memory and device state to disk
  (`images/vm-<id>.state`) and stops it; the server can be rebooted. **Start** resumes it where it was. While
  suspended, disks and hardware cannot be changed. To give up, **Askı Durumunu At**
  (`redox vm discard-suspend 103`). The state file is as large as the memory the guest used; after a QEMU
  upgrade an old state may not resume.

### Hardware

- **UEFI, secure boot, TPM (for Windows 11):** `sudo apt install ovmf swtpm`, then restart the service. With
  secure boot the Microsoft keys are enrolled. Other locations: `--ovmf-dir DIR`, `--swtpm-bin PATH`.
- **CPU type (İşlemci türü):** `redox vm set 103 cpu=Haswell`; `cpu=host` returns to the default, which is the
  server's own CPU and the fastest. If you will move a running guest to a server with a different CPU, choose a
  model both support. Changed while the guest is stopped. A model from the other vendor (`EPYC` on an Intel
  server) is refused. A **newer** generation from the same vendor is not checked: QEMU may drop missing features
  with a warning or the guest may not start.
- **Memory balloon (Bellek balonu):** `redox vm set 103 balloon=true` while stopped. Then **Bellek hedefi** or
  `redox vm balloon 103 4096` shrinks and restores the guest's memory **while it runs** (between 256 MB and the
  guest's memory; 0 empties the balloon). The target is saved and applied again at every start. The guest needs
  the virtio balloon driver (built into Linux; virtio-win on Windows). On real QEMU the device and the command
  were verified; an actual reduction was not, as the test guest had no operating system.
- **Extra disks and network cards:** up to 8 data disks and 3 extra cards per VM. Both can be **hot-plugged**
  into a running VM (`redox vm disk add 103 20`, `redox vm nic add 103 vmbr1`). Removing, growing and other
  hardware changes need the guest stopped. Guests started before this feature must be stopped and started once.
- **Rate limits (Hız sınırları):** disk MB/s and operations/s, network Mbit/s; 0 is unlimited.
  `redox vm set 103 disk-mbps=50 disk-iops=2000 net-mbps=100`. The disk limit is QEMU's own throttling, per disk.
  The network limit is applied with `tc` to the card's TAP interface: it needs `iproute2` and
  `AmbientCapabilities=CAP_NET_ADMIN` in the unit (`redox-host.service` only) and does not apply to NAT cards.
  If a limit cannot be applied the guest is not left running unlimited: the start fails.
- **VLAN tag:** for the first card a tag field (1-4094; 0 untagged); for extra cards write `bridge:tag`
  (`vmbr1:20`). `redox vm set 103 vlan=20 nics=vmbr1:30`. The server creates a TAP interface named
  `rx<id>n<card>`, attaches it to the bridge and makes it an untagged member of that VLAN only. Needs
  `CAP_NET_ADMIN`, `iproute2`, VLAN filtering on the bridge (`ip link set vmbr0 type bridge vlan_filtering 1`)
  and the VLAN tagged on the uplink port (`bridge vlan add dev eth0 vid 20`). Bridge names are case-sensitive.
- **Shared folders (Paylaşımlar):** names such as `ortak, veri:ro` (`:ro` read-only; at most 4), or
  `redox vm set 103 shares=ortak,veri:ro`. Each name is the directory `/var/lib/redox/shares/<name>` on the
  server. In the guest: `mount -t 9p -o trans=virtio,version=9p2000.L ortak /mnt/ortak`. While a share is mounted
  the guest cannot be suspended, snapshotted while running, or moved while running. Windows has no 9p driver.
- **Device passthrough:** write the `lsusb` ID (`046d:c52b`) or the `lspci -D` address (`0000:01:00.0`). USB needs
  a udev rule for the service user; PCI needs IOMMU and `vfio-pci`. **Not tried with real hardware.**
- **Port forwarding:** `host-port:guest-port` (TCP), for NAT guests. Open the port in the firewall separately.

### Images and cloud-init

- **VM from a cloud image:** put the distribution's cloud image (for example
  `debian-12-genericcloud-amd64.qcow2`) in `/var/lib/redox/cloud/`, choose it as the disk image, and give a user
  name and your SSH public key. The disk is copied, grown to the size you ask for, and configured on first boot.
  Password login is off.
- **Editing cloud-init later:** in the hardware tab of a stopped VM, change the user, SSH key, static address
  (`10.0.0.5/24`), gateway and name servers (at most 3), or
  `redox vm set 103 ci-user=ali ci-key='ssh-ed25519 AAAA…' ci-ip=10.0.0.5/24 ci-gw=10.0.0.1 ci-dns=1.1.1.1,9.9.9.9`.
  An empty key (`ci-key=`) turns cloud-init off; an empty address (`ci-ip=`) means DHCP. Every change also changes
  the guest's cloud-init instance ID, so the guest **applies the settings again** at its next start: the new user
  and key are added, SSH host keys may be regenerated, the old user is not removed. On real QEMU the files and
  the drive were verified; the effect **inside a guest** was not verified with a real cloud image.
- **Importing from other hypervisors:** `vmdk`, `vdi`, `vhdx` disks in the same folder are offered as images and
  converted to qcow2. An `.ova` is converted directly; with several disks only the first is taken.
- **Uploading media:** from the storage page, upload an `.iso` or a disk image from your computer (streamed to
  disk, not held in memory), or let the server download it from an `https://` address (public addresses only,
  redirects checked, optional SHA-256). Command line:
  `curl -T debian.iso -H "Authorization: Bearer $REDOX_TOKEN" $REDOX_URL/api/v1/media/debian.iso`,
  `redox media list`, `redox media fetch <url> [sha256]`, `redox media delete <file>`. The limit is 64 GB a file.
- **Export:** *OVA Olarak İndir* in the clone tab of a stopped VM, or
  `curl -H "Authorization: Bearer $REDOX_TOKEN" -o vm.ova $REDOX_URL/api/v1/guests/103/export`.

### Storage, pools and clones

- **Additional storage areas:** add `--storage NAME=/absolute/dir` to the service line (repeatable) to keep VM
  disks on a second disk or a mounted share. The directory must belong to the `redox` user. Backups, media and
  containers always stay in the data directory (`local`). The server refuses to start if a storage that still
  holds guests is removed from the service line.
- **Moving a guest to another storage:** in the clone tab of a VM, or `redox vm move 103 fast`. A running VM is
  moved without stopping it (its disks are mirrored to the new storage, then switched over); if it has snapshots it
  must be stopped first. For a stopped guest all disks
  are copied to the target first, put in place when every copy is complete, and only then removed from the old
  location: an interrupted move leaves the guest where it was. Snapshots travel with the disks. Suspended guests,
  linked clones and templates with linked clones cannot be moved. Containers cannot be moved.
- **Resource pools and quotas:** on the pools page (*Havuzlar*) or with
  `redox pool add web cores=32 memory=65536 disk=500 guests=10`, limit the total cores, memory (MB), disk (GB) and
  guest count of a group of guests; anything omitted is unlimited (at most 32 pools). Put a guest in a pool from
  its options tab or with `redox vm set 103 pool=web` (`pool=` removes it).
  - The quota applies to **allocated** resources, not current usage; stopped guests count.
  - When a quota is full, no guest can join the pool, a guest in it cannot get more cores, memory or disk, and
    cannot be cloned (a clone inherits the pool).
  - A quota cannot be lowered below current use; a pool with guests cannot be deleted.
  - Guests are created without a pool. Since only administrators create and change guests, quotas are a guard
    rail on allocation, not a permission boundary.
- **Templates and linked clones:** *Şablona Çevir* (`redox vm template 103 on`). A template cannot be started and
  its disk does not change. A linked clone (`redox vm clone 103 web-2 --linked`) is ready in under a second and
  stores only its difference from the template. A template with linked clones cannot be deleted.

### Backups

- **Snapshots and backups** are taken **while the VM runs** (a snapshot includes memory). Backups are qcow2 files
  under `/var/lib/redox/backups/`, or chunks if incremental backups are on.
- **Scheduled backup per guest:** a time (`03:30`) and a number of backups to keep, or
  `redox vm set 103 backup-time=03:30 backup-keep=7`. The oldest **automatic** backups beyond that number are
  deleted; manual ones are not touched.
- **Backup jobs (Yedek işleri):** `redox job add nightly time=03:00 days=1-5 tags=web,db keep=7` (at most 16).
  Without `tags` all guests; without `days` every day. Guests are backed up **one at a time**, across all jobs.
  The retention count belongs to the job and applies only to the backups that job took. `redox job run nightly`
  starts it now; `redox job list`, `redox job remove nightly`. The scheduler looks every 30 seconds.
- **Incremental backup:** add `--incremental-backups`. The disk is cut into 4 MiB chunks named by their SHA-256
  and stored once in `backups/chunks/`. There is **no chain**: every backup lists all of its chunks, so any one
  can be deleted. Each chunk is verified on restore; a single mismatch refuses the restore and leaves the disk
  as it is. **Doğrula** or `redox backup verify <backup>` checks on demand. It saves space, not time: the whole
  disk is read every time, unless the next option is on.
- **Changed-block tracking:** add `--changed-block-tracking`. QEMU keeps a persistent map of written blocks, and
  the next backup of a **running** guest reads only those. Whenever the map might be wrong (restore, rollback,
  resize, disk change, unclean stop) the next backup reads the whole disk. At most 6 backups in a row use the map.
- **Extra disks** are part of every backup; on a running guest the copies start in one QEMU transaction.
  Restore brings back the disks that are still attached, and puts nothing in place unless every disk is ready.
- **UEFI and TPM state:** the UEFI variable store and the virtual TPM state are written next to the backup
  (`backups/<backup>.machine.tar`), follow it off-site, and are restored with it. A part that no longer matches
  the guest's hardware is skipped. On a running guest the variable store is copied while QEMU uses it; it is
  rarely written, but it is not frozen at one instant with the disks.
- **Restore as a new guest:** *Yeni konuk olarak aç* in the backups tab, *Yeni Konuk* on the backups page, or
  `redox backup restore-new <backup> <name>` opens a backup as a separate VM instead of overwriting the original.
  The guest's definition is saved with the backup, so it works even after the original guest is deleted. The copy
  does not start by itself and does not inherit schedules or the server's USB/PCI devices. Backups taken before this
  feature use the current guest's settings, and cannot be opened if that guest is gone.
- **Instant recovery:** *Anında aç* or `redox backup instant <backup> <name>` prepares the guest's disks as thin
  layers on top of the backup (no copying, under a second), so the guest can start at once. The guest stores only its
  changes and reads the rest from the backup, so the backup cannot be deleted or pruned until the guest is detached:
  *Yedekten Ayır* in the clone tab, or `redox vm detach <id>`, copies the data into the disks, also while the guest
  runs. Works with full, locally present backups only.
- **Boot test:** *Açarak sına* or `redox backup test <backup>` creates a temporary, network-isolated guest from the
  backup, starts it, checks that it stays running for a while (60 seconds by default; `--backup-test-secs`) and deletes
  it. The result is written to the backup and the task log. With a guest agent the operating system's answer is
  verified too; without one the result says the boot could not be confirmed. Needs free space for the restored disks.
- **File-level restore:** with `sudo apt install libguestfs-tools`, *Dosyalara göz at* browses the folders inside a
  backup and downloads single files or a folder as tar; `redox backup ls <backup> /etc` on the command line. Admins
  only, and every access goes to the audit log. Each listing takes a few seconds. On Ubuntu the kernel image must be
  readable: `sudo chmod 0644 /boot/vmlinuz-*`. **Not tried with real libguestfs** (tested with a fake tool).
- **Weekly, monthly, yearly retention (GFS):** on top of "keep the last N", the newest automatic backup of the last N
  weeks, months and years that have backups is kept. `redox vm set 103 backup-weekly=4 backup-monthly=12
  backup-yearly=2`, or `weekly= monthly= yearly=` on a job. Weeks start on Monday; dates are computed in UTC.
- **Immutable backups (lock):** *Kilitle* (30 days) or `redox backup lock <backup> <days>`: the backup cannot be deleted
  or pruned until that date. A lock can only be extended. `lock=14` on a job locks every backup it takes for 14 days.
  This protects against deletion through the panel and the API; someone with root on the server can still remove the
  file. For true immutability set a default Object Lock retention on your S3 bucket.
- **Several off-site targets (3-2-1):** add `--offsite-copy FILE` (repeatable; same file format as `--offsite`). Every
  backup that reaches the main target is copied to these too, each encrypted with its own passphrase. Restore tries
  the main target first, then the copies. Deleting removes the backup from all of them.
- **Commands in the guest before and after a backup:** `redox vm set 103 backup-pre='/usr/local/bin/pre.sh'
  backup-post='/usr/local/bin/post.sh'`. They run through the guest agent just before a live backup and as soon as the
  disk image is taken. An absolute program path and arguments, no shell. If the pre command fails, or there is no
  agent, the backup is not taken; if the post command fails the backup is discarded and the task says so.
- **Backup window, verification, speed limit:** `until=06:00` on a job stops starting new guests after that time (a
  running backup is not interrupted; a manual run ignores the window). `verify=true` verifies every backup the job
  takes. `--backup-mbps N` on the service line limits the copy speed to N MB/s per disk.
- **Backup report and unprotected guests:** the backups page lists guests with no backup in 7 days
  (`--backup-age-days`; 0 turns it off). `redox backup report` prints the last 24 hours; `--backup-report 07:30` mails
  it every day (needs `--alert-email`).
- **Production checkpoint:** tick **Üretim (belleksiz)** on the snapshots tab, or
  `redox snapshot create 103 before --production`. Only the disks of the running guest are captured, in one
  transaction and with file systems frozen through the guest agent when it is present; memory is not saved. It is
  small, fast and consistent for applications such as databases. Rolling back does not resume the guest at that
  moment, it boots it from that disk state, so the guest must be stopped first.
- **Dynamic memory:** on a guest with the balloon device, `redox vm balloon 103 auto 2048` (or the hardware tab).
  Every 15 seconds the target is set to the memory the guest uses plus a 20% buffer, between your minimum and the
  full amount; it grows at once and shrinks only when the difference is clear. The guest needs the virtio balloon
  driver; without measurements (or without the "available memory" figure) the target is left alone. `auto 0` turns
  it off. **Verified on real QEMU that statistics polling is enabled; not tried with a real guest that has the
  balloon driver.**
- **Resource metering:** the card on the guest's summary tab, `redox usage`, `redox vm usage 103`, and
  `GET /api/v1/usage`. Uptime, CPU (core-hours), memory (GB-hours) and disk read/write accumulate across restarts
  of the guest. `redox vm usage 103 reset` starts a new billing period. Counters are written to disk every five
  minutes. Network traffic is not counted.
- **NIC protection:** `redox vm set 103 nic-guard=true` (or the options tab). The guest can only talk with its own
  MAC address and cannot send DHCP replies or IPv6 router advertisements. Works on bridged cards and needs
  `--firewall` (section 12); rules follow the guest's host interfaces and are refreshed within 15 seconds.
  **The rule generation is tested; not tried under traffic on a real bridge.**
- **Nested virtualization:** `redox vm set 103 nested=true` while the guest is stopped. The guest can run its own
  virtual machines. The host must allow it: `cat /sys/module/kvm_intel/parameters/nested` (`kvm_amd` on AMD) must
  print `Y` or `1`; otherwise put `options kvm_intel nested=1` in `/etc/modprobe.d/kvm.conf` and reboot. Not
  available with the `qemu64` and `kvm64` models.
- **Backup age target (RPO):** `redox vm set 103 rpo=24` (hours). A guest whose last backup is older counts as
  unprotected regardless of the general `--backup-age-days` setting.
- **Unusual change warning:** when an incremental backup contains at least 4 times the median change of the
  guest's earlier backups (and at least 512 MB and 25% of the disk), the backup is flagged, a failed task entry is
  written (which reaches the alert channels) and the daily report lists it. Mass encryption by ransomware leaves
  this trace; so does a large migration. It is a signal to look at, not proof. Needs `--incremental-backups` and
  three earlier backups.
- **Service checks in the boot test:** `redox vm set 103 test-port=22 test-cmd=/usr/local/bin/health.sh`. While a
  backup is boot-tested, the service on that port of the temporary guest must **send data** (the port is forwarded
  only to 127.0.0.1 on the server) and the command is run through the guest agent and must exit with 0. Use the
  command for services that do not send data. **Port forwarding verified on real QEMU; not tried with a real
  guest that has an operating system.**
- **Four-eyes principle:** add `--four-eyes` to the service line. Deleting a backup or a guest then waits for
  **another** administrator (`redox approval list | approve <n> | reject <n>`, or the notice at the top of every
  page). Removing an extra disk, a backup job or a user follows the same rule. The requester cannot approve their own request; requests expire after 24 hours. Needs at least two
  administrator accounts. Retention pruning by the scheduler is exempt.
- **Replication (standby copy):** the **Çoğaltma** card on the protection page (*Koruma*), or `redox replica add <peer> <id> 60`.
  A copy of a peer's VM is refreshed on this server at the interval you give (15 minutes to 1 week); the source
  keeps running and is not suspended. The copy is a stopped guest named `<name>-kopya`; if the source server dies
  you start it here (you lose what was written since the last run). A running copy is never overwritten. **Every
  run copies the whole disk** (not incremental) and needs room for two copies while it runs. Failover is manual.
- **Maintenance mode:** the **Bakım** card on the node page, or `redox maintenance on --drain`. Running guests are
  stopped in reverse boot order; while it is on no guest is started and scheduled backups, power actions,
  replication and automatic restart are paused. The mode is stored in the state file, so guests stay down across a
  server reboot. `redox maintenance off` starts the stopped guests again in boot order. It does not move guests to
  another server.
- **Scheduled boot tests:** add `--backup-test-every 7@03:00` to the service line. The newest backup of every VM is
  boot-tested every 7 days, one at a time, in a four-hour window from 03:00.
- **Disk usage inside guests:** with `qemu-guest-agent` in the guest, the summary tab and `redox vm df 103` show it;
  a disk over 80% raises a warning, over 90% a critical alert. **Tested with a fake agent only.**
- **Network metering:** resource metering also counts traffic of bridged cards. Cards on NAT are not counted.
  **Counter reading is tested; not tried on a real bridge.**
- **Ready cloud images:** the list on the storage page, or `redox media catalog` / `redox media get debian-12`
  (Debian 12/13, Ubuntu 22.04/24.04, AlmaLinux 9, Rocky 9). They are rolling images, so integrity rests on https.
- **Dismissing an unusual-change warning:** **Uyarıyı kapat** in the backup's menu, or `redox backup ack <backup>`.
- **Guest agent:** install `qemu-guest-agent` in the guest. The summary tab then shows the operating system and
  the IP address, and file systems are frozen just before a live backup (`donduruldu` in the list).

What it does not do yet:

- **The default network is user-mode NAT.** For bridges and containers see section 9.
- **No real cluster:** no shared cluster state and (except for replicated guests) no automatic failover. A running VM can be migrated live between servers; see section 10.
- **The guest firewall is off by default;** see section 12.

## 9. Bridged networking and LXC containers

> **This section was not tried on a real server.** The code is tested at process level with fake tools; the
> bridge helper and unprivileged LXC need root to set up. Bridge setup can cut your connection: do it from the
> server's console, not over SSH.

### 9.1 Bridge (guests get addresses from the server's network)

1. Create a bridge. A netplan example (use your own interface instead of `eth0`):

   ```yaml
   # /etc/netplan/60-vmbr0.yaml
   network:
     version: 2
     ethernets:
       eth0: { dhcp4: false }
     bridges:
       vmbr0:
         interfaces: [eth0]
         dhcp4: true
   ```

   ```bash
   sudo netplan try        # reverts unless confirmed within 120 seconds
   ```

2. Allow QEMU's bridge helper:

   ```bash
   echo "allow vmbr0" | sudo tee -a /etc/qemu/bridge.conf
   sudo dpkg-statoverride --update --add root redox 4750 /usr/lib/qemu/qemu-bridge-helper
   ```

   The helper runs with root rights; this lets only the `redox` group use it, so other accounts on the server
   cannot attach themselves to guest networks. (Running the server under another account? Use its group.)

3. Use the unit with bridge and container support (its sandbox is looser; the reasons are in the file):

   ```bash
   sudo cp deploy/redox-host.service /etc/systemd/system/redox.service
   sudo systemctl daemon-reload && sudo systemctl restart redox
   ```

The network box in the forms then offers the bridge. Each guest's MAC address is derived from its ID
(`52:54:00:…`) and does not change. Port forwarding is not used on a bridge.

### 9.2 LXC containers (unprivileged)

```bash
sudo apt install -y lxc uidmap acl
# Give the service user an ID range for containers.
sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 redox
# Allow containers to attach virtual cards to the bridges (at most 20).
printf 'redox veth lxcbr0 20
redox veth vmbr0 20
' | sudo tee -a /etc/lxc/lxc-usernet
# Root inside the container (ID 100000 on the server) must be able to pass through the data directory.
sudo setfacl -m u:100000:x /var/lib/redox
sudo systemctl restart redox
journalctl -u redox -n 20 --no-pager      # shows qemu-kvm+lxc
```

- **Templates:** alpine-3.24, debian-13, debian-12, ubuntu-26.04, ubuntu-24.04, rockylinux-10, rockylinux-9, downloaded from
  `images.linuxcontainers.org` on first use.
- **Network:** NAT attaches the container to LXC's own bridge (`lxcbr0`); a bridge attaches it directly.
- **Console tab** runs one command in the container (as root, at most 20 seconds). No interactive programs.
- **Limits:** memory and cores are enforced; **disk size is not**. No disk resize, templates or linked clones.
- **Clone:** from the clone tab of a stopped container or `redox ct clone 202 new-name` (`lxc-copy`). The copy
  gets its own ID, name and network address; snapshots are not copied.
- **Snapshots** need the container stopped. **Backups also work while it runs:** the container is frozen for the
  duration of the archive and resumes afterwards. A backup is a `tar.gz` of the root file system; restore, with
  the container stopped, empties the root file system and unpacks the archive.
- `--no-lxc` turns containers off.

## 10. Alerts and several servers

### 10.1 Alerts

Every 15 seconds the server checks storage and node fullness, offline nodes, failed tasks and guests that
stopped unexpectedly, plus node memory (90% warning, 97% critical), node CPU (90%) and a running guest's CPU
(95%). Active alerts show at the top of the overview. Load alerts are sent only after 5 minutes without a break.
For notifications add a webhook (Slack, Discord, ntfy, anything that reads a `text`/`content` field):

```
ExecStart=/usr/local/bin/redox serve ... --alert-webhook https://hooks.example.com/…
```

**E-mail:** write a file and add `--alert-email /var/lib/redox/mail.conf`:

```
host = smtp.example.com
port = 465
user = redox@example.com
password = ...
from = redox@example.com
to = me@example.com, oncall@example.com
```

On port 465 the connection is TLS from the start; on 587 it starts plain and is encrypted with STARTTLS. To
choose regardless of the port, write `mode = tls` or `mode = starttls`. The user name, password and message are
never sent before encryption is established. A relay on the same machine (`127.0.0.1`) is used unencrypted.
Tested against a fake SMTP server only, not with a real provider.

**Graphs:** node and guest pages offer the last 10 minutes, 24 hours and 30 days; history is kept in
`metrics.json`.

### Interface language

The interface is available in Turkish and English. Each user chooses with the **EN / TR** button in the top bar (stored
in a browser cookie). With `--lang en` on the service line, visitors without the cookie see English. API error messages
are English with `Accept-Language: en`; the `code` field does not depend on the language. The command line is English
with `REDOX_LANG=en`; `--lang en` also makes the server's startup messages, alert notifications and report e-mail English. See [docs/wiki/en/Language.md](docs/wiki/en/Language.md).

### More notification channels and API limits

- `--alert-ntfy https://ntfy.sh/your-topic` sends alerts to an ntfy topic as plain text.
- `--alert-telegram /etc/redox/telegram.conf` sends them through a Telegram bot. The file (mode 600) holds
  `token = <bot token>` and `chat = <chat id>`. The token is never logged. **Both were tested against a fake
  server only.** For Slack and Discord `--alert-webhook` is enough.
- `redox token create <user> <label> --read-only` creates a read-only token, whatever the owner's role.
- `--api-rate 600` limits the API to 600 requests per minute per user; beyond that requests get 429 with
  `Retry-After`. Pages are not limited. Default: no limit.

### 10.2 Combined view (several servers)

With several RedoX VE servers, make one the hub and see the others' nodes and guests on the servers page
(*Sunucular*). This is **not a real cluster**: the view is read-only and each server manages its own guests.

1. On every **peer**, create a read-only user and a token:

   ```bash
   sudo -u redox redox user add hub --role viewer --users /var/lib/redox/users.json
   sudo -u redox redox token create hub combined --users /var/lib/redox/users.json
   ```

2. On the **hub**, write the peer file (each line `NAME URL TOKEN`):

   ```bash
   sudo install -o redox -g redox -m 0600 /dev/null /var/lib/redox/peers.conf
   sudo -u redox tee /var/lib/redox/peers.conf >/dev/null <<'EOF'
   izmir   https://izmir.example.com    redox_...
   EOF
   ```

3. Add `--peers /var/lib/redox/peers.conf` to the service line and restart. Peers are read every 15 seconds.

The URL must be `https://`; plain `http://` is accepted only for peers on the same machine. `redox peer list`.

**Moving a VM:** *Buraya Al* on the servers page (*Sunucular*), or `redox peer import izmir 103`, copies a **stopped** VM from
a peer to the hub. The token in the peer file must be an **admin** token on the peer. The hub has the peer take
a backup, downloads it, creates the guest and deletes the temporary backup. The source stays in place. A bridge
that does not exist on the hub becomes NAT; USB/PCI passthrough is dropped. Extra disks come with their numbers
and contents, and so do the UEFI variable store and the virtual TPM state. A guest with a TPM is not moved if
its TPM state cannot be fetched. Containers cannot be moved.

**Moving a running guest (*Çalışırken Al*):** the hub suspends the guest on the peer, downloads its disks and
memory state and resumes it here **where it was**. For guests that qualify, **Migrate Live**
(`redox peer migrate <peer> <id>`) moves the guest without stopping it, with a pause of tens of milliseconds; the
source must reach the target's `--migrate-ports` (default 49152-49215). See *Multiple Servers* in the wiki for the
details and limits. The suspend-and-copy path is not zero-downtime: the guest does not respond while
disk and memory are copied. Both servers must run the same RedoX VE version and a similar CPU, and the guest
must have no passthrough device and no mounted share. If the move fails nothing is left on the hub and the
guest stays suspended on the peer. If it succeeds, **delete** the suspended copy on the peer; do not start it.

**There is no automatic failover between servers.** That needs fencing and shared or replicated storage; without
them the same disk could be opened in two places.

## 11. Copying backups off the server

By default backups are on the same disk as the guests. Define an off-site target:

```bash
sudo install -o redox -g redox -m 0600 /dev/null /var/lib/redox/offsite.conf
sudo -u redox tee /var/lib/redox/offsite.conf >/dev/null <<'EOF'
# A mounted directory (second disk, NFS, SMB):
kind = dir
path = /mnt/backup
# or an S3-compatible store (AWS, MinIO, Backblaze B2, Wasabi):
# kind = s3
# endpoint = https://s3.eu-central-1.amazonaws.com
# region = eu-central-1
# bucket = my-backups
# prefix = redox/
# access_key = ...
# secret_key = ...
# Optional: encrypt before sending (at least 12 characters). Without the passphrase the backups cannot be opened.
passphrase = a-long-passphrase
EOF
```

Add `--offsite /var/lib/redox/offsite.conf` to the service line and restart.

- Every new backup is copied at once; if the copy fails the local backup is still valid and the error goes to
  the task log. Later: `redox backup push <backup>`.
- If the local file is lost, restore downloads the off-site copy. Deleting a backup deletes its copy.
- Encryption is on the client (ChaCha20-Poly1305, key from the passphrase with Argon2id): the target sees only
  ciphertext, and a changed or truncated file is refused.
- An encrypted copy is **bound to the backup's name**: if someone with write access to the target swaps two
  backups' files, restore refuses the file. Copies sent before this feature are not bound; they still restore,
  the backups page marks them (*ada bağlı değil*), and pushing the backup again binds them.
- **Incremental backups go off-site incrementally:** chunks are stored once under `chunks/`, and only missing
  chunks are sent. Do not change the passphrase for an existing target, do not edit `chunks/` by hand, and do not
  give the same target to two servers.
- On S3, files over 64 MiB are sent as a multipart upload, so there is no 5 GB limit. If a part fails the upload
  is aborted; it is still wise to enable your provider's rule that removes incomplete multipart uploads.
- The S3 target was tested against a local fake server only. The request signing, including requests with a
  query string, matches three examples from the AWS documentation.

### A separate retention on a copy target (archive)

Put `keep_days = 90` in the file of a copy target (`--offsite-copy`) and that target becomes an archive: a backup
deleted locally (by hand or by retention) stays there for 90 days from the moment it was taken, while it is removed
from the server and from the primary off-site target at once. Such backups are listed in the **Arşiv** card on the
backups page and by `redox backup archive`. `redox backup recover <backup>` puts the record back on the list; it
can then be restored or opened as a new guest like any other backup (the files are fetched from the copy target at
that point). Expired backups are removed from the copy target automatically. `keep_days` only applies to copy
targets.

## 12. Guest firewall

```bash
sudo apt install -y nftables
```

Add `AmbientCapabilities=CAP_NET_ADMIN` to the unit (`redox-host.service` only), add `--firewall` to the service
line and restart.

- In the options tab, tick *Gelen trafiği kısıtla* and write the allowed traffic, one rule a line:
  `tcp/22 from 10.0.0.0/8`, `tcp/80-443`, `udp/53`, `icmp`.
- Inbound connections that match no rule are dropped; connections the guest starts are free.
- Rules apply to bridged guests and all containers (nftables, `bridge` family, by MAC address). They have no
  effect on a NAT guest, and the page says so.
- **Not tried on a real server.** nftables accepts the generated rules, but the development kernel has no
  `bridge` family, so they were never loaded and tested with traffic.

## 13. Roles, single sign-on and sessions

| Role | Can do |
|---|---|
| Admin | Everything. |
| Operator | Start and stop the guests in its scope, use their console, **take** snapshots and backups. Cannot create, delete, change settings or restore. |
| Viewer | Only look. |

```bash
sudo -u redox redox user add oncall --role operator --users /var/lib/redox/users.json
sudo -u redox redox user scope oncall web,test --users /var/lib/redox/users.json   # only guests with these tags
```

An empty scope means all guests. Operators **see** all guests; the scope limits only what they can act on.

**Single sign-on (OpenID Connect):** create a client at your identity provider with the redirect address
`https://redox.example.com/login/oidc/callback`.

```bash
sudo install -o redox -g redox -m 0600 /dev/null /var/lib/redox/oidc.conf
sudo -u redox tee /var/lib/redox/oidc.conf >/dev/null <<'EOF'
issuer = https://id.example.com/realms/company
client_id = redox
client_secret = ...
redirect_url = https://redox.example.com/login/oidc/callback
# claim = email      (or preferred_username, sub)
EOF
sudo -u redox redox user sso ayse ayse@example.com --users /var/lib/redox/users.json
```

Add `--oidc /var/lib/redox/oidc.conf`. The provider only proves identity: the identity must be mapped to a local
user with `redox user sso`; role and scope are local. Tested with a fake provider only.

**Sessions** survive a service restart (`/var/lib/redox/sessions.json` holds digests, not tokens). To sign
everyone out, delete the file and restart.

## 14. Shared storage, disk encryption, security keys, terminal, sound and OCI images

### 14.1 Shared storage (moving without copying disks)

For setups where two or more servers mount the same directory (for example NFS).

```bash
redox shared init /mnt/shared          # once, inside the mounted share
redox serve ... --storage shared=/mnt/shared --shared-storage shared    # on every server
```

- Each guest's disks belong to one server (the ownership record is on the share); only the owner writes.
- If the peer sees the same share, live migration moves only memory. A stopped guest is taken without copying with
  `redox shared take <peer> <id>`; `redox shared list` shows the guests on the share.
- If the owning server has been quiet for at least 30 seconds, `redox shared adopt <storage> <id>` takes the guest
  over. A server that has lost ownership force-stops its copy as soon as it sees the share again.
- **There is no fencing.** Make sure the old server really is down before adopting: a running server that has lost
  the share looks quiet.
- Tested with two servers on one machine and real QEMU; a real NFS share and two physical machines were not tried.

### 14.2 Disk encryption (LUKS)

```bash
head -c 32 /dev/urandom | base64 > /etc/redox/disk.key && chmod 600 /etc/redox/disk.key
redox serve ... --disk-key-file /etc/redox/disk.key
```

Tick **Encrypt disks (LUKS)** when creating a VM (it cannot be switched later). If you lose the key, the disks and
their backups cannot be opened; keep the key apart from the disks. For an encrypted guest, operations that would
expose data or are untested are refused: cloning, suspend, memory snapshots while running (this QEMU version
crashes), disk mirroring, fault tolerance, export and import, instant recovery, disk-copying live migration and
moving storage while running. Backups stay encrypted but do not use the incremental store.

### 14.3 Security keys (WebAuthn)

```bash
redox serve ... --webauthn-origin https://redox.example.net
```

The address must be exactly the one the panel has in the browser; it cannot be an IP address. Each user adds keys on
the **My Account** page. For an account with a key, signing in asks for the key (or the app code, if that is on)
besides the password. On a server started without the option, accounts with keys cannot sign in. A lost key: the
two-step reset button on the Users page, or `redox user keys-reset <name> --users FILE`. Tested end to end with
Chrome's virtual authenticator; no physical key was tried.

### 14.4 Container terminal

**Open Terminal** on a running container's console tab opens an interactive root shell in the browser. The access
rules are those of the live console and are rechecked every 2 seconds while it is open. At most 8 terminals, 30
minutes idle limit. It needs `script` (util-linux) for the pseudo-terminal. Tested with a stand-in driver on a real
pseudo-terminal; **not tried with a real `lxc-attach`**.

### 14.5 Sound

**Add a sound card** in the VM settings (or `redox vm set <id> sound=true`) gives the guest an Intel HD Audio card;
the server needs no sound device. Listen in the live console with **Turn Sound On**. The microphone is not carried
and there is no SPICE. On real QEMU, sound played by the guest was measured on the console socket; playback through
the HDA card with a guest sound driver was not tried.

### 14.6 Containers from OCI (Docker) images

When creating a container, put a name such as `nginx:1.29` in **OCI image** (API: `"os": "oci:nginx:1.29"`). The
container runs the image's program. Such images usually have no DHCP client: give a **Static IP address** and a
**Gateway** (on LXC's NAT bridge for example `10.0.3.50/24` and `10.0.3.1`). Public `linux/amd64` images only;
private registry passwords, device nodes and extended attributes (file capabilities) are not supported. Every
downloaded part is verified against its digest and layers cannot write outside the root directory. Tested against a
fake registry; **a real registry and starting such a container with real LXC were not tried.**

## 15. Token lifetime, sessions, operator permissions, syslog, Let's Encrypt, hot removal

### 15.1 Lifetime of API tokens

Choose 30, 90 or 365 days in **Valid for** in the panel, or on the command line
`redox token create <user> <label> --days 90 --users FILE`. An expired token is refused like a wrong one. A token
without a lifetime never expires, as before.

### 15.2 Open sessions

**My Account** lists open sessions (address, browser, last use); close one or all others. An administrator can close
every session of a user on the **Users** page, also in lockdown. API tokens are not sessions; revoke them separately.

### 15.3 Operator permissions

An operator can be limited to some of four permissions: power, console, snapshot, backup. Use the **Users** page or
`redox user permits <name> backup --users FILE` (`all`: no restriction). The tag scope still applies. An open live
console session does not close at once when the permission is removed (a terminal does).

### 15.4 Sending the audit log to syslog

```bash
redox serve ... --syslog tls://log.example.net:6514 --syslog-ca /etc/redox/log-ca.pem
```

`udp://`, `tcp://` and `tls://` are supported (RFC 5424; octet-counted framing on TCP and TLS). `--syslog-ca` is only
needed for a private certificate authority. Sending never blocks the panel; if the queue is full, records are dropped
and their number reported. **Delivery is not guaranteed**; the local `audit.log` stays the primary record. Not tried
with a real collector such as rsyslog.

### 15.5 A Let's Encrypt certificate

The domain must point to this server and port 80 must be reachable from the internet and free.

```bash
sudo redox acme issue --domain panel.example.net --dir /etc/redox/tls --agree-tos --email you@example.net --staging
```

`--staging` gives a test certificate; when all is well, run it again without the option. `--agree-tos` means you accept
the provider's terms. Start the server with `--tls-cert /etc/redox/tls/tls.crt --tls-key /etc/redox/tls/tls.key`; the
service user must be able to read them. For renewal set up a daily timer running
`redox acme renew --dir /etc/redox/tls` (it renews when fewer than 30 days are left; the server uses the new files
without a restart). Renewed files belong to the user who ran the command. Tested against a fake ACME server; **not
tried with Let's Encrypt.**

### 15.6 Removing cores and memory from a running VM

`redox vm cores <id> <count>` and `redox vm memory <id> <MB>` (and the same forms on the Hardware tab) remove when
given a smaller value. Only memory added while running can be removed, newest part first. The guest operating system
has to release it. **For reliable memory removal a Linux guest must online new memory as movable:** kernel option
`memhp_default_state=online_movable`. Tested on real QEMU with a Linux guest: with the guest's default policy one part
could not be released, with the movable policy all could. Not tried with Windows.

## Known gaps

- **Two-factor login is optional.** Each administrator should turn it on; protect the panel with a VPN or an IP
  restriction as well if you can.
- **The lock is per address.** A distributed attempt is slowed, not stopped; Argon2 makes each try expensive.
- **Login locks are in memory** and reset when the service restarts (sessions are kept).
- **The audit log is request-level** (who, from where, which address); form contents are not recorded. The file
  does not shrink by itself; rotate it with `logrotate`.
- **Backups are on the same server unless you define an off-site target** (section 11).
- **Corruption in the incremental store is not noticed by itself:** new backups reuse stored chunks without
  reading them again. Verify important backups now and then.
- **A multi-disk restore is not atomic:** disks are put in place one by one after all are prepared.
- **Not done:** LVM/ZFS/Ceph/iSCSI storage pools, a shared-state cluster, SPICE, live migration of guests with UEFI or
  a TPM, clipboard and microphone in the live console, live migration of containers and
  moving them to another storage.
- **The QEMU unit** (`redox-qemu.service`) uses a looser sandbox than the simulation unit; the file says why.
- **What was verified where.** The systemd units and the Caddy configuration were not tried on a real server.
  Code and tests ran on Linux (WSL, Ubuntu). On real KVM with NAT networking: snapshots, rollback and backups of
  a running guest; creating from an image; UEFI, secure boot and TPM with real OVMF and swtpm; extra disks and
  a second card; vmdk import; incremental backups and their encrypted off-site copy; linked clones;
  changed-block tracking; OVA export and import; suspend; hot-plug; VLAN and rate limits in a private network
  namespace; the live console; backup jobs; CPU models; the balloon device; backup and restore of UEFI and TPM
  state; moving disks between storages; pool quotas; and moving a VM, including one with extra disks, UEFI and
  TPM, between two servers; restore as a new guest, instant recovery with a live detach, the boot test, locks,
  copies to two off-site targets with a restore from the copy alone, and rate-limited backups (the option was
  accepted; the actual rate was not measured). Tested only with fake tools or fake servers: bridged networking, LXC (including
  clone and live backup), the guest firewall under traffic, S3 (including multipart upload), SMTP (including
  STARTTLS) and OpenID Connect. Not verified: the guest agent in a real guest, cloud-init inside a real cloud
  image, an actual memory reduction through the balloon, a real boot of the server for autostart, and importing
  an exported OVA into VMware or VirtualBox, file-level restore with real libguestfs, backup commands through a real
  guest agent, and delivery of the backup report to a real mail server.
