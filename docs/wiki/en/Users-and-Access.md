[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Users and access

## Roles

| Role | Can do |
|---|---|
| Administrator | Everything. |
| Operator | Power, console, snapshots and backups on guests in scope (by tag). |
| Viewer | Read only. |

Users are created on the server with `redox user add <name> --users FILE` or on the *Users* page.

## API tokens

```bash
redox token create admin script --users /etc/redox/users.json
redox token create admin monitoring --read-only --users /etc/redox/users.json
```

A token carries its owner's role; a `--read-only` token can only read even on an administrator account (for monitoring
tools). The value is shown once.

## Two-step verification and single sign-on

TOTP is turned on from *My Account*. If the phone is lost an administrator resets it on the *Users* page or with
`redox user totp-reset`. OpenID Connect is supported for organization sign-in (INSTALL.md section 13; **not tried** with a
real provider).

## Four-eyes principle

With `--four-eyes`, deleting a backup, a guest, an extra disk, a backup job or a user does not happen at once: it runs
when **another** administrator approves.

```bash
redox approval list
redox approval approve <n>
redox approval reject <n>
```

The requester cannot approve their own request; requests expire after 24 hours. At least two administrator accounts are
needed. Retention pruning by the scheduler is exempt.
