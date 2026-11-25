//! Kullanıcı dosyası: kullanıcılar, roller, iki adımlı doğrulama ve API token'ları.

use super::*;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserFile {
    pub users: Vec<User>,
}

impl UserFile {
    /// Dosya yoksa boş liste döner; bozuksa hata.
    pub fn load(path: &Path) -> Result<UserFile, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{} okunamadı: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserFile::default()),
            Err(e) => Err(format!("{} açılamadı: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        write_private(path, &json).map_err(|e| format!("{} yazılamadı: {e}", path.display()))
    }

    pub fn find(&self, name: &str) -> Option<&User> {
        self.users.iter().find(|u| u.name == name)
    }

    fn find_mut(&mut self, name: &str) -> Result<&mut User, String> {
        self.users
            .iter_mut()
            .find(|u| u.name == name)
            .ok_or_else(|| format!("kullanıcı bulunamadı: {name}"))
    }

    pub fn add_user(&mut self, name: &str, password: &str, role: Role, cost: Cost) -> Result<(), String> {
        if !valid_username(name) {
            return Err(format!(
                "geçersiz kullanıcı adı \"{name}\": 1-32 karakter; küçük harf, rakam, nokta, tire ve alt çizgi kullanın"
            ));
        }
        if self.find(name).is_some() {
            return Err(format!("{name} adında bir kullanıcı zaten var"));
        }
        check_password_policy(password)?;
        self.users.push(User {
            name: name.to_string(),
            password_hash: hash_password(password, cost),
            role,
            tokens: Vec::new(),
            totp_secret: None,
            totp_pending: None,
            scope: Vec::new(),
            sso: None,
        });
        Ok(())
    }

    /// Parolayı değiştirir. Özet değiştiği için kullanıcının açık oturumları da düşer.
    pub fn set_password(&mut self, name: &str, password: &str, cost: Cost) -> Result<(), String> {
        check_password_policy(password)?;
        self.find_mut(name)?.password_hash = hash_password(password, cost);
        Ok(())
    }

    /// Son yönetici silinemez: aksi halde sisteme kimse giremez.
    pub fn remove_user(&mut self, name: &str) -> Result<(), String> {
        let user = self.find(name).ok_or_else(|| format!("kullanıcı bulunamadı: {name}"))?;
        let admins = self.users.iter().filter(|u| u.role == Role::Admin).count();
        if user.role == Role::Admin && admins == 1 {
            return Err(format!("{name} son yönetici; önce başka bir yönetici ekleyin"));
        }
        self.users.retain(|u| u.name != name);
        Ok(())
    }

    /// Rolü değiştirir. Son yönetici izleyiciye çevrilemez.
    pub fn set_role(&mut self, name: &str, role: Role) -> Result<(), String> {
        let admins = self.users.iter().filter(|u| u.role == Role::Admin).count();
        let user = self.find_mut(name)?;
        if user.role == Role::Admin && role != Role::Admin && admins == 1 {
            return Err(format!("{name} son yönetici; önce başka bir yönetici ekleyin"));
        }
        user.role = role;
        Ok(())
    }

    /// Operatörün yetki kapsamını (konuk etiketleri) belirler; boş liste bütün konuklar demektir.
    pub fn set_scope(&mut self, name: &str, mut tags: Vec<String>) -> Result<(), String> {
        if tags.len() > redox_core::MAX_TAGS {
            return Err(format!("en fazla {} etiket yazılabilir", redox_core::MAX_TAGS));
        }
        if let Some(bad) = tags.iter().find(|t| !redox_core::valid_tag(t)) {
            return Err(format!("geçersiz etiket \"{bad}\": küçük harf, rakam ve tire kullanın"));
        }
        tags.sort();
        tags.dedup();
        self.find_mut(name)?.scope = tags;
        Ok(())
    }

    /// Tek oturum açma kimliğini belirler; boş değer eşleşmeyi kaldırır.
    pub fn set_sso(&mut self, name: &str, subject: &str) -> Result<(), String> {
        let subject = subject.trim();
        if subject.len() > 254 || subject.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("geçersiz tek oturum açma kimliği".to_string());
        }
        if !subject.is_empty() && self.users.iter().any(|u| u.name != name && u.sso.as_deref() == Some(subject)) {
            return Err(format!("{subject} başka bir kullanıcıyla eşleşmiş"));
        }
        self.find_mut(name)?.sso = Some(subject.to_string()).filter(|s| !s.is_empty());
        Ok(())
    }

    /// İki adımlı doğrulama kurulumunu başlatır: yeni gizli anahtar üretir ama henüz etkinleştirmez.
    pub fn begin_totp(&mut self, name: &str) -> Result<String, String> {
        let secret = crate::totp::generate_secret();
        self.find_mut(name)?.totp_pending = Some(secret.clone());
        Ok(secret)
    }

    /// Kullanıcı uygulamasındaki kodu girince kurulumu tamamlar.
    pub fn confirm_totp(&mut self, name: &str, code: &str, now: u64) -> Result<(), String> {
        let user = self.find_mut(name)?;
        let pending = user.totp_pending.clone().ok_or("önce kurulumu başlatın")?;
        if crate::totp::verify(&pending, code, now).is_none() {
            return Err("kod doğrulanamadı; uygulamadaki güncel 6 haneli kodu girin".to_string());
        }
        user.totp_secret = Some(pending);
        user.totp_pending = None;
        Ok(())
    }

    pub fn disable_totp(&mut self, name: &str) -> Result<(), String> {
        let user = self.find_mut(name)?;
        user.totp_secret = None;
        user.totp_pending = None;
        Ok(())
    }

    /// Yeni API token'ı üretir. Düz metni yalnızca bu çağrı döndürür.
    pub fn create_token(&mut self, user: &str, label: &str) -> Result<String, String> {
        self.create_token_as(user, label, false)
    }

    /// `read_only`: token yalnızca okuma yapabilir.
    pub fn create_token_as(&mut self, user: &str, label: &str, read_only: bool) -> Result<String, String> {
        let plain = format!("redox_{}", random_token());
        self.add_token(user, label, &plain)?;
        if let Some(token) = self.find_mut(user)?.tokens.iter_mut().find(|t| t.name == label) {
            token.read_only = read_only;
        }
        Ok(plain)
    }

    pub fn add_token(&mut self, user: &str, label: &str, plain: &str) -> Result<(), String> {
        if label.is_empty() || label.len() > 64 {
            return Err("token etiketi 1-64 karakter olmalı".to_string());
        }
        let user = self.find_mut(user)?;
        if user.tokens.iter().any(|t| t.name == label) {
            return Err(format!("\"{label}\" etiketli bir token zaten var"));
        }
        user.tokens.push(ApiToken { name: label.to_string(), sha256: sha256_hex(plain), read_only: false });
        Ok(())
    }

    pub fn revoke_token(&mut self, user: &str, label: &str) -> Result<(), String> {
        let user = self.find_mut(user)?;
        let before = user.tokens.len();
        user.tokens.retain(|t| t.name != label);
        if user.tokens.len() == before {
            return Err(format!("\"{label}\" etiketli token bulunamadı"));
        }
        Ok(())
    }
}
