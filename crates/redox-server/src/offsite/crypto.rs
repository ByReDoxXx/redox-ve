//! Dış kopyaların şifrelenmesi: anahtar türetme, akış şifreleme ve çözme.

use super::*;

/// Anahtar türetme (Argon2id) bilerek pahalıdır; artımlı yedekte binlerce küçük dosya gittiği için
/// aynı parola ve tuzla türetilen anahtar bellekte tutulur.
pub(super) fn derive_key(passphrase: &str, salt: &[u8]) -> Result<ChaCha20Poly1305, String> {
    type Entry = ([u8; 32], Vec<u8>, ChaCha20Poly1305);
    static CACHE: std::sync::Mutex<Vec<Entry>> = std::sync::Mutex::new(Vec::new());
    let owner: [u8; 32] = Sha256::digest(passphrase.as_bytes()).into();
    let cached = |cache: &Vec<Entry>| cache.iter().find(|(who, s, _)| *who == owner && s.as_slice() == salt).map(|(_, _, cipher)| cipher.clone());
    if let Some(cipher) = cached(&CACHE.lock().unwrap_or_else(|e| e.into_inner())) {
        return Ok(cipher);
    }
    let cipher = derive_key_uncached(passphrase, salt)?;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 16 {
        cache.remove(0);
    }
    cache.push((owner, salt.to_vec(), cipher.clone()));
    Ok(cipher)
}

pub fn derive_key_uncached(passphrase: &str, salt: &[u8]) -> Result<ChaCha20Poly1305, String> {
    let params = argon2::Params::new(19_456, 2, 1, Some(32)).map_err(|e| e.to_string())?;
    let mut key = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| format!("şifreleme anahtarı türetilemedi: {e}"))?;
    Ok(ChaCha20Poly1305::new(Key::from_slice(&key)))
}

/// Parça nonce'u: rastgele önek + parça sayacı + "son parça" baytı. Son parça işareti kesilmeyi,
/// sayaç parçaların yer değiştirmesini yakalar.
pub(super) fn nonce(prefix: &[u8; 7], counter: u32, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..7].copy_from_slice(prefix);
    n[7..11].copy_from_slice(&counter.to_be_bytes());
    n[11] = last as u8;
    n
}

/// Şifreli dosyanın boyutu: başlık + veri + her parça için doğrulama etiketi.
pub fn encrypted_len(plain: u64) -> u64 {
    HEADER as u64 + plain + TAG as u64 * (plain / CHUNK as u64 + 1)
}

/// Kaynağı okurken şifreler; böylece şifreli kopya diske yazılmadan doğrudan hedefe akar.
pub struct EncryptReader<R: Read> {
    inner: R,
    cipher: ChaCha20Poly1305,
    prefix: [u8; 7],
    /// Dosyanın hedefteki adı: her parçanın doğrulamasına katılır.
    name: Vec<u8>,
    counter: u32,
    out: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> EncryptReader<R> {
    /// `name`: dosyanın hedefteki (şifreleme eki olmayan) adı.
    pub fn new(inner: R, passphrase: &str, name: &str) -> Result<Self, String> {
        // Tuz sunucu her başladığında bir kez üretilir (anahtar bir kez türetilsin diye); dosyaları
        // birbirinden ayıran, her dosya için yeniden üretilen 56 bitlik nonce önekidir.
        static SALT: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();
        let random = |buf: &mut [u8]| getrandom::getrandom(buf).map_err(|e| format!("rastgele sayı üretilemedi: {e}"));
        let salt = match SALT.get() {
            Some(salt) => *salt,
            None => {
                let mut fresh = [0u8; 16];
                random(&mut fresh)?;
                *SALT.get_or_init(|| fresh)
            }
        };
        let mut prefix = [0u8; 7];
        random(&mut prefix)?;
        let mut out = MAGIC_BOUND.to_vec();
        out.extend_from_slice(&salt);
        out.extend_from_slice(&prefix);
        Ok(EncryptReader { inner, cipher: derive_key(passphrase, &salt)?, prefix, name: name.as_bytes().to_vec(), counter: 0, out, pos: 0, done: false })
    }
}

impl<R: Read> Read for EncryptReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.out.len() {
            if self.done {
                return Ok(0);
            }
            let mut chunk = vec![0u8; CHUNK];
            let mut filled = 0;
            while filled < CHUNK {
                match self.inner.read(&mut chunk[filled..])? {
                    0 => break,
                    n => filled += n,
                }
            }
            // Tam dolu parça son değildir: ardından (boş da olsa) işaretli bir son parça gelir.
            let last = filled < CHUNK;
            let sealed = self
                .cipher
                .encrypt(Nonce::from_slice(&nonce(&self.prefix, self.counter, last)), Payload { msg: &chunk[..filled], aad: &self.name })
                .map_err(|_| std::io::Error::other("şifreleme başarısız"))?;
            self.counter = self.counter.checked_add(1).ok_or_else(|| std::io::Error::other("dosya şifrelemek için çok büyük"))?;
            self.done = last;
            self.out = sealed;
            self.pos = 0;
        }
        let n = buf.len().min(self.out.len() - self.pos);
        buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

pub(super) fn read_full(source: &mut dyn Read, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match source.read(&mut buf[filled..]).map_err(|e| format!("dış kopya okunamadı: {e}"))? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Şifreli akışı çözer. Yanlış parola, değiştirilmiş ya da kesilmiş veri ve başka bir adla gönderilmiş dosya
/// hata verir. `name`: dosyanın hedefteki adı. `require_bound`: ada bağlı olmayan eski biçim kabul edilmez
/// (yoksa hedefe yazabilen biri, yeni bir kopyanın yerine eski biçimli başka bir yedeği koyabilirdi).
pub fn decrypt(source: &mut dyn Read, passphrase: &str, out: &mut dyn Write, name: &str, require_bound: bool) -> Result<(), String> {
    const BAD: &str = "dış kopya çözülemedi: parola yanlış, dosya bozulmuş ya da başka bir yedeğe ait";
    let mut header = [0u8; HEADER];
    let read = read_full(source, &mut header)?;
    let bound = read == HEADER && &header[..8] == MAGIC_BOUND;
    if !bound && (read != HEADER || &header[..8] != MAGIC) {
        return Err("dış kopya şifreli RedoX yedeği değil".to_string());
    }
    if !bound && require_bound {
        return Err("dış kopya bu yedeğin adına bağlı değil (eski biçim): hedefteki dosya başka bir yedekle değiştirilmiş olabilir".to_string());
    }
    let aad: &[u8] = if bound { name.as_bytes() } else { &[] };
    let cipher = derive_key(passphrase, &header[8..24])?;
    let prefix: [u8; 7] = header[24..].try_into().expect("7 bayt");
    let mut buf = vec![0u8; CHUNK + TAG];
    for counter in 0u32.. {
        let n = read_full(source, &mut buf)?;
        if n < TAG {
            return Err("dış kopya eksik (kesilmiş dosya)".to_string());
        }
        let last = n < CHUNK + TAG;
        let plain = cipher.decrypt(Nonce::from_slice(&nonce(&prefix, counter, last)), Payload { msg: &buf[..n], aad }).map_err(|_| BAD.to_string())?;
        out.write_all(&plain).map_err(|e| format!("yedek dosyası yazılamadı: {e}"))?;
        if last {
            return Ok(());
        }
    }
    Err(BAD.to_string())
}
