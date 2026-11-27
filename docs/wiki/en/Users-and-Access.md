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

## Lockdown (change freeze)

`redox lockdown on` (or *Lock the Server* on the node's page) puts the server into lockdown: until an administrator
lifts it, **nobody** can change anything from the web interface or the API, administrators and API tokens included.
Guests keep running, and scheduled backups, schedules and automatic restarts go on. Reading still works; everyone can
log out and manage their own account. It is meant for audits and change-freeze periods, and it survives a restart of
the service.

Lockdown is not a security boundary against someone with a shell on the server: it only closes the management
interface. Requests from peer servers (copying, replication, live migration from this server) are refused as well
while it is on.

## Security keys (WebAuthn)

If the server runs with `--webauthn-origin https://the-panel-address`, each user can add keys on the *My Account*
page. Signing in then asks for the key (or the app code, if that is on) besides the password. For a lost key an
administrator resets the second step on the *Users* page or runs `redox user keys-reset <name>` on the server.

## The account page and tokens

The account page (password, second step, creating tokens) works only with a browser sign-in; requests made with an
API token are refused.

## Token lifetime, sessions and operator permissions

- A lifetime can be chosen when creating a token (**Valid for**, `--days N`).
- *My Account* lists and closes open sessions; an administrator can close all sessions of a user on the *Users* page.
- An operator can be limited to some of power / console / snapshot / backup (`redox user permits <name> backup`).
