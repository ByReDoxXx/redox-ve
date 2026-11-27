//! Sürücünün **gerçek QEMU** ile sınanması. Öbür testler sahte QEMU kullanır; buradakiler yalnızca gerçek
//! ikililer ve bir Linux çekirdeği verildiğinde çalışır, verilmediğinde atlanır:
//!
//! ```text
//! REDOX_REAL_QEMU=/yol/qemu-system-x86_64  REDOX_REAL_QEMU_IMG=/yol/qemu-img
//! REDOX_REAL_KERNEL=/yol/vmlinuz           REDOX_REAL_INITRD=/yol/initrd.gz
//! ```
//!
//! Konuk ajanına bağlı testler ayrıca, içine ajan eklenmiş bir başlangıç dosya sistemi ister
//! (`REDOX_REAL_INITRD_AGENT`; içinde ajanı başlatan `/ga/start` bulunur).
//!
//! Konuk, çekirdeğin başlangıç dosya sistemindeki kabukla açılır (`rdinit=/bin/sh`) ve seri kapıdan sürülür;
//! böylece sürücünün yaptığı işin konuğun **içinden** nasıl göründüğü de doğrulanır.

use super::*;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

pub(super) struct Real {
    pub cfg: QemuConfig,
    pub dir: PathBuf,
    serial: PathBuf,
}

/// Gerçek ikililerle yapılandırma; ortam değişkenleri verilmemişse `None` (test atlanır).
pub(super) fn real() -> Option<Real> {
    real_from("REDOX_REAL_INITRD")
}

/// Gerçek ağlı konukla yapılandırma: sürücünün köprü ağı ucu, ad alanında hazır bekleyen TAP arayüzüne çevrilir
/// (`qemu-bridge-helper` kök yetkisi ister).
pub(super) fn real_net(tap: &str) -> Option<Real> {
    let r = real_from("REDOX_REAL_INITRD_NET")?;
    let wrapped = std::fs::read_to_string(&r.cfg.qemu_bin).unwrap();
    let rewrite = format!("#!/bin/sh\nn=$#; i=0\nwhile [ $i -lt $n ]; do a=\"$1\"; shift; case \"$a\" in bridge,id=net0,*) a='tap,id=net0,ifname={tap},script=no,downscript=no';; esac; set -- \"$@\" \"$a\"; i=$((i+1)); done\n");
    write_script(Path::new(&r.cfg.qemu_bin), &wrapped.replacen("#!/bin/sh\n", &rewrite, 1));
    Some(r)
}

/// Intel ağ sürücüleri eklenmiş konukla yapılandırma.
pub(super) fn real_compat() -> Option<Real> {
    real_from("REDOX_REAL_INITRD_COMPAT")
}

/// İçinde konuk ajanı bulunan konukla yapılandırma.
pub(super) fn real_agent() -> Option<Real> {
    real_from("REDOX_REAL_INITRD_AGENT")
}

fn real_from(initrd_var: &str) -> Option<Real> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| Path::new(v).is_file());
    let (Some(qemu), Some(img), Some(kernel), Some(initrd)) = (var("REDOX_REAL_QEMU"), var("REDOX_REAL_QEMU_IMG"), var("REDOX_REAL_KERNEL"), var(initrd_var)) else {
        eprintln!("ATLANDI: gerçek QEMU verilmedi (REDOX_REAL_QEMU, REDOX_REAL_QEMU_IMG, REDOX_REAL_KERNEL, {initrd_var})");
        return None;
    };
    let dir = PathBuf::from(format!("/tmp/rdx-g{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let serial = dir.join("seri");
    let mut cfg = QemuConfig::new(dir.join("data"));
    // Sürücünün kurduğu komut satırına yalnızca çekirdek ve seri kapı eklenir; gerisi olduğu gibi gerçek QEMU'ya gider.
    cfg.qemu_bin = script(
        &dir,
        "qemu",
        // Makinesiz "kopya tutucu" süreçlere çekirdek ve seri kapı eklenmez.
        &format!("[ \"$1 $2\" = '-machine none' ] && exec '{qemu}' \"$@\"\nrm -f '{serial}'\nprintf '%s\\n' \"$@\" > '{serial}.args'\nexec '{qemu}' \"$@\" -kernel '{kernel}' -initrd '{initrd}' -append 'console=ttyS0 rdinit=/bin/sh quiet' -serial 'unix:{serial},server=on,wait=off'", serial = serial.display()),
    );
    cfg.qemu_img_bin = img;
    cfg.kvm = std::fs::OpenOptions::new().read(true).write(true).open("/dev/kvm").is_ok();
    // Kabuk, kapanma isteğini (ACPI) dinlemez: kısa süre beklenip QEMU kapatılır.
    cfg.shutdown_timeout = Duration::from_secs(2);
    Some(Real { cfg, dir, serial })
}

impl Real {
    /// Konuğun kabuğunda bir komut çalıştırır ve çıktısının satırlarını döndürür.
    pub fn ask(&self, command: &str) -> Vec<String> {
        let mut sock = UnixStream::connect(&self.serial).expect("seri kapı");
        sock.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        // İşaret, komutun yankısında değil yalnızca çıktısında görünür (kabuk toplamayı yapar).
        sock.write_all(format!("{command}; echo BITTI$((40+2))\n").as_bytes()).unwrap();
        let (mut text, mut buf, deadline) = (String::new(), [0u8; 4096], std::time::Instant::now() + Duration::from_secs(30));
        while !text.contains("BITTI42") && std::time::Instant::now() < deadline {
            if let Ok(n) = sock.read(&mut buf) {
                text.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        }
        assert!(text.contains("BITTI42"), "konuk yanıt vermedi: {command}\n{text}");
        text.replace('\r', "").lines().map(str::trim).filter(|l| !l.is_empty() && !l.contains("BITTI") && !["~", "/", "#"].contains(l) && !l.starts_with("~ #") && !l.starts_with("/ #")).map(str::to_string).collect()
    }

    /// Kabuk açılana kadar bekler ve `/proc` ile `/sys` dosya sistemlerini bağlar.
    pub fn boot(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        // Açılmazsa nedenini gösterebilmek için seri kapıdan okunan son çıktı saklanır.
        let mut seen = String::new();
        loop {
            let mut text = String::new();
            if let Ok(mut sock) = UnixStream::connect(&self.serial) {
                sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                let _ = sock.write_all(b"echo HAZIR$((40+2))
");
                let (mut buf, until) = ([0u8; 4096], std::time::Instant::now() + Duration::from_secs(3));
                while !text.contains("HAZIR42") && std::time::Instant::now() < until {
                    match sock.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
                        Err(_) => {}
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(200));
            }
            if text.contains("HAZIR42") {
                break;
            }
            seen.push_str(&text);
            let tail: String = seen.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
            assert!(std::time::Instant::now() < deadline, "konuk açılmadı; seri kapıdan son okunan: {tail:?}");
        }
        self.ask("mount -t proc proc /proc 2>/dev/null; mount -t sysfs sys /sys 2>/dev/null");
    }

    /// Konuğun kullandığı bellek (kB, `/proc/meminfo`). Kendiliğinden etkinleşmemiş bloklar önce etkinleştirilir.
    pub fn mem_kb(&self) -> u64 {
        self.ask("for m in /sys/devices/system/memory/memory*/state; do grep -q offline $m && echo online > $m; done");
        let line = self.ask("grep MemTotal /proc/meminfo").pop().unwrap_or_default();
        line.split_whitespace().nth(1).and_then(|kb| kb.parse().ok()).unwrap_or_else(|| panic!("bellek okunamadı: {line}"))
    }

    /// Konuğun gördüğü `(takılı, etkin)` çekirdek aralıkları.
    pub fn cpus(&self) -> (String, String) {
        let read = |file: &str| self.ask(&format!("cat /sys/devices/system/cpu/{file}")).pop().unwrap_or_default();
        (read("present"), read("online"))
    }
}

/// Çekirdek ekleme, sürücünün kendi koduyla gerçek QEMU'da: konuk yeni çekirdeği görür, askıya alınıp aynı
/// çekirdek sayısıyla döner ve döndükten sonra bir çekirdek daha alır.
#[test]
pub(super) fn real_qemu_adds_cores_and_keeps_them_across_suspend() {
    let Some(real) = real() else { return };
    let d = QemuDriver::new(real.cfg.clone()).unwrap();
    let mut g = guest(&real.cfg, "gercek-cekirdek");
    (g.cores, g.mem_total_mb, g.hw.max_cores) = (2, 1024, 4);
    d.create(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();
    real.boot();
    assert_eq!(real.cpus(), ("0-1".to_string(), "0-1".to_string()));
    // Çekirdekten önce çalışırken takılan disk de askıdan dönüşte aynı aygıt olarak bulunmalı.
    d.add_disk(&g, 1, 1).unwrap();
    d.hot_add_disk(&g, 1).unwrap();
    g.hw.disks.push(redox_core::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 });

    d.hot_add_core(&g).unwrap();
    g.cores = 3;
    std::thread::sleep(Duration::from_secs(2));
    let (present, online) = real.cpus();
    assert_eq!(present, "0-2", "konuk yeni çekirdeği görmeli");
    assert_eq!(online, "0-1", "çekirdek takılı ama konukta etkinleştirilmemiş");
    assert!(!d.online_cores(&g), "konukta ajan yok: sürücü etkinleştiremediğini bildirmeli");
    real.ask("for c in /sys/devices/system/cpu/cpu[0-9]*/online; do echo 1 > $c; done");
    assert_eq!(real.cpus().1, "0-2");

    real.ask("echo isaret-7341 > /tmp/isaret");
    d.suspend(&g).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(real.ask("cat /tmp/isaret"), ["isaret-7341"], "bellek durumu korunmalı");
    assert_eq!(real.cpus(), ("0-2".to_string(), "0-2".to_string()), "konuk üç çekirdekle dönmeli");

    d.hot_add_core(&g).unwrap();
    g.cores = 4;
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(real.cpus().0, "0-3");
    assert!(d.hot_add_core(&g).unwrap_err().contains("boş çekirdek yuvası kalmadı"));

    d.apply(&g, Action::Stop).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    let _ = std::fs::remove_dir_all(&real.dir);
}

/// Bellek ekleme, sürücünün kendi koduyla gerçek QEMU'da: konuk eklenen belleği kullanır, askıya alınıp aynı
/// bellekle döner, döndükten sonra yeniden bellek alır; sıfırdan açılışta eklenen bellek açılış belleğidir.
#[test]
pub(super) fn real_qemu_adds_memory_and_keeps_it_across_suspend() {
    let Some(real) = real() else { return };
    let d = QemuDriver::new(real.cfg.clone()).unwrap();
    let mut g = guest(&real.cfg, "gercek-bellek");
    (g.cores, g.mem_total_mb, g.hw.max_cores, g.hw.max_mem_mb, g.hw.balloon) = (2, 1024, 4, 4096, true);
    let add = |g: &mut Guest, mb: u64| {
        d.hot_add_memory(g, mb).unwrap();
        g.mem_total_mb += mb;
        g.hw.dimms.push(mb);
        std::thread::sleep(Duration::from_secs(3));
    };
    d.create(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();
    real.boot();
    let first = real.mem_kb();

    add(&mut g, 512);
    assert_eq!(real.mem_kb(), first + 512 * 1024, "konuk eklenen belleğin tamamını kullanmalı");
    assert!(!d.online_memory(&g), "konukta ajan yok: sürücü doğrulayamadığını bildirmeli");
    // Bellek düğümü tanımlı makinede çekirdek yuvaları da çalışır.
    d.hot_add_core(&g).unwrap();
    g.cores = 3;
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(real.cpus().0, "0-2");
    add(&mut g, 256);
    assert_eq!(real.mem_kb(), first + 768 * 1024);
    let over = d.hot_add_memory(&g, 4096).unwrap_err();
    assert!(over.contains("bellek konuğa eklenemedi"), "{over}");

    real.ask("echo isaret-5512 > /tmp/isaret");
    d.suspend(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();
    assert_eq!(real.ask("cat /tmp/isaret"), ["isaret-5512"], "bellek durumu korunmalı");
    assert_eq!(real.mem_kb(), first + 768 * 1024, "konuk aynı bellekle dönmeli");
    assert_eq!(real.cpus().0, "0-2");
    add(&mut g, 128);
    assert_eq!(real.mem_kb(), first + 896 * 1024, "döndükten sonra da eklenir");
    d.apply(&g, Action::Stop).unwrap();

    // Sıfırdan açılış: parçalar açılış belleğine katılmıştır, yuvalar yeniden boştur.
    g.hw.dimms.clear();
    d.apply(&g, Action::Start).unwrap();
    real.boot();
    let cold = real.mem_kb();
    assert!(cold.abs_diff(first + 896 * 1024) < 64 * 1024, "sıfırdan açılan konuk toplam belleği görmeli: {cold} kB");
    add(&mut g, 128);
    assert_eq!(real.mem_kb(), cold + 128 * 1024);
    d.apply(&g, Action::Stop).unwrap();
    assert_eq!(status(&d, &g), GuestStatus::Stopped);
    let _ = std::fs::remove_dir_all(&real.dir);
}

/// Canlı taşıma, sürücünün kendi koduyla gerçek QEMU'da: aynı makinede iki veri dizini iki sunucunun yerini
/// tutar. Konuk taşıma boyunca diske yazar; hedefte belleği, diskleri ve sonradan eklenmiş çekirdeği ile
/// belleğiyle kaldığı yerden sürer. Yanlış anahtarla gelen taşıma reddedilir ve konuk kaynakta çalışmayı sürdürür.
#[test]
pub(super) fn real_qemu_migrates_a_running_guest_to_another_server() {
    let (Some(a), Some(b)) = (real(), real()) else { return };
    let (da, db) = (QemuDriver::new(a.cfg.clone()).unwrap(), QemuDriver::new(b.cfg.clone()).unwrap());
    let mut g = guest(&a.cfg, "gercek-tasima");
    (g.cores, g.mem_total_mb, g.hw.max_cores, g.hw.max_mem_mb, g.hw.balloon) = (2, 1024, 4, 2048, true);
    g.hw.disks.push(redox_core::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 });
    da.create(&g).unwrap();
    da.add_disk(&g, 1, 1).unwrap();
    assert_eq!(da.migrate_layout(&g), None, "kapalı konuğun yerleşimi sorulmaz");
    da.apply(&g, Action::Start).unwrap();
    a.boot();
    a.ask("depmod -a 2>/dev/null; modprobe virtio_pci 2>/dev/null; modprobe virtio_blk 2>/dev/null; mount -t devtmpfs dev /dev 2>/dev/null; sleep 1");
    assert_eq!(a.ask("ls /dev/vda /dev/vdb | wc -w"), ["2"], "konuk iki diskini de görmeli");
    // Çalışırken eklenen çekirdek ve bellek de konukla birlikte gider.
    da.hot_add_core(&g).unwrap();
    g.cores = 3;
    da.hot_add_memory(&g, 512).unwrap();
    g.mem_total_mb += 512;
    g.hw.dimms.push(512);
    std::thread::sleep(Duration::from_secs(3));
    let memory = a.mem_kb();
    a.ask("echo birinci-disk | dd of=/dev/vda bs=512 seek=10 conv=notrunc 2>/dev/null; echo ikinci-disk | dd of=/dev/vdb bs=512 seek=7 conv=notrunc 2>/dev/null; sync; echo isaret-4471 > /tmp/isaret");
    a.ask("(i=0; while true; do i=$((i+1)); echo sayac-$i | dd of=/dev/vda bs=512 seek=2000 conv=notrunc 2>/dev/null; echo $i > /tmp/sayac; done) >/dev/null 2>&1 & true");

    // Hedef: aynı kayıt, boş diskler.
    db.create(&g).unwrap();
    db.add_disk(&g, 1, 1).unwrap();
    let layout = da.migrate_layout(&g).expect("çalışan konuğun yerleşimi");
    let (nbd, ram) = (49170 + (std::process::id() % 20) as u16 * 2, 49171 + (std::process::id() % 20) as u16 * 2);
    let key = "5a".repeat(32);

    // Yanlış anahtar: hedef bağlantıyı kabul etmez; konuk kaynakta çalışmayı sürdürür, hedefte iz kalmaz.
    db.migrate_listen(&g, layout, "127.0.0.1", nbd, ram, &key).unwrap();
    assert_eq!(status(&db, &g), GuestStatus::Paused, "hedef taşımayı duraklatılmış bekler");
    let refused = da.migrate_send(&g, "127.0.0.1", nbd, ram, &"6b".repeat(32)).unwrap_err();
    assert!(refused.contains("bağlanılamadı"), "{refused}");
    assert_eq!(status(&da, &g), GuestStatus::Running);
    let before = a.ask("cat /tmp/sayac");
    std::thread::sleep(Duration::from_secs(1));
    assert_ne!(a.ask("cat /tmp/sayac"), before, "konuk kaynakta çalışmayı sürdürmeli");
    db.migrate_abort(&g);
    assert_eq!(status(&db, &g), GuestStatus::Stopped);
    // Kimse dinlemiyorken de konuk yerinde kalır.
    assert!(da.migrate_send(&g, "127.0.0.1", nbd, ram, &key).unwrap_err().contains("bağlanılamadı"));
    assert_eq!(status(&da, &g), GuestStatus::Running);

    // Doğru anahtarla taşıma.
    db.migrate_listen(&g, layout, "127.0.0.1", nbd, ram, &key).unwrap();
    assert!(db.migrate_finish(&g).unwrap_err().contains("tam ulaşmamış"), "bellek gelmeden konuk sürdürülmez");
    let downtime = da.migrate_send(&g, "127.0.0.1", nbd, ram, &key).unwrap();
    eprintln!("canlı taşıma: duraklama {downtime} ms");
    assert!(downtime < 5000, "duraklama kısa olmalı: {downtime} ms");
    assert_eq!(status(&da, &g), GuestStatus::Stopped, "konuk kaynakta kapanmış olmalı");
    db.migrate_finish(&g).unwrap();
    assert_eq!(status(&db, &g), GuestStatus::Running);

    // Hedefte: bellek, çalışan döngü, çekirdekler ve diskler.
    assert_eq!(b.ask("cat /tmp/isaret"), ["isaret-4471"], "bellek durumu korunmalı");
    let first: u64 = b.ask("cat /tmp/sayac").pop().unwrap().parse().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let second: u64 = b.ask("cat /tmp/sayac").pop().unwrap().parse().unwrap();
    assert!(second > first, "konuk hedefte çalışmayı sürdürmeli: {first} -> {second}");
    assert_eq!(b.cpus().0, "0-2");
    assert_eq!(b.mem_kb(), memory, "konuk aynı bellekle sürmeli");
    b.ask("kill %1 2>/dev/null; sleep 1; sync");
    assert_eq!(b.ask("dd if=/dev/vda bs=512 skip=10 count=1 2>/dev/null | head -c 12; echo").pop().as_deref(), Some("birinci-disk"));
    assert_eq!(b.ask("dd if=/dev/vdb bs=512 skip=7 count=1 2>/dev/null | head -c 11; echo").pop().as_deref(), Some("ikinci-disk"));
    let last: u64 = b.ask("cat /tmp/sayac").pop().unwrap().parse().unwrap();
    let on_disk = b.ask("dd if=/dev/vda bs=512 skip=2000 count=1 2>/dev/null | head -n 1").pop().unwrap();
    let written: u64 = on_disk.trim_start_matches("sayac-").parse().unwrap();
    assert!(written >= second && written <= last + 1, "diskteki son yazı konuğun saydığıyla tutmalı: {on_disk}, sayaç {last}");
    // Taşınan konuk hedefte de büyütülebilir.
    db.hot_add_core(&g).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(b.cpus().0, "0-3");

    db.apply(&g, Action::Stop).unwrap();
    assert_eq!(status(&db, &g), GuestStatus::Stopped);
    // Hedefin disk dosyası, konuğun taşıma sırasında yazdığını içerir.
    let raw = b.dir.join("disk0.raw");
    assert!(std::process::Command::new(&b.cfg.qemu_img_bin).args(["convert", "-O", "raw"]).arg(b.cfg.disk_of(&g)).arg(&raw).status().unwrap().success());
    let bytes = std::fs::read(&raw).unwrap();
    assert_eq!(&bytes[10 * 512..10 * 512 + 12], b"birinci-disk");
    assert!(bytes[2000 * 512..2000 * 512 + 16].starts_with(format!("sayac-{written}").as_bytes()));
    let _ = std::fs::remove_dir_all(&a.dir);
    let _ = std::fs::remove_dir_all(&b.dir);
}

/// Konuk ajanına bağlı yollar, gerçek QEMU ve gerçek `qemu-ga` ile: sonradan eklenen çekirdek ve belleğin
/// etkinleştirilmesi, konuğun içine dosya yazılması ve klasörün konukta açılması.
#[test]
pub(super) fn real_guest_agent_brings_hardware_online_and_receives_files() {
    let Some(real) = real_agent() else { return };
    let d = QemuDriver::new(real.cfg.clone()).unwrap();
    let mut g = guest(&real.cfg, "gercek-ajan");
    (g.cores, g.mem_total_mb, g.hw.max_cores, g.hw.max_mem_mb) = (2, 1024, 4, 2048);
    d.create(&g).unwrap();
    d.apply(&g, Action::Start).unwrap();
    real.boot();
    assert_eq!(real.ask("/ga/start").pop().as_deref(), Some("ajan-basladi"));
    assert!(d.guest_agent(&g).is_some(), "ajan yanıt vermeli");

    // Çekirdek: konukta kapalı gelen çekirdeği ajan açar.
    d.hot_add_core(&g).unwrap();
    g.cores = 3;
    assert!(d.online_cores(&g), "ajan yeni çekirdeği etkinleştirmeli");
    assert_eq!(real.cpus(), ("0-2".to_string(), "0-2".to_string()));

    // Bellek: konuk kendiliğinden etkinleştirmiyorsa ajan etkinleştirir.
    real.ask("echo offline > /sys/devices/system/memory/auto_online_blocks");
    let kb = |real: &Real| -> u64 { real.ask("grep MemTotal /proc/meminfo").pop().and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok())).unwrap() };
    let before = kb(&real);
    d.hot_add_memory(&g, 512).unwrap();
    g.mem_total_mb += 512;
    g.hw.dimms.push(512);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(kb(&real), before, "bellek takılı ama konukta henüz etkin değil");
    assert!(d.online_memory(&g), "ajan yeni belleği etkinleştirmeli");
    assert_eq!(kb(&real), before + 512 * 1024);

    // Dosya: parçalar halinde yazılır, içerik bayt bayt aynıdır; var olan dosyanın üzerine yazılır.
    let lines: String = (0..20_000).map(|i| format!("satir-{i:06}
")).collect();
    assert!(lines.len() > 4 * redox_server::qga::FILE_CHUNK);
    assert_eq!(d.guest_put(&g, "/tmp/geri.txt", false, &mut std::io::Cursor::new(lines.as_bytes())).unwrap(), lines.len() as u64);
    assert_eq!(real.ask("wc -c < /tmp/geri.txt").pop().as_deref(), Some(lines.len().to_string().as_str()));
    assert_eq!(real.ask("grep -c '^satir-[0-9]*$' /tmp/geri.txt; head -n 1 /tmp/geri.txt; tail -n 1 /tmp/geri.txt; sort -u /tmp/geri.txt | wc -l").iter().rev().take(4).rev().cloned().collect::<Vec<_>>(), ["20000", "satir-000000", "satir-019999", "20000"]);
    d.guest_put(&g, "/tmp/geri.txt", false, &mut std::io::Cursor::new(b"kisa
")).unwrap();
    assert_eq!(real.ask("cat /tmp/geri.txt"), ["kisa"]);
    let missing = d.guest_put(&g, "/yok/dizin/dosya", false, &mut std::io::Cursor::new(b"x")).unwrap_err();
    assert!(missing.contains("konuk ajanı"), "{missing}");

    // Klasör: tar arşivi konukta var olan dizine açılır, izinler korunur, arşiv geride kalmaz.
    let tree = real.dir.join("agac");
    std::fs::create_dir_all(tree.join("alt")).unwrap();
    std::fs::write(tree.join("a.conf"), "Port 2222
").unwrap();
    std::fs::write(tree.join("alt/gizli"), "anahtar
").unwrap();
    assert!(std::process::Command::new("chmod").arg("600").arg(tree.join("alt/gizli")).status().unwrap().success());
    let archive = std::process::Command::new("tar").arg("-C").arg(&tree).args(["-cf", "-", "."]).output().unwrap().stdout;
    real.ask("mkdir -p /tmp/hedef");
    assert_eq!(d.guest_put(&g, "/tmp/hedef", true, &mut std::io::Cursor::new(&archive)).unwrap(), archive.len() as u64);
    assert_eq!(real.ask("cat /tmp/hedef/a.conf /tmp/hedef/alt/gizli"), ["Port 2222", "anahtar"]);
    assert_eq!(real.ask("ls -l /tmp/hedef/alt/gizli | cut -c1-10").pop().as_deref(), Some("-rw-------"));
    assert_eq!(real.ask("ls /tmp/hedef | grep -c tar").pop().as_deref(), Some("0"), "arşiv konukta bırakılmaz");
    let no_dir = d.guest_put(&g, "/tmp/olmayan", true, &mut std::io::Cursor::new(&archive)).unwrap_err();
    assert!(no_dir.contains("hedef dizin konukta var mı"), "{no_dir}");

    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(&real.dir);
}

/// İşlemci payı ve bellek garantisi, gerçek çekirdeğin denetim gruplarında: QEMU süreci konuğun grubuna alınır
/// ve değerler oraya yazılır. Test sürecinin, alt grup açabildiği bir denetim grubunda çalışması gerekir
/// (`systemd-run --user --scope -p Delegate=yes`); değilse atlanır.
#[test]
pub(super) fn real_qemu_guest_gets_its_cpu_weight_and_memory_guarantee() {
    let Some(real) = real() else { return };
    let d = QemuDriver::new(real.cfg.clone()).unwrap();
    let mut g = guest(&real.cfg, "gercek-pay");
    (g.mem_total_mb, g.hw.cpu_shares, g.hw.mem_reserve_mb) = (512, 300, 256);
    d.create(&g).unwrap();
    match d.apply(&g, Action::Start) {
        Err(e) if e.contains("denetim grubu") => {
            eprintln!("ATLANDI: bu süreç alt denetim grubu açamıyor ({e})");
            assert_eq!(status(&d, &g), GuestStatus::Stopped, "yetki yokken konuk çalışır bırakılmaz");
            return;
        }
        started => started.unwrap(),
    }
    let pid = std::fs::read_to_string(real.cfg.pid_path(g.vmid)).unwrap().trim().to_string();
    let path = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap().lines().find_map(|l| l.strip_prefix("0::").map(str::to_string)).unwrap();
    assert!(path.ends_with(&format!("/vm-{}", g.vmid)), "QEMU konuğun grubunda olmalı: {path}");
    let group = Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
    let read = |file: &str| std::fs::read_to_string(group.join(file)).unwrap().trim().to_string();
    assert_eq!((read("cpu.weight"), read("memory.min")), ("300".to_string(), (256u64 << 20).to_string()));
    assert!(read("cgroup.procs").lines().any(|p| p == pid));

    // Çalışırken değişir.
    (g.hw.cpu_shares, g.hw.mem_reserve_mb) = (50, 0);
    d.configure(&g).unwrap();
    assert_eq!((read("cpu.weight"), read("memory.min")), ("50".to_string(), "0".to_string()));

    d.apply(&g, Action::Stop).unwrap();
    assert!(!group.exists(), "kapanan konuğun grubu kaldırılır");
    let _ = std::fs::remove_dir_all(&real.dir);
}

/// Çalışırken depolama taşıma, gerçek QEMU'da: konuk diske yazmayı sürdürürken iki diski de başka bir dizine
/// geçer; eski dosyalar silinir, konuk durmaz ve yeni alandan yeniden açıldığında verisi yerindedir.
#[test]
pub(super) fn real_qemu_moves_a_running_guests_disks_to_another_storage() {
    let Some(mut real) = real() else { return };
    let fast = real.dir.join("hizli");
    real.cfg.storages = vec![("hizli".to_string(), fast.clone())];
    let d = QemuDriver::new(real.cfg.clone()).unwrap();
    let mut g = guest(&real.cfg, "gercek-depo");
    (g.cores, g.mem_total_mb) = (2, 1024);
    g.hw.disks.push(redox_core::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 });
    d.create(&g).unwrap();
    d.add_disk(&g, 1, 1).unwrap();
    let (main, extra) = (real.cfg.disk_of(&g), real.cfg.extra_disk_of(&g, 1));
    d.apply(&g, Action::Start).unwrap();
    real.boot();
    real.ask("depmod -a 2>/dev/null; modprobe virtio_pci 2>/dev/null; modprobe virtio_blk 2>/dev/null; mount -t devtmpfs dev /dev 2>/dev/null; sleep 1");
    real.ask("echo birinci-disk | dd of=/dev/vda bs=512 seek=10 conv=notrunc 2>/dev/null; echo ikinci-disk | dd of=/dev/vdb bs=512 seek=7 conv=notrunc 2>/dev/null; sync");
    real.ask("(i=0; while true; do i=$((i+1)); echo sayac-$i | dd of=/dev/vda bs=512 seek=2000 conv=notrunc 2>/dev/null; echo $i > /tmp/sayac; done) >/dev/null 2>&1 & true");
    let count = |real: &Real| -> u64 { real.ask("cat /tmp/sayac").pop().unwrap().parse().unwrap() };
    let before = count(&real);

    d.move_storage_live(&g, "hizli").unwrap();
    let (new_main, new_extra) = (fast.join(main.file_name().unwrap()), fast.join(extra.file_name().unwrap()));
    assert!(new_main.exists() && new_extra.exists() && !main.exists() && !extra.exists(), "diskler yeni alanda, eski dosyalar silinmiş olmalı");
    assert_eq!(status(&d, &g), GuestStatus::Running);
    let after = count(&real);
    std::thread::sleep(Duration::from_secs(1));
    assert!(after > before && count(&real) > after, "konuk taşıma boyunca ve sonrasında yazmayı sürdürmeli");
    // Taşınan konuk yeniden taşınabilir ve canlı yedeği etkilenmez: diskler yeni dosyalarından okunur.
    g.storage = "hizli".to_string();
    assert_eq!(real.ask("dd if=/dev/vdb bs=512 skip=7 count=1 2>/dev/null | head -c 11; echo").pop().as_deref(), Some("ikinci-disk"));
    real.ask("kill %1 2>/dev/null; sleep 1; sync");
    let last = count(&real);

    // Yeni alandan sıfırdan açılış: veri yerinde.
    d.apply(&g, Action::Stop).unwrap();
    d.apply(&g, Action::Start).unwrap();
    real.boot();
    real.ask("depmod -a 2>/dev/null; modprobe virtio_pci 2>/dev/null; modprobe virtio_blk 2>/dev/null; mount -t devtmpfs dev /dev 2>/dev/null; sleep 1");
    assert_eq!(real.ask("dd if=/dev/vda bs=512 skip=10 count=1 2>/dev/null | head -c 12; echo").pop().as_deref(), Some("birinci-disk"));
    let on_disk = real.ask("dd if=/dev/vda bs=512 skip=2000 count=1 2>/dev/null | head -n 1").pop().unwrap();
    let written: u64 = on_disk.trim_start_matches("sayac-").parse().unwrap();
    assert!(written + 1 >= last && written <= last + 1, "diskteki son yazı konuğun saydığıyla tutmalı: {on_disk}, sayaç {last}");
    d.apply(&g, Action::Stop).unwrap();
    let _ = std::fs::remove_dir_all(&real.dir);
}
