//! Ortak depo: birden çok sunucunun gördüğü bir dizin (örn. NFS) ve konuk disklerinin sahipliği.
//!
//! Sunucuların ortak durumu yoktur; aynı diske iki sunucunun yazmasını bu dosyalar engeller:
//!
//! - `.redox-shared`: paylaşımın kimliği. Yoksa dizin ortak depo sayılmaz; böylece bağlanmamış bir
//!   paylaşımın boş bağlama noktasına hiçbir şey yazılmaz.
//! - `vm-<kimlik>.owner`: konuğun disklerinin tek sahibi. Diske yazan, disk silen ve konuğu başlatan her
//!   işlem önce buna bakar. Sahipliği yalnızca sahibi devreder; sahibi sustuğunda başka bir sunucu, yöneticinin
//!   isteğiyle devralır.
//! - `vm-<kimlik>.guest.json`: sahibinin yazdığı konuk kaydı; devralan sunucu konuğu bundan kurar.
//! - `server-<kimlik>.alive`: her sunucunun artırdığı sayaç. Saatlere güvenilmez: sayaç bir süre
//!   değişmiyorsa sunucu susmuş sayılır.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use redox_core::{Guest, VmId};
use serde::{Deserialize, Serialize};

const MARKER: &str = ".redox-shared";
const MAX_FILE: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    /// Sunucunun kalıcı kimliği.
    pub server: String,
    /// Gösterim için sunucu (düğüm) adı.
    pub node: String,
}

/// Paylaşımda kayıtlı bir konuk.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub vmid: VmId,
    pub owner: Owner,
    pub guest: Option<Box<Guest>>,
}

/// Kimlik biçimi: 32 onaltılık karakter.
pub fn valid_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn new_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("kimlik üretilemedi: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Sunucunun kalıcı kimliği; ilk çağrıda üretilip `file` içine yazılır.
pub fn server_id(file: &Path) -> Result<String, String> {
    match std::fs::read_to_string(file) {
        Ok(text) if valid_id(text.trim()) => Ok(text.trim().to_string()),
        Ok(_) => Err(format!("{} bozuk: sunucu kimliği okunamadı", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = new_id()?;
            std::fs::write(file, format!("{id}\n")).map_err(|e| format!("{} yazılamadı: {e}", file.display()))?;
            Ok(id)
        }
        Err(e) => Err(format!("{} okunamadı: {e}", file.display())),
    }
}

/// Küçük bir dosyayı okur; sembolik bağ ve olağan dışı dosya kabul edilmez. Yoksa `None`.
fn read_small(path: &Path) -> Result<Option<String>, String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{} okunamadı: {e}", path.display())),
    };
    if !meta.file_type().is_file() || meta.len() > MAX_FILE {
        return Err(format!("{} olağan bir dosya değil ya da çok büyük", path.display()));
    }
    let mut text = String::new();
    std::fs::File::open(path).and_then(|f| f.take(MAX_FILE).read_to_string(&mut text)).map_err(|e| format!("{} okunamadı: {e}", path.display()))?;
    Ok(Some(text))
}

/// Dosyayı yarım görülmeyecek biçimde değiştirir.
fn replace(path: &Path, text: &str) -> Result<(), String> {
    let temp = path.with_extension(format!("yeni-{}", new_id()?));
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("{} yazılamadı: {e}", path.display())
    })
}

/// Bir dizini ortak depo olarak hazırlar; paylaşımın kimliğini döndürür. Zaten hazırsa kimliği değişmez.
pub fn init(dir: &Path) -> Result<String, String> {
    if !dir.is_dir() {
        return Err(format!("{} bir dizin değil", dir.display()));
    }
    let marker = dir.join(MARKER);
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&marker) {
        Ok(mut file) => {
            let id = new_id()?;
            file.write_all(format!("{id}\n").as_bytes()).map_err(|e| format!("{} yazılamadı: {e}", marker.display()))?;
            Ok(id)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => share_id(dir),
        Err(e) => Err(format!("{} yazılamadı: {e}", marker.display())),
    }
}

/// Paylaşımın kimliği. İşaret dosyası yoksa paylaşım bağlı değildir (ya da hiç hazırlanmamıştır).
pub fn share_id(dir: &Path) -> Result<String, String> {
    match read_small(&dir.join(MARKER))? {
        Some(text) if valid_id(text.trim()) => Ok(text.trim().to_string()),
        _ => Err(format!("{} ortak depo olarak hazır değil: paylaşım bağlı mı? İlk kurulumda: redox shared init <dizin>", dir.display())),
    }
}

fn owner_path(dir: &Path, vmid: VmId) -> PathBuf {
    dir.join(format!("vm-{vmid}.owner"))
}

fn manifest_path(dir: &Path, vmid: VmId) -> PathBuf {
    dir.join(format!("vm-{vmid}.guest.json"))
}

pub fn read_owner(dir: &Path, vmid: VmId) -> Result<Option<Owner>, String> {
    share_id(dir)?;
    let path = owner_path(dir, vmid);
    let Some(text) = read_small(&path)? else { return Ok(None) };
    let owner: Owner = serde_json::from_str(&text).map_err(|_| format!("{} bozuk: sahiplik okunamadı", path.display()))?;
    if !valid_id(&owner.server) {
        return Err(format!("{} bozuk: sahiplik okunamadı", path.display()));
    }
    Ok(Some(owner))
}

fn foreign(vmid: VmId, owner: &Owner) -> String {
    format!("{vmid} konuğunun ortak depodaki diskleri {} sunucusuna ait", owner.node)
}

/// Konuğun diskleri bu sunucunun mu? Değilse (ya da sahiplik kaydı yoksa) hata.
pub fn require(dir: &Path, vmid: VmId, server: &str) -> Result<(), String> {
    match read_owner(dir, vmid)? {
        Some(owner) if owner.server == server => Ok(()),
        Some(owner) => Err(foreign(vmid, &owner)),
        None => Err(format!("{vmid} konuğunun ortak depoda sahiplik kaydı yok")),
    }
}

/// Sahipsiz konuğu üstlenir. Zaten bu sunucununsa bir şey yapmaz; başkasınınsa hata. İki sunucu aynı anda
/// denese de yalnızca biri başarır (dosya yalnızca yoksa oluşturulur).
pub fn claim(dir: &Path, vmid: VmId, me: &Owner) -> Result<(), String> {
    share_id(dir)?;
    let path = owner_path(dir, vmid);
    let text = serde_json::to_string(me).map_err(|e| e.to_string())?;
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file.write_all(text.as_bytes()).and_then(|()| file.sync_all()).map_err(|e| format!("{} yazılamadı: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match read_owner(dir, vmid)? {
            Some(owner) if owner.server == me.server => Ok(()),
            Some(owner) => Err(format!("{}; başka bir kimlik seçin", foreign(vmid, &owner))),
            None => Err(format!("{} okunamadı", path.display())),
        },
        Err(e) => Err(format!("{} yazılamadı: {e}", path.display())),
    }
}

/// Sahipliği değiştirirken tutulan kilit: iki değişiklik birbirinin üzerine yazmasın.
fn locked<T>(dir: &Path, vmid: VmId, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let path = dir.join(format!("vm-{vmid}.lock"));
    std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{} var: sahiplik şu an değiştiriliyor. Yarıda kalmış bir işlemden kaldıysa dosyayı elle silin", path.display()),
        _ => format!("{} yazılamadı: {e}", path.display()),
    })?;
    let result = work();
    let _ = std::fs::remove_file(&path);
    result
}

/// Sahipliği `from` sunucusundan `to` sunucusuna geçirir. Şu anki sahip `from` değilse değişmez.
pub fn transfer(dir: &Path, vmid: VmId, from: &str, to: &Owner) -> Result<(), String> {
    share_id(dir)?;
    locked(dir, vmid, || {
        require(dir, vmid, from)?;
        replace(&owner_path(dir, vmid), &serde_json::to_string(to).map_err(|e| e.to_string())?)
    })
}

/// Silinen konuğun sahiplik kaydını ve kaydını kaldırır (yalnızca sahibi).
pub fn release(dir: &Path, vmid: VmId, server: &str) -> Result<(), String> {
    locked(dir, vmid, || {
        require(dir, vmid, server)?;
        let _ = std::fs::remove_file(manifest_path(dir, vmid));
        std::fs::remove_file(owner_path(dir, vmid)).map_err(|e| format!("sahiplik kaydı silinemedi: {e}"))
    })
}

/// Konuk kaydını paylaşıma yazar (yalnızca sahibi). Değişmediyse dosyaya dokunulmaz.
pub fn publish(dir: &Path, guest: &Guest, server: &str) -> Result<(), String> {
    require(dir, guest.vmid, server)?;
    // Çalışırken değişen ölçümler yazılmaz: dosya yalnızca konuk değişince değişsin.
    let mut record = guest.clone();
    (record.cpu_pct, record.mem_used_mb, record.started_at, record.lock, record.ip, record.agent) = (0, 0, None, None, None, None);
    record.status = redox_core::GuestStatus::Stopped;
    let text = serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?;
    let path = manifest_path(dir, guest.vmid);
    if read_small(&path)?.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    replace(&path, &text)
}

pub fn manifest(dir: &Path, vmid: VmId) -> Result<Option<Guest>, String> {
    let Some(text) = read_small(&manifest_path(dir, vmid))? else { return Ok(None) };
    let guest: Guest = serde_json::from_str(&text).map_err(|_| format!("{vmid} konuğunun paylaşımdaki kaydı okunamadı"))?;
    if guest.vmid != vmid {
        return Err(format!("{vmid} konuğunun paylaşımdaki kaydı başka bir konuğu anlatıyor"));
    }
    Ok(Some(guest))
}

/// Paylaşımdaki bütün sahiplik kayıtları, kimliğe göre sıralı.
pub fn scan(dir: &Path) -> Result<Vec<Entry>, String> {
    share_id(dir)?;
    let mut ids: Vec<VmId> = std::fs::read_dir(dir)
        .map_err(|e| format!("{} okunamadı: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("vm-")?.strip_suffix(".owner")?.parse().ok())
        .collect();
    ids.sort_unstable();
    Ok(ids.into_iter().filter_map(|vmid| Some(Entry { vmid, owner: read_owner(dir, vmid).ok()??, guest: manifest(dir, vmid).ok().flatten().map(Box::new) })).collect())
}

fn beat_path(dir: &Path, server: &str) -> PathBuf {
    dir.join(format!("server-{server}.alive"))
}

pub fn beat(dir: &Path, server: &str, counter: u64) -> Result<(), String> {
    share_id(dir)?;
    replace(&beat_path(dir, server), &counter.to_string())
}

/// Sunucunun son yazdığı sayaç; hiç yazmadıysa `None`.
pub fn read_beat(dir: &Path, server: &str) -> Result<Option<String>, String> {
    share_id(dir)?;
    if !valid_id(server) {
        return Err("geçersiz sunucu kimliği".to_string());
    }
    read_small(&beat_path(dir, server))
}

#[cfg(test)]
mod tests {
    use super::*;
    use redox_core::Cluster;

    fn share(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("redox-ortak-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn owner(n: u8) -> Owner {
        Owner { server: format!("{n:032x}"), node: format!("sunucu-{n}") }
    }

    #[test]
    fn nothing_is_written_to_an_unmounted_share() {
        let dir = share("bagsiz");
        for outcome in [claim(&dir, 100, &owner(1)), require(&dir, 100, &owner(1).server), beat(&dir, &owner(1).server, 1), scan(&dir).map(|_| ())] {
            assert!(outcome.unwrap_err().contains("paylaşım bağlı mı"));
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "boş bağlama noktasına dosya yazılmaz");
        let id = init(&dir).unwrap();
        assert!(valid_id(&id));
        assert_eq!(init(&dir).unwrap(), id, "yeniden hazırlamak kimliği değiştirmez");
        assert!(init(&dir.join("yok")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_guest_has_exactly_one_owner_and_only_the_owner_can_give_it_away() {
        let dir = share("sahip");
        init(&dir).unwrap();
        let (a, b) = (owner(1), owner(2));
        assert!(require(&dir, 100, &a.server).unwrap_err().contains("sahiplik kaydı yok"));
        claim(&dir, 100, &a).unwrap();
        claim(&dir, 100, &a).unwrap();
        assert!(claim(&dir, 100, &b).unwrap_err().contains("sunucu-1 sunucusuna ait"));
        assert!(require(&dir, 100, &b.server).is_err());
        assert!(transfer(&dir, 100, &b.server, &b).unwrap_err().contains("sunucu-1"), "sahip olmayan devredemez");
        assert!(release(&dir, 100, &b.server).is_err());
        transfer(&dir, 100, &a.server, &b).unwrap();
        assert_eq!(read_owner(&dir, 100).unwrap(), Some(b.clone()));
        assert!(require(&dir, 100, &a.server).is_err(), "devreden artık yazamaz");
        // Yarıda kalmış bir değişikliğin kilidi sahipliği korur.
        std::fs::write(dir.join("vm-100.lock"), "").unwrap();
        assert!(transfer(&dir, 100, &b.server, &a).unwrap_err().contains("elle silin"));
        std::fs::remove_file(dir.join("vm-100.lock")).unwrap();
        release(&dir, 100, &b.server).unwrap();
        assert_eq!(read_owner(&dir, 100).unwrap(), None);
        std::fs::write(dir.join("vm-101.owner"), "bozuk").unwrap();
        assert!(require(&dir, 101, &a.server).unwrap_err().contains("bozuk"), "okunamayan sahiplik izin vermez");
        assert!(claim(&dir, 101, &a).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_owner_publishes_the_record_and_heartbeats_are_counters() {
        let dir = share("kayit");
        init(&dir).unwrap();
        let (a, b) = (owner(1), owner(2));
        let mut guest = Cluster::demo(0).guests[0].clone();
        claim(&dir, guest.vmid, &a).unwrap();
        assert!(publish(&dir, &guest, &b.server).is_err());
        guest.cpu_pct = 73;
        publish(&dir, &guest, &a.server).unwrap();
        let stored = manifest(&dir, guest.vmid).unwrap().unwrap();
        assert_eq!((stored.vmid, stored.cpu_pct, stored.status), (guest.vmid, 0, redox_core::GuestStatus::Stopped));
        let written = std::fs::metadata(manifest_path(&dir, guest.vmid)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        guest.cpu_pct = 12;
        publish(&dir, &guest, &a.server).unwrap();
        assert_eq!(std::fs::metadata(manifest_path(&dir, guest.vmid)).unwrap().modified().unwrap(), written, "yalnızca ölçüm değiştiyse yazılmaz");
        let found = scan(&dir).unwrap();
        assert_eq!((found.len(), found[0].owner.clone(), found[0].guest.is_some()), (1, a.clone(), true));
        std::fs::copy(manifest_path(&dir, guest.vmid), manifest_path(&dir, 999)).unwrap();
        assert!(manifest(&dir, 999).unwrap_err().contains("başka bir konuğu"));
        assert_eq!(read_beat(&dir, &a.server).unwrap(), None);
        beat(&dir, &a.server, 5).unwrap();
        assert_eq!(read_beat(&dir, &a.server).unwrap().as_deref(), Some("5"));
        assert!(read_beat(&dir, "../x").is_err());
        let id_file = dir.join("server-id");
        let id = server_id(&id_file).unwrap();
        assert_eq!(server_id(&id_file).unwrap(), id);
        std::fs::write(&id_file, "kisa").unwrap();
        assert!(server_id(&id_file).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
