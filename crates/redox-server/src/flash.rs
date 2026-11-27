//! Sayfa bildirimlerinin imzası.
//!
//! Bir işlemin sonucu ("silindi", "parola hatalı") yönlendirmenin adresinde taşınır. İmzasız olsaydı, hazırlanmış
//! bir bağlantı panelin kendi bildirim kutusunda dilediği metni gösterebilirdi ("oturumunuz askıya alındı, şu
//! numarayı arayın"). Bu yüzden metin sunucunun anahtarıyla imzalanır ve yalnızca imzası tutan, taze bildirim
//! gösterilir. Anahtar süreçle birlikte doğar: yeniden başlatmadan önceki bağlantılar bildirim göstermez.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::OnceLock;
use subtle::ConstantTimeEq;

/// Bildirim bu süre boyunca gösterilir (sayfayı yenilemek ya da geri dönmek için yeterli).
pub const TTL_SECS: u64 = 10 * 60;

fn key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        // Rastgele sayı alınamıyorsa sunucu zaten oturum da açamaz; burada sessizce sabit anahtara düşülmez.
        getrandom::getrandom(&mut key).expect("rastgele sayı üretilemedi");
        key
    })
}

/// `kind` (`notice` ya da `error`), metin ve anın imzası (onaltılık).
pub fn sign(kind: &str, message: &str, at: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key()).expect("HMAC her uzunlukta anahtar alır");
    mac.update(kind.as_bytes());
    mac.update(&[0]);
    mac.update(&at.to_be_bytes());
    mac.update(message.as_bytes());
    mac.finalize().into_bytes()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Bildirim bu sunucunun ürettiği, süresi geçmemiş bir bildirim mi?
pub fn valid(kind: &str, message: &str, at: u64, signature: &str, now: u64) -> bool {
    let fresh = at <= now.saturating_add(60) && now < at.saturating_add(TTL_SECS);
    fresh && bool::from(sign(kind, message, at).as_bytes().ct_eq(signature.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fresh_untouched_notices_verify() {
        let s = sign("error", "Parola hatalı.", 1_000);
        assert!(valid("error", "Parola hatalı.", 1_000, &s, 1_000 + TTL_SECS - 1));
        assert!(!valid("error", "Parola hatalı.", 1_000, &s, 1_000 + TTL_SECS), "süresi geçti");
        assert!(!valid("error", "Oturumunuz askıya alındı.", 1_000, &s, 1_001), "metin değişti");
        assert!(!valid("notice", "Parola hatalı.", 1_000, &s, 1_001), "tür değişti");
        assert!(!valid("error", "Parola hatalı.", 1_001, &s, 1_001), "an değişti");
        assert!(!valid("error", "Parola hatalı.", 1_000, "", 1_001) && !valid("error", "Parola hatalı.", 1_000, "00", 1_001));
        assert!(!valid("error", "Parola hatalı.", 9_000, &sign("error", "Parola hatalı.", 9_000), 1_000), "gelecekten gelen an");
    }
}
