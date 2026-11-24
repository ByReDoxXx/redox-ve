//! Simüle edilmiş kullanım geçmişi.
//!
//! Gerçek bir hipervizör sürücüsü bağlanana kadar grafikler buradan beslenir. Seri,
//! tohuma göre belirlenimcidir: aynı konuk her yenilemede aynı eğriyi çizer.

/// `base` çevresinde salınan, 1-97 aralığında `n` noktalı bir yüzde serisi üretir.
/// `base <= 0` ise (kapalı konuk) düz bir taban çizgisi döner.
pub fn series(seed: u64, base: f64, n: usize) -> Vec<f64> {
    if base <= 0.0 {
        return vec![1.0; n];
    }
    let mut x = seed % 233_280;
    let mut next = || {
        x = (x * 9301 + 49_297) % 233_280;
        x as f64 / 233_280.0
    };
    (0..n)
        .map(|i| {
            let wave = ((i as f64) / 5.0 + seed as f64).sin() * 8.0;
            (base + (next() - 0.5) * 24.0 + wave).clamp(3.0, 97.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_per_seed() {
        assert_eq!(series(102, 58.0, 48), series(102, 58.0, 48));
        assert_ne!(series(102, 58.0, 48), series(103, 58.0, 48));
    }

    #[test]
    fn stays_in_bounds() {
        for base in [1.0, 50.0, 99.0, 500.0] {
            let s = series(7, base, 200);
            assert_eq!(s.len(), 200);
            assert!(s.iter().all(|v| (3.0..=97.0).contains(v)), "base {base}");
        }
    }

    #[test]
    fn flat_when_idle() {
        assert_eq!(series(9, 0.0, 5), vec![1.0; 5]);
        assert_eq!(series(9, -3.0, 2), vec![1.0; 2]);
    }

    #[test]
    fn huge_seed_does_not_overflow() {
        assert_eq!(series(u64::MAX, 40.0, 10).len(), 10);
    }
}
