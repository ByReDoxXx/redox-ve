[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Networking and security

## NAT and bridges

- **NAT (default):** the guest reaches the internet and is not visible from outside. For inbound access use port
  forwarding (`server-port:guest-port`, server port 1024 or higher).
- **Bridge:** the guest gets an address from the server's network. The bridge must exist on the server and be allowed in
  `/etc/qemu/bridge.conf` (INSTALL.md section 9).
- **VLAN:** a tag on a bridged card (`vmbr1:20`). The bridge needs VLAN filtering and the service CAP_NET_ADMIN.

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
