//! Disk şifreleme: konuk diskleri LUKS'lu qcow2 olarak tutulur.
//!
//! Anahtar, sunucunun `--disk-key-file` ile verilen dosyasındaki paroladır; QEMU ve `qemu-img` onu dosyadan
//! okur (komut satırında görünmez). Neyi korur: diskin, ortak deponun ve yedeklerin kopyasını ele geçiren biri
//! anahtar olmadan veriyi okuyamaz. Neyi korumaz: anahtar dosyasına da erişen birine karşı koruma yoktur; bu
//! yüzden anahtar, disklerin durduğu yerden ayrı bir yerde tutulmalıdır.
//!
//! Şifreli konukta yalnızca verinin şifreli kaldığı bilinen işlemler yapılır; diğerleri reddedilir.

use super::*;

pub const SECRET_ID: &str = "diskkey";
const MIN_KEY: usize = 16;
const MAX_KEY: usize = 4096;

/// Anahtar dosyası kullanılabilir mi? Başka kullanıcıların okuyabildiği dosya kabul edilmez.
pub fn check_key_file(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("disk anahtarı okunamadı ({}): {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("disk anahtarı olağan bir dosya değil: {}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o007 != 0 {
            return Err(format!("disk anahtarını herkes okuyabiliyor; izinlerini daraltın: chmod 600 {}", path.display()));
        }
    }
    let key = std::fs::read(path).map_err(|e| format!("disk anahtarı okunamadı ({}): {e}", path.display()))?;
    if key.len() < MIN_KEY || key.len() > MAX_KEY || key.contains(&0) || std::str::from_utf8(&key).is_err() {
        return Err(format!("disk anahtarı {MIN_KEY} ile {MAX_KEY} bayt arasında bir metin olmalı (örn. head -c 32 /dev/urandom | base64 > anahtar)"));
    }
    Ok(())
}

/// `-drive` argümanına eklenen anahtar başvurusu.
pub(super) fn drive_key(g: &Guest) -> &'static str {
    if g.hw.encrypted {
        ",encrypt.key-secret=diskkey"
    } else {
        ""
    }
}

impl QemuDriver {
    /// `-object` / `--object` tanımı: anahtar dosyadan okunur.
    pub(super) fn key_object(&self) -> Result<String, String> {
        let path = self.cfg.disk_key.as_deref().ok_or("bu sunucuda disk anahtarı yok: şifreli konuklar için sunucuyu --disk-key-file ile başlatın")?;
        check_key_file(path)?;
        Ok(format!("secret,id={SECRET_ID},file={}", opt(path)))
    }

    /// Şifreli imajı açan `--image-opts` değeri.
    fn image_opts(path: &Path) -> String {
        format!("driver=qcow2,encrypt.key-secret={SECRET_ID},file.filename={}", opt(path))
    }

    /// Şifreli bir imaj üzerinde `qemu-img <komut> --object … [önce] --image-opts <imaj> [sonra]` çalıştırır.
    pub(super) fn img_encrypted(&self, command: &[&str], path: &Path, after: &[&str], what: &str) -> Result<(), String> {
        let (object, image) = (self.key_object()?, Self::image_opts(path));
        let mut args: Vec<&std::ffi::OsStr> = vec![command[0].as_ref(), "--object".as_ref(), object.as_ref()];
        args.extend(command[1..].iter().map(AsRef::<std::ffi::OsStr>::as_ref));
        args.extend(["--image-opts".as_ref(), image.as_ref() as &std::ffi::OsStr]);
        args.extend(after.iter().map(AsRef::<std::ffi::OsStr>::as_ref));
        self.img(&args, what)
    }

    pub(super) fn create_encrypted(&self, path: &Path, size_gb: u64) -> Result<(), String> {
        let (object, size) = (self.key_object()?, format!("{size_gb}G"));
        let options = format!("encrypt.format=luks,encrypt.key-secret={SECRET_ID}");
        self.img(&["create".as_ref(), "--object".as_ref(), object.as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-o".as_ref(), options.as_ref(), path.as_os_str(), size.as_ref()], "şifreli disk imajı oluşturulamadı")
    }

    /// Aynı boyutta (bayt) boş şifreli imaj: çalışan konuğun yedeği bunun içine yazılır.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) fn create_encrypted_bytes(&self, path: &Path, bytes: u64) -> Result<(), String> {
        let (object, size) = (self.key_object()?, bytes.to_string());
        let options = format!("encrypt.format=luks,encrypt.key-secret={SECRET_ID}");
        self.img(&["create".as_ref(), "--object".as_ref(), object.as_ref(), "-f".as_ref(), "qcow2".as_ref(), "-o".as_ref(), options.as_ref(), path.as_os_str(), size.as_ref()], "şifreli yedek imajı oluşturulamadı")
    }

    /// Şifresiz bir kaynağı (bulut imajı) şifreli diske çevirir.
    pub(super) fn convert_encrypted(&self, source: &Path, target: &Path, what: &str) -> Result<(), String> {
        let object = self.key_object()?;
        let options = format!("encrypt.format=luks,encrypt.key-secret={SECRET_ID}");
        self.img(&["convert".as_ref(), "--object".as_ref(), object.as_ref(), "-O".as_ref(), "qcow2".as_ref(), "-o".as_ref(), options.as_ref(), source.as_os_str(), target.as_os_str()], what)
    }

    /// İmaj şifreli mi? (`qemu-img info` anahtarsız okur.)
    pub(super) fn is_encrypted_image(&self, path: &Path) -> Result<bool, String> {
        let info = self.img_json(&["info".as_ref(), "-U".as_ref(), "--output=json".as_ref(), "-f".as_ref(), "qcow2".as_ref(), path.as_os_str()], "disk bilgisi okunamadı")?;
        Ok(info["encrypted"].as_bool() == Some(true))
    }

    /// Verinin şifresiz bir yere yazılabileceği ya da anahtarsız bir yere gideceği işlemler şifreli konukta yapılmaz.
    pub(super) fn plain_only(guest: &Guest, what: &str) -> Result<(), String> {
        if guest.hw.encrypted {
            return Err(format!("şifreli diskli konukta {what} desteklenmiyor"));
        }
        Ok(())
    }
}
