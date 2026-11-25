//! Parola özeti (Argon2), rastgele değerler, parola kuralları ve özel dosya yazımı.

use super::*;

/// Parola özetinin maliyeti. `Test` yalnızca testleri hızlandırmak içindir.
#[derive(Debug, Clone, Copy)]
pub enum Cost {
    Production,
    Test,
}

pub(super) fn argon(cost: Cost) -> Argon2<'static> {
    let params = match cost {
        // OWASP önerisi: Argon2id, 19 MiB bellek, 2 tur, 1 iş parçacığı.
        Cost::Production => Params::new(19_456, 2, 1, None),
        Cost::Test => Params::new(8, 1, 1, None),
    }
    .expect("sabit Argon2 parametreleri geçerli");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub(super) fn fill_random(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("işletim sistemi rastgele sayı üretemedi");
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hash_password(password: &str, cost: Cost) -> String {
    let mut salt = [0u8; 16];
    fill_random(&mut salt);
    let salt = SaltString::encode_b64(&salt).expect("16 baytlık tuz kodlanabilir");
    argon(cost)
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2 özetleme başarısız olmamalı")
        .to_string()
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// 256 bit rastgele, onaltılık token (oturum ve API için).
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes);
    hex(&bytes)
}

pub fn sha256_hex(input: &str) -> String {
    hex(&Sha256::digest(input.as_bytes()))
}

pub fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

pub fn check_password_policy(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!("parola en az {MIN_PASSWORD_CHARS} karakter olmalı"));
    }
    if password.len() > 1024 {
        return Err("parola 1024 bayttan uzun olamaz".to_string());
    }
    Ok(())
}

/// Dosyayı yalnızca sahibinin okuyabileceği izinlerle ve atomik olarak yazar.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}
