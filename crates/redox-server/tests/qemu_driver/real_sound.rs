//! Ses: gerçek QEMU'da ses kartı konuğa görünür ve konuğun çaldığı ses, canlı konsolun kullandığı VNC
//! soketinden gelir.
//!
//! Sınama konuğunda ses sürücüsü yoktur. Ses, PC hoparlörü G/Ç kapılarından doğrudan programlanarak çalınır;
//! hoparlör de ses kartıyla aynı ses aygıtına bağlıdır. Böylece QEMU → VNC yolu uçtan uca ölçülür.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use super::real_qemu::{real, Real};
use super::*;
use redox_core::SnapshotOp;

fn take<const N: usize>(sock: &mut UnixStream) -> [u8; N] {
    let mut buf = [0u8; N];
    sock.read_exact(&mut buf).expect("VNC okunamadı");
    buf
}

/// Parolasız RFB 3.8 açılışı; ses uzantısını ister ve sunucunun onayını bekler.
fn listen(r: &Real, vmid: u32) -> UnixStream {
    let mut sock = UnixStream::connect(r.cfg.vnc_path(vmid)).expect("VNC soketi");
    sock.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    take::<12>(&mut sock);
    sock.write_all(b"RFB 003.008\n").unwrap();
    let count = take::<1>(&mut sock)[0];
    for _ in 0..count {
        take::<1>(&mut sock);
    }
    sock.write_all(&[1]).unwrap();
    assert_eq!(take::<4>(&mut sock), [0; 4]);
    sock.write_all(&[1]).unwrap();
    let init = take::<24>(&mut sock);
    let mut name = vec![0; u32::from_be_bytes(init[20..24].try_into().unwrap()) as usize];
    sock.read_exact(&mut name).unwrap();
    // Kodlamalar: ham (0) ve QEMU ses uzantısı (-259).
    let mut message = vec![2, 0, 0, 2];
    message.extend_from_slice(&0i32.to_be_bytes());
    message.extend_from_slice(&(-259i32).to_be_bytes());
    sock.write_all(&message).unwrap();
    let ack = take::<16>(&mut sock);
    assert_eq!((ack[0], &ack[12..16]), (0, &(-259i32).to_be_bytes()[..]), "sunucu ses uzantısını onaylamalı: {ack:?}");
    sock
}

/// Sesi açar (16 bit işaretli, iki kanal, 44100 Hz) ve `want` bayt toplayana kadar okur.
fn record(sock: &mut UnixStream, want: usize) -> Vec<u8> {
    sock.write_all(&[255, 1, 0, 2, 3, 2, 0, 0, 0xac, 0x44]).unwrap();
    sock.write_all(&[255, 1, 0, 0]).unwrap();
    let (mut heard, deadline) = (Vec::new(), Instant::now() + Duration::from_secs(30));
    while heard.len() < want && Instant::now() < deadline {
        let head = take::<4>(sock);
        assert_eq!((head[0], head[1]), (255, 1), "beklenmeyen ileti: {head:?}");
        if u16::from_be_bytes([head[2], head[3]]) == 2 {
            let mut data = vec![0; u32::from_be_bytes(take::<4>(sock)) as usize];
            sock.read_exact(&mut data).unwrap();
            heard.extend_from_slice(&data);
        }
    }
    heard
}

fn port(r: &Real, port: u16, value: u8) {
    r.ask(&format!("printf '\\{value:03o}' | dd of=/dev/port bs=1 seek={port} 2>/dev/null"));
}

#[test]
fn real_qemu_sends_the_guests_sound_over_the_console_socket() {
    let Some(r) = real() else { return };
    let driver = QemuDriver::new(r.cfg.clone()).unwrap();
    let mut g = Cluster::demo(0).guests.remove(0);
    (g.vmid, g.name, g.kind, g.bridge, g.cores, g.mem_total_mb, g.disk_gb) = (931, "ses".into(), GuestKind::Vm, "nat".into(), 1, 384, 1);
    g.storage = "local".into();
    g.iso = None;
    g.forwards.clear();
    g.snapshots.clear();
    g.hw = redox_core::Hardware { sound: true, ..Default::default() };
    driver.create(&g).unwrap();
    driver.apply(&g, Action::Start).unwrap();
    r.boot();
    // Ses kartı konuğun PCI veri yolunda görünür (sınıf 0403: HD Audio).
    assert!(!r.ask("grep -l 0x0403 /sys/bus/pci/devices/*/class").is_empty(), "ses kartı konukta görünmeli");

    let mut sock = listen(&r, 931);
    r.ask("[ -e /dev/port ] || mknod /dev/port c 1 4");
    // 8253 zamanlayıcısının 2. kanalı: 1193182 / 1193 ≈ 1000 Hz kare dalga; 0x61 hoparlörü açar.
    port(&r, 0x43, 0xb6);
    port(&r, 0x42, 0xa9);
    port(&r, 0x42, 0x04);
    port(&r, 0x61, 3);
    let heard = record(&mut sock, 44100 * 4);
    port(&r, 0x61, 0);
    assert!(heard.len() >= 44100 * 4, "bir saniyelik ses gelmeli: {} bayt", heard.len());

    // Sol kanalda saniyedeki yükselen sıfır geçişi sayısı ≈ frekans.
    let left: Vec<i16> = heard.as_chunks::<4>().0.iter().map(|f| i16::from_le_bytes([f[0], f[1]])).collect();
    let middle = {
        let (low, high) = left.iter().fold((i16::MAX, i16::MIN), |(l, h), s| (l.min(*s), h.max(*s)));
        assert!(i32::from(high) - i32::from(low) > 2000, "duyulur genlik olmalı: {low}..{high}");
        ((i32::from(low) + i32::from(high)) / 2) as i16
    };
    let crossings = left.windows(2).filter(|w| w[0] <= middle && w[1] > middle).count();
    let hertz = crossings as f64 * 44100.0 / left.len() as f64;
    eprintln!("ölçülen: {hertz:.1} Hz, {} bayt", heard.len());
    // Akışın başı sessiz olabilir; bu yüzden alt sınır gevşektir.
    assert!((700.0..=1100.0).contains(&hertz), "yaklaşık 1000 Hz bekleniyordu: {hertz:.0} Hz");

    // Ses kartı takılıyken bellek anlık görüntüsü alınabilir: kartın durumu da kaydedilebilir olmalı.
    drop(sock);
    driver.snapshot(&g, SnapshotOp::Create("sesli")).expect("bellek anlık görüntüsü");
    assert!(!r.ask("grep -l 0x0403 /sys/bus/pci/devices/*/class").is_empty());

    // Kart kapalıyken çıkarılabilir.
    driver.apply(&g, Action::Stop).unwrap();
    g.hw.sound = false;
    driver.apply(&g, Action::Start).unwrap();
    r.boot();
    assert!(r.ask("grep -l 0x0403 /sys/bus/pci/devices/*/class || true").iter().all(|l| !l.starts_with("/sys")));
    driver.apply(&g, Action::Stop).unwrap();
}
