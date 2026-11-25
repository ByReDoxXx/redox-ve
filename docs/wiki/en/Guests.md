[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Guests

A guest is a virtual machine (KVM) or a container (LXC).

## Creating

The *New Guest* button, or the API. A virtual machine is installed in one of three ways:

- **Installation media (ISO):** upload it on the *Storage* page and choose it when creating the guest.
- **Ready cloud image:** download one from *Storage → Ready cloud images* (Debian, Ubuntu, AlmaLinux, Rocky) or with
  `redox media get debian-12`. Choose the image when creating the guest and give a user name and SSH key (cloud-init).
- **Clone of a template:** see below.

If the disk and network card are not visible while installing Windows, download the **Windows drivers (virtio-win)**
media from the catalog (`redox media get virtio-win`) and load the drivers from it during setup; afterwards install the
balloon driver and the guest agent from the same media.

## Power

Start, Stop, Restart, Pause, Resume. Stop first asks the guest to shut down through ACPI and forces it after a timeout.
**Suspend** writes memory to disk and stops the guest; starting it resumes where it left off.

Scheduled power actions (the *Options* tab) start, stop or restart on given days and times. *Start at boot* and the boot
order are on the same tab. *Restart if it stops by itself* restarts a crashed guest within about 15 seconds.

## Console

- **Live console:** real-time screen, keyboard and mouse (the only page that uses JavaScript).
- **Console tab:** no JavaScript; the screenshot refreshes every 2 seconds and you can send text and special keys.
- Command execution in containers and in machines with the guest agent.

## Clones and templates

The *Clone and Move* tab. A stopped guest can be cloned (a full copy). **Converting** a virtual machine **to a template**
makes it unstartable, but **linked clones** are made from it in seconds (they keep only their difference).

## Guest agent

With `qemu-guest-agent` installed in the guest: the IP address and operating system are shown, file systems are frozen
for backups taken while running, disk usage inside the guest is read, and pre/post backup commands can run.

## Resource pools

The *Pools* page. A pool puts a quota on the total cores, memory and disk a group of guests may allocate. You put a guest
into a pool from its *Options* tab.
