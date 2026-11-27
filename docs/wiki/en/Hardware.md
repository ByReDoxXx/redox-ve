[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Hardware

Hardware settings (the *Hardware* tab) change while the guest is **stopped**, except where "while running" is noted.

| Setting | Note |
|---|---|
| Cores, memory | While stopped. |
| CPU type | Default `host` (fastest). Choose a common model if you will move the guest to a server with a different CPU. |
| Nested virtualization | The guest runs its own virtual machines. `nested` must be on in the host's `kvm_intel`/`kvm_amd` module. |
| Maximum cores | When above 0 the guest starts with this many core slots and cores can be added **while running**: `redox vm set 103 max-cores=8`, then with the guest running `redox vm cores 103 6`. Upwards only; stop the guest to lower it. With the agent (qemu-guest-agent) in the guest the new cores come online by themselves; without it you may need `echo 1 > /sys/devices/system/cpu/cpuN/online` in the guest. |
| CPU shares | The guest's share when the server runs short of CPU (*Options* tab or `redox vm set 103 cpu-shares=200`). A guest with 200 shares gets twice the CPU of one with 100; 0 is normal (100), the range is 10 to 10000. Nobody is throttled while the CPU is idle. Can be changed **while running**. |
| Memory reservation | Memory that is not taken back from the guest when the server runs short (`redox vm set 103 mem-reserve=4096`). The reservations of running guests cannot add up to more than the server's memory: a guest that does not fit is not started. The balloon cannot take the guest below its reservation. Can be changed **while running**. |
| Maximum memory | When above 0 the guest starts with free memory slots and memory can be added **while running**: `redox vm set 103 max-memory=16384`, then with the guest running `redox vm memory 103 8192`. In steps of 128 MB and upwards only; at most 8 additions per start, and a stop and start frees the slots. Linux guests mostly use new memory by themselves; when they do not, the guest agent brings it online, and without the agent you need `echo online > /sys/devices/system/memory/memoryN/state` in the guest. Memory that has not been added takes no room on the server. |
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

`redox vm move 103 fast` or the *Clone and Move* tab. A stopped guest is moved by copying its files, snapshots
included. A running virtual machine is moved **without stopping it**: its disks are mirrored to the new storage while
it keeps running, then switched over, and the old files are removed. Only the current content is mirrored, so a
running guest that has snapshots must be stopped to be moved.
