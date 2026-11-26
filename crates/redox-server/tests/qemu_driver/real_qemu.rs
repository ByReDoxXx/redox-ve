//! Sürücünün **gerçek QEMU** ile sınanması. Öbür testler sahte QEMU kullanır; buradakiler yalnızca gerçek
//! ikililer ve bir Linux çekirdeği verildiğinde çalışır, verilmediğinde atlanır:
//!
//! ```text
//! REDOX_REAL_QEMU=/yol/qemu-system-x86_64  REDOX_REAL_QEMU_IMG=/yol/qemu-img
//! REDOX_REAL_KERNEL=/yol/vmlinuz           REDOX_REAL_INITRD=/yol/initrd.gz
//! ```
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
    let var = |name: &str| std::env::var(name).ok().filter(|v| Path::new(v).is_file());
    let (Some(qemu), Some(img), Some(kernel), Some(initrd)) = (var("REDOX_REAL_QEMU"), var("REDOX_REAL_QEMU_IMG"), var("REDOX_REAL_KERNEL"), var("REDOX_REAL_INITRD")) else {
        eprintln!("ATLANDI: gerçek QEMU verilmedi (REDOX_REAL_QEMU, REDOX_REAL_QEMU_IMG, REDOX_REAL_KERNEL, REDOX_REAL_INITRD)");
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
        &format!("rm -f '{serial}'\nprintf '%s\\n' \"$@\" > '{serial}.args'\nexec '{qemu}' \"$@\" -kernel '{kernel}' -initrd '{initrd}' -append 'console=ttyS0 rdinit=/bin/sh quiet' -serial 'unix:{serial},server=on,wait=off'", serial = serial.display()),
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
        text.replace('\r', "").lines().map(str::trim).filter(|l| !l.is_empty() && !l.contains("BITTI") && !l.starts_with("~ #") && !l.starts_with("/ #")).map(str::to_string).collect()
    }

    /// Kabuk açılana kadar bekler ve `/proc` ile `/sys` dosya sistemlerini bağlar.
    pub fn boot(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            let ready = UnixStream::connect(&self.serial).is_ok_and(|mut sock| {
                sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let _ = sock.write_all(b"echo HAZIR$((40+2))\n");
                let (mut text, mut buf, until) = (String::new(), [0u8; 4096], std::time::Instant::now() + Duration::from_secs(3));
                while !text.contains("HAZIR42") && std::time::Instant::now() < until {
                    if let Ok(n) = sock.read(&mut buf) {
                        text.push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
                }
                text.contains("HAZIR42")
            });
            if ready {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "konuk açılmadı");
        }
        self.ask("mount -t proc proc /proc 2>/dev/null; mount -t sysfs sys /sys 2>/dev/null");
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
