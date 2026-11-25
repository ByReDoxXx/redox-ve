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

There is no zero-downtime live migration and no automatic failover.

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
