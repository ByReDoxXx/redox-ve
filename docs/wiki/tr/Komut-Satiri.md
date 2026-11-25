[Ana sayfa](Ana-Sayfa.md) · [Kurulum](Kurulum.md) · [Konuklar](Konuklar.md) · [Yedekleme](Yedekleme.md) · [Ağ ve güvenlik](Ag-ve-Guvenlik.md) · [Komut satırı](Komut-Satiri.md) · [Sorun giderme](Sorun-Giderme.md)

> English: [../en/Home.md](../en/Home.md)

# Komut satırı

Aynı `redox` dosyası hem sunucuyu çalıştırır hem istemcidir. İstemci için:

```bash
export REDOX_URL=https://redox.example.com
export REDOX_TOKEN=redox_...
redox help
```

## Sık kullanılanlar

```bash
redox cluster status
redox vm list
redox vm start 103            # stop, reboot, pause, resume
redox vm set 103 cores=4 memory=8192 nested=true
redox vm disk add 103 50
redox vm balloon 103 4096     # ya da: auto 2048
redox vm move 103 hizli
redox snapshot create 103 once [--production]
redox backup create 103
redox backup list
redox backup restore <yedek>
redox job add gece time=03:00 days=1-5 tags=web
redox pool add web cores=32 memory=65536 disk=500
redox replica add ankara 500 60
redox maintenance on --drain
redox approval list
redox media catalog
redox task list
redox usage
```

`redox vm set` anahtarları: `name cores memory os iso bridge onboot restart boot-order boot-delay schedules disk-mbps
disk-iops net-mbps vlan cpu balloon nested nic-guard firewall rpo test-port test-cmd backup-time backup-keep backup-pre
backup-post` ve diğerleri; tam liste `redox help` çıktısındadır.

İngilizce çıktı için `REDOX_LANG=en` ortam değişkenini verin ([Dil](Dil.md)).
