[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Ağ ve güvenlik

## NAT ve köprü

- **NAT (varsayılan):** konuk internete çıkar, dışarıdan görünmez. Dışarıdan erişim için port yönlendirme
  (`sunucu-portu:konuk-portu`, sunucu portu 1024 ve üzeri).
- **Köprü:** konuk sunucunun ağından adres alır. Köprü sunucuda hazır olmalı ve `/etc/qemu/bridge.conf` içinde izinli
  olmalıdır (KURULUM.md 9. bölüm).
- **VLAN:** köprüdeki karta etiket (`vmbr1:20`). Köprüde VLAN süzme açık ve hizmette CAP_NET_ADMIN olmalıdır.

## Konuk güvenlik duvarı

Hizmeti `--firewall` ile başlatın (nftables). Konuğun *Seçenekler* sekmesinde **Gelen trafiği kısıtla** açıkken yalnızca
yazdığınız kurallara uyan gelen bağlantılar geçer:

```
tcp/22
tcp/80-443
udp/53 from 192.168.1.0/24
icmp
```

Yalnızca köprüdeki konuklarda etkilidir.

## Ağ kartı koruması

`redox vm set 103 nic-guard=true`. Konuk yalnızca kendi MAC adresiyle konuşabilir; ağa DHCP yanıtı ve IPv6 yönlendirici
duyurusu gönderemez. Köprüdeki kartlarda etkilidir ve `--firewall` ister.

> Güvenlik duvarı ve ağ kartı korumasının kural üretimi sınanmıştır; gerçek bir köprüde trafik altında **denenmemiştir**.

## Panelin kendisi

- Paneli doğrudan internete açmayın: TLS sonlandıran bir vekilin arkasına koyun, mümkünse VPN ya da IP kısıtı ekleyin.
- Her yönetici iki adımlı doğrulamayı açsın (*Hesabım*).
- Durum değiştiren her istek denetim günlüğüne yazılır (*Denetim Günlüğü*).
- Arayüz JavaScript çalıştırmaz (içerik güvenlik ilkesi `script-src 'none'`; yalnızca canlı konsol sayfası istisnadır).
