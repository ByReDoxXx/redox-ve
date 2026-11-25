[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Backups

A backup is a copy of the guest's disks (including extra disks, UEFI variables and TPM state) in the server's backup
directory. A running virtual machine is backed up without interruption.

## Taking and restoring

The guest's *Backups* tab, or:

```bash
redox backup create 103
redox backup list
redox backup restore <backup>             # onto the guest (which must be stopped)
redox backup restore-new <backup> <name>  # as a separate guest (even if the original was deleted)
redox backup instant <backup> <name>      # instant recovery: the guest starts on top of the backup at once
redox vm detach <id>                      # detaches an instantly recovered guest from its backup
```

**File-level restore:** browse a backup on the *Backups* page and download single files (needs `libguestfs-tools` on the
server; administrators only, every access is written to the audit log).

## Scheduling and retention

- **Per guest:** time and number of backups to keep on the *Options* tab; plus weekly/monthly/yearly (GFS).
- **Backup jobs:** the *Backups* page. A job backs up the guests with a tag (or all), one after another; a window end,
  lock period and verification can be set. `redox job add nightly time=03:00 days=1-5 tags=web`
- **Lock:** a locked backup cannot be deleted by anyone until the lock expires.
- **Backup age target (RPO):** `redox vm set 103 rpo=24`. A guest whose last backup is older shows as unprotected.

## Verify what you rely on

- **Verify:** a corruption check.
- **Boot test:** the backup is started in an isolated temporary guest. Optionally data must arrive from a TCP port and a
  command in the guest must succeed: `redox vm set 103 test-port=22 test-cmd=/usr/local/bin/health.sh`
- **Scheduled tests:** `--backup-test-every 7@03:00` tests the newest backup of every guest weekly.
- **Unusual-change warning:** when an incremental backup changes many times the usual amount, the backup is flagged (a
  possible sign of ransomware). After checking, dismiss it with `redox backup ack <backup>`.
- **Daily report:** `--backup-report 07:30` (needs e-mail settings) or `redox backup report`.
- **Storage forecast:** the *Backups* page shows in how many days the storage fills up, from the backup data added in
  the last two weeks; a warning is raised 14 days before and a critical alert 3 days before. Space returned by deleted
  backups is not counted, so the estimate is cautious.

## Copying off the server

`--offsite FILE` defines the primary target (a directory or an S3-compatible store), `--offsite-copy FILE` extra ones.
With a `passphrase` in the file backups are encrypted before they are sent. `keep_days = 90` in a copy target's file
turns it into an **archive**: a backup deleted locally stays there for 90 days and can be recovered from
*Backups → Archive*.

Local backups are not encrypted; keep the backup directory on an encrypted volume (LUKS).

## Incremental backups

With `--incremental-backups` backups go into a shared chunk store: each adds only new data yet restores on its own, and
deleting one does not break the others. On a running VM, changed-block tracking reads only what changed.

## Encrypting local backups

```
redox serve … --incremental-backups --backup-passphrase-file /etc/redox/backup-passphrase
```

The file holds one line with a passphrase of at least 12 characters (`chmod 600`). From then on the chunks of virtual
machine backups and the container archives are written to disk encrypted with ChaCha20-Poly1305; the key is derived from the passphrase with
Argon2id. The *Backups* page shows an **encrypted store** label.

- **If you lose the passphrase these backups cannot be opened.** Keep it somewhere outside the server too.
- An encrypted store does not open without the passphrase or with a wrong one; the server refuses to start.
- Backups taken before encryption was enabled stay as they are (unencrypted) and remain readable.
- Not encrypted: full (non-incremental) virtual machine backups and the backup indexes (chunk hashes and disk
  sizes). Restores and exports create temporary plain files that are deleted when the job ends.
- Chunks sent to an off-site target are sent encrypted as well (twice if the target has its own passphrase).
- An encrypted store keeps no plain cache for incremental replication; replication from this server copies in full on
  every run.
