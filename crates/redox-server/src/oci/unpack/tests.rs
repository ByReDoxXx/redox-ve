//! Katman açıcının testleri. Saldırı katmanlarında kökün **yanındaki** "kanarya" dizinine hiçbir şeyin
//! yazılmadığı ve oradan hiçbir şeyin silinmediği doğrulanır.

use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Sınama katmanı kurucu.
#[derive(Default)]
struct Layer(Vec<u8>);

impl Layer {
    fn raw(mut self, name: &str, flag: u8, link: &str, mode: u32, data: &[u8]) -> Self {
        self.0.extend_from_slice(&header(name, flag, link, mode, 0, 0, data.len() as u64));
        self.0.extend_from_slice(data);
        self.0.resize(self.0.len().div_ceil(BLOCK) * BLOCK, 0);
        self
    }
    fn file(self, name: &str, data: &str) -> Self {
        self.raw(name, b'0', "", 0o644, data.as_bytes())
    }
    fn dir(self, name: &str) -> Self {
        self.raw(name, b'5', "", 0o755, &[])
    }
    fn link(self, name: &str, target: &str) -> Self {
        self.raw(name, b'2', target, 0o777, &[])
    }
    fn hard(self, name: &str, target: &str) -> Self {
        self.raw(name, b'1', target, 0o644, &[])
    }
    fn end(mut self) -> Vec<u8> {
        self.0.resize(self.0.len() + 2 * BLOCK, 0);
        self.0
    }
}

struct Place {
    base: PathBuf,
    root: PathBuf,
    canary: PathBuf,
}

impl Place {
    fn new(name: &str) -> Place {
        let base = std::env::temp_dir().join(format!("rdx-oci-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, canary) = (base.join("kok"), base.join("kanarya"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&canary).unwrap();
        std::fs::write(canary.join("onemli"), "dokunma").unwrap();
        Place { base, root, canary }
    }

    fn apply(&self, layer: Vec<u8>) -> Result<(), String> {
        let mut budget = 1 << 20;
        unpack(&mut layer.as_slice(), &self.root, &mut budget, false)
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn exists(&self, name: &str) -> bool {
        std::fs::symlink_metadata(self.root.join(name)).is_ok()
    }

    fn untouched(&self) {
        let names: Vec<_> = std::fs::read_dir(&self.canary).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["onemli"], "kanarya dizinine yazılmış");
        assert_eq!(std::fs::read_to_string(self.canary.join("onemli")).unwrap(), "dokunma");
        let beside: usize = std::fs::read_dir(&self.base).unwrap().count();
        assert_eq!(beside, 2, "kökün yanında yeni bir şey oluşmuş");
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn files_directories_links_and_modes_are_created() {
    let p = Place::new("duz");
    p.apply(Layer::default().dir("./").dir("usr/").dir("usr/bin/").raw("usr/bin/arac", b'0', "", 0o4755, b"ikili").link("bin", "usr/bin").hard("usr/bin/arac2", "usr/bin/arac").file("derin/a/b/c.txt", "x").raw("dev/null", b'3', "", 0o666, &[]).raw("boru", b'6', "", 0o644, &[]).end()).unwrap();
    assert_eq!(p.read("bin/arac"), "ikili");
    assert_eq!(std::fs::metadata(p.root.join("usr/bin/arac")).unwrap().permissions().mode() & 0o7777, 0o4755);
    assert_eq!(std::fs::metadata(p.root.join("usr/bin/arac")).unwrap().ino(), std::fs::metadata(p.root.join("usr/bin/arac2")).unwrap().ino());
    assert_eq!(p.read("derin/a/b/c.txt"), "x");
    assert!(!p.exists("dev/null") && !p.exists("boru"), "aygıt ve boru oluşturulmaz");
    // Üst katman, birleşik /usr bağının içinden yazar: bağ izlenir ama kökün içinde kalır.
    p.apply(Layer::default().file("bin/yeni", "2").end()).unwrap();
    assert_eq!(p.read("usr/bin/yeni"), "2");
}

#[test]
fn nothing_escapes_the_root_through_names_or_links() {
    let p = Place::new("kacis");
    let outside = p.canary.display().to_string();
    // Mutlak hedefli bağın içine yazma, imajın köküne göre çözülür.
    p.apply(Layer::default().link("tuzak", &outside).file("tuzak/sizinti", "kotu").end()).unwrap();
    p.untouched();
    assert_eq!(p.read(&format!("{}/sizinti", outside.trim_start_matches('/'))), "kotu");
    // Göreli bağ kökün üstüne çıkamaz.
    p.apply(Layer::default().link("yukari", "../../../../../../kanarya").file("yukari/sizinti2", "kotu").end()).unwrap();
    p.untouched();
    assert_eq!(p.read("kanarya/sizinti2"), "kotu");
    // Mutlak ad kökün altına açılır; ".." içeren ad reddedilir.
    p.apply(Layer::default().file("/etc/mutlak", "1").end()).unwrap();
    assert_eq!(p.read("etc/mutlak"), "1");
    for name in ["../kanarya/onemli", "a/../../kanarya/onemli", "a/.."] {
        assert!(p.apply(Layer::default().file(name, "ezildi").end()).unwrap_err().contains(".."), "{name}");
    }
    p.untouched();
}

#[test]
fn an_existing_link_is_replaced_not_followed() {
    let p = Place::new("ezme");
    let target = p.canary.join("onemli").display().to_string();
    p.apply(Layer::default().link("dosya", &target).end()).unwrap();
    p.apply(Layer::default().file("dosya", "yeni").end()).unwrap();
    assert_eq!(p.read("dosya"), "yeni");
    assert!(!std::fs::symlink_metadata(p.root.join("dosya")).unwrap().file_type().is_symlink());
    // Dizin girişi de bağın yerine geçer.
    p.apply(Layer::default().link("dizin", &p.canary.display().to_string()).dir("dizin/").file("dizin/f", "1").end()).unwrap();
    assert_eq!(p.read("dizin/f"), "1");
    p.untouched();
}

#[test]
fn hard_links_cannot_reach_outside() {
    let p = Place::new("sabit");
    assert!(p.apply(Layer::default().hard("h", "../kanarya/onemli").end()).is_err());
    // Mutlak hedef kökün altında aranır; orada yoktur.
    assert!(p.apply(Layer::default().hard("h", &p.canary.join("onemli").display().to_string()).end()).is_err());
    // Bağ üzerinden hedef de kökün içinde çözülür.
    assert!(p.apply(Layer::default().link("k", &p.canary.display().to_string()).hard("h", "k/onemli").end()).is_err());
    // Sembolik bağa sabit bağ, bağın kendisini bağlar; hedefin içeriği paylaşılmaz.
    p.apply(Layer::default().link("s", &p.canary.join("onemli").display().to_string()).hard("h2", "s").end()).unwrap();
    assert!(std::fs::symlink_metadata(p.root.join("h2")).unwrap().file_type().is_symlink());
    p.untouched();
    assert_eq!(std::fs::metadata(p.canary.join("onemli")).unwrap().nlink(), 1);
}

#[test]
fn whiteouts_delete_lower_content_only_inside_the_root() {
    let p = Place::new("silme");
    p.apply(Layer::default().file("d/eski", "1").file("d/alt/eski", "1").file("kalir", "1").file("gider", "1").end()).unwrap();
    // Dizin işareti: alt katmanın içeriği gider, bu katmanın (işaretten önce gelen) dosyaları kalır.
    p.apply(Layer::default().file("d/yeni/dosya", "2").file("d/.wh..wh..opq", "").file("d/yeni2", "2").file(".wh.gider", "").end()).unwrap();
    assert!(!p.exists("d/eski") && !p.exists("d/alt") && !p.exists("gider"));
    assert_eq!((p.read("d/yeni/dosya"), p.read("d/yeni2"), p.read("kalir")), ("2".into(), "2".into(), "1".into()));
    // Bağ üzerinden silme işareti kökün dışındaki dosyayı silemez.
    p.apply(Layer::default().link("dis", &p.canary.display().to_string()).end()).unwrap();
    p.apply(Layer::default().file("dis/.wh.onemli", "").file("dis/.wh..wh..opq", "").end()).unwrap();
    p.apply(Layer::default().file(".wh..", "").file(".wh...", "").file(".wh.", "").end()).unwrap();
    p.untouched();
    assert_eq!(p.read("kalir"), "1", "noktalı silme işaretleri yok sayılır");
}

#[test]
fn damaged_oversized_and_looping_layers_are_refused() {
    let p = Place::new("bozuk");
    assert!(p.apply(Layer::default().link("a", "b").link("b", "a").file("a/x", "1").end()).unwrap_err().contains("döngüsü"));
    assert!(p.apply(Layer::default().file("f", "1").file("f/x", "1").end()).unwrap_err().contains("dizin değil"));
    let mut cut = Layer::default().file("g", &"x".repeat(2000)).end();
    cut.truncate(BLOCK + 100);
    assert!(p.apply(cut).unwrap_err().contains("bozuk"));
    let mut tampered = Layer::default().file("g", "1").end();
    tampered[0] = b'z';
    assert!(p.apply(tampered).unwrap_err().contains("başlık toplamı"));
    let big = Layer::default().file("bir", &"x".repeat(600)).file("iki", &"x".repeat(600)).end();
    let mut budget = 1000;
    assert!(unpack(&mut big.as_slice(), &p.root, &mut budget, false).unwrap_err().contains("sığmıyor"));
    // Dev boyut bildiren giriş bellek ayırmadan reddedilir.
    let huge = Layer(header("dev", b'0', "", 0o644, 0, 0, 0o77777777777).to_vec()).end();
    assert!(p.apply(huge).unwrap_err().contains("sığmıyor"));
    let long = Layer::default().raw("x", b'L', "", 0, &vec![b'a'; 100_000]).end();
    assert!(p.apply(long).unwrap_err().contains("çok büyük"));
    p.untouched();
}

/// Gerçek GNU tar'ın ürettiği arşivler (uzun adlar, Türkçe karakterler, bağlar) aynen açılır.
#[test]
fn archives_written_by_gnu_tar_unpack_identically() {
    let p = Place::new("gnu");
    let source = p.canary.join("kaynak");
    let long = format!("uzun/{}/{}", "d".repeat(120), "ş".repeat(60));
    std::fs::create_dir_all(source.join(&long)).unwrap();
    std::fs::write(source.join(&long).join("içerik.txt"), "merhaba dünya").unwrap();
    std::fs::write(source.join("büyük"), vec![7u8; 100_000]).unwrap();
    std::os::unix::fs::symlink(format!("{long}/içerik.txt"), source.join("kısayol")).unwrap();
    std::fs::hard_link(source.join("büyük"), source.join("büyük2")).unwrap();
    for format in ["gnu", "pax", "ustar"] {
        let root = p.root.join(format);
        std::fs::create_dir_all(&root).unwrap();
        let out = std::process::Command::new("tar").args(["--format", format, "-C"]).arg(&source).args(["-cf", "-", "büyük", "büyük2", "kısayol", "uzun"]).output().unwrap();
        // ustar çok uzun adları taşıyamaz; tar o girişleri atlayıp hata kodu döndürür.
        if format == "ustar" && !out.status.success() {
            continue;
        }
        assert!(out.status.success(), "{format}: {}", String::from_utf8_lossy(&out.stderr));
        let mut budget = 1 << 20;
        unpack(&mut out.stdout.as_slice(), &root, &mut budget, false).unwrap_or_else(|e| panic!("{format}: {e}"));
        assert_eq!(std::fs::read_to_string(root.join(&long).join("içerik.txt")).unwrap(), "merhaba dünya", "{format}");
        assert_eq!(std::fs::read(root.join("büyük2")).unwrap(), vec![7u8; 100_000], "{format}");
        assert_eq!(std::fs::read_to_string(root.join("kısayol")).unwrap(), "merhaba dünya", "{format}");
    }
}

#[test]
fn files_inside_the_root_are_read_without_leaving_it() {
    let p = Place::new("okuma");
    std::fs::write(p.canary.join("passwd"), "sunucunun-parolalari").unwrap();
    p.apply(Layer::default().link("etc/passwd", &p.canary.join("passwd").display().to_string()).file("gercek", "icerde").link("etc/gecerli", "/gercek").link("etc/ust", "../../../gercek").end()).unwrap();
    assert_eq!(read_inside(&p.root, "etc/passwd", 4096), None);
    assert_eq!(read_inside(&p.root, "etc/gecerli", 4096).as_deref(), Some("icerde"));
    assert_eq!(read_inside(&p.root, "etc/ust", 4096).as_deref(), Some("icerde"));
    assert_eq!(read_inside(&p.root, "gercek", 3), None, "sınırdan büyük dosya okunmaz");
    assert_eq!(read_inside(&p.root, "yok/dosya", 4096), None);
}

#[test]
fn the_server_added_file_goes_through_the_same_extractor() {
    let p = Place::new("ek");
    p.apply(Layer::default().link("etc/resolv.conf", &p.canary.join("onemli").display().to_string()).end()).unwrap();
    p.apply(single_file_layer("etc/resolv.conf", b"nameserver 10.0.3.1\n")).unwrap();
    assert_eq!(p.read("etc/resolv.conf"), "nameserver 10.0.3.1\n");
    p.untouched();
    assert!(Path::new(&p.root).join("etc/resolv.conf").is_file());
}
