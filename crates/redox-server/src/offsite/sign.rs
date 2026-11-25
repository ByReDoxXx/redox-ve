//! S3 istek imzası (SigV4) ve küçük XML/URI yardımcıları.

use super::*;

/// AWS'nin URI kodlaması: harf, rakam ve `-_.~` dışındaki her bayt `%XX` olur.
pub fn uri_encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// XML yanıtındaki ilk `<etiket>değer</etiket>` değerini döndürür.
pub(super) fn xml_value(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].replace("&quot;", "\"").replace("&amp;", "&"))
}

pub(super) fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC her uzunlukta anahtar kabul eder");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// AWS Signature Version 4 `Authorization` başlığı. `headers` ada göre sıralı ve küçük harfli olmalıdır.
#[allow(clippy::too_many_arguments)]
pub fn sigv4(method: &str, path: &str, headers: &[(&str, &str)], payload_hash: &str, amz_date: &str, region: &str, access_key: &str, secret_key: &str) -> String {
    sigv4_query(method, path, "", headers, payload_hash, amz_date, region, access_key, secret_key)
}

/// Sorgu dizgili istekler için imza. `query` kurallı biçimde olmalıdır: ada göre sıralı `ad=değer` çiftleri,
/// ad ve değerler `uri_encode` ile kodlanmış, değersiz parametre `ad=` olarak.
#[allow(clippy::too_many_arguments)]
pub fn sigv4_query(method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload_hash: &str, amz_date: &str, region: &str, access_key: &str, secret_key: &str) -> String {
    let date = &amz_date[..8];
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let signed = headers.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
    let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\n{payload_hash}");
    let scope = format!("{date}/{region}/s3/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
    let key = ["s3", "aws4_request"].iter().fold(hmac(&hmac(format!("AWS4{secret_key}").as_bytes(), date), region), |k, part| hmac(&k, part));
    format!("AWS4-HMAC-SHA256 Credential={access_key}/{scope},SignedHeaders={signed},Signature={}", hex(&hmac(&key, &to_sign)))
}
