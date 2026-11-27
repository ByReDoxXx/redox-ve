[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Kullanıcılar ve yetkiler

## Roller

| Rol | Yapabildikleri |
|---|---|
| Yönetici | Her şey. |
| Operatör | Kapsamındaki (etiketlerine göre) konuklarda güç, konsol, anlık görüntü ve yedek. |
| İzleyici | Yalnızca okuma. |

Kullanıcılar sunucuda `redox user add <ad> --users DOSYA` ile ya da *Kullanıcılar* sayfasındaki **Kullanıcı Ekle** düğmesiyle oluşturulur.

*Kullanıcılar* sayfasında her satırın sonunda bir **Yönet** menüsü vardır: oturumları kapatmak ve ikinci adımı sıfırlamak
menüden tek tıkla yapılır; rol, operatör yetkileri ve kapsamı, parola sıfırlama, API token'ları ve silme o kullanıcının
kendi sayfasındadır (menüden ya da kullanıcının adına tıklayarak açılır).

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
onaylayınca yürür. Birini **yönetici yapmak** da onay ister (panelden eklenen yeni yönetici, onaylanana kadar izleyici
olarak durur); yoksa tek bir yönetici kendine ikinci bir hesap açıp kendi isteğini onaylayabilirdi. Aynı nedenle bu
ilke açıkken bir yönetici, **başka bir yöneticinin** parolasını ya da ikinci adımını panelden sıfırlayamaz, onun adına
token üretemez; kurtarma yolu sunucudaki `redox user passwd`, `redox user totp-reset` komutlarıdır.

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

## Güvenlik anahtarı (WebAuthn)

Sunucu `--webauthn-origin https://panelin-adresi` ile çalışıyorsa her kullanıcı *Hesabım* sayfasından anahtar
ekleyebilir. Girişte parolaya ek olarak anahtar (ya da açıksa uygulama kodu) istenir. Kaybolan anahtar için yönetici
*Kullanıcılar* sayfasından ikinci adımı sıfırlar ya da sunucuda `redox user keys-reset <ad>` çalıştırır.

## Hesap sayfası ve token'lar

Hesap sayfası (parola, ikinci adım, token üretme) yalnızca tarayıcıda oturum açarak kullanılır; API token'ıyla
yapılan istekler reddedilir. Token üretmek, ikinci adımı kurmak ya da kapatmak ve güvenlik anahtarı eklemek parolanızı
yeniden ister: açık bırakılmış bir oturumla bunlar yapılamaz. Bu formlarda art arda beş hatalı parola, hesabın bu
işlemlerini 15 dakika kilitler.

## Token süresi, oturumlar ve operatör yetkileri

- Token oluştururken **Geçerlilik** seçilebilir (`--days N`).
- *Hesabım* sayfasında açık oturumlar görülür ve kapatılır; yönetici *Kullanıcılar* sayfasından bir kullanıcının tüm
  oturumlarını kapatabilir.
- Panelden operatör eklerken ya da bir hesabı operatör yaparken yetkiler ve kapsam aynı formda seçilir (kutular boş
  gelir, en az biri işaretlenmelidir): hesap, sonradan daraltılana kadar her konukta her şeye yetkili kalmaz.
- Operatör, güç / konsol / anlık görüntü / yedek yetkilerinin yalnızca bir kısmıyla sınırlandırılabilir
  (`redox user permits <ad> backup`).
