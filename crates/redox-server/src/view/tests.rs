use super::*;
use crate::state::HISTORY_LEN;

#[test]
fn resample_right_aligns_and_averages() {
    let empty = resample(&[], 48);
    assert_eq!(empty.len(), 48);
    assert!(empty.iter().all(|v| *v == 1.0), "veri yokken taban çizgisi");

    // Tek örnek en sağdaki dilime düşer.
    let one = resample(&[80.0], 48);
    assert_eq!(one.len(), 48);
    assert!(one[..47].iter().all(|v| *v == 1.0) && one[47] > 1.0);

    // Dolu geçmiş: sabit değer sabit kalır, sınır dışı değerler kırpılır.
    let full = resample(&vec![50.0; HISTORY_LEN], 48);
    assert!(full.iter().all(|v| (*v - 50.0).abs() < 1e-9));
    let clipped = resample(&vec![500.0; HISTORY_LEN], 48);
    assert!(clipped.iter().all(|v| *v == 100.0));

    // Sınırdan uzun seri: yalnızca en yeni örnekler kullanılır.
    let mut long = vec![10.0; HISTORY_LEN];
    long.extend(vec![90.0; HISTORY_LEN]);
    assert!(resample(&long, 48).iter().all(|v| (*v - 90.0).abs() < 1e-9));
}

#[test]
fn query_encoding() {
    assert_eq!(enc("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    assert_eq!(enc("güvenli-_.~"), "g%C3%BCvenli-_.~");
}
