[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Networking and security

## NAT and bridges

- **NAT (default):** the guest reaches the internet and is not visible from outside. For inbound access use port
  forwarding (`server-port:guest-port`, server port 1024 or higher).
- **Bridge:** the guest gets an address from the server's network. The bridge must exist on the server and be allowed in
  `/etc/qemu/bridge.conf` (INSTALL.md section 9).
- **VLAN:** a tag on a bridged card (`vmbr1:20`). The bridge needs VLAN filtering and the service CAP_NET_ADMIN.

## Distributed switches

A distributed switch is a guest network that exists under the same name on several servers and is joined between them
by a VXLAN tunnel. You define it **once** (name, VNI, MTU and the tunnel addresses of **all** member servers), send it
to the peers, and select its name as the network of a guest, wherever that guest runs.

```
redox switch set vds-web 100 10.0.0.1,10.0.0.2,10.0.0.3          # on one server
redox switch push vds-web                                       # send the same definition to every peer
redox switch list                                               # local state and comparison with the peers
redox switch remove vds-web --everywhere
```

The *Cluster* page has the same in a **Distributed switches** card.

* The name must start with `vds`. The definition is identical on every server; each one recognises **its own** address
  among the members and uses it as the local end of the tunnel.
* The management service stays unprivileged: it only writes the definitions to `<data-dir>/vswitch.json`. The network
  is built by `redox-net.service` (root, `CAP_NET_ADMIN`), which `redox-net.path` starts whenever the file changes and
  which also runs at boot. `install.sh` sets both up.
* The helper does not trust the file: every field is validated again, commands run without a shell, and it only touches
  devices **it created itself** (marked with `ifalias`). A bridge such as `vmbr0` can never be joined to a tunnel, even
  by a compromised management service. It also maintains the list `qemu-bridge-helper` reads
  (`/etc/qemu/redox-vds.conf`).
* To see what it would do: `redox net sync /var/lib/redox/vswitch.json --dry-run`; to see why it failed:
  `journalctl -u redox-net`.
* A peer's definition that differs is shown as *definition differs*; it is never overwritten until you send yours.
* A switch cannot be removed while a guest on that server uses it.

**Limits.** The tunnel is **not encrypted** (UDP 4789): use it only on a network you trust, or run it inside an
encrypted one such as WireGuard. Broadcasts are copied to every member, so it is meant for a handful of servers (at
most 16). The MTU must leave 50 bytes for VXLAN: 1450 on an ordinary 1500-byte network, and guests must use it too.
There is one switch per VNI and no per-port policies; VLAN tags and rate limits are still set on each guest.

## A network shared between servers (VXLAN)

RedoX VE does not create network devices; guests attach to bridges you set up. To put guests on several servers in
the same network you can join the bridges with a VXLAN tunnel. RedoX writes the commands and you run them as root on
each server:

```
redox net vxlan vmbr100 100 10.0.0.1 10.0.0.2,10.0.0.3     # on server 1 (local address, then the peers)
redox net vxlan vmbr100 100 10.0.0.2 10.0.0.1,10.0.0.3     # on server 2
```

After running the printed `ip` commands, pick `vmbr100` as the bridge in guests. The tunnel takes 50 bytes: use MTU
1450 in guests or raise the MTU between the servers. The commands do not survive a reboot; to make them
permanent add `--unit` to the same command: the output is a ready systemd unit that builds the tunnel at every boot
(save it as `/etc/systemd/system/redox-vxlan-100.service` and turn it on with `systemctl enable --now`). The tunnel is not encrypted: over an untrusted network, run it inside an encrypted
link such as WireGuard.

**Tested:** the generated commands were run with real `iproute2` between two isolated network namespaces and ping
passed through the tunnel. Not tried on two physical servers or with running guests. There is no centrally managed
SDN (zones, virtual network definitions, automatic rollout).

## Guest firewall

Start the service with `--firewall` (nftables). With **Restrict incoming traffic** on in the guest's *Options* tab only
incoming connections matching your rules pass:

```
tcp/22
tcp/80-443
udp/53 from 192.168.1.0/24
icmp
```

Effective on bridged guests only.

## NIC protection

`redox vm set 103 nic-guard=true`. The guest can only talk with its own MAC address and cannot send DHCP replies or IPv6
router advertisements. Works on bridged cards and needs `--firewall`.

> Rule generation for the firewall and NIC protection is tested; they have **not** been tried under traffic on a real bridge.

## The panel itself

- Do not expose the panel directly: put it behind a TLS-terminating proxy, and add a VPN or IP restriction if you can.
- Every administrator should turn on two-step verification (*My Account*).
- Every state-changing request is written to the audit log (*Audit Log*).
- The interface runs no JavaScript (content security policy `script-src 'none'`). Three pages are exceptions: the live
  console, the container terminal and the security key step; they load only the server's own script.

## Outbound rules, security groups, address sets

```bash
redox firewall set save office 10.0.0.0/24,192.168.1.5
redox firewall group save web in='tcp/443,tcp/22 from @office' out='udp/53'
redox vm set 103 firewall=true fw-groups=web fw-out-restrict=true fw-out='tcp/443 to @office'
```

Definitions are not sent to peer servers automatically.

## Server networking from the panel

```bash
redox hostnet guest vnet1 10.10.0.1/24 dhcp=10.10.0.50-10.10.0.200 nat=true
redox hostnet bond bond0 active-backup enp2s0,enp3s0
redox hostnet vlan bond0 20
redox hostnet bridge vmbr1 ports=bond0.20 vlan-aware=true
```

`redox-hostnet.service` builds the network (DHCP needs `dnsmasq-base`). The server's management card cannot be moved into a bridge from the panel. To check by hand: `redox net apply /var/lib/redox/hostnet.json --dry-run`.

## TLS and LDAP

```bash
redox tls self-signed server.example.com --dir /var/lib/redox/tls
redox serve ... --tls-cert /var/lib/redox/tls/tls.crt --tls-key /var/lib/redox/tls/tls.key
```

For Let's Encrypt give certbot `deploy/certbot-redox-hook.sh` as `--deploy-hook`. LDAP settings file:

```
url = ldaps://dc.example.com
bind = uid={user},ou=people,dc=example,dc=com
```

Try it with `redox ldap test ldap.conf ali`, start the server with `--ldap ldap.conf`, and mark the user with `redox user ldap ali on --users …`. At least one administrator keeps a local password.

## Syslog and Let's Encrypt

`--syslog tls://host:6514` sends a copy of the audit records elsewhere. `redox acme issue ... --agree-tos` obtains a
Let's Encrypt certificate and `redox acme renew` renews it. Neither has been tried against the real external
services yet; see `INSTALL.md`, section 15.
