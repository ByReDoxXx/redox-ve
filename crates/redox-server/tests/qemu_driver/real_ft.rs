//! Hata toleransı, sürücünün kendi koduyla **gerçek QEMU**'da ve gerçek ağda.
//!
//! `real_qemu.rs` koşullarına ek olarak ağ aygıtı kurabilen bir ad alanı ister: `br0` köprüsü (10.5.0.1/24),
//! ona bağlı `tapa` ve `tapb` arayüzleri, `REDOX_REAL_FT=1` ve içinde virtio ağ modülleri bulunan başlangıç
//! dosya sistemi (`REDOX_REAL_INITRD_NET`; modüller `/netmod` altında). Verilmezse atlanır.

use super::real_qemu::{real_net, Real};
use super::*;
use redox_core::FtLink;

fn until(seconds: u64, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "zamanında olmadı: {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn pid_of(cfg: &QemuConfig, vmid: u32) -> String {
    std::fs::read_to_string(cfg.pid_path(vmid)).unwrap().trim().to_string()
}

fn signal(name: &str, pid: &str) {
    assert!(std::process::Command::new("kill").args([name, pid]).status().unwrap().success(), "kill {name} {pid}");
}

fn ping() -> bool {
    std::process::Command::new("ping").args(["-c", "1", "-W", "1", "10.5.0.2"]).output().is_ok_and(|o| o.status.success())
}

const SETUP: &str = "depmod -a 2>/dev/null; modprobe virtio_pci 2>/dev/null; modprobe virtio_blk 2>/dev/null; for m in failover net_failover virtio_net; do insmod /netmod/$m.ko.xz 2>/dev/null || { xz -dc /netmod/$m.ko.xz > /tmp/$m.ko; insmod /tmp/$m.ko; }; done; mount -t devtmpfs dev /dev 2>/dev/null; sleep 1; ip addr add 10.5.0.2/24 dev eth0; ip link set eth0 up";

fn write(vm: &Real, sector: u64, text: &str) {
    vm.ask(&format!("echo {text} | dd of=/dev/vda bs=512 seek={sector} conv=notrunc 2>/dev/null; sync"));
}

/// Komutun çıktısını işaretli satırdan alır: devralınan ikizin uçbirimi birincilin imleç konumunu taşıdığı için
/// komut yankısı başka yerden kırılabilir ve "son satır" yankının bir parçası olabilir.
fn value(vm: &Real, command: &str) -> String {
    let lines = vm.ask(&format!("echo \"DEGER=$({command})=SON\""));
    lines.iter().rev().find_map(|l| l.split_once("DEGER=").and_then(|(_, rest)| rest.split_once("=SON")).map(|(v, _)| v.to_string()).filter(|v| !v.starts_with("$("))).unwrap_or_else(|| panic!("değer okunamadı: {command}: {lines:?}"))
}

fn read(vm: &Real, sector: u64) -> String {
    value(vm, &format!("dd if=/dev/vda bs=512 skip={sector} count=1 2>/dev/null | head -n 1"))
}

fn counter(vm: &Real) -> u64 {
    value(vm, "cat /tmp/sayac").parse().unwrap()
}

fn link(base: u16) -> FtLink {
    FtLink { bind: "127.0.0.1".into(), primary_host: "127.0.0.1".into(), secondary_host: "127.0.0.1".into(), mirror_port: base, compare_port: base + 1, nbd_port: base + 2, migrate_port: base + 3, local_ports: (base + 4, base + 5) }
}

/// Konuk ikiziyle eşzamanlı çalışır. İkiz donunca ağ kesilir ve ikizi bırakınca geri gelir; birincil yeniden
/// korunabilir. Birincil ölünce ikiz konuk yeniden başlamadan sürer; sonradan kapatılıp olağan yoldan açılan
/// konukta devralmadan önce ve sonra yazılan her şey diskindedir.
#[test]
pub(super) fn real_qemu_keeps_a_protected_guest_alive_when_either_side_is_lost() {
    if std::env::var("REDOX_REAL_FT").is_err() {
        eprintln!("ATLANDI: hata toleransı için ağ ad alanı gerekir (REDOX_REAL_FT)");
        return;
    }
    let (Some(a), Some(b)) = (real_net("tapa"), real_net("tapb")) else { return };
    let (da, db) = (QemuDriver::new(a.cfg.clone()).unwrap(), QemuDriver::new(b.cfg.clone()).unwrap());
    let mut g = guest(&a.cfg, "gercek-ft");
    (g.cores, g.mem_total_mb, g.disk_gb, g.bridge, g.hw.ft) = (1, 512, 1, "br0".to_string(), true);
    assert_eq!(redox_core::ft_blocker(&g), None);
    da.create(&g).unwrap();
    da.apply(&g, Action::Start).unwrap();
    a.boot();
    a.ask(SETUP);
    until(20, "konuk ağda", ping);
    write(&a, 10, "korumadan-once");
    a.ask("echo bellek-7781 > /tmp/isaret; (i=0; while true; do i=$((i+1)); echo $i > /tmp/sayac; sleep 1; done) >/dev/null 2>&1 & true");
    let bytes = g.disk_gb << 30;
    let key = "4b".repeat(32);

    // İkiz yokken kurulum başarısız olur ve konuk etkilenmez.
    let refused = da.ft_protect(&g, &link(49500), &key).unwrap_err();
    assert!(refused.contains("bağlanılamadı"), "{refused}");
    assert!(ping() && status(&da, &g) == GuestStatus::Running, "başarısız kurulum konuğa dokunmaz");

    // 1) Koruma, ardından donan ikiz.
    let ports = link(49510);
    db.ft_standby_start(&g, bytes, &ports, &key).unwrap();
    let started = std::time::Instant::now();
    da.ft_protect(&g, &ports, &key).unwrap();
    eprintln!("koruma kuruldu: {} ms", started.elapsed().as_millis());
    assert!(da.ft_status(&g).is_some_and(|s| s.engaged && !s.paused), "{:?}", da.ft_status(&g));
    assert!(db.ft_status(&g).is_some_and(|s| s.engaged), "{:?}", db.ft_status(&g));
    until(10, "korunurken ağ", ping);
    write(&a, 20, "korunurken-1");
    // Yalnızca birincilin gönderdiği paket: denetim noktası alınmadan dışarı çıkamaz. İkiz bu adımda çökmemeli.
    assert_eq!(value(&a, "ping -c 1 10.5.0.1 >/dev/null 2>&1; echo $?"), "0", "ilk korumada ağ onayı");
    assert!(db.ft_status(&g).is_some_and(|s| s.engaged), "ikiz denetim noktasından sonra yaşamalı");
    let twin = pid_of(&b.cfg, g.vmid);
    signal("-STOP", &twin);
    std::thread::sleep(Duration::from_secs(3));
    assert!(!ping(), "ikiz donukken konuğun ağı dışarı paket veremez");
    let started = std::time::Instant::now();
    da.ft_drop_secondary(&g).unwrap();
    until(10, "ikiz bırakılınca ağ", ping);
    eprintln!("donan ikizden kurtarma: {} ms", started.elapsed().as_millis());
    assert!(da.ft_status(&g).is_some_and(|s| !s.engaged && !s.paused));
    write(&a, 30, "tek-basina");
    signal("-CONT", &twin);
    db.ft_standby_stop(&g);
    assert_eq!(status(&db, &g), GuestStatus::Stopped);

    // 2) Aynı birincil yeniden korunur ve bu kez birincil ölür.
    let ports = link(49520);
    db.ft_standby_start(&g, bytes, &ports, &key).unwrap();
    da.ft_protect(&g, &ports, &key).unwrap();
    until(10, "yeniden korunurken ağ", ping);
    write(&a, 40, "korunurken-2");
    // COLO'nun sözü **ağdan görünen** durum içindir: ikiz kendi başına çalışır (seri konsoldaki komut ona gitmez),
    // ama birincilin bir paketi, ikiz aynı duruma getirilmeden (denetim noktası) dışarı bırakılmaz. Konuk ağdan
    // ses verebildiyse, o ana kadar yazdıkları ikizdedir.
    assert_eq!(value(&a, "ping -c 1 10.5.0.1 >/dev/null 2>&1; echo $?"), "0", "konuk ağdan onay gönderebilmeli");
    let before = counter(&a);
    signal("-9", &pid_of(&a.cfg, g.vmid));
    let started = std::time::Instant::now();
    db.ft_take_over(&g).unwrap();
    until(10, "devralınca ağ", ping);
    eprintln!("birincil öldükten sonra devralma: {} ms", started.elapsed().as_millis());
    assert_eq!(status(&db, &g), GuestStatus::Running);
    assert_eq!(value(&b, "cat /tmp/isaret"), "bellek-7781", "konuk yeniden başlamadı: belleği duruyor");
    let after = counter(&b);
    assert!(after + 3 >= before, "çalışan döngü sürüyor: {before} -> {after}");
    for (sector, text) in [(10, "korumadan-once"), (20, "korunurken-1"), (30, "tek-basina"), (40, "korunurken-2")] {
        assert_eq!(read(&b, sector), text, "devralınan ikizde {sector}");
    }
    write(&b, 50, "devralma-sonrasi");

    // 3) En kritik kısım: kapatıp olağan yoldan açınca (yalnızca asıl disk dosyası) her şey yerinde mi?
    std::thread::sleep(Duration::from_secs(5));
    db.apply(&g, Action::Stop).unwrap();
    assert_eq!(status(&db, &g), GuestStatus::Stopped);
    db.ft_standby_stop(&g);
    db.apply(&g, Action::Start).unwrap();
    b.boot();
    b.ask(SETUP);
    for (sector, text) in [(10, "korumadan-once"), (20, "korunurken-1"), (30, "tek-basina"), (40, "korunurken-2"), (50, "devralma-sonrasi")] {
        assert_eq!(read(&b, sector), text, "yeniden açılışta {sector}");
    }
    db.apply(&g, Action::Stop).unwrap();
    for dir in [&a.dir, &b.dir] {
        let _ = std::fs::remove_dir_all(dir);
    }
}
