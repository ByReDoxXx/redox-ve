//! Güvenlik anahtarları (WebAuthn): kayıt, silme ve girişin ikinci adımı.
//!
//! Her istek için tek kullanımlık bir "sınama" üretilir ve kısa süre bellekte tutulur. Tarayıcı yalnızca
//! hangi sınamaya yanıt verdiğini gösteren bir bilet taşır.

use super::*;
use crate::oidc::{base64url, base64url_decode};
use crate::webauthn::{self, Assertion, Registration, Relying};

pub const MAX_KEYS: usize = 10;
/// Anahtara dokunmak için tanınan süre.
const CHALLENGE_SECS: u64 = 120;
const MAX_PENDING: usize = 1000;
const MAX_PENDING_PER_USER: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityKey {
    pub name: String,
    /// Anahtar kimliği (base64url).
    pub id: String,
    /// COSE imza türü.
    pub alg: i32,
    /// SubjectPublicKeyInfo (base64url).
    pub public_key: String,
    #[serde(default)]
    pub sign_count: u32,
    pub added: u64,
}

/// Tarayıcıya verilecek sınama.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChallenge {
    pub ticket: String,
    pub challenge: String,
    pub rp_id: String,
    pub user: String,
    /// Kayıtta: yeniden eklenmeyecek anahtarlar. Girişte: kabul edilen anahtarlar.
    pub key_ids: Vec<String>,
}

/// Tarayıcıdan dönen yanıt; ikili alanlar base64url.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct KeyResponse {
    pub ticket: String,
    pub id: String,
    pub client_data: String,
    pub auth_data: String,
    /// Yalnızca girişte.
    pub signature: String,
    /// Yalnızca kayıtta.
    pub public_key: String,
    pub alg: String,
}

#[derive(Clone)]
pub(super) struct Pending {
    user: String,
    password_hash: String,
    challenge: String,
    /// Kayıtta anahtarın adı; girişte `None`.
    register: Option<String>,
    expires: u64,
}

fn valid_key_name(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= 40 && !name.chars().any(char::is_control)
}

fn bytes(text: &str) -> Result<Vec<u8>, String> {
    base64url_decode(text).ok_or_else(|| "güvenlik anahtarı yanıtı geçersiz".to_string())
}

impl UserFile {
    pub fn remove_key(&mut self, user: &str, id: &str) -> Result<String, String> {
        let user = self.find_mut(user)?;
        let at = user.keys.iter().position(|k| k.id == id).ok_or("güvenlik anahtarı bulunamadı")?;
        Ok(user.keys.remove(at).name)
    }

    pub fn clear_keys(&mut self, user: &str) -> Result<(), String> {
        self.find_mut(user)?.keys.clear();
        Ok(())
    }

    /// İkinci adımın tamamını (doğrulama uygulaması ve güvenlik anahtarları) kapatır.
    pub fn reset_second_step(&mut self, name: &str) -> Result<(), String> {
        self.disable_totp(name)?;
        self.clear_keys(name)
    }
}

impl Auth {
    fn relying(&self) -> Result<&Relying, String> {
        self.webauthn.as_ref().ok_or_else(|| "güvenlik anahtarı için sunucu --webauthn-origin ile başlatılmalı".to_string())
    }

    fn new_challenge(&self, user: &User, register: Option<String>, now: u64) -> Result<KeyChallenge, String> {
        let rp = self.relying()?;
        let (ticket, challenge) = (random_token(), base64url(random_token().as_bytes()));
        let mut pending = lock(&self.key_challenges);
        pending.retain(|_, p| p.expires > now);
        // Bir kullanıcı için en fazla birkaç açık sınama: en eskisi düşer.
        while pending.values().filter(|p| p.user == user.name).count() >= MAX_PENDING_PER_USER {
            let oldest = pending.iter().filter(|(_, p)| p.user == user.name).min_by_key(|(_, p)| p.expires).map(|(k, _)| k.clone());
            let Some(oldest) = oldest else { break };
            pending.remove(&oldest);
        }
        if pending.len() >= MAX_PENDING {
            return Err("çok fazla açık güvenlik anahtarı isteği; biraz sonra yeniden deneyin".to_string());
        }
        pending.insert(ticket.clone(), Pending { user: user.name.clone(), password_hash: user.password_hash.clone(), challenge: challenge.clone(), register, expires: now + CHALLENGE_SECS });
        Ok(KeyChallenge { ticket, challenge, rp_id: rp.id.clone(), user: user.name.clone(), key_ids: user.keys.iter().map(|k| k.id.clone()).collect() })
    }

    /// Bileti harcar: yanıt doğru olsun olmasın aynı sınama ikinci kez kullanılamaz.
    fn take_challenge(&self, ticket: &str, now: u64) -> Option<Pending> {
        lock(&self.key_challenges).remove(ticket).filter(|p| p.expires > now)
    }

    /// Girişte parola doğrulandıktan sonraki adım.
    pub(super) fn begin_key_login(&self, user: &User, now: u64) -> Result<KeyChallenge, String> {
        self.new_challenge(user, None, now)
    }

    pub fn begin_key_registration(&self, user: &str, name: &str, now: u64) -> Result<KeyChallenge, String> {
        let name = name.trim();
        if !valid_key_name(name) {
            return Err("anahtar adı 1-40 karakter olmalı".to_string());
        }
        let users = self.users();
        let user = users.find(user).ok_or("kullanıcı bulunamadı")?;
        if user.keys.len() >= MAX_KEYS {
            return Err(format!("en fazla {MAX_KEYS} güvenlik anahtarı eklenebilir"));
        }
        if user.keys.iter().any(|k| k.name == name) {
            return Err(format!("\"{name}\" adlı bir güvenlik anahtarı zaten var"));
        }
        self.new_challenge(user, Some(name.to_string()), now)
    }

    /// Oturum sahibinin kayıt yanıtını denetler ve anahtarı ekler. Anahtarın adını döndürür.
    pub fn finish_key_registration(&self, user: &str, response: &KeyResponse, now: u64) -> Result<String, String> {
        let rp = self.relying()?.clone();
        let expired = || "güvenlik anahtarı isteğinin süresi doldu; yeniden başlatın".to_string();
        let pending = self.take_challenge(&response.ticket, now).filter(|p| p.user == user).ok_or_else(expired)?;
        let name = pending.register.clone().ok_or_else(expired)?;
        let (id, client_data, auth_data, public_key) = (bytes(&response.id)?, bytes(&response.client_data)?, bytes(&response.auth_data)?, bytes(&response.public_key)?);
        let alg: i32 = response.alg.parse().map_err(|_| "güvenlik anahtarı yanıtı geçersiz".to_string())?;
        let count = webauthn::verify_registration(&rp, &pending.challenge, &Registration { credential_id: &id, client_data: &client_data, auth_data: &auth_data, alg, public_key: &public_key })?;
        let key = SecurityKey { name: name.clone(), id: base64url(&id), alg, public_key: base64url(&public_key), sign_count: count, added: now };
        self.edit(|file| {
            if file.users.iter().any(|u| u.keys.iter().any(|k| k.id == key.id)) {
                return Err("bu güvenlik anahtarı zaten kayıtlı".to_string());
            }
            let user = file.find_mut(user)?;
            if user.password_hash != pending.password_hash {
                return Err(expired());
            }
            if user.keys.len() >= MAX_KEYS || user.keys.iter().any(|k| k.name == key.name) {
                return Err(expired());
            }
            user.keys.push(key);
            Ok(())
        })?;
        Ok(name)
    }

    /// Girişin ikinci adımı. Hatalı yanıt, hatalı parola gibi sayılır. `Ok`: (oturum token'ı, kullanıcı adı).
    pub fn finish_key_login(&self, client: &str, response: &KeyResponse, now: u64) -> Result<(String, String), (Option<String>, LoginError, String)> {
        let Some(pending) = self.take_challenge(&response.ticket, now).filter(|p| p.register.is_none()) else {
            return Err((None, LoginError::Invalid, "güvenlik anahtarı isteğinin süresi doldu; yeniden giriş yapın".to_string()));
        };
        let name = pending.user.clone();
        let limit = attempt_key(client, &name);
        if let Some(wait) = self.locked_for(&limit, now) {
            return Err((Some(name), LoginError::Locked(wait), "giriş kilitli".to_string()));
        }
        let verified = self.relying().and_then(|rp| {
            let rp = rp.clone();
            let (id, client_data, auth_data, signature) = (bytes(&response.id)?, bytes(&response.client_data)?, bytes(&response.auth_data)?, bytes(&response.signature)?);
            let id = base64url(&id);
            // Sayaç denetimi ile güncellemesi aynı kilit altında yapılır.
            self.edit(|file| {
                let user = file.find_mut(&name)?;
                if user.password_hash != pending.password_hash {
                    return Err("kullanıcı bu arada değişti".to_string());
                }
                let stored = user.keys.iter_mut().find(|k| k.id == id).ok_or("bu güvenlik anahtarı hesaba kayıtlı değil")?;
                let public_key = bytes(&stored.public_key)?;
                stored.sign_count = webauthn::verify_assertion(&rp, &pending.challenge, &Assertion { client_data: &client_data, auth_data: &auth_data, signature: &signature }, stored.alg, &public_key, stored.sign_count)?;
                Ok(user.clone())
            })
        });
        match verified {
            Ok(user) => {
                self.forget_failures(&limit);
                Ok((self.open_session(user, now), name))
            }
            Err(why) => {
                let error = self.count_failure(limit, now);
                Err((Some(name), error, why))
            }
        }
    }
}
