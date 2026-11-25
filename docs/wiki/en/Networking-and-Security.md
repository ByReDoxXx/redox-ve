[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Networking and security

## NAT and bridges

- **NAT (default):** the guest reaches the internet and is not visible from outside. For inbound access use port
  forwarding (`server-port:guest-port`, server port 1024 or higher).
- **Bridge:** the guest gets an address from the server's network. The bridge must exist on the server and be allowed in
  `/etc/qemu/bridge.conf` (INSTALL.md section 9).
- **VLAN:** a tag on a bridged card (`vmbr1:20`). The bridge needs VLAN filtering and the service CAP_NET_ADMIN.

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
- The interface runs no JavaScript (content security policy `script-src 'none'`; the live console page is the exception).
