[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Hardware

Hardware settings (the *Hardware* tab) change while the guest is **stopped**, except where "while running" is noted.

| Setting | Note |
|---|---|
| Cores, memory | While stopped. |
| CPU type | Default `host` (fastest). Choose a common model if you will move the guest to a server with a different CPU. |
| Nested virtualization | The guest runs its own virtual machines. `nested` must be on in the host's `kvm_intel`/`kvm_amd` module. |
| Memory balloon | When attached, the memory target changes **while running**. |
| Dynamic memory | RedoX sets the target from the guest's usage: `redox vm balloon 103 auto 2048`. Needs the balloon driver in the guest. |
| Extra disks (up to 8) | Can be added **while running**. Disks can only grow. |
| Extra network cards (up to 3) | Can be added **while running**. |
| Rate limits | Disk MB/s and IOPS; network Mbit/s (bridged cards only). |
| Firmware | BIOS, UEFI, UEFI + secure boot. Windows 11 needs UEFI + secure boot + TPM. |
| Virtual TPM 2.0 | Needs `swtpm`. Its state is included in backups. |
| Passthrough | USB (`lsusb` ID) and PCI (`lspci -D` address). **Not tried with real hardware.** |
| Shared folders | virtio-9p. In the guest: `mount -t 9p -o trans=virtio,version=9p2000.L name /mnt/name` |
| cloud-init | User, SSH key, static address, gateway, name servers. |

## Moving between storages

`redox vm move 103 fast` or the *Clone and Move* tab. The guest must be stopped; disks and snapshots move together.
