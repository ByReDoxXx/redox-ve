//! Dosya düzeyinde geri yükleme: yedeğin içindeki dosyaları, yedeği geri yüklemeden listeler ve indirir.
//!
//! Disk imajının içindeki dosya sistemini okumak için `guestfish` (libguestfs) kullanılır: imaj salt okunur
//! açılır (`--ro`), konuğun işletim sistemi kendiliğinden bulunur (`-i`) ve dosya sistemleri bağlanır. Kök
//! yetkisi gerekmez; her çağrı küçük bir yardımcı sanal makine başlattığı için birkaç saniye sürer.
//!
//! Yol, konuğun içindeki mutlak yoldur. Komut satırına ayrı bir argüman olarak verilir (kabuk yok); `/` ile
//! başlamak zorunda olduğu için seçenek ya da komut ayırıcısı (`:`) sanılamaz.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Yedeğin içindeki bir dizin girdisi.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub name: String,
    /// `d` dizin, `r` dosya, `l` bağ, diğerleri (aygıt, soket) `?`.
    pub kind: char,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == 'd'
    }
}

/// Konuk içindeki mutlak yol: `/` ile başlar, denetim karakteri ve `..` parçası içermez.
pub fn valid_path(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 4096 && !path.chars().any(|c| c.is_control()) && !path.split('/').any(|part| part == "..")
}

/// Çalışan konuğun içindeki hedef yol: mutlak olmalıdır (`/...` ya da Windows'ta `C:\...`); denetim
/// karakteri içermez. Yolu konuk ajanı yorumlar; kabuktan geçmez.
pub fn valid_target(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/');
    (path.starts_with('/') || drive) && path.len() <= 1024 && !path.chars().any(|c| c.is_control())
}

/// Yolu sadeleştirir: yinelenen ve sondaki bölüleri atar (`/etc//` → `/etc`).
pub fn normalize(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    format!("/{}", parts.join("/"))
}

/// Üst dizin; kökün üstü yine köktür.
pub fn parent(path: &str) -> String {
    match normalize(path).rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((up, _)) => up.to_string(),
    }
}

pub fn join(dir: &str, name: &str) -> String {
    normalize(&format!("{dir}/{name}"))
}

/// `guestfish readdir` çıktısını ayrıştırır:
///
/// ```text
/// [0] = {
///   ino: 2
///   ftyp: d
///   name: etc
/// }
/// ```
///
/// `.` ve `..` atılır; dizinler önce, sonra ada göre sıralanır.
pub fn parse_readdir(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut kind = '?';
    for line in text.lines() {
        let line = line.trim_start();
        if let Some(value) = line.strip_prefix("ftyp: ") {
            kind = match value.trim() {
                "d" => 'd',
                "r" => 'r',
                "l" => 'l',
                _ => '?',
            };
        } else if let Some(name) = line.strip_prefix("name: ") {
            if name != "." && name != ".." && !name.is_empty() {
                entries.push(Entry { name: name.to_string(), kind });
            }
            kind = '?';
        }
    }
    entries.sort_by(|a, b| (!a.is_dir(), &a.name).cmp(&(!b.is_dir(), &b.name)));
    entries
}

fn base(bin: &str, image: &Path) -> Command {
    let mut command = Command::new(bin);
    command.arg("--ro").arg("-a").arg(image).arg("-i").stdin(Stdio::null());
    command
}

/// Hata çıktısının anlamlı son satırı; libguestfs'in sık rastlanan iki durumu açıklanır.
fn explain(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("bilinmeyen hata").trim().to_string();
    if text.contains("no operating system") || text.contains("could not detect") {
        "yedekte tanınan bir işletim sistemi bulunamadı (boş disk ya da desteklenmeyen dosya sistemi)".to_string()
    } else if text.contains("supermin") || text.contains("vmlinuz") {
        format!("guestfish yardımcı sistemini kuramadı; çoğunlukla çekirdek dosyası okunamıyordur (sudo chmod 0644 /boot/vmlinuz-*): {last}")
    } else {
        last
    }
}

/// Dizini listeler.
pub fn list(bin: &str, image: &Path, path: &str) -> Result<Vec<Entry>, String> {
    if !valid_path(path) {
        return Err(format!("geçersiz yol: {path}"));
    }
    let out = base(bin, image).arg("readdir").arg(normalize(path)).output().map_err(|e| format!("{bin} çalıştırılamadı: {e}"))?;
    if !out.status.success() {
        return Err(format!("yedekteki dosyalar okunamadı: {}", explain(&out.stderr)));
    }
    Ok(parse_readdir(&String::from_utf8_lossy(&out.stdout)))
}

/// Çalışan `guestfish` sürecinin çıktısı. Okuyucu bırakılınca süreç sonlandırılır ve (varsa) geçici imaj silinir.
pub struct Stream {
    child: Child,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.child.stdout.as_mut() {
            Some(out) => out.read(buf),
            None => Ok(0),
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

/// Bir dosyayı (`tar` yanlışsa) ya da bir dizinin tar arşivini (`tar` doğruysa) akıtır.
/// `cleanup`: akış bitince çalışır (geçici imajı silmek için).
pub fn open(bin: &str, image: &Path, path: &str, tar: bool, cleanup: Option<Box<dyn FnOnce() + Send>>) -> Result<Stream, String> {
    if !valid_path(path) {
        if let Some(cleanup) = cleanup {
            cleanup();
        }
        return Err(format!("geçersiz yol: {path}"));
    }
    let verb = if tar { "tar-out" } else { "download" };
    match base(bin, image).arg(verb).arg(normalize(path)).arg("-").stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => Ok(Stream { child, cleanup }),
        Err(e) => {
            if let Some(cleanup) = cleanup {
                cleanup();
            }
            Err(format!("{bin} çalıştırılamadı: {e}"))
        }
    }
}

/// `guestfish` kurulu mu?
pub fn detect(bin: &str) -> bool {
    Command::new(bin).arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_validated_and_normalized() {
        for ok in ["/", "/etc", "/home/ali/Belgeler ve Notlar/a.txt", "/C:/Users", "/-rf", "/a:b"] {
            assert!(valid_path(ok), "{ok}");
        }
        for bad in ["", "etc", "-a", ":", "/etc/../shadow", "/..", "/a\nb", "/a\0b"] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&format!("/{}", "a".repeat(5000))));
        assert_eq!((normalize("/etc//ssh/"), normalize("/"), normalize("/./a/./b")), ("/etc/ssh".to_string(), "/".to_string(), "/a/b".to_string()));
        assert_eq!((parent("/etc/ssh"), parent("/etc"), parent("/")), ("/etc".to_string(), "/".to_string(), "/".to_string()));
        assert_eq!((join("/", "etc"), join("/etc/", "ssh")), ("/etc".to_string(), "/etc/ssh".to_string()));
    }

    #[test]
    fn readdir_output_is_parsed_and_sorted() {
        let text = "[0] = {\n  ino: 2\n  ftyp: d\n  name: .\n}\n[1] = {\n  ino: 2\n  ftyp: d\n  name: ..\n}\n[2] = {\n  ino: 12\n  ftyp: r\n  name: hostname\n}\n[3] = {\n  ino: 13\n  ftyp: d\n  name: ssh\n}\n[4] = {\n  ino: 14\n  ftyp: l\n  name: mtab\n}\n[5] = {\n  ino: 15\n  ftyp: s\n  name: ad: iki noktali\n}\n";
        let entries = parse_readdir(text);
        assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.kind)).collect::<Vec<_>>(), [("ssh", 'd'), ("ad: iki noktali", '?'), ("hostname", 'r'), ("mtab", 'l')]);
        assert!(parse_readdir("").is_empty());
        assert!(explain(b"libguestfs: error: inspect_os: no operating system was found").contains("işletim sistemi bulunamadı"));
        assert!(explain(b"supermin: failed to find a suitable kernel (host_cpu=x86_64)\nvmlinuz not readable").contains("chmod 0644 /boot/vmlinuz"));
        assert_eq!(explain(b"libguestfs: error: readdir: /yok: No such file or directory\n\n"), "libguestfs: error: readdir: /yok: No such file or directory");
    }

    /// Sahte guestfish: `<imaj>.d` dizinini konuğun kökü sayar.
    #[cfg(unix)]
    fn fake(dir: &Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("guestfish");
        std::fs::write(
            &script,
            r#"#!/bin/sh
[ "$1" = --version ] && { echo "guestfish 1.52.0"; exit 0; }
[ "$1" = --ro ] && [ "$2" = -a ] && [ "$4" = -i ] || { echo "beklenmeyen argümanlar: $*" >&2; exit 2; }
ROOT="$3.d"; VERB=$5; P=$6
[ -d "$ROOT" ] || { echo "libguestfs: error: inspect_os: no operating system was found on this disk" >&2; exit 1; }
case "$VERB" in
  readdir)
    [ -d "$ROOT$P" ] || { echo "libguestfs: error: readdir: $P: No such file or directory" >&2; exit 1; }
    i=0
    for f in . .. $(ls -A "$ROOT$P"); do
      if [ -d "$ROOT$P/$f" ]; then t=d; else t=r; fi
      printf '[%d] = {\n  ino: %d\n  ftyp: %s\n  name: %s\n}\n' $i $i $t "$f"; i=$((i+1))
    done ;;
  download) [ "$7" = - ] && cat "$ROOT$P" ;;
  tar-out) [ "$7" = - ] && tar -C "$ROOT$P" -cf - . ;;
  *) echo "bilinmeyen komut $VERB" >&2; exit 2 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script.display().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn files_are_listed_downloaded_and_archived_through_guestfish() {
        let dir = std::env::temp_dir().join(format!("redox-guestfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("yedek.qcow2.d/etc/ssh")).unwrap();
        std::fs::write(dir.join("yedek.qcow2.d/etc/hostname"), "web-01\n").unwrap();
        std::fs::write(dir.join("yedek.qcow2.d/etc/ssh/sshd_config"), "Port 22\n").unwrap();
        let bin = fake(&dir);
        let image = dir.join("yedek.qcow2");
        assert!(detect(&bin) && !detect("/yok/guestfish"));

        assert_eq!(list(&bin, &image, "/").unwrap(), [Entry { name: "etc".into(), kind: 'd' }]);
        assert_eq!(list(&bin, &image, "/etc/").unwrap().iter().map(|e| (e.name.as_str(), e.kind)).collect::<Vec<_>>(), [("ssh", 'd'), ("hostname", 'r')]);
        assert!(list(&bin, &image, "/yok").unwrap_err().contains("No such file or directory"));
        assert!(list(&bin, &image, "etc").unwrap_err().contains("geçersiz yol"));
        assert!(list(&bin, &dir.join("bos.qcow2"), "/").unwrap_err().contains("işletim sistemi bulunamadı"));
        assert!(list("/yok/guestfish", &image, "/").unwrap_err().contains("çalıştırılamadı"));

        let mut content = String::new();
        open(&bin, &image, "/etc/hostname", false, None).unwrap().read_to_string(&mut content).unwrap();
        assert_eq!(content, "web-01\n");
        // Dizin tar olarak akar; akış bitince temizlik çalışır.
        let cleaned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cleaned.clone();
        let mut archive = Vec::new();
        open(&bin, &image, "/etc/ssh", true, Some(Box::new(move || flag.store(true, std::sync::atomic::Ordering::SeqCst)))).unwrap().read_to_end(&mut archive).unwrap();
        assert!(archive.windows(11).any(|w| w == b"sshd_config") && archive.windows(7).any(|w| w == b"Port 22"));
        assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst), "geçici imaj akıştan sonra silinir");
        // Geçersiz yolda da temizlik yapılır.
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = flag.clone();
        assert!(open(&bin, &image, "/a/../b", false, Some(Box::new(move || seen.store(true, std::sync::atomic::Ordering::SeqCst)))).is_err());
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod target_tests {
    use super::valid_target;

    #[test]
    fn a_restore_target_in_the_guest_must_be_an_absolute_path() {
        for good in ["/etc/hostname", "/home/ali/Yeni Klasör/a.txt", "C:\\Users\\ali\\a.txt", "d:/veri"] {
            assert!(valid_target(good), "{good}");
        }
        for bad in ["", "etc/hostname", "~/a", "C:a.txt", "/etc/a\nb", ":\\a"] {
            assert!(!valid_target(bad), "{bad:?}");
        }
        assert!(!valid_target(&format!("/{}", "a".repeat(1024))));
    }
}
