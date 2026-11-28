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

- **Live console:** real-time screen, keyboard and mouse (uses JavaScript). On a VM with a sound card, **Turn Sound On**
  also plays its audio; the microphone is not carried.
- **Container terminal:** **Open Terminal** on a running container gives an interactive root shell in the browser (uses
  JavaScript). Not yet tried with a real `lxc-attach`.
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

## Typing text into the console

The live console has no clipboard sharing. Instead, text entered in the box at the top of the console page is sent
to the guest as keystrokes with **Type** (for long passwords and commands). No agent is needed in the guest. Only
characters on a US keyboard are typed and the guest's keyboard layout must be US English; with another layout symbols
come out differently. The client logic is tested; it was not tried on a real guest.

## Sound, disk encryption and OCI images

- **Sound card:** *Add a sound card* in the settings or `redox vm set <id> sound=true` (with the VM stopped).
- **Disk encryption:** if the server runs with `--disk-key-file`, tick *Encrypt disks (LUKS)* when creating a VM. If
  the key is lost the disk cannot be opened; cloning, suspend, export and some other operations are refused.
- **OCI image:** when creating a container put a name such as `nginx:1.29` in *OCI image*; give a static IP and a
  gateway for networking. Public linux/amd64 images only. Not yet tried with a real registry or real LXC.
- **Tailscale in a container:** give it the tunnel device when creating it, or under **Hardware > Devices** while it is
  stopped (on the command line `redox vm set <id> tun=true`), then install Tailscale inside and run `tailscale up`. Not yet tried in a real LXC container.

Details and limits: `INSTALL.md`, section 14, in the repository.
