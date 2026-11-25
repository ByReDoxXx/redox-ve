use super::*;

static DIRS: AtomicU64 = AtomicU64::new(0);

/// 8 baytlık parçalarla geçici bir depo.
fn store() -> (Store, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rdx-chunks-{}-{}", std::process::id(), DIRS.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (Store::with_chunk_size(&dir, 8), dir)
}

fn put(s: &Store, id: &str, data: &[u8]) -> Result<Stats, String> {
    s.write(id, &mut [(0, &mut &data[..], data.len() as u64)])
}

fn restored(s: &Store, dir: &Path, id: &str) -> Result<Vec<u8>, String> {
    let path = dir.join("geri.raw");
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    // Eski içerik kalmamalı: dosya önce çöple doldurulur.
    file.write_all(&[0xff; 100]).unwrap();
    s.read(id, 0, &mut file)?;
    drop(file);
    Ok(std::fs::read(&path).unwrap())
}

fn chunk_files(dir: &Path) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir.join(CHUNKS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|group| std::fs::read_dir(group.path()).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>())
        .collect();
    all.sort();
    all
}

/// Dört parça: veri, sıfır, veri, kısa son parça (3 bayt).
fn disk() -> Vec<u8> {
    let mut d = b"AAAAAAAA".to_vec();
    d.extend([0u8; 8]);
    d.extend(b"BBBBBBBB");
    d.extend(b"CCC");
    d
}

#[test]
fn roundtrip_keeps_every_byte_and_skips_zero_chunks() {
    let (s, dir) = store();
    let data = disk();
    let stats = put(&s, "vm-1-1", &data).unwrap();
    assert_eq!((stats.size, stats.chunks, stats.new_chunks), (27, 3, 3), "sıfır parça saklanmaz");
    assert_eq!(stats.stored, stats.added);
    assert!(s.has("vm-1-1") && !s.has("vm-1-2"));
    assert_eq!(chunk_files(&dir).len(), 3);
    assert_eq!(restored(&s, &dir, "vm-1-1").unwrap(), data);
    assert_eq!(s.verify("vm-1-1").unwrap(), 3);
    assert!(put(&s, "vm-1-1", &data).unwrap_err().contains("zaten var"), "var olan yedeğin üzerine yazılmaz");

    // Tümü sıfır ve boş diskler de çalışır.
    assert_eq!(put(&s, "sifir", &[0u8; 20]).unwrap().chunks, 0);
    assert_eq!(restored(&s, &dir, "sifir").unwrap(), vec![0u8; 20]);
    put(&s, "bos", &[]).unwrap();
    assert_eq!(restored(&s, &dir, "bos").unwrap(), Vec::<u8>::new());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn several_disks_form_one_backup() {
    let (s, dir) = store();
    let (main, data) = (disk(), b"VERIVERIAAAAAAAAson".to_vec());
    let stats = s.write("a", &mut [(0, &mut &main[..], main.len() as u64), (2, &mut &data[..], data.len() as u64)]).unwrap();
    // İkinci diskteki "AAAAAAAA" parçası asıl diskte de var: bir kez saklanır.
    assert_eq!((stats.size, stats.chunks), (46, 5));
    assert_eq!(s.disks("a").unwrap(), [(0, 27), (2, 19)]);
    let read = |disk: u8| -> Result<Vec<u8>, String> {
        let path = dir.join("geri.raw");
        let mut file = std::fs::File::create(&path).unwrap();
        s.read("a", disk, &mut file)?;
        Ok(std::fs::read(&path).unwrap())
    };
    assert_eq!(read(0).unwrap(), main);
    assert_eq!(read(2).unwrap(), data);
    assert!(read(1).unwrap_err().contains("ek disk 1 yok"));
    assert_eq!(s.verify("a").unwrap(), 5);

    // Ek diske özgü bir parça bozulursa doğrulama yakalar; asıl disk yine okunur.
    let only_extra = chunk_files(&dir).into_iter().find(|p| {
        let packed = std::fs::read(p).unwrap();
        zstd::bulk::decompress(&packed, 64).unwrap() == b"VERIVERI"
    });
    std::fs::write(only_extra.unwrap(), b"bozuk").unwrap();
    assert!(s.verify("a").is_err() && read(2).is_err());
    assert_eq!(read(0).unwrap(), main);

    // Aynı disk numarası iki kez verilemez; yarım yedek kaydedilmez.
    assert!(s.write("b", &mut [(1, &mut &main[..], 27), (1, &mut &main[..], 27)]).unwrap_err().contains("iki kez"));
    assert!(!s.has("b"));
    // Tek diskli eski dizin (disk numarası alanı yok) asıl disk sayılır.
    put(&s, "eski", &main).unwrap();
    let path = dir.join(format!("eski{INDEX_SUFFIX}"));
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["disks"][0].as_object_mut().unwrap().remove("id");
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(s.disks("eski").unwrap(), [(0, 27)]);
    assert_eq!(restored(&s, &dir, "eski").unwrap(), main);
    std::fs::remove_dir_all(dir).unwrap();
}

/// `image` üzerindeki değişiklikleri (bölge listesiyle) temel yedeğe işler ve geri okunan diski döndürür.
fn delta(s: &Store, dir: &Path, id: &str, base: &str, image: &[u8], ranges: &[(u64, u64)]) -> Result<Vec<u8>, String> {
    // Ham dosyada yalnızca değişen bölgeler geçerlidir: geri kalanı çöple doldurulur ki okunursa fark edilsin.
    let mut sparse = vec![0xEEu8; image.len()];
    for (start, len) in ranges {
        let (a, b) = (*start as usize, ((*start + *len) as usize).min(image.len()));
        if a < b {
            sparse[a..b].copy_from_slice(&image[a..b]);
        }
    }
    let path = dir.join("delta.raw");
    std::fs::write(&path, &sparse).unwrap();
    let mut file = std::fs::File::open(&path).unwrap();
    s.write_delta(id, base, &mut [DeltaDisk { id: 0, size: image.len() as u64, raw: &mut file, ranges: ranges.to_vec() }])?;
    restored(s, dir, id)
}

#[test]
fn delta_backup_equals_a_full_read() {
    let (s, dir) = store();
    let first = disk(); // AAAAAAAA 00000000 BBBBBBBB CCC
    put(&s, "a", &first).unwrap();
    let before = chunk_files(&dir).len();

    // Parçanın ortasında birkaç bayt, parça sınırını aşan bir yazma ve kısa son parça.
    let mut second = first.clone();
    second[2..5].copy_from_slice(b"xyz");
    second[14..18].copy_from_slice(b"1234");
    second[25..27].copy_from_slice(b"ZZ");
    let ranges = [(2, 3), (14, 4), (25, 2)];
    assert_eq!(delta(&s, &dir, "b", "a", &second, &ranges).unwrap(), second);
    assert_eq!(s.verify("b").unwrap(), 4);
    assert_eq!(restored(&s, &dir, "a").unwrap(), first, "temel yedek değişmez");

    // Hiçbir şey değişmemişse yeni parça yazılmaz; yedek yine eksiksizdir.
    let stats = {
        let path = dir.join("bos.raw");
        std::fs::write(&path, vec![0xEEu8; second.len()]).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        s.write_delta("c", "b", &mut [DeltaDisk { id: 0, size: second.len() as u64, raw: &mut file, ranges: Vec::new() }]).unwrap()
    };
    assert_eq!((stats.new_chunks, stats.added, stats.chunks), (0, 0, 4));
    assert_eq!(restored(&s, &dir, "c").unwrap(), second);

    // Bir parça tümüyle sıfırlanır (saklanmaz), sıfır olan parça dolar.
    let mut third = second.clone();
    third[0..8].fill(0);
    third[8..16].copy_from_slice(b"DOLUDOLU");
    assert_eq!(delta(&s, &dir, "d", "c", &third, &[(0, 16)]).unwrap(), third);
    let index: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(format!("d{INDEX_SUFFIX}"))).unwrap()).unwrap();
    assert_eq!(index["disks"][0]["chunks"][0], "", "sıfırlanan parça saklanmaz");
    assert!(chunk_files(&dir).len() > before);

    // Zincir değil: aradaki yedekler silinse de sonuncusu tam geri yüklenir.
    for id in ["a", "b", "c"] {
        s.delete(id).unwrap();
    }
    assert_eq!(restored(&s, &dir, "d").unwrap(), third);
    assert_eq!(s.verify("d").unwrap(), 3);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delta_backup_refuses_a_base_that_does_not_fit() {
    let (s, dir) = store();
    let first = disk();
    put(&s, "a", &first).unwrap();
    let mut bigger = first.clone();
    bigger.extend(b"EKLENDI!");
    assert!(delta(&s, &dir, "b", "a", &bigger, &[(0, 4)]).unwrap_err().contains("boyutu"), "büyütülmüş disk");
    assert!(delta(&s, &dir, "b", "yok", &first, &[(0, 4)]).is_err(), "silinmiş temel yedek");
    assert!(delta(&s, &dir, "b", "a", &first, &[(20, 100)]).unwrap_err().contains("dışına"));
    assert!(delta(&s, &dir, "a", "a", &first, &[(0, 4)]).unwrap_err().contains("zaten var"));
    // Temel yedeğin bozuk parçası değişen parçanın altındaysa yeni yedek alınmaz.
    let victim = chunk_files(&dir).into_iter().find(|p| zstd::bulk::decompress(&std::fs::read(p).unwrap(), 64).unwrap() == b"AAAAAAAA").unwrap();
    std::fs::write(&victim, b"bozuk").unwrap();
    assert!(delta(&s, &dir, "b", "a", &first, &[(0, 4)]).unwrap_err().contains("parça bozuk"));
    // Temelin parçası tümüyle kaybolmuşsa, o parça değişmemiş olsa bile yeni yedek eksik kalırdı: reddedilir.
    std::fs::remove_file(&victim).unwrap();
    assert!(delta(&s, &dir, "b", "a", &first, &[(16, 4)]).unwrap_err().contains("parça eksik"));
    assert!(!s.has("b"), "yarım yedek kaydedilmez");
    // Disk sayısı değişmişse de temel kullanılamaz.
    let path = dir.join("iki.raw");
    std::fs::write(&path, &first).unwrap();
    let (mut f0, mut f1) = (std::fs::File::open(&path).unwrap(), std::fs::File::open(&path).unwrap());
    let mut two = [DeltaDisk { id: 0, size: 27, raw: &mut f0, ranges: vec![] }, DeltaDisk { id: 1, size: 27, raw: &mut f1, ranges: vec![] }];
    assert!(s.write_delta("c", "a", &mut two).unwrap_err().contains("uyuşmuyor"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn second_backup_stores_only_changed_chunks() {
    let (s, dir) = store();
    let first = disk();
    put(&s, "a", &first).unwrap();
    let mut second = first.clone();
    second[16..24].copy_from_slice(b"DDDDDDDD"); // üçüncü parça değişti
    let stats = put(&s, "b", &second).unwrap();
    assert_eq!((stats.chunks, stats.new_chunks), (3, 1), "yalnızca değişen parça yazılır");
    assert!(stats.added < stats.stored);
    assert_eq!(chunk_files(&dir).len(), 4);
    // Değişmeyen disk hiç yer kaplamaz.
    let again = put(&s, "c", &second).unwrap();
    assert_eq!((again.new_chunks, again.added), (0, 0));
    // Aynı içerik disk içinde yinelenirse bir kez saklanır.
    let twins = put(&s, "ikiz", b"EEEEEEEEEEEEEEEE").unwrap();
    assert_eq!((twins.chunks, twins.new_chunks), (1, 1));

    assert_eq!(restored(&s, &dir, "a").unwrap(), first, "eski yedek yeni yedekten etkilenmez");
    assert_eq!(restored(&s, &dir, "b").unwrap(), second);
    assert_eq!(restored(&s, &dir, "ikiz").unwrap(), b"EEEEEEEEEEEEEEEE");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deleting_a_backup_never_breaks_the_others() {
    let (s, dir) = store();
    let first = disk();
    let mut second = first.clone();
    second[0..8].copy_from_slice(b"ZZZZZZZZ");
    put(&s, "a", &first).unwrap();
    put(&s, "b", &second).unwrap();
    assert_eq!(chunk_files(&dir).len(), 4);

    // Eski (temel) yedek silinir: yenisi tam olarak geri yüklenir.
    assert!(s.delete("a").unwrap());
    assert!(!s.has("a"));
    assert_eq!(chunk_files(&dir).len(), 3, "yalnızca a'ya özgü parça gider");
    assert_eq!(restored(&s, &dir, "b").unwrap(), second);
    assert_eq!(s.verify("b").unwrap(), 3);
    assert!(restored(&s, &dir, "a").is_err());
    assert!(!s.delete("a").unwrap(), "olmayanı silmek hata değildir");

    assert!(s.delete("b").unwrap());
    assert!(chunk_files(&dir).is_empty(), "son yedek gidince depo boşalır");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn corruption_is_detected_not_restored() {
    let (s, dir) = store();
    let data = disk();
    put(&s, "a", &data).unwrap();
    let victim = chunk_files(&dir).remove(0);
    let good = std::fs::read(&victim).unwrap();

    // Geçerli ama başka içerikli parça: özet uyuşmaz.
    std::fs::write(&victim, zstd::bulk::compress(b"XXXXXXXX", 3).unwrap()).unwrap();
    assert!(restored(&s, &dir, "a").unwrap_err().contains("parça bozuk"));
    assert!(s.verify("a").unwrap_err().contains("parça bozuk"));
    // Açıldığında beklenenden uzun çıkan parça.
    std::fs::write(&victim, zstd::bulk::compress(&[7u8; 4096], 3).unwrap()).unwrap();
    assert!(s.verify("a").is_err());
    // Kesilmiş ve anlamsız veri.
    std::fs::write(&victim, &good[..good.len() / 2]).unwrap();
    assert!(s.verify("a").is_err());
    std::fs::write(&victim, b"zstd degil").unwrap();
    assert!(s.verify("a").is_err());
    // Eksik parça.
    std::fs::remove_file(&victim).unwrap();
    assert!(restored(&s, &dir, "a").unwrap_err().contains("parça eksik"));
    // Parça yerine konunca yedek yeniden sağlamdır.
    std::fs::write(&victim, &good).unwrap();
    assert_eq!(restored(&s, &dir, "a").unwrap(), data);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn wrong_source_length_leaves_no_backup() {
    let (s, dir) = store();
    let data = disk();
    assert!(s.write("kisa", &mut [(0, &mut &data[..], 40)]).unwrap_err().contains("okunamadı"));
    assert!(s.write("uzun", &mut [(0, &mut &data[..], 16)]).unwrap_err().contains("uzun"));
    assert!(!s.has("kisa") && !s.has("uzun"), "yarım yedek kaydedilmez");
    // Yarım kalanların parçaları bir sonraki çöp toplamada gider.
    assert!(!chunk_files(&dir).is_empty());
    s.gc().unwrap().unwrap();
    assert!(chunk_files(&dir).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn gc_deletes_nothing_when_unsure() {
    let (s, dir) = store();
    put(&s, "a", &disk()).unwrap();
    put(&s, "b", b"QQQQQQQQ").unwrap();
    // b'nin dizini okunamaz hale gelir: hangi parçaların gerektiği bilinemez.
    std::fs::write(dir.join(format!("b{INDEX_SUFFIX}")), b"{ bozuk").unwrap();
    std::fs::remove_file(dir.join(format!("a{INDEX_SUFFIX}"))).unwrap();
    assert!(s.gc().unwrap_err().contains("çöp toplama atlandı"));
    assert_eq!(chunk_files(&dir).len(), 4, "belirsizlikte hiçbir parça silinmez");

    // Yedekleme ya da geri yükleme sürerken çöp toplama çalışmaz.
    std::fs::remove_file(dir.join(format!("b{INDEX_SUFFIX}"))).unwrap();
    {
        let _busy = s.gate.read().unwrap();
        assert_eq!(s.gc().unwrap(), None);
        assert_eq!(chunk_files(&dir).len(), 4);
    }
    // Yarım kalmış geçici dosyalar da toplanır.
    let stray = chunk_files(&dir)[0].with_file_name(".yarim.tmp");
    std::fs::write(&stray, b"x").unwrap();
    assert_eq!(s.gc().unwrap().unwrap().0, 5);
    assert!(chunk_files(&dir).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tampered_index_is_rejected() {
    let (s, dir) = store();
    put(&s, "a", &disk()).unwrap();
    let path = dir.join(format!("a{INDEX_SUFFIX}"));
    let original: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let broken = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut v = original.clone();
        edit(&mut v);
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        s.verify("a").unwrap_err()
    };
    // Özet dosya yoluna dönüşür: dizin dışına çıkan değer kabul edilmez.
    assert!(broken(&|v| v["disks"][0]["chunks"][0] = "../../../etc/passwd".into()).contains("geçersiz parça özeti"));
    assert!(broken(&|v| v["disks"][0]["chunks"][0] = "AB".repeat(32).into()).contains("geçersiz parça özeti"));
    assert!(broken(&|v| v["disks"][0]["size"] = 4096.into()).contains("parça sayısı"));
    assert!(broken(&|v| v["format"] = 9.into()).contains("biçim sürümü"));
    assert!(broken(&|v| v["chunk_size"] = 0.into()).contains("parça boyutu"));
    assert!(broken(&|v| v["disks"] = serde_json::json!([])).contains("disk içermiyor"));
    std::fs::remove_dir_all(dir).unwrap();
}

/// Dizin tabanlı sahte dış hedef; hangi işlemin kaç kez yapıldığını sayar.
struct FakeRemote {
    dir: PathBuf,
    log: Mutex<Vec<String>>,
}

impl FakeRemote {
    fn new(dir: &Path) -> Self {
        let dir = dir.join("uzak");
        std::fs::create_dir_all(&dir).unwrap();
        FakeRemote { dir, log: Mutex::new(Vec::new()) }
    }
    /// Belirtilen türden işlemleri döndürür ve günlüğü boşaltır.
    fn take(&self, verb: &str) -> Vec<String> {
        let all: Vec<String> = self.log.lock().unwrap().drain(..).collect();
        all.into_iter().filter(|line| line.starts_with(verb)).collect()
    }
    fn note(&self, verb: &str, name: &str) {
        self.log.lock().unwrap().push(format!("{verb} {name}"));
    }
    fn chunks(&self) -> usize {
        chunk_files(&self.dir).len()
    }
}

impl Remote for FakeRemote {
    fn exists(&self, name: &str) -> Result<bool, String> {
        self.note("exists", name);
        Ok(self.dir.join(name).is_file())
    }
    fn put(&self, name: &str, local: &Path) -> Result<u64, String> {
        self.note("put", name);
        let dest = self.dir.join(name);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(local, dest).map_err(|e| format!("gönderilemedi: {e}"))
    }
    fn get(&self, name: &str, local: &Path) -> Result<(), String> {
        self.note("get", name);
        std::fs::copy(self.dir.join(name), local).map(|_| ()).map_err(|e| format!("hedefte yok ({name}): {e}"))
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.note("delete", name);
        let _ = std::fs::remove_file(self.dir.join(name));
        Ok(())
    }
}

/// Aynı hedefe bakan, yerel deposu boş ikinci bir sunucu.
fn fresh(dir: &Path, name: &str) -> (Store, PathBuf) {
    let local = dir.join(name);
    std::fs::create_dir_all(&local).unwrap();
    (Store::with_chunk_size(&local, 8), local)
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn push_sends_only_what_the_target_lacks() {
    let (s, dir) = store();
    let remote = FakeRemote::new(&dir);
    let first = disk();
    let mut second = first.clone();
    second[16..24].copy_from_slice(b"DDDDDDDD");
    put(&s, "a", &first).unwrap();
    put(&s, "b", &second).unwrap();

    assert_eq!(s.push("a", &[], &remote).unwrap().0, 3);
    let puts = remote.take("put");
    assert_eq!(puts.len(), 5, "depo işareti, üç parça ve dizin: {puts:?}");
    assert!(puts[0].ends_with("redox-store.json"));
    assert!(puts.last().unwrap().ends_with("a.rdxi.json"), "dizin en son gönderilir");
    assert_eq!(remote.chunks(), 3);

    // İkinci yedek: a'nın dış kopyası olduğu biliniyor; ortak parçalar sorulmadan atlanır.
    assert_eq!(s.push("b", &ids(&["a"]), &remote).unwrap().0, 1);
    let log: Vec<String> = remote.log.lock().unwrap().clone();
    assert_eq!(log.iter().filter(|l| l.starts_with("exists")).count(), 3, "depo işareti, a'nın dizini ve yalnızca yeni parça sorulur: {log:?}");
    assert_eq!(remote.take("put").len(), 2);
    assert_eq!(remote.chunks(), 4);

    // Yeniden gönderme (bilinen yok): her parça sorulur, eksik olan tamamlanır.
    let lost = chunk_files(&remote.dir).remove(0);
    std::fs::remove_file(&lost).unwrap();
    remote.take("exists");
    let resent = s.push("a", &[], &remote).unwrap().0 + s.push("b", &[], &remote).unwrap().0;
    assert_eq!(resent, 1, "hedefte kaybolan parça yeniden gider");
    assert_eq!(remote.take("exists").len(), 8, "bilinen yokken her parça sorulur (ve iki kez depo işareti)");
    assert_eq!(remote.chunks(), 4);

    // Yerelde parçası eksik yedek gönderilemez ve hedefe dizini konmaz.
    put(&s, "c", b"KKKKKKKK").unwrap();
    for path in chunk_files(&dir) {
        if !path.starts_with(&remote.dir) && !chunk_files(&remote.dir).iter().any(|r| r.file_name() == path.file_name()) {
            std::fs::remove_file(path).unwrap();
        }
    }
    assert!(s.push("c", &[], &remote).is_err());
    assert!(!remote.dir.join("c.rdxi.json").exists());
    assert!(s.push("yok", &[], &remote).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn push_does_not_trust_the_offsite_flag_when_the_target_was_replaced() {
    let (s, dir) = store();
    let old = FakeRemote::new(&dir);
    put(&s, "a", &disk()).unwrap();
    put(&s, "b", &disk()).unwrap();
    s.push("a", &[], &old).unwrap();
    // Hedef başka (boş) bir yere çevrilir; a hâlâ "dış kopyası var" diye biliniyor.
    let fresh_dir = dir.join("yeni-hedef");
    let new = FakeRemote::new(&fresh_dir);
    assert_eq!(s.push("b", &ids(&["a"]), &new).unwrap().0, 3, "a'nın dizini yeni hedefte yok: bütün parçalar gönderilir");
    let (other, local) = fresh(&dir, "baska");
    assert!(other.pull("b", &new).unwrap());
    assert_eq!(restored(&other, &local, "b").unwrap(), disk());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pull_restores_a_lost_store_and_verifies_every_chunk() {
    let (s, dir) = store();
    let remote = FakeRemote::new(&dir);
    let data = disk();
    put(&s, "a", &data).unwrap();
    s.push("a", &[], &remote).unwrap();

    let (other, local) = fresh(&dir, "yeni");
    assert!(!other.pull("yok", &remote).unwrap(), "hedefte olmayan yedek: yanlış, hata değil");
    assert!(other.pull("a", &remote).unwrap());
    assert_eq!(restored(&other, &local, "a").unwrap(), data);
    assert_eq!(other.verify("a").unwrap(), 3);
    remote.take("get");
    // Parçalar zaten yereldeyse yalnızca dizin iner.
    std::fs::remove_file(local.join(format!("a{INDEX_SUFFIX}"))).unwrap();
    assert!(other.pull("a", &remote).unwrap());
    assert_eq!(remote.take("get").len(), 1);

    // Hedefte bozulmuş parça: indirme reddedilir, yerelde ne dizin ne bozuk parça kalır.
    let (third, local3) = fresh(&dir, "ucuncu");
    let victim = chunk_files(&remote.dir).remove(1);
    let good = std::fs::read(&victim).unwrap();
    std::fs::write(&victim, zstd::bulk::compress(b"XXXXXXXX", 3).unwrap()).unwrap();
    assert!(third.pull("a", &remote).unwrap_err().contains("parça bozuk"));
    assert!(!third.has("a"));
    let leftovers: Vec<PathBuf> = chunk_files(&local3).into_iter().filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with('.')).collect();
    assert!(leftovers.is_empty() && std::fs::read_dir(&local3).unwrap().flatten().all(|e| !e.file_name().to_str().unwrap().starts_with('.')), "geçici dosya kalmaz");
    // Hedefte eksik parça.
    std::fs::remove_file(&victim).unwrap();
    assert!(third.pull("a", &remote).unwrap_err().contains("indirilemedi"));
    assert!(!third.has("a"));
    std::fs::write(&victim, &good).unwrap();
    assert!(third.pull("a", &remote).unwrap());
    assert_eq!(restored(&third, &local3, "a").unwrap(), data);

    // Hedefteki dizin değiştirilmişse (dizin dışına çıkan ad) reddedilir.
    let index = remote.dir.join(format!("a{INDEX_SUFFIX}"));
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    v["disks"][0]["chunks"][0] = "../../x".into();
    std::fs::write(&index, serde_json::to_vec(&v).unwrap()).unwrap();
    let (fourth, _) = fresh(&dir, "dorduncu");
    assert!(fourth.pull("a", &remote).unwrap_err().contains("geçersiz"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pull_heals_corrupt_local_chunks_and_rejects_a_swapped_index() {
    let (s, dir) = store();
    let remote = FakeRemote::new(&dir);
    let data = disk();
    put(&s, "a", &data).unwrap();
    put(&s, "b", b"BASKAVERI").unwrap();
    s.push("a", &[], &remote).unwrap();
    s.push("b", &[], &remote).unwrap();
    // Yerelde bir parça bozulur: yedek okunamaz; hedeften çekince onarılır.
    let victim = chunk_files(&dir).into_iter().find(|p| !p.starts_with(&remote.dir) && zstd::bulk::decompress(&std::fs::read(p).unwrap(), 64).unwrap() == b"AAAAAAAA").unwrap();
    std::fs::write(&victim, b"bozuldu").unwrap();
    assert!(restored(&s, &dir, "a").is_err());
    remote.take("get");
    assert!(s.pull("a", &remote).unwrap());
    assert_eq!(remote.take("get").len(), 2, "dizin ve yalnızca bozuk parça iner");
    assert_eq!(restored(&s, &dir, "a").unwrap(), data);

    // Hedefte a'nın dizini b'ninkiyle değiştirilirse (geçerli, parçaları sağlam bir dizin) fark edilir.
    std::fs::copy(remote.dir.join(format!("b{INDEX_SUFFIX}")), remote.dir.join(format!("a{INDEX_SUFFIX}"))).unwrap();
    let (other, _) = fresh(&dir, "yeni");
    assert!(other.pull("a", &remote).unwrap_err().contains("başka bir yedeğe ait"));
    assert!(!other.has("a"));
    // Yerelde de aynı: dizin dosyası başka bir yedeğin adına kopyalanırsa okunmaz.
    std::fs::copy(dir.join(format!("b{INDEX_SUFFIX}")), dir.join(format!("c{INDEX_SUFFIX}"))).unwrap();
    assert!(s.verify("c").unwrap_err().contains("başka bir yedeğe ait"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn push_refuses_a_target_written_with_another_passphrase() {
    let (s, dir) = store();
    let target = dir.join("hedef");
    std::fs::create_dir_all(&target).unwrap();
    let config = |pass: &str| crate::offsite::Config::parse(&format!("kind = dir\npath = {}\npassphrase = {pass}\n", target.display())).unwrap();
    put(&s, "a", &disk()).unwrap();
    put(&s, "b", &disk()).unwrap();
    s.push("a", &[], &config("ilk-parola-123456")).unwrap();
    // Parola değişti: eski parçalar yeni parolayla açılamaz; "hedefte var" diye atlanmamalı.
    let err = s.push("b", &[], &config("yeni-parola-654321")).unwrap_err();
    assert!(err.contains("parolası değiştiyse"), "{err}");
    assert!(!target.join(format!("b{INDEX_SUFFIX}.enc")).exists(), "açılamayacak bir dış kopya bırakılmaz");
    s.push("b", &[], &config("ilk-parola-123456")).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unpush_removes_only_chunks_no_other_backup_needs() {
    let (s, dir) = store();
    let remote = FakeRemote::new(&dir);
    let first = disk();
    let mut second = first.clone();
    second[0..8].copy_from_slice(b"ZZZZZZZZ");
    put(&s, "a", &first).unwrap();
    put(&s, "b", &second).unwrap();
    s.push("a", &[], &remote).unwrap();
    s.push("b", &ids(&["a"]), &remote).unwrap();
    assert_eq!(remote.chunks(), 4);
    assert!(!s.unpush("yok", &[], &remote).unwrap());

    // a yerelde ve hedefte silinir: b'nin kullandığı parçalar hedefte kalır.
    s.delete("a").unwrap();
    assert!(s.unpush("a", &ids(&["b"]), &remote).unwrap());
    assert_eq!(remote.chunks(), 3);
    assert!(!remote.dir.join(format!("a{INDEX_SUFFIX}")).exists());
    let (other, local) = fresh(&dir, "yeni");
    assert!(other.pull("b", &remote).unwrap());
    assert_eq!(restored(&other, &local, "b").unwrap(), second);

    // Yerel depo tümüyle kaybolmuş olsa da: hedefte kalan diğer yedeğin dizini okunur, parçaları korunur.
    put(&s, "c", &first).unwrap();
    s.push("c", &[], &remote).unwrap();
    assert_eq!(remote.chunks(), 4);
    let (empty, _) = fresh(&dir, "bos");
    assert!(empty.unpush("c", &ids(&["b"]), &remote).unwrap());
    assert_eq!(remote.chunks(), 3, "yalnızca c'ye özgü parça gider");
    let (again, local2) = fresh(&dir, "yine");
    assert!(again.pull("b", &remote).unwrap());
    assert_eq!(restored(&again, &local2, "b").unwrap(), second);

    // Son yedek de silinince hedef boşalır.
    s.delete("b").unwrap();
    s.delete("c").unwrap();
    assert!(s.unpush("b", &[], &remote).unwrap());
    assert_eq!(remote.chunks(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unpush_keeps_every_chunk_when_it_cannot_tell_what_is_needed() {
    let (s, dir) = store();
    let remote = FakeRemote::new(&dir);
    put(&s, "a", &disk()).unwrap();
    put(&s, "b", &disk()).unwrap();
    s.push("a", &[], &remote).unwrap();
    s.push("b", &ids(&["a"]), &remote).unwrap();
    // b yalnızca hedefte kalmış ve dizini okunamıyor: a silinirken hiçbir parça silinmez.
    let (empty, _) = fresh(&dir, "bos");
    std::fs::write(remote.dir.join(format!("b{INDEX_SUFFIX}")), b"{ bozuk").unwrap();
    let err = empty.unpush("a", &ids(&["b"]), &remote).unwrap_err();
    assert!(err.contains("parçaları hedefte bırakıldı"), "{err}");
    assert_eq!(remote.chunks(), 3);
    assert!(!remote.dir.join(format!("a{INDEX_SUFFIX}")).exists(), "dizin silinir: yedek artık listelenmez");

    // Yerelde okunamayan bir dizin de aynı sonucu verir.
    s.push("a", &[], &remote).unwrap();
    std::fs::write(dir.join(format!("b{INDEX_SUFFIX}")), b"{ bozuk").unwrap();
    assert!(s.unpush("a", &[], &remote).unwrap_err().contains("bırakıldı"));
    assert_eq!(remote.chunks(), 3);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sealed_store_keeps_chunks_encrypted_on_disk() {
    const PASS: &str = "dogru-at-pil-zimba";
    let (plain, dir) = store();
    // Şifrelemeden önce alınmış yedek olduğu gibi kalır ve okunur.
    put(&plain, "eski", b"ESKIESKI").unwrap();
    assert!(!plain.is_sealed() && !seal_marked(&dir));
    assert!(Store::with_chunk_size(&dir, 8).sealed("kisa").err().unwrap().contains("en az 12"));
    let s = Store::with_chunk_size(&dir, 8).sealed(PASS).unwrap();
    assert!(s.is_sealed() && seal_marked(&dir));
    let data = disk();
    put(&s, "yeni", &data).unwrap();
    assert_eq!(restored(&s, &dir, "yeni").unwrap(), data);
    assert_eq!(restored(&s, &dir, "eski").unwrap(), b"ESKIESKI");
    assert_eq!(s.verify("yeni").unwrap(), 3);
    let sealed: Vec<PathBuf> = chunk_files(&dir).into_iter().filter(|f| std::fs::read(f).unwrap().starts_with(b"RDXSEAL1")).collect();
    assert_eq!((chunk_files(&dir).len(), sealed.len()), (4, 3), "yeni parçalar şifreli, eski parça olduğu gibi");

    // Parolasız açılan depo şifreli parçayı okuyamaz ve nedenini söyler; yanlış parola baştan reddedilir.
    assert!(restored(&plain, &dir, "yeni").unwrap_err().contains("--backup-passphrase-file"));
    assert!(Store::with_chunk_size(&dir, 8).sealed("yanlis-parola-12345").err().unwrap().contains("parolası yanlış"));
    // Aynı parolayla yeniden açılan depo okur (anahtar işaret dosyasındaki tuzdan türetilir).
    let again = Store::with_chunk_size(&dir, 8).sealed(PASS).unwrap();
    assert_eq!(restored(&again, &dir, "yeni").unwrap(), data);

    // Şifreli bir parça başka bir parçanın adına konursa ya da değiştirilirse çözülmez.
    let (a, b) = (sealed[0].clone(), sealed[1].clone());
    let original = std::fs::read(&a).unwrap();
    std::fs::copy(&b, &a).unwrap();
    assert!(restored(&again, &dir, "yeni").unwrap_err().contains("şifresi çözülemedi"));
    let mut tampered = original.clone();
    *tampered.last_mut().unwrap() ^= 1;
    std::fs::write(&a, &tampered).unwrap();
    assert!(again.verify("yeni").unwrap_err().contains("şifresi çözülemedi"));
    std::fs::write(&a, &original).unwrap();
    assert_eq!(again.verify("yeni").unwrap(), 3);
    std::fs::remove_dir_all(dir).unwrap();
}
