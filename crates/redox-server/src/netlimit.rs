//! Ağ hız sınırı: konuğun köprüdeki kartlarının sunucu tarafındaki TAP arayüzlerine `tc` ile uygulanır.
//!
//! QEMU'nun kendi ağ hız sınırı yoktur. Köprü yardımcısının açtığı TAP arayüzünün adı, QEMU sürecinin
//! `/proc/<pid>/fdinfo` kayıtlarından okunur. Sunucudan konuğa giden trafik (konuğun indirmesi) arayüzün
//! çıkışında biçimlendirilir (`tbf`); konuktan gelen trafik (yüklemesi) girişte sınırlanır (`police`).
//! `tc`, CAP_NET_ADMIN yetkisi ister.

use std::path::Path;
use std::process::Command;

fn valid_ifname(name: &str) -> bool {
    !name.is_empty() && name.len() <= 15 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Bir `fdinfo` kaydındaki TAP arayüz adı (`iff:\ttap0`); kayıt TAP değilse `None`.
pub fn parse_fdinfo(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("iff:")).map(|n| n.trim().to_string()).filter(|n| valid_ifname(n))
}

/// Sürecin açık tuttuğu TAP arayüzleri, dosya tanıtıcı sırasıyla. `fdinfo_dir`: `/proc/<pid>/fdinfo`.
pub fn taps_of(fdinfo_dir: &Path) -> Vec<String> {
    let mut found: Vec<(u32, String)> = std::fs::read_dir(fdinfo_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let fd: u32 = e.file_name().to_str()?.parse().ok()?;
            Some((fd, parse_fdinfo(&std::fs::read_to_string(e.path()).ok()?)?))
        })
        .collect();
    found.sort();
    let mut taps: Vec<String> = Vec::new();
    for (_, name) in found {
        if !taps.contains(&name) {
            taps.push(name);
        }
    }
    taps
}

/// Bir arayüzü iki yönde `mbps` Mbit/sn ile sınırlayan `tc` komutları.
pub fn commands(tap: &str, mbps: u32) -> Vec<Vec<String>> {
    let rate = format!("{mbps}mbit");
    // Patlama payı: düşük hızlarda en az 32 kb, yüksek hızlarda hızla orantılı (zamanlayıcı adımını karşılar).
    let burst = format!("{}kb", mbps.max(32));
    let words = |list: &[&str]| list.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    vec![
        words(&["qdisc", "replace", "dev", tap, "root", "tbf", "rate", &rate, "burst", &burst, "latency", "50ms"]),
        words(&["qdisc", "add", "dev", tap, "handle", "ffff:", "ingress"]),
        words(&["filter", "add", "dev", tap, "parent", "ffff:", "protocol", "all", "prio", "1", "u32", "match", "u32", "0", "0", "police", "rate", &rate, "burst", &burst, "drop", "flowid", ":1"]),
    ]
}

/// Sınırı verilen arayüzlerin hepsine uygular; ilk hatada durur.
pub fn apply(tc_bin: &str, taps: &[String], mbps: u32) -> Result<(), String> {
    for tap in taps.iter().filter(|t| valid_ifname(t)) {
        for args in commands(tap, mbps) {
            let out = Command::new(tc_bin).args(&args).output().map_err(|e| format!("{tc_bin} çalıştırılamadı: {e} (iproute2 kurulu mu?)"))?;
            if !out.status.success() {
                let message = String::from_utf8_lossy(&out.stderr);
                return Err(format!("tc {}: {}", args[..4].join(" "), message.lines().last().unwrap_or("bilinmeyen hata").trim()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_names_come_from_fdinfo_in_descriptor_order() {
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t0104002\nmnt_id:\t27\niff:\ttap3\n").as_deref(), Some("tap3"));
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t02\n"), None, "TAP olmayan tanıtıcı");
        assert_eq!(parse_fdinfo("iff:\ttap0; rm -rf /\n"), None, "geçersiz ad komuta ulaşmaz");
        assert_eq!(parse_fdinfo("iff:\tcok-uzun-bir-arayuz-adi\n"), None);

        let dir = std::env::temp_dir().join(format!("redox-fdinfo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (fd, text) in [("3", "pos:\t0\n"), ("12", "iff:\ttap7\n"), ("9", "iff:\ttap2\n"), ("15", "iff:\ttap7\n"), ("x", "iff:\ttap9\n")] {
            std::fs::write(dir.join(fd), text).unwrap();
        }
        assert_eq!(taps_of(&dir), ["tap2", "tap7"], "sayı sırasıyla, yinelenmeden");
        assert!(taps_of(&dir.join("yok")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn commands_shape_both_directions() {
        let c: Vec<String> = commands("tap0", 100).iter().map(|a| a.join(" ")).collect();
        assert_eq!(
            c,
            [
                "qdisc replace dev tap0 root tbf rate 100mbit burst 100kb latency 50ms",
                "qdisc add dev tap0 handle ffff: ingress",
                "filter add dev tap0 parent ffff: protocol all prio 1 u32 match u32 0 0 police rate 100mbit burst 100kb drop flowid :1",
            ]
        );
        assert!(commands("tap0", 5)[0].join(" ").contains("rate 5mbit burst 32kb"), "düşük hızda patlama payı tabanı");
    }

    #[test]
    fn missing_tc_is_a_clear_error() {
        let err = apply("redox-olmayan-tc", &["tap0".to_string()], 10).unwrap_err();
        assert!(err.contains("iproute2 kurulu mu"), "{err}");
        assert!(apply("redox-olmayan-tc", &[], 10).is_ok(), "arayüz yoksa komut çalıştırılmaz");
        assert!(apply("redox-olmayan-tc", &["tap0;x".to_string()], 10).is_ok(), "geçersiz ad atlanır");
    }
}
