//! Disk şifrelemenin gerçek QEMU ile sınanması.
//!
//! Ölçüt: konuk diske ayırt edici bir metin yazar; ardından veri dizinlerindeki **bütün dosyalar** bayt bayt
//! taranır. Metin disk, anlık görüntü, yedek ya da geçici dosyaların hiçbirinde görünmemelidir. Aynı
//! tarama önce şifresiz bir konukta metni bulur; böylece taramanın çalıştığı da kanıtlanır.

use std::path::{Path, PathBuf};

use super::real_qemu::{real, Real};
use super::*;

const MARK: &str = "REDOXGIZLI-7f3a9c1e5b2d-VERI";

fn guest(vmid: u32, encrypted: bool) -> Guest {
    let mut g = Cluster::demo(0).guests.remove(0);
    (g.vmid, g.name, g.kind, g.bridge, g.cores, g.mem_total_mb, g.disk_gb) = (vmid, format!("sifre-{vmid}"), GuestKind::Vm, "nat".into(), 1, 384, 1);
    g.storage = "local".into();
    g.iso = None;
    g.forwards.clear();
    g.snapshots.clear();
    g.hw = redox_core::Hardware { encrypted, ..Default::default() };
    g
}

/// İçinde `MARK` geçen dosyalar.
fn leaks(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() && std::fs::read(&path).is_ok_and(|data| data.windows(MARK.len()).any(|w| w == MARK.as_bytes())) {
                found.push(path);
            }
        }
    }
    found
}

fn load_disks(r: &Real) {
    r.ask("for m in virtio_pci virtio_blk; do modprobe $m 2>/dev/null; done; mount -t devtmpfs dev /dev 2>/dev/null; sleep 1; echo");
}

/// Diske, `sector`dan başlayarak metnin binlerce kopyasını yazar.
fn write_mark(r: &Real, device: &str, sector: u64) {
    r.ask(&format!("echo {MARK} > /tmp/m; for i in 1 2 3 4 5 6 7 8 9 10 11; do cat /tmp/m /tmp/m > /tmp/n; mv /tmp/n /tmp/m; done; dd if=/tmp/m of={device} bs=512 seek={sector} conv=notrunc 2>/dev/null; sync; echo"));
}

fn read_disk(r: &Real, device: &str, sector: u64) -> String {
    r.ask(&format!("echo 3 > /proc/sys/vm/drop_caches; dd if={device} bs=512 skip={sector} count=1 2>/dev/null | head -n 1; echo")).join(" ")
}

fn encrypted_image(r: &Real, path: &Path) -> bool {
    let out = std::process::Command::new(&r.cfg.qemu_img_bin).args(["info", "-U", "--output=json"]).arg(path).output().unwrap();
    let info: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    info["encrypted"] == true
}

#[test]
fn real_qemu_keeps_an_encrypted_guests_data_unreadable_everywhere() {
    let Some(mut r) = real() else { return };
    let key = r.dir.join("anahtar");
    std::fs::write(&key, "sinama-disk-anahtari-0123456789abcdef").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    r.cfg.disk_key = Some(key.clone());
    r.cfg.incremental_backups = true;
    r.cfg.cbt = true;
    let fast = r.dir.join("hizli");
    std::fs::create_dir_all(&fast).unwrap();
    r.cfg.storages = vec![("hizli".to_string(), fast.clone())];
    let data = r.cfg.data_dir.clone();
    let scan = || {
        let mut found = leaks(&data);
        found.extend(leaks(&fast));
        found
    };

    // Denetim: şifresiz konukta tarama metni bulur.
    let driver = QemuDriver::new(r.cfg.clone()).unwrap();
    let plain = guest(200, false);
    driver.create(&plain).unwrap();
    driver.apply(&plain, Action::Start).unwrap();
    r.boot();
    load_disks(&r);
    write_mark(&r, "/dev/vda", 200);
    driver.apply(&plain, Action::Stop).unwrap();
    assert!(!scan().is_empty(), "tarama şifresiz diskte metni bulmalı; bulamıyorsa sınama bir şey kanıtlamaz");
    driver.destroy(&plain).unwrap();
    assert!(scan().is_empty(), "{:?}", scan());
    eprintln!("denetim: şifresiz diskte metin bulunuyor");

    let mut g = guest(201, true);
    driver.create(&g).unwrap();
    assert!(encrypted_image(&r, &r.cfg.disk_of(&g)));
    driver.apply(&g, Action::Start).unwrap();
    r.boot();
    load_disks(&r);
    write_mark(&r, "/dev/vda", 200);
    assert!(read_disk(&r, "/dev/vda", 200).contains(MARK), "konuk kendi verisini okur");
    assert!(scan().is_empty(), "çalışan şifreli disk: {:?}", scan());

    // Çalışırken takılan disk de şifrelidir.
    driver.add_disk(&g, 1, 1).unwrap();
    driver.hot_add_disk(&g, 1).unwrap();
    g.hw.disks = vec![redox_core::hw::ExtraDisk { id: 1, size_gb: 1, serial: 1 }];
    assert!(encrypted_image(&r, &r.cfg.extra_disk_of(&g, 1)));
    std::thread::sleep(Duration::from_secs(3));
    let seen = r.ask("test -b /dev/vdb && echo IKINCI-DISK; echo");
    if seen.iter().any(|l| l == "IKINCI-DISK") {
        write_mark(&r, "/dev/vdb", 300);
        assert!(read_disk(&r, "/dev/vdb", 300).contains(MARK));
        assert!(scan().is_empty(), "takılan disk: {:?}", scan());
        eprintln!("çalışırken takılan şifreli disk konukta yazıldı");
    } else {
        eprintln!("ATLANDI: sınama konuğu takılan diski görmedi; imajın şifreli olduğu doğrulandı");
    }

    // Bellekli anlık görüntü bu QEMU sürümünde şifreli konuğu çökertir: reddedilir ve konuk sürer.
    let refused = driver.snapshot(&g, SnapshotOp::Create("canli")).unwrap_err();
    assert!(refused.contains("bellekli anlık görüntü alınamaz"), "{refused}");
    assert_eq!(status(&driver, &g), GuestStatus::Running);
    // Belleksiz (yalnızca disk) görüntü alınabilir ve konuk çalışmayı sürdürür.
    driver.snapshot_disk_only(&g, "belleksiz").unwrap();
    assert_eq!(status(&driver, &g), GuestStatus::Running);
    assert!(read_disk(&r, "/dev/vda", 200).contains(MARK), "görüntüden sonra konuk yaşıyor");
    assert!(scan().is_empty(), "belleksiz anlık görüntü: {:?}", scan());
    // Çalışırken yedek: artımlı depo açık olsa da şifreli konuk tam ve şifreli yedeklenir.
    driver.backup(&g, "yedek-canli").unwrap();
    let main = r.cfg.backups_dir().join("yedek-canli.qcow2");
    assert!(main.is_file() && encrypted_image(&r, &main) && encrypted_image(&r, &r.cfg.backups_dir().join("yedek-canli.disk1.qcow2")));
    assert!(scan().is_empty(), "canlı yedek: {:?}", scan());
    driver.verify_backup("yedek-canli", &[1]).unwrap();
    // İkinci canlı yedek: önceki yedeğin hedef düğümleri geride kalmamıştır.
    driver.backup(&g, "yedek-canli-2").unwrap();
    // Şifreli verinin çözülmüş bir yere gidebileceği işlemler reddedilir.
    for (what, outcome) in [
        ("askı", driver.suspend(&g)),
        ("canlı depolama taşıma", driver.move_storage_live(&g, "hizli")),
        ("yansıtma", driver.mirror_start(&g, "127.0.0.1", 49700, &"a".repeat(64))),
        ("dışa aktarma", driver.export_disks(&g).map(|_| ())),
        ("klon", driver.clone_disk(&g, &guest(250, true))),
    ] {
        assert!(outcome.as_ref().is_err_and(|e| e.contains("desteklenmiyor")), "{what}: {outcome:?}");
    }
    driver.apply(&g, Action::Stop).unwrap();
    assert!(scan().is_empty(), "kapandıktan sonra: {:?}", scan());
    eprintln!("çalışan konuk: disk, takılan disk, belleksiz görüntü ve canlı yedek şifreli");

    // Kapalıyken: görüntü, yedek (dosya kopyası), doğrulama, büyütme.
    driver.snapshot(&g, SnapshotOp::Create("kapali")).unwrap();
    driver.backup(&g, "yedek-kapali").unwrap();
    assert!(encrypted_image(&r, &r.cfg.backups_dir().join("yedek-kapali.qcow2")));
    driver.verify_backup("yedek-kapali", &[1]).unwrap();
    driver.snapshot(&g, SnapshotOp::Rollback("kapali")).unwrap();
    driver.snapshot(&g, SnapshotOp::Delete("kapali")).unwrap();
    driver.resize_disk(&g, 2).unwrap();
    g.disk_gb = 2;
    assert!(scan().is_empty(), "kapalı işlemler: {:?}", scan());

    // Veri bozulup yedekten geri yüklenince geri gelir.
    driver.apply(&g, Action::Start).unwrap();
    r.boot();
    load_disks(&r);
    assert!(r.ask("cat /sys/block/vda/size").iter().any(|l| l == "4194304"), "büyütülen disk 2 GB görünür");
    r.ask("dd if=/dev/zero of=/dev/vda bs=512 seek=200 count=64 conv=notrunc 2>/dev/null; sync; echo");
    assert!(!read_disk(&r, "/dev/vda", 200).contains(MARK));
    driver.apply(&g, Action::Stop).unwrap();
    driver.restore(&g, "yedek-canli", &[1]).unwrap();
    assert!(encrypted_image(&r, &r.cfg.disk_of(&g)));
    // Başka depoya taşınan disk dosyası da şifreli kalır.
    driver.move_storage(&g, "hizli").unwrap();
    g.storage = "hizli".into();
    assert!(r.cfg.disk_of(&g).starts_with(&fast) && encrypted_image(&r, &r.cfg.disk_of(&g)));
    driver.apply(&g, Action::Start).unwrap();
    r.boot();
    load_disks(&r);
    assert!(read_disk(&r, "/dev/vda", 200).contains(MARK), "yedekten dönen veri okunur");
    driver.apply(&g, Action::Stop).unwrap();
    assert!(scan().is_empty(), "geri yükleme ve taşıma: {:?}", scan());
    eprintln!("kapalı konuk: görüntü, yedek, geri yükleme, büyütme ve taşıma şifreli");

    // Yanlış anahtar ve anahtarsız sunucu: konuk açılmaz.
    std::fs::write(&key, "baska-bir-anahtar-0123456789abcdef").unwrap();
    let wrong = driver.apply(&g, Action::Start).unwrap_err();
    assert!(status(&driver, &g) == GuestStatus::Stopped, "{wrong}");
    let mut bare = r.cfg.clone();
    bare.disk_key = None;
    let keyless = QemuDriver::new(bare).unwrap();
    assert!(keyless.apply(&g, Action::Start).unwrap_err().contains("disk anahtarı yok"));
    assert!(!keyless.disk_encryption() && keyless.create(&guest(202, true)).is_err());
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(driver.apply(&g, Action::Start).unwrap_err().contains("herkes okuyabiliyor"));
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&key, "sinama-disk-anahtari-0123456789abcdef").unwrap();

    // Şifresiz bir bulut imajından şifreli disk: imajdaki metin diskte okunamaz, konuk okur.
    std::fs::create_dir_all(r.cfg.cloud_dir()).unwrap();
    let mut image = vec![0u8; 4 * 1024 * 1024];
    let line = format!("{MARK}\n");
    image[512 * 100..512 * 100 + line.len()].copy_from_slice(line.as_bytes());
    let source = r.cfg.cloud_dir().join("taban.raw");
    std::fs::write(&source, &image).unwrap();
    let mut from_image = guest(203, true);
    from_image.hw.image = Some("taban.raw".into());
    driver.create(&from_image).unwrap();
    std::fs::remove_file(&source).unwrap();
    assert!(scan().is_empty(), "imajdan oluşturma: {:?}", scan());
    driver.apply(&from_image, Action::Start).unwrap();
    r.boot();
    load_disks(&r);
    assert!(read_disk(&r, "/dev/vda", 100).contains(MARK));
    driver.apply(&from_image, Action::Stop).unwrap();
    eprintln!("disk şifreleme: tamam");
}
