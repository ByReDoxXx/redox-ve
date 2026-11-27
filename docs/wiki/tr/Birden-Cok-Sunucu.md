[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Birden çok sunucu

RedoX VE'de ortak küme durumu yoktur: her sunucu kendi konuklarını yönetir. Sunucuları birbirine **eş** olarak
tanıtırsınız (`--peers DOSYA`, her satır `AD ADRES TOKEN`); *Küme* sayfası hepsini birlikte gösterir.

## Taşıma

*Küme* sayfasında eşteki konuğun yanındaki düğmeler:

- **Buraya Al:** kapalı sanal makineyi yedeği üzerinden bu sunucuya kopyalar. Kaynak yerinde kalır; çalıştığını
  görünce orada siz silersiniz.
- **Çalışırken Al:** çalışan konuğu orada askıya alır, diskini ve belleğini kopyalar, burada kaldığı yerden sürdürür.
  Konuk yeniden başlamaz ama kopyalama boyunca yanıt vermez.

- **Konteynerler:** *Buraya Al* konteyneri yedeği üzerinden kopyalar. Bu sunucuda aynı dağıtımın şablonundan boş bir
  konteyner oluşturulur, sonra kök dosya sistemi yedekle değiştirilir. Kaynak çalışmaya devam eder (yedek sırasında
  kısa süre dondurulur); kopya kapalı gelir. Bu sunucuda LXC kurulu olmalıdır. **Sahte LXC araçlarıyla sınandı; gerçek
  LXC ile denenmedi.**

**Canlı taşıma:** çalışan sanal makine **kapatılmadan** da taşınır (*Küme* sayfasında *Canlı Taşı* ya da hedefte `redox peer migrate <eş> <kimlik>`). Hedef, konuğun QEMU'sunu duraklatılmış ve bekler halde açar; kaynak, konuk çalışmayı sürdürürken her diski hedefe aynalar, sonra belleği taşır. Konuk yalnızca son bellek sayfaları için duraklar (testlerde onlarca milisaniye) ve hedefte aynı kimlikle sürer. Konuğun kaydı ve diskleri kaynakta kalır: kapalıdır ve kendiliğinden yeniden açılmayacak biçimde işaretlenir; her şey yolundaysa orada silersiniz.

İki QEMU birbirine doğrudan bağlanır: kaynak, hedefe `--migrate-ports` ile verilen kapılardan ulaşabilmelidir (varsayılan 49152-49215, taşıma başına iki kapı; yalnızca sunucular arasında açın). Kaynağın hedefi gördüğü adres isteğin geldiği adresten farklıysa hedefte `--migrate-host` verin. İki bağlantı da yönetim bağlantısı üzerinden iletilen tek kullanımlık bir anahtarla şifrelenir (ön paylaşımlı anahtarlı TLS); bu yüzden sunucular arasında `https` kullanın.

Her konuk canlı taşınamaz: UEFI ya da sanal TPM, takılı kurulum kalıbı, cloud-init sürücüsü, paylaşılan klasör, aktarılmış sunucu aygıtı ve bir şablona ya da yedeğe bağlı disk taşımayı engeller (ileti hangisi olduğunu söyler); aynı kimlik hedefte boş olmalı, iki sunucuda aynı QEMU sürümü ve uyumlu bir işlemci modeli bulunmalıdır. Gerçek QEMU 10.2 ile, bir Linux konukla, tek makinede (iki sunucu, yerel ağ arayüzü) sınandı; iki fiziksel makine arasında, Windows konukla ya da büyük ve yoğun konuklarla denenmedi.

**Sunucuyu bakım için boşaltma:** `redox maintenance on --evacuate <eş>` (ya da *Küme* sayfasında eşin yanındaki *Konuklarımı Buraya Boşalt*) bu sunucuyu bakım kipine alır ve çalışan sanal makinelerini teker teker, kapatmadan eşe taşır. Canlı taşınamayan konuklar yerinde kalır; onları kapatmak için `--drain` ekleyin (bakım bitince yeniden başlatılırlar). Taşımayı eş yürüttüğü için eşin kendi `--peers` dosyasında bu sunucu tanımlı olmalıdır; adresinden tanınamıyorsa oradaki adını `--as` ile verin. Taşınan konuklar bakım bitince eşte çalışmayı sürdürür. Birini geri getirmek için bu sunucudan *Canlı Taşı* kullanın: konuk aynı kimlikle döner ve geride kalan kapalı kopyanın yerini alır (yedekleri ona bağlı kalır). Elle başlatılmış bir kopya ise kendi başına bir konuk sayılır ve üzerine yazılmaz. İki sunucu arasında sahte QEMU ile sınandı; alttaki taşıma, gerçek QEMU'da sınananla aynıdır.

## Yük dengeleme önerisi

Bu sunucu sıkıştığında (bellek ya da işlemci %80 ve üstü) *Küme* sayfasında **Yük dengeleme önerisi** kartı görünür;
`redox balance` aynı şeyi yazar: çalışan hangi sanal makinenin hangi eşe canlı taşınmasının işe yarayacağı ve
taşımadan sonra iki taraftaki bellek kullanımı. Kendiliğinden bir şey taşınmaz; her satırda bir *Gönder* düğmesi
vardır (`redox peer send <eş> <kimlik>`), o sanal makineyi bu sunucudan eşe canlı taşır.

Öneri yalnızca canlı taşınabilen çalışan sanal makineleri kapsar, taşımadan sonra kendisi %70 belleğin üstüne
çıkacak ya da aynı kimliği zaten kullanan bir eşi önermez ve en çok üç taşıma listeler. Eşlerin son okunan
durumundan hesaplanır; bu yüzden taşımaları birer birer uygulayın.

## Sunucu profili

Profil, bir sunucunun yönetim ayarlarının bir dosyadaki halidir; birkaç sunucuyu aynı düzende tutmak içindir:

```
redox profile export > profil.json      # örnek alınan sunucuda
redox profile diff profil.json          # başka bir sunucuda: ne farklı?
redox profile apply profil.json         # havuzları ve yedek işlerini eşitle
```

İki bölümü vardır. **Seçenekler**, hizmetin başlatma seçenekleri ve sunucunun yapabildikleridir (sürücü, güvenlik
duvarı, UEFI, dört göz, depolama alanları, taşıma kapıları vb.); hizmet çalışırken değişmezler, bu yüzden yalnızca
karşılaştırılır ve fark birim dosyasında elle düzeltilir. **Havuzlar** ve **yedek işleri** durumda tutulur ve
uygulanır: profildekiler eklenir ya da güncellenir, burada olup profilde olmayanlara dokunulmaz (listelenir). Profil
kullanıcıları, token'ları ve eş sunucu adreslerini taşımaz.

Kendiliğinden devralma için aşağıya bakın.

## Eşzamanlı disk yansıtma

Yansıtma açıkken **çalışan sanal makinenin her disk yazması eş sunucudaki bir kopyaya da gider** ve ikisi de
almadan yazma tamamlanmaz. Sanal makinenin çalıştığı sunucu ölürse, konuğa "yazıldı" denen her şey kopyadadır.
(Aşağıdaki çoğaltma aralıklarla kopyalar: son turdan beri yazılanlar kaybolur.)

```
redox mirror add 103 ankara                 # 103'ün çalıştığı sunucuda
redox mirror list                           # eşitleniyor, eşit, koptu (nedeniyle)
redox mirror off 103 [--delete-copy]

redox mirror copies                         # eşte: tuttuğu kopyalar
redox mirror promote <kaynak> 103           # devralma: kopyadan KAPALI bir konuk oluşturur
redox mirror delete-copy <kaynak> 103
```

Aynı işlemler *Küme* sayfasındaki **Disk yansıtma** ve **Tutulan kopyalar** kartlarındadır.

**Bir şey bozulunca ne olur**

* **Eş yanıt vermez olursa** (elektrik kesintisi, kopan bağlantı). Yazmalar eşzamanlı olduğu için konuğun disk
  yazmaları bekler. Yalnızca yerel hipervizöre soran bir bekçi, takılan ilerlemeyi 8 saniye sonra görür, bağlantıyı
  keser ve bekleyen yazmaların tamamlanmasını sağlar. Konuk çalışmayı sürdürür; yansıtma *koptu* görünür ve kopya o
  andan sonra geridedir.
* **Kopmadan sonra** dakikada bir yeniden denenir. Yeni eşitleme dosyaların **yeni bir kuşağına** yapılır; son
  eksiksiz kopya, yenisi tamamlanana kadar durur. Eşte **iki kat disk alanı** hesaplayın.
* **Kaynak sunucu ölürse.** Eşte yaklaşık 30 saniye bekleyin (kaynak hâlâ "eşit" bildirirken devralma reddedilir),
  konuğun orada gerçekten kapalı olduğunu doğrulayın, sonra devralıp yeni konuğu başlatın.

**Devralma bilerek elledir.** Eş, "kaynak kapandı" ile "aramızdaki bağlantı koptu" durumlarını ayıramaz; ikincisinde
konuk kaynakta çalışmayı sürdürür ve iki yerde birden başlatmak veriyi bozar. *Son eşit an*a bakın: yansıtma kaynak
ölmeden önce koptuysa kopya o kadar eskidir.

**Sınırlar**

* Konuk başına bir eşte bir kopya. Bu, çok sunuculu bir depolama havuzu değildir: veri dilimleme, kendiliğinden
  yeniden dengeleme ve ortak veri deposu yoktur.
* **Yedek, anlık görüntü ve askıya alma yansıtmayı duraklatır** (QEMU yansıtılan diskte bunlara izin vermez). Sonra
  **baştan** eşitlenir: her biri ağ üzerinden tam bir kopyaya mal olur ve bitene kadar eşte yalnızca eski kopya
  vardır. Dahili anlık görüntüler kopyaya girmez.
* Yansıtılan konuk canlı taşınamaz, depolaması da taşınamaz; önce yansıtmayı kapatın.
* Bekçi RedoX hizmetinin parçasıdır. Hizmet durdurulmuşken eş donarsa konuğun yazmaları, bağlantı zaman aşımına
  uğrayana kadar bekler.
* Birden çok diskli konukta bir diskin bağlantısı koparsa, yansıtma durmadan önce öbür diske birkaç yazma daha
  (en çok bir saniyelik) gidebilir; bu durumda kopyanın diskleri tam aynı andan olmayabilir.
* Yazma hızını sunucular arası ağ belirler. UEFI, sanal TPM, konteyner, şablon ve bağlı diskler desteklenmez.
* Kopya, eşin taşıma kapılarından (`--migrate-ports`) TLS ve tek kullanımlık anahtarla şifreli gider.

## Hata toleransı

Korunan sanal makinenin ikinci bir sunucuda **eşzamanlı çalışan bir ikizi** vardır (QEMU COLO). Sunucu ölürse konuk
**yeniden başlamadan ikizde sürer**: bellek, süreçler ve disk yerindedir.

**Üç sunucu** gerekir: birincil, ikiz sunucusu ve bir **hakem**. Birincilin `--peers` dosyasına ikiz sunucusunu ve
hakemi, ikiz sunucusununkine de hakemi **aynı adla** yazın. Konuğun köprüsü iki sunucuda da bulunmalıdır (dağıtık
anahtar bunu sağlar).

```
redox ft on 103 yedek hakem        # 103 kapalıyken: <kimlik> <ikiz sunucusu> <hakem>
redox vm start 103                 # ikiz kendiliğinden kurulur
redox ft list                      # hazırlanıyor, korunuyor, korumasız (nedeniyle) …
redox ft twins                     # ikiz sunucusunda
redox ft off 103
```

Aynı işlemler *Küme* sayfasındaki **Hata toleransı** kartındadır.

**Hangi konuk korunabilir.** **Tek diskli** ve **köprüye bağlı tek kartlı**, ek donanımı olmayan bir sanal makine:
çalışırken ekleme, balon, UEFI, TPM, aygıt aktarımı, paylaşılan klasör, ISO, cloud-init, VLAN etiketi, konuk güvenlik
duvarı ve hız sınırı olamaz. Konuk `q35` yerine eski **`pc` (i440fx) makine türüyle** açılır: QEMU 10.2'de `q35`
üzerinde COLO, ikizi ilk denetim noktasında çökertiyor. `q35` ile kurulmuş bir konuk, özellikle Windows, `pc`
üzerinde açılmak için sürücü değişikliği isteyebilir; güvenmeden önce bunu deneyin.

**Neyin sözü verilir.** **Ağda görünmüş** olan her şey korunur: bir paket birincilden ancak ikiz aynısını ürettiyse
ya da iki taraf aynı hale getirildikten (denetim noktası) sonra çıkar. İkiz konsolu ve ağ dışındaki girdileri görmez
ve denetim noktaları arasında (en çok yaklaşık 20 saniye) kendi başına çalışır. Son denetim noktasından beri ağdan
hiç dışarı çıkmamış iş **kaybolabilir**.

**Hiçbir zaman iki yerde birden değil.** İki sunucu birbirini kaybettiğinde her biri ötekini ölmüş sanabilir. Kararı
hakem verir: **ilk soran kazanır**, öbürü kendi kopyasını sonlandırır.

| Ne olursa | Sonuç |
|---|---|
| İkiz sunucusu ölür ya da donar | Konuğun ağı yaklaşık 5 saniye durur, sonra tek başına sürer; 2 dakika sonra yeniden korunmaya çalışılır |
| Birincil sunucu ölür | İkiz devralır; ağ yaklaşık 5 saniye durur |
| İkisinin arası kopar | İkisi de hakeme sorar; biri sürer, öbürü kopyasını sonlandırır |
| Birincil hakeme de ulaşamaz | Hakem yanıt verene kadar konuğu **duraklatır** |
| İkiz hakeme ulaşamaz | Beklemeyi sürdürür; hakemsiz asla devralmaz |

Durdurma, yeniden başlatma ve korumayı kapatma önce hakemin oyunu alır; böylece ikiz, bilerek durdurduğunuz bir
konuğu asla devralamaz. Devralmadan sonra eski birincil *yedek sunucu devraldı* gösterir ve o konuğu **başlatmayı
reddeder**. Konuğun öbür sunucuda kapalı olduğundan emin olduktan sonra, eski kaydı serbest bırakmak için orada hata
toleransını kapatın (`redox ft off`); diski güncel değildir.

**Sınırlar**

* Korunurken **yedek, anlık görüntü, askıya alma, duraklatma, taşıma ve disk işlemi yoktur**; zamanlanmış yedekler de
  buna dahildir (bu iletiyle başarısız olurlar). Yedek almak için korumayı kapatıp konuğu yeniden başlatın.
* Konuğun iki kopyada farklı çıkan ağ paketleri, denetim noktasına kadar yaklaşık **3 saniye** tutulur. Ağ ağırlıklı
  ya da gecikmeye duyarlı işler korumasız hallerinden çok daha yavaş olabilir. Başarım **ölçülmedi**.
* Devralmadan sonra yeni konuk **korumasızdır**; yeniden korumak için o sunucuda hata toleransını açın.
* Disk yazmaları ve bellek, TLS ve tek kullanımlık anahtarla gider. İki **paket karşılaştırma bağlantısı
  şifresizdir** ve konuğun ağ trafiğinin bir kopyasını taşır: ayrılmış, güvenilen bir ağ kullanın.
* İki sunucuda aynı işlemci ve aynı QEMU sürümü gerekir. Korunan her konuk iki tarafta da bellek kullanır.
* Birincildeki RedoX hizmeti durdurulmuşken ikiz donarsa konuğun ağı kapalı kalır.
* Linux konukla ve üç sunucu **tek fiziksel makinedeyken** denendi. Ayrı fiziksel sunucularda, Windows konukla, yük
  altında ve uzun süre denenmedi. Güvenmeden önce kendi düzeninizde sınayın.

## Çoğaltma (yedek kopya)

```bash
redox replica add <eş> <kimlik> 60     # 60 dakikada bir
redox replica list
redox replica run <eş> <kimlik>        # şimdi
redox replica remove <eş> <kimlik>
```

Eşteki sanal makinenin ya da konteynerin kopyası bu sunucuda düzenli tazelenir; kaynak çalışmaya devam eder. Kopya `<ad>-kopya` adıyla
kapalı durur. Kaynak sunucu ölürse kopyayı buradan **siz** başlatırsınız; kayıp, son turdan beri yazılanlardır.
Çalışan bir kopyanın üzerine yazılmaz. **Artımlı:** sanal makinelerde ilk turdan sonra yalnızca değişen 4 MiB'lik bloklar indirilir; değişmeyenler eski
kopyadan alınır ve indirilen her blok SHA-256 özetiyle doğrulanır. *Çoğaltma* kartı son turda kaç bloğun indiğini
gösterir. Kaynak sunucu yine de her turda diskin tamamını okur (özet çıkarmak için); kazanç ağdadır. Artımlı aktarım
bir nedenle başarısız olursa o disk kendiliğinden tam olarak indirilir. Konteynerler her turda tam kopyalanır.

## Bakım kipi

Sunucuyu güncellemeden ya da yeniden başlatmadan önce:

```bash
redox maintenance on --drain   # çalışan konukları sırayla kapatır
# ... güncelleme, yeniden başlatma ...
redox maintenance off          # kapatılanları açılış sırasıyla yeniden başlatır
```

Kipteyken hiçbir konuk başlatılmaz; zamanlanmış yedekler, güç işlemleri ve çoğaltma durur. Kip durum dosyasında
saklanır: sunucu yeniden başlasa da konuklar kendiliğinden açılmaz. Konukları başka sunucuya taşımaz.

## Kendiliğinden devralma (hakemli)

Çoğaltılan bir konuk için, kaynak sunucu ölünce yedek kopyanın kendiliğinden başlatılmasını isteyebilirsiniz:

```
redox replica failover ankara 103 10     # ankara'ya 10 dakika ulaşılamazsa
redox replica failover ankara 103 off
```

Arayüzde *Çoğaltma* kartındaki **Devralma** sütunundan da ayarlanır (0 kapatır; 5 ile 1440 dakika arası).

Bu sunucu, "kaynak öldü" ile "benim ağım koptu" durumlarını tek başına ayıramaz. Bu yüzden başlatmadan önce bir
**hakeme**, yani `--peers` dosyasında tanımlı başka bir RedoX sunucusuna sorar:

- Hiçbir hakeme ulaşılamıyorsa devralınmaz (kopan büyük olasılıkla bu sunucudur).
- Ulaşılan bir hakem kaynağı çalışır görüyorsa devralınmaz (kopan yalnızca aradaki bağlantıdır). Bunun için hakemin
  `--peers` dosyasında kaynak sunucu, aynı adresle ya da aynı adla tanımlı olmalıdır; hakem kaynağı hiç tanımıyorsa
  oyu yalnızca "ağ ayakta" anlamına gelir.
- Engel kalkana dek dakikada bir yeniden denenir; neden kartta **engellendi** etiketinin üstünde görünür.

Bir çoğaltma yalnızca bir kez devralınır. Kaynak geri gelince **yedek kopyayı ve kaynağı aynı anda çalışır
bırakmayın**: birini kapatın. Kopya kapatılıp bir çoğaltma turu başarıyla bitince devralma yeniden kurulur. Bakım
kipinde devralma yapılmaz.

**Sınırlar:** hakem de kaynak da bu sunucudan aynı anda kopmuş ama kaynak aslında çalışıyorsa hiçbir yöntem bunu
göremez; kaynağı zorla kapatma (fencing) yoktur. Konuğun adresi kopyaya kendiliğinden taşınmaz: kopya aynı ağdaysa aynı
adresle açılır, değilse DNS'i ya da yönlendirmeyi siz değiştirirsiniz. Sahte QEMU ve üç sunuculu testle sınandı;
gerçek bir kesintide denenmedi.

## Taşımada duraklamayı kısaltma (ön kopya)

Çalışan bir sanal makine taşınırken kaynakta askıya alınır, diski ve belleği kopyalanır, burada kaldığı yerden sürer;
konuk kopyalama boyunca duraklar. `redox serve … --move-precopy` ile (konuğu **alan** sunucuda) diskler önce konuk
çalışırken indirilir; askıya alındıktan sonra yalnızca o arada değişen 4 MiB'lik bloklar ve bellek durumu gelir.
Görevlerdeki `vm precopy eş/kimlik 3/250` satırı, duraklama sırasında 250 bloktan 3'ünün indirildiğini söyler.

Ne zaman işe yarar: sunucular arasındaki ağ disklerden yavaşsa (örneğin 1 Gbit ağ, SSD diskler). Kaynak sunucu
duraklama sırasında diski yine baştan sona okur (yedek ve özet için), bu yüzden çok hızlı ağda kazanç olmaz, hatta
süre uzayabilir; seçenek bu nedenle varsayılan olarak kapalıdır. Kaynak sunucu yedeklerini şifreli saklıyorsa ön
kopya yapılamaz ve taşıma olağan yoldan sürer.

Bu, canlı taşıma **değildir**: konuk yine duraklar, yalnızca daha kısa. Koşulları sağlayan konuklarda (yukarıdaki)
*Canlı Taşı* daha iyi seçimdir. Ön kopya sahte QEMU ile sınandı; duraklaması gerçek QEMU ile ölçülmedi.

## Devralmadan sonra geri dönüş

Kendiliğinden bir geri dönüş yoktur; adımları siz atarsınız. Kaynak sunucu (ankara) geri geldiğinde:

1. **Önce ankara'daki eski konuğun çalışmadığından emin olun.** Açılışta başlat işaretliyse kendiliğinden açılmış
   olabilir; aynı adresle iki kopya ağı karıştırır. Kapatın ve *açılışta başlat* işaretini kaldırın.
2. Ankara'da *Birleşik görünüm* sayfasından, bu sunucuda çalışan kopyayı **Buraya Al** ile geri taşıyın (ankara'nın
   `--peers` dosyasında bu sunucu tanımlı olmalıdır). Çalışan konuk bellek durumuyla taşınır; devralmadan beri
   yazılanlar korunur.
3. Taşınan konuğun çalıştığını gördükten sonra ankara'daki eski konuğu ve bu sunucuda askıda kalan kopyayı silin.
4. Bu sunucudaki çoğaltma kaydını kaldırıp yeni konuk kimliğiyle yeniden ekleyin; ilk tur bittiğinde devralma
   yeniden kurulur.

Bu sıra elle denenmedi; her adım tek tek sınanmış işlemlerden (taşıma, silme, çoğaltma) oluşur.
