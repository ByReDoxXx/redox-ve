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
| Graphics card sharing (vGPU) | Enter a mediated device ID (mdev UUID) in the PCI field instead of an address; one card is split between several guests. You create the mediated device on the server (the card and its driver must support vGPU). **Not tried with real hardware.** |
| Shared folders | virtio-9p. In the guest: `mount -t 9p -o trans=virtio,version=9p2000.L name /mnt/name` |
| cloud-init | User, SSH key, static address, gateway, name servers. |

## Storage on ZFS and LVM

A storage is a directory (`--storage name=/path`); disks live there as qcow2 image files. The directory may be a ZFS
dataset, a file system on an LVM (or LVM-thin) volume, or an NFS share: RedoX detects this when the server starts
and shows it in the **Type** column of the *Storages* table. ZFS features such as compression, checksums and
snapshots come from the file system itself.

```
zfs create -o mountpoint=/srv/fast tank/redox     # or: lvcreate -V 500G -T vg0/pool -n redox && mkfs.ext4 …
redox serve … --storage fast=/srv/fast
```

There is no built-in pool management that puts every disk on its own ZFS volume (zvol) or logical volume.

## Moving between storages

`redox vm move 103 fast` or the *Clone and Move* tab. The guest must be stopped; disks and snapshots move together.
