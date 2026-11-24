//! İki adımlı doğrulama: zamana dayalı tek kullanımlık kodlar (TOTP, RFC 6238; HMAC-SHA1, 6 hane, 30 sn).

use hmac::{Hmac, Mac};
use sha1::Sha1;

pub const STEP_SECS: u64 = 30;
const DIGITS: u32 = 6;
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, dolgusuz (kimlik doğrulama uygulamalarının beklediği biçim).
pub fn base32_encode(bytes: &[u8]) -> String {
    let (mut out, mut buffer, mut bits) = (String::new(), 0u32, 0u32);
    for &b in bytes {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=' && *c != b' ') {
        let value = ALPHABET.iter().position(|a| *a == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// 160 bit rastgele gizli anahtar, base32.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 20];
    getrandom::getrandom(&mut bytes).expect("işletim sistemi rastgele sayı üretemedi");
    base32_encode(&bytes)
}

/// Verilen zaman adımının kodu (RFC 4226 dinamik kırpma).
pub fn code_at(secret: &[u8], step: u64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC her uzunlukta anahtar kabul eder");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let value = u32::from_be_bytes([digest[offset], digest[offset + 1], digest[offset + 2], digest[offset + 3]]) & 0x7fff_ffff;
    value % 10u32.pow(DIGITS)
}

/// Kodu doğrular; saat kayması için bir önceki ve bir sonraki adım da kabul edilir.
/// Eşleşen adımı döndürür (aynı kodun yeniden kullanılmasını engellemek için saklanır).
pub fn verify(secret_b32: &str, code: &str, now: u64) -> Option<u64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secret = base32_decode(secret_b32)?;
    let wanted: u32 = code.parse().ok()?;
    let current = now / STEP_SECS;
    // Zamanlama saldırısına karşı bütün adaylar denenir; erken çıkılmaz.
    let mut matched = None;
    for step in [current.saturating_sub(1), current, current + 1] {
        if code_at(&secret, step) == wanted {
            matched = Some(step);
        }
    }
    matched
}

/// Kimlik doğrulama uygulamasının QR koddan okuduğu adres.
pub fn otpauth_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    let enc = crate::view::enc;
    format!(
        "otpauth://totp/{}:{}?secret={secret_b32}&issuer={}&algorithm=SHA1&digits=6&period=30",
        enc(issuer),
        enc(account),
        enc(issuer)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_matches_rfc4648_vectors() {
        for (plain, encoded) in [("", ""), ("f", "MY"), ("fo", "MZXQ"), ("foo", "MZXW6"), ("foob", "MZXW6YQ"), ("fooba", "MZXW6YTB"), ("foobar", "MZXW6YTBOI")] {
            assert_eq!(base32_encode(plain.as_bytes()), encoded);
            assert_eq!(base32_decode(encoded).unwrap(), plain.as_bytes());
        }
        assert_eq!(base32_decode("mzxw 6ytb oi==").unwrap(), b"foobar", "küçük harf, boşluk ve dolgu kabul edilir");
        assert_eq!(base32_decode("MZXW1"), None, "alfabe dışı karakter reddedilir");
    }

    #[test]
    fn codes_match_rfc6238_sha1_vectors() {
        // RFC 6238 Ek B (SHA1, anahtar "12345678901234567890"); 8 haneli değerlerin son 6 hanesi.
        let secret = b"12345678901234567890";
        for (time, code) in [(59u64, 287_082), (1_111_111_109, 81_804), (1_111_111_111, 50_471), (1_234_567_890, 5_924), (2_000_000_000, 279_037), (20_000_000_000, 353_130)] {
            assert_eq!(code_at(secret, time / STEP_SECS), code, "t={time}");
        }
    }

    #[test]
    fn verify_accepts_adjacent_steps_only() {
        let secret = base32_encode(b"12345678901234567890");
        let now = 1_111_111_111;
        let current = format!("{:06}", code_at(b"12345678901234567890", now / STEP_SECS));
        assert_eq!(verify(&secret, &current, now), Some(now / STEP_SECS));
        assert_eq!(verify(&secret, &format!(" {current} "), now), Some(now / STEP_SECS), "boşluklar yok sayılır");
        assert_eq!(verify(&secret, &current, now + STEP_SECS), Some(now / STEP_SECS), "bir adım geriden gelen kod geçerli");
        assert_eq!(verify(&secret, &current, now - STEP_SECS), Some(now / STEP_SECS));
        assert_eq!(verify(&secret, &current, now + 2 * STEP_SECS), None, "iki adım eski kod geçersiz");
        for bad in ["", "12345", "1234567", "abcdef", "12 345", "-12345"] {
            assert_eq!(verify(&secret, bad, now), None, "{bad:?}");
        }
        assert_eq!(verify("geçersiz!", &current, now), None);
        // Baştaki sıfırlar korunur: 005924.
        assert_eq!(verify(&secret, "005924", 1_234_567_890), Some(1_234_567_890 / STEP_SECS));
        assert_eq!(verify(&secret, "5924", 1_234_567_890), None);
    }

    #[test]
    fn secrets_and_uri() {
        let (a, b) = (generate_secret(), generate_secret());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert_eq!(base32_decode(&a).unwrap().len(), 20);
        assert_eq!(
            otpauth_uri("RedoX VE", "admin", "ABC"),
            "otpauth://totp/RedoX%20VE:admin?secret=ABC&issuer=RedoX%20VE&algorithm=SHA1&digits=6&period=30"
        );
    }
}
