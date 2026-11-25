//! Yerel deponun şifrelenmesi: parçalar diske şifreli yazılır.
//!
//! Anahtar paroladan Argon2id ile türetilir; tuz ve parolanın doğruluğunu sınayan küçük bir denetim değeri
//! depodaki `redox-seal.json` dosyasında durur. Her parça kendi rastgele nonce'uyla ChaCha20-Poly1305 ile
//! şifrelenir ve içerik özetine (dosya adına) bağlanır: başka bir parçanın yerine konan dosya çözülmez.
//! Dizin dosyaları (parça özetleri ve disk boyutları) şifrelenmez.

use super::*;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};

pub(super) const MAGIC: &[u8; 8] = b"RDXSEAL1";
const MARKER: &str = "redox-seal.json";
const CHECK: &[u8] = b"redox-ve yerel yedek deposu";
const NONCE: usize = 12;

pub struct Seal {
    cipher: ChaCha20Poly1305,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    salt: String,
    check: String,
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

/// Depo daha önce şifrelenmiş mi?
pub fn marked(dir: &Path) -> bool {
    dir.join(MARKER).is_file()
}

impl Seal {
    /// Deponun anahtarını açar; depo ilk kez şifreleniyorsa işaret dosyasını oluşturur.
    pub(super) fn open(dir: &Path, passphrase: &str) -> Result<Seal, String> {
        if passphrase.len() < 12 {
            return Err("yerel yedek parolası en az 12 karakter olmalıdır".to_string());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{} oluşturulamadı: {e}", dir.display()))?;
        let path = dir.join(MARKER);
        match std::fs::read(&path) {
            Ok(text) => {
                let marker: Marker = serde_json::from_slice(&text).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
                let (salt, check) = unhex(&marker.salt).zip(unhex(&marker.check)).ok_or_else(|| format!("{} bozuk", path.display()))?;
                let seal = Seal { cipher: crate::offsite::derive_key_uncached(passphrase, &salt)? };
                match seal.unseal(&check, MARKER.as_bytes()) {
                    Ok(plain) if plain == CHECK => Ok(seal),
                    _ => Err("yerel yedek parolası yanlış: bu depo başka bir parolayla şifrelenmiş".to_string()),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut salt = [0u8; 16];
                getrandom::getrandom(&mut salt).map_err(|e| format!("rastgele sayı üretilemedi: {e}"))?;
                let seal = Seal { cipher: crate::offsite::derive_key_uncached(passphrase, &salt)? };
                let marker = Marker { salt: hex(&salt), check: hex(&seal.seal(CHECK, MARKER.as_bytes())?) };
                write_atomic(&path, &serde_json::to_vec(&marker).map_err(|e| e.to_string())?)?;
                Ok(seal)
            }
            Err(e) => Err(format!("{} okunamadı: {e}", path.display())),
        }
    }

    /// `MAGIC + nonce + şifreli veri`; `aad` (parçanın özeti) doğrulamaya katılır.
    pub(super) fn seal(&self, data: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; NONCE];
        getrandom::getrandom(&mut nonce).map_err(|e| format!("rastgele sayı üretilemedi: {e}"))?;
        let sealed = self.cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: data, aad }).map_err(|_| "parça şifrelenemedi".to_string())?;
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    pub(super) fn unseal(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>, ()> {
        let body = sealed.strip_prefix(&MAGIC[..]).filter(|b| b.len() >= NONCE).ok_or(())?;
        self.cipher.decrypt(Nonce::from_slice(&body[..NONCE]), Payload { msg: &body[NONCE..], aad }).map_err(|_| ())
    }
}
