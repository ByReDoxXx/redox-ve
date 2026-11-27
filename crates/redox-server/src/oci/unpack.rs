//! İmaj katmanını (tar) bir kök dizine açar.
//!
//! Katman **güvenilmez** veridir. Hiçbir giriş kökün dışına yazamaz: yoldaki her sembolik bağ, kök dizin
//! dosya sisteminin kökü sayılarak çözülür (chroot gibi). Mutlak bir bağ kökün altına, `..` ise en fazla köke
//! kadar çıkar. Yolun son parçası hiç izlenmez; üzerine yazılan bağ silinip yerine giriş konur.
//!
//! Silme işaretleri (whiteout): `.wh.<ad>` alt katmandaki `<ad>`ı siler; `.wh..wh..opq` o dizinin alt
//! katmanlardan gelen içeriğini siler.
//!
//! Aygıt düğümleri ve adlandırılmış borular atlanır (yetkisiz kullanıcı oluşturamaz; `/dev`'i LXC kurar).
//! Genişletilmiş öznitelikler (xattr) taşınmaz.

use std::io::Read;

const BLOCK: usize = 512;
const MAX_NAME: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    HardLink,
    /// Aygıt, boru ve benzeri: açılmaz.
    Other,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: Vec<u8>,
    pub link: Vec<u8>,
    pub kind: Kind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
}

fn damaged(what: &str) -> String {
    format!("imaj katmanı bozuk: {what}")
}

fn read_block(input: &mut dyn Read) -> Result<Option<[u8; BLOCK]>, String> {
    let mut block = [0u8; BLOCK];
    let mut filled = 0;
    while filled < BLOCK {
        match input.read(&mut block[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(damaged("yarım blok")),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(damaged(&e.to_string())),
        }
    }
    Ok(Some(block))
}

/// Sekizlik sayı ya da GNU'nun büyük değerler için kullandığı ikili biçim.
fn number(field: &[u8]) -> Result<u64, String> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        let mut value: u64 = u64::from(field[0] & 0x7f);
        for b in &field[1..] {
            value = value.checked_mul(256).ok_or_else(|| damaged("sayı çok büyük"))? + u64::from(*b);
        }
        return Ok(value);
    }
    let mut value: u64 = 0;
    for &b in field.iter().skip_while(|b| **b == b' ').take_while(|b| !matches!(**b, 0 | b' ')) {
        if !(b'0'..=b'7').contains(&b) {
            return Err(damaged("sayı alanı"));
        }
        value = value.checked_mul(8).ok_or_else(|| damaged("sayı çok büyük"))? + u64::from(b - b'0');
    }
    Ok(value)
}

fn text(field: &[u8]) -> Vec<u8> {
    field.iter().take_while(|b| **b != 0).copied().collect()
}

fn checksum_ok(block: &[u8; BLOCK]) -> bool {
    let sum: u64 = block.iter().enumerate().map(|(i, b)| u64::from(if (148..156).contains(&i) { b' ' } else { *b })).sum();
    number(&block[148..156]).is_ok_and(|stored| stored == sum)
}

/// Girişin verisini (blok dolgusuyla birlikte) okur ve en fazla `limit` baytını döndürür.
fn small_body(input: &mut dyn Read, size: u64, limit: usize) -> Result<Vec<u8>, String> {
    if size > limit as u64 {
        return Err(damaged("başlık verisi çok büyük"));
    }
    let mut data = Vec::new();
    for _ in 0..size.div_ceil(BLOCK as u64) {
        data.extend_from_slice(&read_block(input)?.ok_or_else(|| damaged("yarım veri"))?);
    }
    data.truncate(size as usize);
    Ok(data)
}

/// PAX kayıtları: `uzunluk anahtar=değer\n`.
fn pax(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|b| *b == b' ').ok_or_else(|| damaged("pax kaydı"))?;
        let length: usize = std::str::from_utf8(&rest[..space]).ok().and_then(|t| t.parse().ok()).ok_or_else(|| damaged("pax uzunluğu"))?;
        if length <= space + 1 || length > rest.len() || rest[length - 1] != b'\n' {
            return Err(damaged("pax uzunluğu"));
        }
        let record = &rest[space + 1..length - 1];
        let equals = record.iter().position(|b| *b == b'=').ok_or_else(|| damaged("pax kaydı"))?;
        out.push((String::from_utf8_lossy(&record[..equals]).into_owned(), record[equals + 1..].to_vec()));
        rest = &rest[length..];
    }
    Ok(out)
}

/// Sıradaki girişi okur; uzun ad ve PAX başlıkları girişe işlenir. Arşiv bittiyse `None`.
pub fn next_entry(input: &mut dyn Read) -> Result<Option<Entry>, String> {
    let (mut long_name, mut long_link, mut extended) = (None, None, Vec::new());
    loop {
        let Some(block) = read_block(input)? else { return Ok(None) };
        if block.iter().all(|b| *b == 0) {
            return Ok(None);
        }
        if !checksum_ok(&block) {
            return Err(damaged("başlık toplamı"));
        }
        let size = number(&block[124..136])?;
        match block[156] {
            b'L' => long_name = Some(text(&small_body(input, size, MAX_NAME + 1)?)),
            b'K' => long_link = Some(text(&small_body(input, size, MAX_NAME + 1)?)),
            b'x' => extended = pax(&small_body(input, size, 64 * 1024)?)?,
            b'g' => drop(small_body(input, size, 64 * 1024)?),
            b'S' => return Err("imaj katmanında seyrek dosya var; desteklenmiyor".to_string()),
            flag => {
                let mut name = text(&block[..100]);
                // ustar: uzun adın baş tarafı ayrı alandadır.
                if &block[257..262] == b"ustar" && block[262] == 0 {
                    let prefix = text(&block[345..500]);
                    if !prefix.is_empty() {
                        name = [prefix, vec![b'/'], name].concat();
                    }
                }
                let kind = match flag {
                    b'0' | 0 | b'7' if name.ends_with(b"/") => Kind::Directory,
                    b'0' | 0 | b'7' => Kind::File,
                    b'1' => Kind::HardLink,
                    b'2' => Kind::Symlink,
                    b'5' => Kind::Directory,
                    _ => Kind::Other,
                };
                let id = |value: u64| u32::try_from(value).map_err(|_| damaged("kimlik çok büyük"));
                let mut entry = Entry {
                    name: long_name.take().unwrap_or(name),
                    link: long_link.take().unwrap_or_else(|| text(&block[157..257])),
                    kind,
                    mode: (number(&block[100..108])? & 0o7777) as u32,
                    uid: id(number(&block[108..116])?)?,
                    gid: id(number(&block[116..124])?)?,
                    size,
                };
                for (key, value) in std::mem::take(&mut extended) {
                    let parsed = || std::str::from_utf8(&value).ok().and_then(|t| t.parse::<u64>().ok()).ok_or_else(|| damaged("pax sayısı"));
                    match key.as_str() {
                        "path" => entry.name = value,
                        "linkpath" => entry.link = value,
                        "size" => entry.size = parsed()?,
                        "uid" => entry.uid = id(parsed()?)?,
                        "gid" => entry.gid = id(parsed()?)?,
                        _ => {}
                    }
                }
                if entry.name.len() > MAX_NAME || entry.link.len() > MAX_NAME || entry.name.contains(&0) || entry.link.contains(&0) {
                    return Err(damaged("ad çok uzun ya da geçersiz"));
                }
                return Ok(Some(entry));
            }
        }
    }
}

/// Giriş adını parçalara ayırır. `..` içeren ad reddedilir; baştaki `/` ve `.` parçaları atılır.
pub fn components(name: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut parts = Vec::new();
    for part in name.split(|b| *b == b'/') {
        match part {
            b"" | b"." => {}
            b".." => return Err(damaged("yolda \"..\" var")),
            other => parts.push(other.to_vec()),
        }
    }
    Ok(parts)
}

/// ustar başlığı (ad ve bağ en çok 100 bayt).
pub fn header(name: &str, flag: u8, link: &str, mode: u32, uid: u32, gid: u32, size: u64) -> [u8; BLOCK] {
    let mut block = [0u8; BLOCK];
    let mut put = |at: usize, bytes: &[u8]| block[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, &name.as_bytes()[..name.len().min(100)]);
    put(100, format!("{mode:07o}\0").as_bytes());
    put(108, format!("{uid:07o}\0").as_bytes());
    put(116, format!("{gid:07o}\0").as_bytes());
    put(124, format!("{size:011o}\0").as_bytes());
    put(136, b"00000000000\0");
    put(156, &[flag]);
    put(157, &link.as_bytes()[..link.len().min(100)]);
    put(257, b"ustar\x0000");
    put(148, b"        ");
    let sum: u64 = block.iter().map(|b| u64::from(*b)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block
}

/// Tek dosyalık bir katman. Konteynere sunucudan dosya yazmak için de güvenli açıcı kullanılır: imajdaki bir
/// sembolik bağ yazmayı kökün dışına yönlendiremez.
pub fn single_file_layer(name: &str, content: &[u8]) -> Vec<u8> {
    let mut out = header(name, b'0', "", 0o644, 0, 0, content.len() as u64).to_vec();
    out.extend_from_slice(content);
    out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
    out.resize(out.len() + 2 * BLOCK, 0);
    out
}

#[cfg(unix)]
pub use unix::{read_inside, unpack};

#[cfg(unix)]
mod unix {
    use super::*;
    use std::collections::HashSet;
    use std::ffi::OsStr;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{lchown, OpenOptionsExt, PermissionsExt};

    /// Bir yolda izlenecek en fazla sembolik bağ.
    const MAX_LINKS: usize = 40;
    const MAX_ENTRIES: u64 = 2_000_000;

    fn os(bytes: &[u8]) -> &OsStr {
        OsStr::from_bytes(bytes)
    }

    /// `parts` dizinini kök içinde çözer; eksik dizinleri oluşturur. Dönen yolda sembolik bağ yoktur.
    fn directory(root: &Path, parts: &[Vec<u8>], create: bool) -> Result<Option<PathBuf>, String> {
        let mut pending: Vec<Vec<u8>> = parts.iter().rev().cloned().collect();
        let mut resolved: Vec<Vec<u8>> = Vec::new();
        let mut followed = 0;
        while let Some(part) = pending.pop() {
            match part.as_slice() {
                b"" | b"." => continue,
                b".." => {
                    resolved.pop();
                    continue;
                }
                _ => {}
            }
            let mut path = root.to_path_buf();
            path.extend(resolved.iter().map(|p| os(p)));
            path.push(os(&part));
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    followed += 1;
                    if followed > MAX_LINKS {
                        return Err(damaged("sembolik bağ döngüsü"));
                    }
                    let target = std::fs::read_link(&path).map_err(|e| damaged(&e.to_string()))?.into_os_string().into_vec();
                    // Mutlak hedef, makinenin değil imajın köküne göredir.
                    if target.first() == Some(&b'/') {
                        resolved.clear();
                    }
                    pending.extend(target.split(|b| *b == b'/').rev().map(<[u8]>::to_vec));
                }
                Ok(meta) if meta.is_dir() => resolved.push(part),
                Ok(_) => return Err(damaged("yolun bir parçası dizin değil")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                    std::fs::create_dir(&path).map_err(|e| format!("{} oluşturulamadı: {e}", path.display()))?;
                    // Hizmetin dosya maskesinden bağımsız olarak herkes geçebilmelidir.
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                    resolved.push(part);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(format!("{} okunamadı: {e}", path.display())),
            }
        }
        let mut path = root.to_path_buf();
        path.extend(resolved.iter().map(|p| os(p)));
        Ok(Some(path))
    }

    /// Kök içindeki bir dosyayı okur; bağlar kökün içinde çözülür. Dosya yoksa ya da okunamıyorsa `None`.
    pub fn read_inside(root: &Path, name: &str, limit: u64) -> Option<String> {
        let mut parts: Vec<Vec<u8>> = name.split('/').filter(|p| !p.is_empty()).map(|p| p.as_bytes().to_vec()).collect();
        for _ in 0..MAX_LINKS {
            let last = parts.pop()?;
            let path = directory(root, &parts, false).ok()??.join(os(&last));
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).ok()?.into_os_string().into_vec();
                if target.first() == Some(&b'/') {
                    parts.clear();
                }
                // `..` parçaları dizin çözümünde kökle sınırlanır.
                parts.extend(target.split(|b| *b == b'/').filter(|p| !p.is_empty()).map(<[u8]>::to_vec));
                continue;
            }
            if !meta.is_file() || meta.len() > limit {
                return None;
            }
            return std::fs::read_to_string(&path).ok();
        }
        None
    }

    /// Varsa siler; bağlar izlenmez.
    fn remove(path: &Path) -> Result<(), String> {
        let result = match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            Err(_) => return Ok(()),
        };
        result.map_err(|e| format!("{} silinemedi: {e}", path.display()))
    }

    /// `keep` içinde olmayan (alt katmanlardan gelen) her şeyi siler.
    fn clear_lower(dir: &Path, keep: &HashSet<PathBuf>) -> Result<(), String> {
        for child in std::fs::read_dir(dir).map_err(|e| format!("{} okunamadı: {e}", dir.display()))?.flatten() {
            let path = child.path();
            if !keep.contains(&path) {
                remove(&path)?;
            } else if child.file_type().is_ok_and(|t| t.is_dir()) {
                clear_lower(&path, keep)?;
            }
        }
        Ok(())
    }

    /// Veriyi `out`'a (verilmezse boşa) aktarır ve blok dolgusunu atlar.
    fn body(input: &mut dyn Read, size: u64, mut out: Option<&mut std::fs::File>) -> Result<(), String> {
        let mut left = size;
        for _ in 0..size.div_ceil(BLOCK as u64) {
            let block = read_block(input)?.ok_or_else(|| damaged("yarım veri"))?;
            let take = left.min(BLOCK as u64) as usize;
            if let Some(file) = out.as_deref_mut() {
                file.write_all(&block[..take]).map_err(|e| format!("dosya yazılamadı: {e}"))?;
            }
            left -= take as u64;
        }
        Ok(())
    }

    fn own(path: &Path, entry: &Entry, chown: bool) -> Result<(), String> {
        if chown {
            lchown(path, Some(entry.uid), Some(entry.gid)).map_err(|e| format!("{} sahipliği ayarlanamadı ({}:{}): {e}", path.display(), entry.uid, entry.gid))?;
        }
        // İzinler sahiplikten sonra: sahiplik değişimi setuid bitlerini siler.
        if entry.kind != Kind::Symlink {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(entry.mode)).map_err(|e| format!("{} izinleri ayarlanamadı: {e}", path.display()))?;
        }
        Ok(())
    }

    /// Bir katmanı `root` içine açar. `budget`: yazılabilecek en fazla bayt (katmanlar boyunca azalır).
    /// `chown`: arşivdeki sahiplikler uygulansın mı (kök ya da eşlenmiş kök olarak çalışırken).
    pub fn unpack(input: &mut dyn Read, root: &Path, budget: &mut u64, chown: bool) -> Result<(), String> {
        // Bu katmanda oluşturulanlar: dizin işareti yalnızca alt katmanların içeriğini siler.
        let mut made: HashSet<PathBuf> = HashSet::new();
        let mut count = 0u64;
        while let Some(entry) = next_entry(input)? {
            count += 1;
            if count > MAX_ENTRIES {
                return Err(damaged("çok fazla giriş"));
            }
            let mut parts = components(&entry.name)?;
            let carries_data = matches!(entry.kind, Kind::File | Kind::Other);
            let Some(last) = parts.pop() else {
                // "./" kök dizinin kendisidir.
                body(input, if carries_data { entry.size } else { 0 }, None)?;
                continue;
            };
            if let Some(hidden) = last.strip_prefix(b".wh.") {
                if let Some(parent) = directory(root, &parts, false)? {
                    if hidden == b".wh..opq" {
                        clear_lower(&parent, &made)?;
                    } else if !hidden.is_empty() && hidden != b"." && hidden != b".." && !made.contains(&parent.join(os(hidden))) {
                        remove(&parent.join(os(hidden)))?;
                    }
                }
                body(input, if carries_data { entry.size } else { 0 }, None)?;
                continue;
            }
            if entry.kind == Kind::Other {
                body(input, entry.size, None)?;
                continue;
            }
            let parent = directory(root, &parts, true)?.expect("oluşturma istendi");
            let path = parent.join(os(&last));
            match entry.kind {
                Kind::Directory => {
                    if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                        remove(&path)?;
                        std::fs::create_dir(&path).map_err(|e| format!("{} oluşturulamadı: {e}", path.display()))?;
                    }
                }
                Kind::File => {
                    *budget = budget.checked_sub(entry.size).ok_or("imaj konteynere ayrılan diske sığmıyor; disk boyutunu büyütün")?;
                    remove(&path)?;
                    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("{} oluşturulamadı: {e}", path.display()))?;
                    body(input, entry.size, Some(&mut file))?;
                }
                Kind::Symlink => {
                    remove(&path)?;
                    std::os::unix::fs::symlink(os(&entry.link), &path).map_err(|e| format!("{} bağı oluşturulamadı: {e}", path.display()))?;
                }
                Kind::HardLink => {
                    let mut target = components(&entry.link)?;
                    let name = target.pop().ok_or_else(|| damaged("boş sabit bağ"))?;
                    let source = directory(root, &target, false)?.ok_or_else(|| damaged("sabit bağın hedefi yok"))?.join(os(&name));
                    if std::fs::symlink_metadata(&source).is_ok_and(|m| m.is_dir()) {
                        return Err(damaged("dizine sabit bağ"));
                    }
                    if source != path {
                        remove(&path)?;
                        // Bağın kendisi bağlanır; hedefi izlenmez.
                        std::fs::hard_link(&source, &path).map_err(|e| format!("{} sabit bağı oluşturulamadı: {e}", path.display()))?;
                    }
                }
                Kind::Other => unreachable!("yukarıda atlandı"),
            }
            // Üst dizinler de bu katmanın parçasıdır: sonradan gelen bir dizin işareti onları silmez.
            made.extend(path.ancestors().take_while(|a| *a != root).map(Path::to_path_buf));
            // Sabit bağ, hedefiyle aynı dosyadır: sahiplik ve izinleri hedefinkidir.
            if entry.kind != Kind::HardLink {
                own(&path, &entry, chown)?;
            }
        }
        // Sıkıştırıcının yazdığı kalan baytlar tüketilir; yoksa borunun öbür ucu hata verir.
        std::io::copy(input, &mut std::io::sink()).map_err(|e| damaged(&e.to_string()))?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests;
