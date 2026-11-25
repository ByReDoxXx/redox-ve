[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Kurulum

Ayrıntılı, adım adım kılavuz deponun kökündeki [KURULUM.md](../../../KURULUM.md) dosyasıdır. Bu sayfa özetidir.

## Gerekenler

- 64 bit Linux sunucu (Debian 12 / Ubuntu 22.04 ve sonrası), KVM desteği (`/dev/kvm`).
- `qemu-system-x86`, `qemu-utils`; UEFI için `ovmf`, sanal TPM için `swtpm`, konteynerler için `lxc`.
- Derlemek için Rust (stable).

## Hızlı başlangıç

```bash
cargo build --release
sudo install -m 755 target/release/redox /usr/local/bin/redox

# İlk yönetici (parola sorulur, en az 12 karakter)
redox user add admin --users /etc/redox/users.json

# Hizmet
redox serve --addr 127.0.0.1:8006 --users /etc/redox/users.json \
  --driver qemu --data-dir /var/lib/redox --state /var/lib/redox/state.json
```

Arayüz `http://127.0.0.1:8006` adresindedir. Dışarı açmak için önüne TLS sonlandıran bir vekil (Caddy) koyun ve
`--behind-tls-proxy` ekleyin; ayrıntı KURULUM.md 5. bölümde.

## Sık kullanılan hizmet seçenekleri

| Seçenek | Ne yapar |
|---|---|
| `--driver qemu` | Gerçek sürücü. Verilmezse simülasyon çalışır (deneme için). |
| `--state DOSYA` | Durumu kalıcı kılar. Verilmezse her şey bellekte kalır. |
| `--storage ad=/yol` | Ek depolama alanı. |
| `--offsite DOSYA`, `--offsite-copy DOSYA` | Yedeklerin dış hedefleri ([Yedekleme](Yedekleme.md)). |
| `--incremental-backups` | Artımlı (parçalı) yedek. |
| `--peers DOSYA` | Eş sunucular ([Birden çok sunucu](Birden-Cok-Sunucu.md)). |
| `--firewall` | Konuk güvenlik duvarı ve ağ kartı koruması (nftables, CAP_NET_ADMIN). |
| `--four-eyes` | Silme işlemleri ikinci yöneticinin onayını bekler. |
| `--lang en` | Çerezi olmayan ziyaretçiye İngilizce arayüz ([Dil](Dil.md)). |
| `--alert-webhook`, `--alert-email`, `--alert-ntfy`, `--alert-telegram` | Uyarı kanalları. |
| `--backup-report 07:30`, `--backup-age-days 7`, `--backup-test-every 7@03:00` | Yedek raporu, korumasız konuk eşiği, zamanlanmış sınama. |
| `--api-rate 600` | API hız sınırı (kullanıcı başına dakikada). |
