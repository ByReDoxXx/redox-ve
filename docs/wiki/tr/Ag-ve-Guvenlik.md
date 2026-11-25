[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Ağ ve güvenlik

## NAT ve köprü

- **NAT (varsayılan):** konuk internete çıkar, dışarıdan görünmez. Dışarıdan erişim için port yönlendirme
  (`sunucu-portu:konuk-portu`, sunucu portu 1024 ve üzeri).
- **Köprü:** konuk sunucunun ağından adres alır. Köprü sunucuda hazır olmalı ve `/etc/qemu/bridge.conf` içinde izinli
  olmalıdır (KURULUM.md 9. bölüm).
- **VLAN:** köprüdeki karta etiket (`vmbr1:20`). Köprüde VLAN süzme açık ve hizmette CAP_NET_ADMIN olmalıdır.

## Sunucular arası ortak ağ (VXLAN)

RedoX VE ağ aygıtı oluşturmaz; konuklar sizin kurduğunuz köprülere bağlanır. Birkaç sunucudaki konukların aynı ağda
olması için köprüleri VXLAN tüneliyle birleştirebilirsiniz. Komutları RedoX yazar, siz her sunucuda root olarak
çalıştırırsınız:

```
redox net vxlan vmbr100 100 10.0.0.1 10.0.0.2,10.0.0.3     # 1. sunucuda (yerel adres, sonra eşler)
redox net vxlan vmbr100 100 10.0.0.2 10.0.0.1,10.0.0.3     # 2. sunucuda
```

Çıktıdaki `ip` komutlarını çalıştırdıktan sonra konuklarda köprü olarak `vmbr100` seçilir. Tünel 50 bayt yer kaplar:
konuklarda MTU 1450 kullanın ya da sunucular arası ağda MTU'yu yükseltin. Komutlar yeniden başlatmada kalıcı
değildir; kalıcı olması için bir systemd birimine ya da ağ yapılandırmanıza ekleyin. Tünel şifrelenmez: güvenilmeyen
ağ üzerinden geçiyorsa WireGuard gibi şifreli bir bağlantının içinden geçirin.

**Sınandı:** üretilen komutlar gerçek `iproute2` ile iki yalıtılmış ağ alanı arasında çalıştırıldı ve tünelden
ping geçti. İki ayrı fiziksel sunucuda ve çalışan konuklarla denenmedi. Merkezi yönetilen bir SDN (bölgeler, sanal ağ
tanımları, otomatik dağıtım) yoktur.

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
