[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Troubleshooting

**A guest does not start.** Read the message of the failed task on the *Tasks* page. Common causes: no `/dev/kvm`, or the
service user is not in the `kvm` group; the bridge is not allowed in `/etc/qemu/bridge.conf`; `ovmf` (UEFI) or `swtpm`
(TPM) is not installed; nested virtualization is requested but off on the host.

**"The server is in maintenance mode".** `redox maintenance off`.

**A guest does not start by itself.** Maintenance mode may be on; or the guest crashed too often within 10 minutes and
restarting was given up (the task log says so).

**Backups are not taken.** Check the "Unprotected guests" card on the *Backups* page and the *Tasks* page. A second
backup of the same guest cannot be taken in the same second. If the pre-backup command fails, the backup is skipped on
purpose.

**A backup cannot be deleted.** It may be locked (nobody can delete it until the lock expires), an instantly recovered
guest may be running on top of it, or the four-eyes principle is on and a second administrator must approve
(`redox approval list`).

**The dynamic memory target does not change.** Without the virtio balloon driver in the guest, or if it does not report
"available memory", the target is left alone.

**Disks inside the guest are not shown.** `qemu-guest-agent` must run in the guest; the figures are read once a minute.

**With NIC protection on the guest has no network.** If the guest talks with a MAC address other than its own (a bridge
or nested VM inside it), its frames are dropped; turn the protection off for that guest.

**Replication says "failed over".** The standby copy is running; it is not overwritten. Stop the copy or remove the
replica.

**The API returns 429.** The `--api-rate` limit was exceeded; wait for `Retry-After`.

**The interface is in the wrong language.** Use the EN / TR button in the top bar, or delete the `lang` cookie
([Language](Language.md)).

For anything else, the service log: `journalctl -u redox -e`.

## Support bundle

When you report a problem, attach a support bundle: `redox support > bundle.json`, or *Download* under *Support bundle*
on the *Servers* page (administrators only). It is one JSON file with the version, what the driver can do on this
server, the settings, nodes and storage, every guest's hardware, the last 200 tasks and a few host facts (kernel, load,
memory, addresses).

It contains **no** users, passwords, tokens, peer tokens or audit log, and the commands that run before and after
backups and the SSH keys of guests are masked. It does contain guest names, network addresses and task messages, so
read it before you send it. The management service's own log is not in it; get that with `journalctl -u redox`.
