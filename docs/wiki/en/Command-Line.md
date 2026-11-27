[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Command line

The same `redox` file runs the server and is the client. For the client:

```bash
export REDOX_URL=https://redox.example.com
export REDOX_TOKEN=redox_...
export REDOX_LANG=en
redox help
```

## Common commands

```bash
redox cluster status
redox vm list
redox vm start 103            # stop, reboot, pause, resume
redox vm set 103 cores=4 memory=8192 nested=true
redox vm disk add 103 50
redox vm balloon 103 4096     # or: auto 2048
redox vm cores 103 6          # raises a running guest to 6 cores (first, while stopped: vm set 103 max-cores=8)
redox vm memory 103 8192      # raises a running guest's memory to 8192 MB (first, while stopped: vm set 103 max-memory=16384)
redox vm move 103 fast
redox snapshot create 103 before [--production]
redox backup create 103
redox backup list
redox backup restore <backup>
redox backup put <backup> /etc/hostname      # writes one file from the backup into the running guest (needs the guest agent)
redox job add nightly time=03:00 days=1-5 tags=web
redox pool add web cores=32 memory=65536 disk=500
redox replica add ankara 500 60
redox maintenance on --drain
redox approval list
redox media catalog
redox task list
redox usage
```

`redox vm set` keys: `name cores max-cores cpu-shares memory max-memory mem-reserve os iso bridge onboot restart boot-order boot-delay schedules disk-mbps disk-iops
net-mbps vlan cpu balloon nested nic-guard firewall rpo test-port test-cmd backup-time backup-keep backup-pre backup-post`
and more; `redox help` prints the full list.

For English output set `REDOX_LANG=en` ([Language](Language.md)). The examples below assume it:
