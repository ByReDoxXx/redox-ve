//! WebAuthn (güvenlik anahtarı) yanıtlarının sunucu tarafı denetimi (W3C WebAuthn Level 2, §7.1 ve §7.2).
//!
//! Kanıtlama (attestation) istenmez ve denetlenmez: anahtarın üreticisi sorulmaz, yalnızca aynı anahtarın
//! sonradan imza atabildiği doğrulanır. Açık anahtar, tarayıcının verdiği SPKI biçiminde saklanır; bu yüzden
//! CBOR/COSE ayrıştırılmaz. Desteklenen imzalar: ES256, EdDSA (Ed25519) ve RS256.

use ring::signature::{self, UnparsedPublicKey, VerificationAlgorithm};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const ALG_ES256: i32 = -7;
pub const ALG_EDDSA: i32 = -8;
pub const ALG_RS256: i32 = -257;

/// Kullanıcı anahtara dokundu.
const FLAG_PRESENT: u8 = 0x01;
/// Yanıtta yeni anahtarın bilgisi var (yalnızca kayıtta).
const FLAG_ATTESTED: u8 = 0x40;
const MAX_FIELD: usize = 4096;

/// Güvenen taraf: panelin tarayıcıda açıldığı adres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relying {
    /// `https://ad[:port]`; tarayıcının bildirdiği kaynakla birebir aynı olmalıdır.
    pub origin: String,
    /// Alan adı: anahtar bu ada bağlanır.
    pub id: String,
}

impl Relying {
    pub fn parse(text: &str) -> Result<Relying, String> {
        let text = text.trim().trim_end_matches('/');
        let bad = || format!("geçersiz --webauthn-origin \"{text}\": https://alan-adı[:port] biçiminde olmalı");
        let (scheme, rest) = text.split_once("://").ok_or_else(bad)?;
        let (host, port) = match rest.split_once(':') {
            Some((host, port)) => (host, Some(port.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(bad)?)),
            None => (rest, None),
        };
        let named = !host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-') && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        if !named {
            return Err(bad());
        }
        // Tarayıcılar IP adresine güvenlik anahtarı bağlamaz.
        if host.split('.').all(|label| label.bytes().all(|b| b.is_ascii_digit())) {
            return Err("--webauthn-origin bir IP adresi olamaz: tarayıcılar güvenlik anahtarını yalnızca bir alan adına bağlar".to_string());
        }
        let default_port = match scheme {
            "https" => 443,
            // Şifresiz bağlantıya tarayıcı yalnızca kendi makinesinde izin verir.
            "http" if host == "localhost" => 80,
            _ => return Err(bad()),
        };
        let origin = match port {
            Some(port) if port != default_port => format!("{scheme}://{host}:{port}"),
            _ => format!("{scheme}://{host}"),
        };
        Ok(Relying { origin, id: host.to_string() })
    }
}

fn sized<'a>(bytes: &'a [u8], what: &str) -> Result<&'a [u8], String> {
    if bytes.is_empty() || bytes.len() > MAX_FIELD {
        return Err(format!("güvenlik anahtarı yanıtı geçersiz: {what}"));
    }
    Ok(bytes)
}

/// Tarayıcının imzalattığı bağlam: doğru işlem, bizim ürettiğimiz sınama, bizim adresimiz.
fn check_client_data(rp: &Relying, bytes: &[u8], kind: &str, challenge: &str) -> Result<(), String> {
    let data: Value = serde_json::from_slice(sized(bytes, "clientDataJSON")?).map_err(|_| "güvenlik anahtarı yanıtı geçersiz: clientDataJSON".to_string())?;
    if data["type"].as_str() != Some(kind) {
        return Err("güvenlik anahtarı yanıtı başka bir işleme ait".to_string());
    }
    if data["challenge"].as_str() != Some(challenge) {
        return Err("güvenlik anahtarı yanıtındaki sınama uyuşmuyor".to_string());
    }
    if data["origin"].as_str() != Some(rp.origin.as_str()) {
        return Err(format!("güvenlik anahtarı yanıtı başka bir adresten: {} (beklenen {})", data["origin"].as_str().unwrap_or("?"), rp.origin));
    }
    if data["crossOrigin"].as_bool() == Some(true) {
        return Err("güvenlik anahtarı başka bir sitenin çerçevesinden kullanılamaz".to_string());
    }
    Ok(())
}

/// Doğrulayıcı verisinin ortak başı: alan adı özeti, bayraklar ve imza sayacı.
fn check_auth_data(rp: &Relying, bytes: &[u8]) -> Result<(u8, u32), String> {
    let bytes = sized(bytes, "authenticatorData")?;
    if bytes.len() < 37 {
        return Err("güvenlik anahtarı yanıtı geçersiz: authenticatorData".to_string());
    }
    if bytes[..32] != Sha256::digest(rp.id.as_bytes())[..] {
        return Err("güvenlik anahtarı yanıtı başka bir alan adına ait".to_string());
    }
    let flags = bytes[32];
    if flags & FLAG_PRESENT == 0 {
        return Err("güvenlik anahtarına dokunulmadı".to_string());
    }
    Ok((flags, u32::from_be_bytes([bytes[33], bytes[34], bytes[35], bytes[36]])))
}

/// DER: beklenen etiketli ilk öğenin içeriği ve ondan sonrası.
fn der(bytes: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&found, rest) = bytes.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 2 || rest.len() < count {
            return None;
        }
        (rest[..count].iter().fold(0, |sum, b| sum << 8 | usize::from(*b)), &rest[count..])
    };
    (found == tag && rest.len() >= length).then(|| rest.split_at(length))
}

/// SubjectPublicKeyInfo içinden, imza türüne uygun anahtar baytları.
fn public_key(alg: i32, spki: &[u8]) -> Result<(&'static dyn VerificationAlgorithm, &[u8]), String> {
    const P256: [u8; 26] = [0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00];
    const ED25519: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    const RSA: [u8; 13] = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00];
    let bad = || "güvenlik anahtarının açık anahtarı geçersiz".to_string();
    match alg {
        ALG_ES256 => {
            let point = spki.strip_prefix(&P256[..]).filter(|p| p.len() == 65 && p[0] == 4).ok_or_else(bad)?;
            Ok((&signature::ECDSA_P256_SHA256_ASN1, point))
        }
        ALG_EDDSA => {
            let key = spki.strip_prefix(&ED25519[..]).filter(|k| k.len() == 32).ok_or_else(bad)?;
            Ok((&signature::ED25519, key))
        }
        ALG_RS256 => {
            let (outer, tail) = der(spki, 0x30).ok_or_else(bad)?;
            let (algorithm, rest) = der(outer, 0x30).ok_or_else(bad)?;
            let (bits, extra) = der(rest, 0x03).ok_or_else(bad)?;
            if !tail.is_empty() || !extra.is_empty() || algorithm != RSA {
                return Err(bad());
            }
            let key = bits.strip_prefix(&[0]).ok_or_else(bad)?;
            Ok((&signature::RSA_PKCS1_2048_8192_SHA256, key))
        }
        other => Err(format!("desteklenmeyen imza türü: {other}")),
    }
}

/// Kayıt yanıtı: tarayıcının `create()` sonucu.
pub struct Registration<'a> {
    pub credential_id: &'a [u8],
    pub client_data: &'a [u8],
    pub auth_data: &'a [u8],
    pub alg: i32,
    pub public_key: &'a [u8],
}

/// Yeni anahtarı denetler ve başlangıç imza sayacını döndürür.
pub fn verify_registration(rp: &Relying, challenge: &str, r: &Registration) -> Result<u32, String> {
    check_client_data(rp, r.client_data, "webauthn.create", challenge)?;
    let (flags, count) = check_auth_data(rp, r.auth_data)?;
    let bad = || "güvenlik anahtarı yanıtı geçersiz: yeni anahtarın kimliği".to_string();
    // Başlığın ardından: 16 bayt model kimliği, 2 bayt uzunluk, anahtar kimliği.
    if flags & FLAG_ATTESTED == 0 || r.auth_data.len() < 55 {
        return Err(bad());
    }
    let length = usize::from(u16::from_be_bytes([r.auth_data[53], r.auth_data[54]]));
    if !(16..=1023).contains(&length) || r.auth_data.get(55..55 + length) != Some(r.credential_id) {
        return Err(bad());
    }
    public_key(r.alg, sized(r.public_key, "açık anahtar")?)?;
    Ok(count)
}

/// Giriş yanıtı: tarayıcının `get()` sonucu.
pub struct Assertion<'a> {
    pub client_data: &'a [u8],
    pub auth_data: &'a [u8],
    pub signature: &'a [u8],
}

/// Saklanan anahtarla imzayı denetler ve yeni imza sayacını döndürür.
pub fn verify_assertion(rp: &Relying, challenge: &str, a: &Assertion, alg: i32, spki: &[u8], stored_count: u32) -> Result<u32, String> {
    check_client_data(rp, a.client_data, "webauthn.get", challenge)?;
    let (_, count) = check_auth_data(rp, a.auth_data)?;
    let (algorithm, key) = public_key(alg, spki)?;
    let mut signed = a.auth_data.to_vec();
    signed.extend_from_slice(&Sha256::digest(a.client_data));
    UnparsedPublicKey::new(algorithm, key).verify(&signed, sized(a.signature, "imza")?).map_err(|_| "güvenlik anahtarının imzası doğrulanamadı".to_string())?;
    // Sayaç tutan anahtarda sayaç hep büyür; geriye gitmesi anahtarın kopyalandığını düşündürür.
    if (count != 0 || stored_count != 0) && count <= stored_count {
        return Err("güvenlik anahtarının sayacı geriye gitti; anahtar kopyalanmış olabilir".to_string());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_must_be_a_domain_over_https_or_this_machine() {
        assert_eq!(Relying::parse("https://pve.ornek.net:8006/").unwrap(), Relying { origin: "https://pve.ornek.net:8006".into(), id: "pve.ornek.net".into() });
        assert_eq!(Relying::parse("https://pve.ornek.net:443").unwrap().origin, "https://pve.ornek.net", "tarayıcı olağan portu yazmaz");
        assert_eq!(Relying::parse("http://localhost:8006").unwrap().id, "localhost");
        for bad in ["http://pve.ornek.net", "http://127.0.0.1:8006", "https://10.0.0.5", "https://[::1]:8006", "https://a.net/yol", "https://kim@a.net", "https://A.net", "https://a..net", "https://a.net:0", "https://a.net:70000", "a.net", "ftp://a.net", "https://"] {
            assert!(Relying::parse(bad).is_err(), "{bad}");
        }
    }

    // Node'un kendi kriptografisiyle üretilmiş vektör: ring RSA anahtarı üretmez, bağımsız bir imzalayıcı da olur.
    const RS256_SPKI: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAoFNpMPzWvBYF4NKJOMpxeG8hg0QM1Q4TIjsStWVtTEn6rAB-ObwNSFn5Ado2p8sqfkSRr7zXQXuX7RZc_ZnvqFxlfeYZ3O8i9NnbSi0k2Skc2QAYxg5uOvCbH-XkDe2tyFpqNzKunt_KvGy8cQttYE1qPIzz3q1ioNhrQsgQIs7jHQ8ZBMbSty8BkHDwbFsQYrJXdm5j2cqY1W3Iu0co7xAihvYzvnfhVM_IvXGhRga_5xT__R-8rvdG6liqbYOQIVvuQX3wdO7MGkXYPyk7J6scYpaOHeQ7bsK7ghHZryyBoxjfPE8gKYdnpNxeosjGH23SC5mvz7ogDelK3G5lwwIDAQAB";
    const RS256_SIGNATURE: &str = "d9mhI1Rm13_Ms8g5wWRmVH5yfcLBaIgVUzzSQyyZ4CVixUJ8mhkRE46oNqsi_g4rBkdnpDY-U2Hx_U53HXIFirx64WXGwRMv_cxASZJwsFc_KqIMCOeLBSzT002q8LY_hQldXP8j6vQkzxISAZpmKczqBIZHYn-bb5_992-DK9KRGugyYntp0-I2ffB2bYptId3myXQrXn15FHfl9NZbgZfGZF0UGngsqUWvn83FF1ZyzsSslq02RbdxvGSMu8LuFE0zJVUuWQ3ygEAxl86gOkxL7avDJxA1T5c0gJqPafGhxwfphjUj-QlLN2H7MsGl0w-qIMb_wgi_7uIjXkD91Q";

    #[test]
    fn an_rs256_signature_from_another_implementation_is_accepted_and_tampering_is_not() {
        use crate::oidc::base64url_decode;
        let rp = Relying::parse("https://pve.ornek.net").unwrap();
        let client_data = br#"{"type":"webauthn.get","challenge":"c2luYW1h","origin":"https://pve.ornek.net"}"#;
        let mut auth_data = Sha256::digest(b"pve.ornek.net").to_vec();
        auth_data.extend_from_slice(&[1, 0, 0, 0, 7]);
        let (spki, signature) = (base64url_decode(RS256_SPKI).unwrap(), base64url_decode(RS256_SIGNATURE).unwrap());
        let check = |auth_data: &[u8], signature: &[u8], spki: &[u8], count| verify_assertion(&rp, "c2luYW1h", &Assertion { client_data, auth_data, signature }, ALG_RS256, spki, count);
        assert_eq!(check(&auth_data, &signature, &spki, 6), Ok(7));
        assert!(check(&auth_data, &signature, &spki, 7).unwrap_err().contains("sayacı"));
        let mut changed = auth_data.clone();
        changed[36] = 8;
        assert!(check(&changed, &signature, &spki, 6).unwrap_err().contains("imzası"));
        let mut broken = signature.clone();
        broken[10] ^= 1;
        assert!(check(&auth_data, &broken, &spki, 6).is_err());
        let mut longer = spki.clone();
        longer.push(0);
        assert!(check(&auth_data, &signature, &longer, 6).unwrap_err().contains("açık anahtarı"));
        assert!(public_key(ALG_ES256, &spki).is_err() && public_key(-35, &spki).is_err());
    }

    #[test]
    fn der_reader_rejects_truncated_and_oversized_lengths() {
        assert_eq!(der(&[0x30, 2, 1, 2, 9], 0x30), Some((&[1u8, 2][..], &[9u8][..])));
        for bad in [&[][..], &[0x30][..], &[0x30, 3, 1][..], &[0x31, 0][..], &[0x30, 0x80][..], &[0x30, 0x83, 0, 0, 1, 1][..], &[0x30, 0x82, 0xff][..]] {
            assert_eq!(der(bad, 0x30), None, "{bad:?}");
        }
    }
}
