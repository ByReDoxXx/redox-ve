[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Multiple servers

RedoX VE has no shared cluster state: each server manages its own guests. Servers are introduced to each other as
**peers** (`--peers FILE`, each line `NAME ADDRESS TOKEN`); the *Cluster* page shows them together.

## Moving

Buttons next to a peer's guest on the *Cluster* page:

- **Copy Here:** copies a stopped virtual machine to this server through its backup. The source stays; you delete it
  there once you see it running here.
- **Move Running:** suspends the running guest there, copies its disk and memory and resumes it here. The guest does not
  reboot but does not respond during the copy.

- **Containers:** *Copy Here* copies a container through its backup. An empty container is created here from the
  same distribution template and its root file system is replaced by the backup. The source keeps running (frozen
  briefly during the backup); the copy arrives stopped. LXC must be installed on this server. **Tested with fake LXC
  tools; not tried with real LXC.**

**Live migration:** a running VM can also be moved **without stopping it** (*Migrate Live* on the *Cluster* page, or `redox peer migrate <peer> <id>` on the target). The target starts the guest's QEMU paused and waiting, the source mirrors every disk to it while the guest keeps running, then migrates the memory; the guest pauses only for the last memory pages (tens of milliseconds in the tests) and continues on the target with the same ID. The guest's record and disks stay on the source, stopped and marked so that they do not start again by themselves; delete them once you are satisfied.

The two QEMU processes connect to each other directly: the source must be able to reach the target on the ports given with `--migrate-ports` (default 49152-49215, two per migration; open them between the servers only). If the address the source sees the target under differs from where the request came from, set `--migrate-host` on the target. Both connections are encrypted with a one-time key (TLS with a pre-shared key) that travels over the management connection, so use `https` between the servers.

Not every guest can be moved live: UEFI or virtual TPM, an attached installation image, a cloud-init drive, shared folders, passed-through host devices and disks that depend on a template or backup block it (the message says which); the same ID must be free on the target, and both servers need the same QEMU version and a compatible CPU model. Tested on real QEMU 10.2 with a Linux guest on one machine (two servers, loopback network); not tried between two physical machines, with a Windows guest, or with large and busy guests.

**Evacuating a server for maintenance:** `redox maintenance on --evacuate <peer>` (or *Evacuate My Guests Here* next to the peer on the *Cluster* page) puts this server into maintenance mode and moves its running virtual machines to the peer one by one, without stopping them. Guests that cannot be migrated live stay where they are; add `--drain` to stop those instead (they start again when maintenance ends). The peer does the pulling, so it must list this server in its own `--peers` file; if it cannot be recognised by address, give the name it has there with `--as`. The moved guests keep running on the peer when maintenance ends. To bring one back, use *Migrate Live* from this server: the guest returns with the same ID and replaces the stopped copy that was left behind (its backups stay attached). A left-behind copy that has been started by hand is treated as a guest in its own right and is never replaced. Tested with fake QEMU between two servers; the migration underneath is the same one tested on real QEMU.

## Load balancing advice

When this server is busy (memory or CPU at 80% or more), the *Cluster* page shows a **Load balancing advice** card and
`redox balance` prints the same thing: which running virtual machine would help if it were live-migrated to which
peer, and what the memory use would be on both sides afterwards. Nothing is moved automatically; each line has a
*Send* button (`redox peer send <peer> <id>`), which live-migrates that virtual machine from this server to the peer.

The advice only covers running virtual machines that can be live-migrated, never suggests a peer that would itself
go above 70% memory or that already uses the same ID, and lists at most three moves. It is computed from the last
state read from the peers, so apply it one move at a time.

## Server profile

A profile is one server's management settings in a file, so that several servers can be kept alike:

```
redox profile export > profile.json     # on the reference server
redox profile diff profile.json         # on another server: what differs?
redox profile apply profile.json        # bring pools and backup jobs in line
```

It has two parts. **Options** are the service's start options and what the server can do (driver, firewall, UEFI,
four-eyes, storages, migration ports and so on); they do not change while the service runs, so they are only compared,
and a difference has to be fixed in the unit file by hand. **Pools** and **backup jobs** live in the state and are
applied: what the profile has is added or updated, and what exists here but not in the profile is left alone (and
listed). The profile carries no users, tokens or peer addresses.

For automatic failover see below.

## Synchronous disk mirroring

With mirroring on, **every disk write of a running virtual machine also goes to a copy on a peer server**, and the
write does not complete until both have it. If the server running the VM dies, the copy has everything the guest was
told is written. (Replication, below, copies at intervals: what was written since the last run is lost.)

```
redox mirror add 103 ankara                 # on the server where VM 103 runs
redox mirror list                           # syncing, in sync, broken (with the reason)
redox mirror off 103 [--delete-copy]

redox mirror copies                         # on the peer: the copies it holds
redox mirror promote <source> 103           # take over: creates a STOPPED guest from the copy
redox mirror delete-copy <source> 103
```

The *Cluster* page has the same in the **Disk mirroring** and **Copies held here** cards.

**What happens when something fails**

* **The peer stops responding** (power cut, broken link). Because writes are synchronous, the guest's disk writes
  wait. A watchdog that only asks the local hypervisor notices the stuck progress after 8 seconds, cuts the
  connection and lets the waiting writes finish. The guest keeps running; the mirror shows *broken* and the copy is
  behind from that moment.
* **After a break** it retries every minute. A new synchronisation goes into a **new generation** of files; the last
  complete copy is kept until the new one is complete. Plan for **twice the disk space** on the peer.
* **The source server dies.** On the peer, wait about 30 seconds (while the source still reports "in sync", take-over
  is refused), check that the guest really is off there, then take over and start the new guest.

**Take-over is manual, on purpose.** The peer cannot tell "the source is down" from "the link between us is lost";
in the second case the guest is still running on the source, and starting it twice corrupts data. Look at *Last in
sync*: if the mirror broke before the source died, the copy is that old.

**Limits**

* One copy per guest, on one peer. This is not a storage pool across many servers: there is no striping, no automatic
  rebalancing and no shared datastore.
* **Backups, snapshots and suspend pause the mirror** (QEMU does not allow them on a mirrored disk). It then resyncs
  **from the beginning**, so each of these costs a full copy over the network, and until that finishes the peer only
  has the older copy. Internal snapshots are not part of the copy.
* A mirrored guest cannot be live-migrated or have its storage moved; turn mirroring off first.
* The watchdog is part of the RedoX service. If the service is stopped while a peer hangs, the guest's writes wait
  until the connection times out.
* With several disks, if the connection of one disk breaks, another can get a few more writes (up to about a second)
  before mirroring stops, so in that case the disks of the copy may not be from exactly the same moment.
* Write speed is limited by the network between the servers. UEFI, virtual TPM, containers, templates and linked
  disks are not supported.
* The copy goes over the peer's migration ports (`--migrate-ports`), encrypted with TLS and a one-time key.

## Fault tolerance

A protected virtual machine has a **twin running in lock-step** on a second server (QEMU COLO). If the server dies,
the guest **continues on the twin without rebooting**: memory, processes and disk are there.

It needs **three servers**: the primary, the twin server and a **witness**. Give the primary's `--peers` file entries
for the twin server and the witness, and give the twin server an entry for the witness **under the same name**. The
guest's bridge must exist on both servers (a distributed switch does this).

```
redox ft on 103 yedek hakem        # while VM 103 is stopped: <id> <twin server> <witness>
redox vm start 103                 # the twin is built automatically
redox ft list                      # preparing, protected, unprotected (with the reason) …
redox ft twins                     # on the twin server
redox ft off 103
```

The *Cluster* page has the same in the **Fault tolerance** card.

**What guest can be protected.** A virtual machine with **one disk** and **one bridged network card**, and no extra
hardware: no hot-add, balloon, UEFI, TPM, passthrough, shared folders, ISO, cloud-init, VLAN tag, guest firewall or
rate limits. It starts with the older **`pc` (i440fx) machine type** instead of `q35`: with QEMU 10.2, COLO crashes the
twin at its first checkpoint on `q35`. A guest installed on `q35`, Windows in particular, may need driver changes to
boot on `pc`; check this before you rely on it.

**What is guaranteed.** Everything that became **visible on the network** survives: a packet leaves the primary only
when the twin has produced the same one, or after the two have been made identical (a checkpoint). The twin does not
see the console or anything else that is not network traffic, and between checkpoints (at most about 20 seconds) it
runs on its own. Work that never left the guest over the network since the last checkpoint **can be lost**.

**Never in two places.** When the two servers lose each other, each may think the other is dead. The witness decides:
**the first one to ask wins**, and the other ends its own copy.

| What happens | Result |
|---|---|
| Twin server dies or freezes | The guest's network stops for about 5 seconds, then it continues alone; protection is retried after 2 minutes |
| Primary server dies | The twin takes over; the network stops for about 5 seconds |
| The link between the two breaks | Both ask the witness; one continues, the other ends its copy |
| The primary cannot reach the witness either | It **pauses** the guest until the witness answers |
| The twin cannot reach the witness | It keeps waiting; it never takes over without the witness |

Stopping, restarting or turning protection off first claims the witness vote, so the twin can never take over a guest
you stopped on purpose. After a take-over the old primary shows *the standby server took over* and **refuses to start**
that guest. Only after you have made sure the guest is off on the other server, turn fault tolerance off there
(`redox ft off`) to release the old record; its disk is out of date.

**Limits**

* While protected there are **no backups, snapshots, suspend, pause, migration or disk operations**, scheduled backups
  included (they fail with that message). To back up, turn protection off and restart the guest.
* Network packets of the guest that differ between the two copies are held for about **3 seconds** until a
  checkpoint. Chatty or latency-sensitive workloads can be much slower than unprotected ones. Performance was **not**
  measured.
* After a take-over the new guest is **unprotected**; to protect it again, turn fault tolerance on on that server.
* Disk writes and memory are sent with TLS and a one-time key. The two **packet comparison connections are not
  encrypted** and carry a copy of the guest's network traffic: use a dedicated, trusted network.
* Both servers need the same CPU and the same QEMU version. Each protected guest uses its memory on both.
* If the RedoX service on the primary is stopped while the twin hangs, the guest's network stays blocked.
* Tested with a Linux guest, with the three servers **on one physical machine**. It has not been tested on separate
  physical servers, with Windows guests, under load or for long periods. Test your own setup before you rely on it.

## Replication (standby copy)

```bash
redox replica add <peer> <id> 60     # every 60 minutes
redox replica list
redox replica run <peer> <id>        # now
redox replica remove <peer> <id>
```

A copy of the peer's virtual machine or container is refreshed on this server regularly; the source keeps running. The copy is a
stopped guest named `<name>-kopya`. If the source server dies **you** start the copy here; you lose what was written
since the last run. A running copy is never overwritten. **Incremental:** for virtual machines, after the first run only the 4 MiB blocks that changed are downloaded; the
rest is taken from the old copy, and every downloaded block is verified by its SHA-256 hash. The *Replication* card
shows how many blocks the last run downloaded. The source server still reads the whole disk on every run (to hash
it); the saving is on the network. If the incremental transfer fails for any reason, that disk is downloaded in full
automatically. Containers are copied in full on every run.

## Maintenance mode

Before updating or rebooting the server:

```bash
redox maintenance on --drain   # stops the running guests in order
# ... update, reboot ...
redox maintenance off          # starts them again in boot order
```

While it is on no guest is started, and scheduled backups, power actions and replication are paused. The mode is kept in
the state file: guests stay down across a reboot. It does not move guests to another server.

## Automatic failover (with a witness)

For a replicated guest you can ask for the standby copy to be started automatically when the source server dies:

```
redox replica failover ankara 103 10     # if ankara is unreachable for 10 minutes
redox replica failover ankara 103 off
```

It can also be set in the **Failover** column of the *Replication* card (0 turns it off; 5 to 1440 minutes).

This server alone cannot tell "the source died" from "my own network is down". So before starting anything it asks a
**witness**: another RedoX server listed in the `--peers` file.

- If no witness can be reached, nothing is started (this server is probably the isolated one).
- If a reachable witness sees the source running, nothing is started (only the link in between is down). For this
  the witness must have the source in its own `--peers` file under the same address or the same name; a witness that
  does not know the source only proves that the network is up.
- It retries every minute until the obstacle is gone; the reason is shown on the **blocked** label in the card.

A replica fails over only once. When the source comes back, **do not leave the standby copy and the source running
at the same time**: stop one of them. Once the copy is stopped and a replication run succeeds, failover is armed
again. Nothing fails over in maintenance mode.

**Limits:** if both the witness and this server lose the source at the same time while the source is in fact
running, no method can see that; there is no fencing (forcibly powering off the source). The guest's address does not
move by itself: on the same network the copy comes up with the same address, otherwise you change DNS or routing.
Tested with fake QEMU and a three-server test; not tried in a real outage.

## Shortening the pause of a move (pre-copy)

When a running virtual machine is moved it is suspended on the source, its disk and memory are copied, and it
continues here where it left off; the guest is paused for the whole copy. With `redox serve … --move-precopy` (on
the server that **receives** the guest) the disks are downloaded first while the guest is still running; after it is
suspended only the 4 MiB blocks that changed in the meantime and the memory state are transferred. The task line
`vm precopy peer/id 3/250` says 3 of 250 blocks were downloaded during the pause.

When it helps: when the network between the servers is slower than the disks (for example a 1 Gbit network with
SSDs). The source server still reads the whole disk during the pause (for the backup and the hashes), so on a very
fast network there is no gain and the pause may even get longer; that is why the option is off by default. If the
source server stores its backups encrypted, pre-copy is not possible and the move continues the ordinary way.

This is **not** live migration: the guest still pauses, only for less time. For guests that qualify, *Migrate Live*
(above) is the better choice. Pre-copy was tested with fake QEMU; its pause was not measured with real QEMU.

## Failing back after a failover

There is no automatic failback; you take the steps. When the source server (ankara) is back:

1. **First make sure the old guest on ankara is not running.** If it is marked to start at boot it may have come up
   by itself; two copies with the same address confuse the network. Stop it and clear *start at boot*.
2. On ankara, in the *Combined view*, move the copy running on this server back with **Copy Here** (this server
   must be listed in ankara's `--peers` file). A running guest moves with its memory state; everything written since
   the failover is kept.
3. Once the moved guest is running, delete the old guest on ankara and the suspended copy left on this server.
4. Remove the replication on this server and add it again with the new guest ID; when the first run finishes,
   failover is armed again.

This sequence was not tried by hand; each step is an operation that is tested on its own (move, delete, replicate).

## Placement tags and automatic balancing

Guests tagged `ayri-<group>` are kept off the same server, `birlikte-<group>` guests are not split, and a `sabit` guest is never moved. With `redox serve --auto-balance`, one guest is live-migrated if the server stays busy for 5 minutes or a keep-apart rule is broken (at least 15 minutes between moves). The decision rules are unit tested; automatic moves have not been run between real servers.

## Shared storage

If two servers mount the same directory (for example NFS): run `redox shared init <dir>` once and start every server
with `serve --storage shared=<dir> --shared-storage shared`. Live migration then moves only memory; take a stopped
guest with `redox shared take <peer> <id>`, and a quiet server's guest with `redox shared adopt <storage> <id>`. There
is no fencing: make sure the old server is down before adopting. Not tried with a real NFS share or two physical
machines.
