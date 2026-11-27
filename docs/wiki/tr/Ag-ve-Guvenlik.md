[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Ağ ve güvenlik

## NAT ve köprü

- **NAT (varsayılan):** konuk internete çıkar, dışarıdan görünmez. Dışarıdan erişim için port yönlendirme
  (`sunucu-portu:konuk-portu`, sunucu portu 1024 ve üzeri).
- **Köprü:** konuk sunucunun ağından adres alır. Köprü sunucuda hazır olmalı ve `/etc/qemu/bridge.conf` içinde izinli
  olmalıdır (KURULUM.md 9. bölüm).
- **VLAN:** köprüdeki karta etiket (`vmbr1:20`). Köprüde VLAN süzme açık ve hizmette CAP_NET_ADMIN olmalıdır.

## Dağıtık anahtarlar

Dağıtık anahtar, birden çok sunucuda aynı adla bulunan ve aralarında VXLAN tüneliyle birleşen bir konuk ağıdır.
**Bir kez** tanımlarsınız (ad, VNI, MTU ve **bütün** üye sunucuların tünel adresleri), eşlere gönderirsiniz ve konuk
hangi sunucuda çalışırsa çalışsın ağ olarak bu adı seçersiniz.

```
redox switch set vds-web 100 10.0.0.1,10.0.0.2,10.0.0.3          # bir sunucuda
redox switch push vds-web                                       # aynı tanımı bütün eşlere gönder
redox switch list                                               # bu sunucudaki durum ve eşlerle karşılaştırma
redox switch remove vds-web --everywhere
```

Aynı işlemler *Ağ* sayfasındaki **Dağıtık anahtarlar** kartındadır.

* Ad `vds` ile başlamalıdır. Tanım her sunucuda birebir aynıdır; her sunucu üyeler arasından **kendi** adresini tanır
  ve tünelin yerel ucu olarak onu kullanır.
* Yönetim hizmeti yetkisiz kalır: tanımları yalnızca `<veri-dizini>/vswitch.json` dosyasına yazar. Ağı
  `redox-net.service` kurar (root, `CAP_NET_ADMIN`); `redox-net.path` dosya her değiştiğinde onu başlatır, açılışta da
  çalışır. İkisini de `install.sh` kurar.
* Yardımcı dosyaya güvenmez: her alanı yeniden doğrular, komutları kabuksuz çalıştırır ve yalnızca **kendi kurduğu**
  (`ifalias` ile işaretli) aygıtlara dokunur. `vmbr0` gibi bir köprü, yönetim hizmeti ele geçirilse bile tünele
  bağlanamaz. `qemu-bridge-helper`'ın okuduğu listeyi de (`/etc/qemu/redox-vds.conf`) o yazar.
* Ne yapacağını görmek için: `redox net sync /var/lib/redox/vswitch.json --dry-run`; neden başarısız olduğunu görmek
  için: `journalctl -u redox-net`.
* Eşteki tanım farklıysa *tanımı farklı* görünür; siz göndermedikçe üzerine yazılmaz.
* O sunucuda bir konuk kullanırken anahtar silinemez.

**Sınırlar.** Tünel **şifrelenmez** (UDP 4789): yalnızca güvendiğiniz bir ağda kullanın ya da WireGuard gibi şifreli
bir ağın içinden geçirin. Yayın trafiği her üyeye kopyalanır; birkaç sunucu içindir (en çok 16). MTU, VXLAN'a 50 bayt
bırakmalıdır: olağan 1500 baytlık ağda 1450; konuklar da bunu kullanmalıdır. VNI başına bir anahtar vardır, kapı
başına ilke yoktur; VLAN etiketi ve hız sınırı yine konuk başına ayarlanır.

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
değildir; kalıcı olması için aynı komutun sonuna `--unit` yazın: çıktı, her açılışta tüneli kuran hazır bir systemd
birimidir (`/etc/systemd/system/redox-vxlan-100.service` olarak kaydedip `systemctl enable --now` ile açın). Tünel şifrelenmez: güvenilmeyen
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
- Arayüz JavaScript çalıştırmaz (içerik güvenlik ilkesi `script-src 'none'`). Üç sayfa istisnadır: canlı konsol,
  konteyner terminali ve güvenlik anahtarı adımı; bunlar yalnızca sunucunun kendi betiğini yükler.

## Giden kurallar, güvenlik grupları, adres kümeleri

```bash
redox firewall set save ofis 10.0.0.0/24,192.168.1.5
redox firewall group save web in='tcp/443,tcp/22 from @ofis' out='udp/53'
redox vm set 103 firewall=true fw-groups=web fw-out-restrict=true fw-out='tcp/443 to @ofis'
```

Tanımlar eş sunuculara kendiliğinden gönderilmez.

## Panelden sunucu ağı

```bash
redox hostnet guest vnet1 10.10.0.1/24 dhcp=10.10.0.50-10.10.0.200 nat=true
redox hostnet bond bond0 active-backup enp2s0,enp3s0
redox hostnet vlan bond0 20
redox hostnet bridge vmbr1 ports=bond0.20 vlan-aware=true
```

Aynı işlemler *Ağ* sayfasındaki **Sunucu ağı** kartındadır. Ağı `redox-hostnet.service` kurar (DHCP için `dnsmasq-base`). Sunucunun yönetim kartı panelden köprüye taşınamaz. Elle denemek için: `redox net apply /var/lib/redox/hostnet.json --dry-run`.

## TLS ve LDAP

```bash
redox tls self-signed sunucu.ornek.com --dir /var/lib/redox/tls
redox serve ... --tls-cert /var/lib/redox/tls/tls.crt --tls-key /var/lib/redox/tls/tls.key
```

Let's Encrypt için `deploy/certbot-redox-hook.sh` dosyasını certbot'a `--deploy-hook` olarak verin. LDAP ayar dosyası:

```
url = ldaps://dc.ornek.com
bind = uid={user},ou=people,dc=ornek,dc=com
```

`redox ldap test ayar.conf ali` ile deneyin, sunucuyu `--ldap ayar.conf` ile başlatın, `redox user ldap ali on --users …` ile kullanıcıyı işaretleyin. En az bir yönetici yerel parolayla kalır.

## Syslog ve Let's Encrypt

`--syslog tls://ad:6514` denetim kayıtlarının kopyasını dışarı gönderir. `redox acme issue ... --agree-tos`
Let's Encrypt sertifikası alır, `redox acme renew` yeniler. İkisi de gerçek dış hizmetlerle henüz denenmedi;
ayrıntılar `KURULUM.md`, 15. bölüm.
