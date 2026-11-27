//! Uyumluluk aygıtlarının gerçek QEMU ile sınanması: disk denetleyicileri, ağ kartı modelleri, iki CD sürücüsü,
//! çalışırken kalıp değiştirme ve TRIM.
//!
//! Konuk testi ayrıca Intel ağ sürücüleri eklenmiş bir başlangıç dosya sistemi ister (`REDOX_REAL_INITRD_COMPAT`;
//! modüller ve küçük bir TRIM aracı `/netmod` altındadır).

use super::real_qemu::{real, real_compat, Real};
use super::*;

use redox_core::hw::{DiskBus, NicModel};
use redox_server::qemu::build_args;

/// BIOS'un diskten okuyup çalıştırdığı 512 baytlık açılış kesimi: seri kapıya `mesaj`ı yazıp durur.
fn boot_sector(message: &str) -> Vec<u8> {
    let mut code: Vec<u8> = vec![
        0x31, 0xC0, // xor ax, ax
        0x8E, 0xD8, // mov ds, ax
        0xBA, 0xF8, 0x03, // mov dx, 0x3F8 (COM1)
        0xBE, 0x15, 0x7C, // mov si, mesaj
        0xAC, // lodsb
        0x84, 0xC0, // test al, al
        0x74, 0x03, // jz dur
        0xEE, // out dx, al
        0xEB, 0xF8, // jmp lodsb
        0xF4, // dur: hlt
        0xEB, 0xFD, // jmp dur
    ];
    code.extend_from_slice(message.as_bytes());
    code.push(0);
    code.resize(510, 0);
    code.extend_from_slice(&[0x55, 0xAA]);
    code
}

fn guest(vmid: u32, bus: DiskBus, nic: NicModel) -> Guest {
    let mut c = Cluster::demo(0);
    let spec = redox_core::GuestSpec {
        vmid: Some(vmid),
        name: format!("uyum-{vmid}"),
        kind: GuestKind::Vm,
        node: "pve-ist-01".into(),
        cores: 2,
        memory_mb: 512,
        disk_gb: 1,
        storage: "local-zfs".into(),
        os: "linux".into(),
        bridge: "nat".into(),
        onboot: false,
        iso: None,
        hw: redox_core::Hardware { disk_bus: bus, nic_model: nic, ..Default::default() },
    };
    c.create_guest(&spec, "u", 0).unwrap();
    c.guest(vmid).unwrap().clone()
}

fn write_iso(r: &Real, name: &str, marker: &str) {
    let mut data = marker.as_bytes().to_vec();
    data.resize(2048 * 64, 0);
    std::fs::create_dir_all(r.cfg.iso_dir()).unwrap();
    std::fs::write(r.cfg.iso_dir().join(name), data).unwrap();
}

/// Konuğun BIOS'u asıl diskten, seçilen denetleyiciyle açılabiliyor mu? Çekirdek verilmeden gerçek QEMU açılır.
#[test]
fn real_qemu_bios_boots_from_the_disk_on_every_controller() {
    let Some(r) = real() else { return };
    let qemu = std::env::var("REDOX_REAL_QEMU").unwrap();
    for (i, bus) in DiskBus::ALL.into_iter().enumerate() {
        let g = guest(170 + i as u32, bus, NicModel::Virtio);
        let driver = QemuDriver::new(r.cfg.clone()).unwrap();
        driver.create(&g).unwrap();
        let raw = r.dir.join(format!("acilis-{i}.raw"));
        let mut image = boot_sector(&format!("DISKTEN-{}", bus.as_str()));
        image.resize(8 * 1024 * 1024, 0);
        std::fs::write(&raw, image).unwrap();
        let disk = r.cfg.disk_of(&g);
        let converted = std::process::Command::new(&r.cfg.qemu_img_bin).args(["convert", "-f", "raw", "-O", "qcow2"]).arg(&raw).arg(&disk).status().unwrap();
        assert!(converted.success());

        let serial = r.dir.join(format!("acilis-{i}.log"));
        let mut args = build_args(&r.cfg, &g);
        // Arka plana geçiren seçenekler kalır; seri çıktı dosyaya yazılır.
        args.extend(["-serial".to_string(), format!("file:{}", serial.display())]);
        std::fs::create_dir_all(r.cfg.run_dir()).unwrap();
        let out = std::process::Command::new(&qemu).args(&args).output().unwrap();
        assert!(out.status.success(), "{}: {}", bus.as_str(), String::from_utf8_lossy(&out.stderr));
        let wanted = format!("DISKTEN-{}", bus.as_str());
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        let mut seen = String::new();
        while !seen.contains(&wanted) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            seen = String::from_utf8_lossy(&std::fs::read(&serial).unwrap_or_default()).into_owned();
        }
        let pid = std::fs::read_to_string(r.cfg.pid_path(g.vmid)).unwrap_or_default();
        let _ = std::process::Command::new("kill").args(["-9", pid.trim()]).status();
        assert!(seen.contains(&wanted), "{} diskinden açılmadı; seri çıktı: {seen:?}", bus.as_str());
        eprintln!("BIOS açılışı {}: tamam", bus.as_str());
    }
}

#[test]
fn real_qemu_guest_sees_the_chosen_controller_card_and_both_cd_drives() {
    let cases = [(DiskBus::Sata, NicModel::E1000e, "e1000e", "sda"), (DiskBus::VirtioScsi, NicModel::E1000, "e1000", "sda"), (DiskBus::Virtio, NicModel::Virtio, "virtio_net", "vda")];
    for (i, (bus, nic, driver_name, disk)) in cases.into_iter().enumerate() {
        let Some(r) = real_compat() else { return };
        let mut g = guest(180 + i as u32, bus, nic);
        write_iso(&r, "bir.iso", "KALIP-BIR");
        write_iso(&r, "iki.iso", "KALIP-IKI");
        write_iso(&r, "uc.iso", "KALIP-UC");
        g.iso = Some("bir.iso".into());
        g.hw.iso2 = Some("iki.iso".into());
        let driver = QemuDriver::new(r.cfg.clone()).unwrap();
        driver.create(&g).unwrap();
        driver.apply(&g, Action::Start).unwrap();
        r.boot();
        r.ask("for m in virtio_pci virtio_blk scsi_mod sd_mod sr_mod virtio_scsi libahci ahci; do modprobe $m 2>/dev/null; done; mount -t devtmpfs dev /dev 2>/dev/null; echo");
        r.ask("for m in failover net_failover virtio_net e1000 e1000e; do [ -f /netmod/$m.ko.xz ] && insmod /netmod/$m.ko.xz 2>/dev/null; done; sleep 2; echo");

        let found = r.ask(&format!("test -b /dev/{disk} && echo DISK-VAR; readlink -f /sys/block/{disk}"));
        assert!(found.iter().any(|l| l == "DISK-VAR"), "{}: {found:?}", bus.as_str());
        let path = found.iter().find(|l| l.starts_with("/sys/devices")).cloned().unwrap_or_default();
        let expected = match bus {
            DiskBus::Sata => "/ata",
            DiskBus::VirtioScsi => "/virtio",
            DiskBus::Virtio => "/virtio",
        };
        assert!(path.contains(expected) && path.contains("/host") == (bus != DiskBus::Virtio), "{}: {path}", bus.as_str());
        let card = r.ask("basename $(readlink -f /sys/class/net/eth0/device/driver)");
        assert!(card.iter().any(|l| l == driver_name), "{}: {card:?}", nic.device());

        // Kalıp değiştikten sonraki ilk okuma "ortam değişti" diye boş dönebilir: bir kez de yeniden denenir.
        let once = |n: u8| r.ask(&format!("echo 1 > /proc/sys/vm/drop_caches; dd if=/dev/sr{n} bs=2048 count=1 2>/dev/null | head -c 9; echo")).join(" ");
        let read = |n: u8| {
            let first = once(n);
            if first.contains("KALIP-") { first } else { once(n) }
        };
        assert!(read(0).contains("KALIP-BIR") && read(1).contains("KALIP-IKI"), "iki sürücü sırayla: {} / {}", read(0), read(1));
        driver.change_media(&g, 2, Some("uc.iso")).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        assert!(read(1).contains("KALIP-UC"), "ikinci sürücü yeni kalıbı gösterir: {}", read(1));
        driver.change_media(&g, 1, None).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        assert!(!read(0).contains("KALIP-"), "boşaltılan sürücüden okunmaz: {}", read(0));
        driver.change_media(&g, 1, Some("iki.iso")).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        assert!(read(0).contains("KALIP-IKI"), "boş sürücüye yeniden takılır: {}", read(0));
        assert!(driver.change_media(&g, 2, Some("yok.iso")).is_err(), "olmayan dosya hata verir");

        // TRIM: yazılan alan imaj dosyasını büyütür, konukta bırakılınca geri verilir.
        use std::os::unix::fs::MetadataExt;
        let used = || std::fs::metadata(r.cfg.disk_of(&g)).unwrap().blocks() * 512 / (1024 * 1024);
        r.ask(&format!("dd if=/dev/urandom of=/dev/{disk} bs=1M count=64 2>/dev/null; sync"));
        let full = used();
        let discard = r.ask(&format!("/netmod/trim /dev/{disk} 64; sync"));
        assert!(discard.iter().any(|l| l == "TRIM-TAMAM"), "konuk diskte TRIM yapabiliyor: {discard:?}");
        std::thread::sleep(Duration::from_secs(1));
        let after = used();
        eprintln!("TRIM {}: {full} MB -> {after} MB", bus.as_str());
        assert!(full >= 60 && after + 50 <= full, "{full} MB -> {after} MB");
        driver.apply(&g, Action::Stop).unwrap();
        eprintln!("konuk içi {} + {}: tamam", bus.as_str(), nic.device());
    }
}
