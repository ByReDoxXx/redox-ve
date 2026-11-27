[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Kullanıcılar ve yetkiler

## Roller

| Rol | Yapabildikleri |
|---|---|
| Yönetici | Her şey. |
| Operatör | Kapsamındaki (etiketlerine göre) konuklarda güç, konsol, anlık görüntü ve yedek. |
| İzleyici | Yalnızca okuma. |

Kullanıcılar sunucuda `redox user add <ad> --users DOSYA` ile ya da *Kullanıcılar* sayfasından oluşturulur.

## API token'ları

```bash
redox token create admin betik --users /etc/redox/users.json
redox token create admin izleme --read-only --users /etc/redox/users.json
```

Token, sahibinin rolünü taşır; `--read-only` token yönetici hesabında bile yalnızca okuyabilir (izleme araçları için).
Değer bir kez gösterilir.

## İki adımlı doğrulama ve tek oturum açma

*Hesabım* sayfasından TOTP açılır. Telefon kaybolursa bir yönetici *Kullanıcılar* sayfasından ya da
`redox user totp-reset` ile sıfırlar. Kurumsal giriş için OpenID Connect desteklenir (KURULUM.md 13. bölüm; gerçek bir
sağlayıcıyla **denenmedi**).

## Dört göz ilkesi

`--four-eyes` ile yedek, konuk, ek disk, yedek işi ve kullanıcı silme hemen yapılmaz: **başka** bir yönetici
onaylayınca yürür.

```bash
redox approval list
redox approval approve <no>
redox approval reject <no>
```

İstek sahibi kendi isteğini onaylayamaz; 24 saatte onaylanmayan istek düşer. En az iki yönetici hesabı gerekir.
Zamanlayıcının saklama budaması bu kuralın dışındadır.

## Kilit kipi (değişiklik dondurma)

`redox lockdown on` (ya da düğüm sayfasındaki *Sunucuyu Kilitle*) sunucuyu kilit kipine alır: bir yönetici kilidi
kaldırana kadar web arayüzünden ve API'den **hiç kimse** değişiklik yapamaz; yöneticiler ve API token'ları dahil.
Konuklar çalışmayı, zamanlanmış yedekler, zamanlamalar ve kendiliğinden yeniden başlatma yürümeyi sürdürür. Okuma
çalışır; herkes oturumunu kapatabilir ve kendi hesabını yönetebilir. Denetim ve değişiklik dondurma dönemleri içindir;
hizmet yeniden başlasa da kilit durur.

Kilit kipi, sunucuda kabuk erişimi olan birine karşı bir güvenlik sınırı değildir: yalnızca yönetim arayüzünü kapatır.
Kilit açıkken eş sunuculardan gelen istekler de (kopyalama, çoğaltma, bu sunucudan canlı taşıma) reddedilir.
