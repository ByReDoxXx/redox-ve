//! Sertifika isteği (PKCS #10) üretmek ve sertifikanın bitiş tarihini okumak için gereken kadar DER.

fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match body.len() {
        n if n < 0x80 => out.push(n as u8),
        n if n < 0x100 => out.extend_from_slice(&[0x81, n as u8]),
        n => out.extend_from_slice(&[0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(body);
    out
}

fn sequence(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

/// P-256 açık anahtarının SubjectPublicKeyInfo başı; ardından 65 baytlık nokta gelir.
const P256_SPKI: [u8; 26] = [0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00];
const OID_EXTENSION_REQUEST: [u8; 11] = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x0e];
const OID_SUBJECT_ALT_NAME: [u8; 5] = [0x06, 0x03, 0x55, 0x1d, 0x11];
const ECDSA_WITH_SHA256: [u8; 12] = [0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];

/// Alan adlarını `subjectAltName` olarak taşıyan, `sign` ile imzalanmış sertifika isteği. Konu adı boştur
/// (ACME adı SAN'dan okur).
pub fn request(point: &[u8], names: &[String], sign: impl FnOnce(&[u8]) -> Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
    let alternatives: Vec<u8> = names.iter().flat_map(|n| tlv(0x82, n.as_bytes())).collect();
    let extension = sequence(&[&OID_SUBJECT_ALT_NAME, &tlv(0x04, &tlv(0x30, &alternatives))]);
    let attribute = sequence(&[&OID_EXTENSION_REQUEST, &tlv(0x31, &sequence(&[&extension]))]);
    let spki = [&P256_SPKI[..], point].concat();
    let info = sequence(&[&tlv(0x02, &[0]), &sequence(&[]), &spki, &tlv(0xa0, &attribute)]);
    let signature = sign(&info)?;
    Ok(sequence(&[&info, &ECDSA_WITH_SHA256, &tlv(0x03, &[&[0u8][..], &signature].concat())]))
}

/// `(etiket, içerik, kalan)`.
fn read(bytes: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = bytes.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 3 || rest.len() < count {
            return None;
        }
        (rest[..count].iter().fold(0, |sum, b| sum << 8 | usize::from(*b)), &rest[count..])
    };
    (rest.len() >= length).then(|| (tag, &rest[..length], &rest[length..]))
}

/// UTCTime (`YYAAGGSSDDsnZ`) ya da GeneralizedTime → Unix saniyesi.
fn time(tag: u8, text: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(text).ok()?.strip_suffix('Z')?;
    let full = match tag {
        // RFC 5280: iki haneli yıl 50'den küçükse 2000'ler.
        0x17 if text.len() == 12 => format!("{}{text}", if &text[..2] < "50" { "20" } else { "19" }),
        0x18 if text.len() == 14 => text.to_string(),
        _ => return None,
    };
    let parsed = chrono::NaiveDateTime::parse_from_str(&full, "%Y%m%d%H%M%S").ok()?;
    u64::try_from(parsed.and_utc().timestamp()).ok()
}

/// Sertifikanın (DER) geçerliliğinin bittiği an.
pub fn not_after(certificate: &[u8]) -> Option<u64> {
    let (0x30, outer, _) = read(certificate)? else { return None };
    let (0x30, tbs, _) = read(outer)? else { return None };
    let mut rest = tbs;
    // İsteğe bağlı sürüm alanı.
    if rest.first() == Some(&0xa0) {
        rest = read(rest)?.2;
    }
    // Seri numarası, imza algoritması, veren.
    for _ in 0..3 {
        rest = read(rest)?.2;
    }
    let (0x30, validity, _) = read(rest)? else { return None };
    let (_, _, after) = read(validity)?;
    let (tag, text, _) = read(after)?;
    time(tag, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lengths_round_trip_and_times_are_read() {
        for size in [0, 1, 127, 128, 255, 256, 4000] {
            let encoded = tlv(0x04, &vec![7; size]);
            assert_eq!(read(&encoded), Some((0x04, &vec![7u8; size][..], &[][..])));
        }
        assert_eq!(read(&[0x30, 5, 1]), None);
        assert_eq!(time(0x17, b"700101000000Z"), Some(0), "50 ve üstü 1900'lerdir");
        assert_eq!(time(0x17, b"690101000000Z"), None, "1970 öncesi gösterilemez");
        assert_eq!(time(0x17, b"260101000000Z"), Some(1_767_225_600));
        assert_eq!(time(0x18, b"20260101000000Z"), Some(1_767_225_600));
        assert_eq!(time(0x17, b"260101000000"), None);
        assert_eq!(not_after(&[1, 2, 3]), None);
    }
}
