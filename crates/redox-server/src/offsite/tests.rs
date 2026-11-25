use super::*;

#[test]
fn keep_days_makes_a_copy_target_an_archive() {
    let base = format!("kind = dir
path = {}
", std::env::temp_dir().display());
    let cfg = Config::parse(&format!("{base}keep_days = 30
")).unwrap();
    assert_eq!(cfg.keep_days, 30);
    assert!(cfg.keeps(1_000_000, 1_000_000 + 29 * 86_400) && !cfg.keeps(1_000_000, 1_000_000 + 30 * 86_400));
    assert!(!Config::parse(&base).unwrap().keeps(1_000_000, 1_000_001), "süre yoksa saklanmaz");
    assert!(Config::parse(&format!("{base}keep_days = cok
")).unwrap_err().contains("keep_days"));
}

const PASS: &str = "dogru-at-pil-zimba";
const NAME: &str = "vm-103-1.qcow2";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("redox-offsite-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("yerel")).unwrap();
    std::fs::create_dir_all(dir.join("uzak")).unwrap();
    dir
}

fn dir_config(dir: &Path, encrypted: bool) -> Config {
    let pass = if encrypted { format!("passphrase = {PASS}\n") } else { String::new() };
    Config::parse(&format!("kind = dir\npath = {}\n{pass}", dir.join("uzak").display())).unwrap()
}

fn encrypt_all(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    EncryptReader::new(data, PASS, NAME).unwrap().read_to_end(&mut out).unwrap();
    out
}

#[test]
fn parses_and_validates_the_config_file() {
    let s3 = Config::parse("# hedef\nkind = s3\nendpoint = https://s3.eu-central-1.amazonaws.com/\nbucket = yedek-kovam\nregion = eu-central-1\nprefix = redox/\naccess_key = AKIA123\nsecret_key = gizli/anahtar+1\npassphrase = cok-gizli-parola\n").unwrap();
    assert_eq!(s3.describe(), "s3://yedek-kovam/redox/ (şifreli)");
    let debug = format!("{s3:?}");
    assert!(!debug.contains("gizli") && !debug.contains("AKIA"), "{debug}");
    assert!(matches!(&s3.target, Target::S3 { host, .. } if host == "s3.eu-central-1.amazonaws.com"));

    let abs = std::env::temp_dir().display().to_string();
    for (text, needle) in [
        ("", "\"kind\" eksik"),
        ("kind = ftp", "bilinmeyen tür"),
        ("kind = dir", "\"path\" eksik"),
        ("kind = dir\npath = goreli/yol", "tam yol"),
        (format!("kind = dir\npath = {abs}\nfazla = 1").as_str(), "bilinmeyen anahtar"),
        ("kind dir", "satır 1"),
        (format!("kind = dir\npath = {abs}\npassphrase = kisa").as_str(), "en az 12 karakter"),
        ("kind = s3\nendpoint = http://s3.example.com\nbucket = abc\naccess_key = a\nsecret_key = b", "şifresiz http"),
        ("kind = s3\nendpoint = https://s3.example.com\nbucket = Kova_1\naccess_key = a\nsecret_key = b", "geçersiz kova"),
        ("kind = s3\nendpoint = https://s3.example.com\nbucket = abc\nprefix = ../x/\naccess_key = a\nsecret_key = b", "geçersiz önek"),
        ("kind = s3\nendpoint = https://s3.example.com\nbucket = abc\naccess_key = a", "\"secret_key\" eksik"),
    ] {
        let err = Config::parse(text).unwrap_err();
        assert!(err.contains(needle), "{text:?}: {err}");
    }
}

#[test]
fn signature_matches_the_aws_documentation_example() {
    // AWS belgelerindeki "GET Object" örneği (Signature Version 4, S3).
    let auth = sigv4(
        "GET",
        "/test.txt",
        &[
            ("host", "examplebucket.s3.amazonaws.com"),
            ("range", "bytes=0-9"),
            ("x-amz-content-sha256", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            ("x-amz-date", "20130524T000000Z"),
        ],
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "20130524T000000Z",
        "us-east-1",
        "AKIAIOSFODNN7EXAMPLE",
        "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    );
    assert_eq!(
        auth,
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    );
}

#[test]
fn query_signature_matches_the_aws_documentation_examples() {
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let sign = |query: &str| {
        sigv4_query(
            "GET",
            "/",
            query,
            &[("host", "examplebucket.s3.amazonaws.com"), ("x-amz-content-sha256", EMPTY), ("x-amz-date", "20130524T000000Z")],
            EMPTY,
            "20130524T000000Z",
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        )
    };
    // "GET Bucket Lifecycle" (değersiz parametre) ve "Get Bucket (List Objects)" (iki parametre) örnekleri.
    assert!(sign("lifecycle=").ends_with("Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"), "{}", sign("lifecycle="));
    assert!(sign("max-keys=2&prefix=J").ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"), "{}", sign("max-keys=2&prefix=J"));
    assert_eq!(uri_encode("aZ09-_.~ /+="), "aZ09-_.~%20%2F%2B%3D");
    assert_eq!(xml_value("<R><UploadId>ab&amp;c</UploadId></R>", "UploadId").as_deref(), Some("ab&c"));
    assert_eq!(xml_value("<R></R>", "UploadId"), None);
    assert_eq!(xml_escape("\"e1\""), "&quot;e1&quot;");
}

#[test]
fn encryption_round_trips_at_chunk_boundaries() {
    for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 2 * CHUNK + 777] {
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        let sealed = encrypt_all(&data);
        assert_eq!(sealed.len() as u64, encrypted_len(len as u64), "{len}");
        assert!(len < 64 || !sealed.windows(32).any(|w| w == &data[..32]), "düz metin sızmaz");
        let mut plain = Vec::new();
        decrypt(&mut sealed.as_slice(), PASS, &mut plain, NAME, true).unwrap();
        assert_eq!(plain, data, "{len}");
    }
    assert_ne!(encrypt_all(b"ayni veri"), encrypt_all(b"ayni veri"), "her şifrelemede nonce yenilenir");
}

#[test]
fn tampering_truncation_and_wrong_passphrase_are_rejected() {
    let data = vec![7u8; 2 * CHUNK + 100];
    let sealed = encrypt_all(&data);
    let try_decrypt = |bytes: &[u8], pass: &str| decrypt(&mut &bytes[..], pass, &mut Vec::new(), NAME, true);
    assert!(try_decrypt(&sealed, PASS).is_ok());
    assert!(try_decrypt(&sealed, "yanlis-parola-123").unwrap_err().contains("parola yanlış"));

    // Ada bağlılık: aynı dosya başka bir yedeğin adıyla (hedefte yer değiştirilmiş) çözülmez.
    let as_other = decrypt(&mut sealed.as_slice(), PASS, &mut Vec::new(), "vm-104-9.qcow2", true).unwrap_err();
    assert!(as_other.contains("başka bir yedeğe ait"), "{as_other}");
    assert!(decrypt(&mut sealed.as_slice(), PASS, &mut Vec::new(), "vm-103-1.disk1.qcow2", false).is_err(), "aynı yedeğin başka diski de sayılmaz");
    assert!(sealed.starts_with(MAGIC_BOUND));

    let mut flipped = sealed.clone();
    flipped[HEADER + 10] ^= 1;
    assert!(try_decrypt(&flipped, PASS).unwrap_err().contains("bozulmuş"));
    // Başlığı eski biçim gibi gösterme (bağlılığı atlatma denemesi): parçalar adla mühürlü olduğu için çözülmez.
    let mut downgraded = sealed.clone();
    downgraded[..8].copy_from_slice(MAGIC);
    assert!(decrypt(&mut downgraded.as_slice(), PASS, &mut Vec::new(), NAME, false).unwrap_err().contains("çözülemedi"));
    assert!(decrypt(&mut downgraded.as_slice(), PASS, &mut Vec::new(), NAME, true).unwrap_err().contains("adına bağlı değil"));

    // Parça sınırında kesme: son parça işareti olmadığı için fark edilir.
    let first_two = HEADER + 2 * (CHUNK + TAG);
    assert!(try_decrypt(&sealed[..first_two], PASS).is_err(), "son parça atılmış");
    assert!(try_decrypt(&sealed[..HEADER + CHUNK + TAG], PASS).is_err(), "son iki parça atılmış");
    assert!(try_decrypt(&sealed[..sealed.len() - 1], PASS).is_err());

    // Parçaların yeri değiştirilirse sayaç tutmaz.
    let mut swapped = sealed.clone();
    let (a, b) = (HEADER, HEADER + CHUNK + TAG);
    let first: Vec<u8> = swapped[a..b].to_vec();
    swapped.copy_within(b..b + CHUNK + TAG, a);
    swapped[b..b + CHUNK + TAG].copy_from_slice(&first);
    assert!(try_decrypt(&swapped, PASS).is_err());

    assert!(try_decrypt(b"qcow2 gibi duz bir dosya, sifreli degil..", PASS).unwrap_err().contains("şifreli RedoX yedeği değil"));
}

#[test]
fn directory_target_push_pull_delete() {
    for encrypted in [false, true] {
        let dir = temp(if encrypted { "dir-enc" } else { "dir" });
        let cfg = dir_config(&dir, encrypted);
        let local = dir.join("yerel/vm-103-1.qcow2");
        let data = vec![42u8; CHUNK + 5];
        std::fs::write(&local, &data).unwrap();

        let remote = dir.join("uzak").join(if encrypted { "vm-103-1.qcow2.enc" } else { "vm-103-1.qcow2" });
        assert_eq!(cfg.push(&local).unwrap(), std::fs::metadata(&remote).unwrap().len());
        assert_eq!(std::fs::read(&remote).unwrap() == data, !encrypted, "şifreliyse hedefteki içerik farklıdır");
        assert_eq!(std::fs::read_dir(dir.join("uzak")).unwrap().count(), 1, "geçici dosya kalmaz");

        std::fs::remove_file(&local).unwrap();
        cfg.pull(&local).unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), data);

        cfg.delete(&local).unwrap();
        assert!(!remote.exists());
        cfg.delete(&local).unwrap();
        assert!(cfg.pull(&dir.join("yerel/yok.qcow2")).unwrap_err().contains("dış kopya açılamadı"));
        assert!(!dir.join("yerel/yok.indiriliyor").exists(), "başarısız indirme iz bırakmaz");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn missing_target_directory_and_bad_names_are_errors() {
    let dir = temp("eksik");
    let cfg = dir_config(&dir, false);
    std::fs::remove_dir_all(dir.join("uzak")).unwrap();
    let local = dir.join("yerel/vm-1-1.qcow2");
    std::fs::write(&local, "x").unwrap();
    assert!(cfg.push(&local).unwrap_err().contains("bağlı değil"));
    let odd = dir.join("yerel/garip ad.qcow2");
    std::fs::write(&odd, "x").unwrap();
    assert!(cfg.push(&odd).unwrap_err().contains("geçersiz yedek dosyası adı"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Bellekte nesne tutan asgari S3 taklidi: PUT/GET/HEAD/DELETE, imza başlığı zorunlu.
type Store = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>;

fn fake_s3() -> (u16, Store) {
    use std::io::{BufRead, BufReader};
    let store = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<String, Vec<u8>>::new()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let objects = store.clone();
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            let mut reader = BufReader::new(socket);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
            let (path, query) = target.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((target.clone(), String::new()));
            let param = |name: &str| query.split('&').find_map(|pair| pair.strip_prefix(&format!("{name}=")).map(str::to_string));
            let (mut length, mut signed) = (0usize, false);
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim().to_ascii_lowercase();
                if header.is_empty() {
                    break;
                }
                if let Some(v) = header.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                signed |= header.starts_with("authorization: aws4-hmac-sha256 credential=test-key/") && header.contains("signedheaders=host;x-amz-content-sha256;x-amz-date");
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            let mut map = objects.lock().unwrap();
            // Parçalı yükleme: parçalar `yol#yükleme#numara` adıyla bekler, tamamlanınca birleştirilir.
            let mut etag = None;
            let (status, payload): (&str, Vec<u8>) = match (signed, method.as_str()) {
                (false, _) => ("403 Forbidden", Vec::new()),
                (_, "POST") if query == "uploads=" => ("200 OK", b"<InitiateMultipartUploadResult><UploadId>yukleme/1+a=</UploadId></InitiateMultipartUploadResult>".to_vec()),
                (_, "PUT") if param("partNumber").is_some() => {
                    let number = param("partNumber").unwrap();
                    if param("uploadId").as_deref() != Some("yukleme%2F1%2Ba%3D") {
                        ("404 Not Found", Vec::new())
                    } else if path.contains("parca-hatasi") && number == "2" {
                        ("500 Internal Server Error", Vec::new())
                    } else {
                        etag = Some(format!("\"e{number}\""));
                        map.insert(format!("{path}#yukleme#{number}"), body);
                        ("200 OK", Vec::new())
                    }
                }
                (_, "POST") if param("uploadId").is_some() => {
                    let listed = String::from_utf8_lossy(&body).to_string();
                    let numbers: Vec<String> = listed.split("<PartNumber>").skip(1).filter_map(|p| p.split('<').next().map(str::to_string)).collect();
                    let tags_ok = numbers.iter().all(|n| listed.contains(&format!("<ETag>&quot;e{n}&quot;</ETag>")));
                    let mut whole = Vec::new();
                    let all = numbers.iter().all(|n| map.remove(&format!("{path}#yukleme#{n}")).map(|part| whole.extend(part)).is_some());
                    if all && tags_ok && !numbers.is_empty() && !path.contains("birlesme-hatasi") {
                        map.insert(path, whole);
                        ("200 OK", b"<CompleteMultipartUploadResult><ETag>\"tamam\"</ETag></CompleteMultipartUploadResult>".to_vec())
                    } else {
                        ("200 OK", b"<Error><Code>InvalidPart</Code></Error>".to_vec())
                    }
                }
                (_, "DELETE") if param("uploadId").is_some() => {
                    map.retain(|key, _| !key.starts_with(&format!("{path}#yukleme#")));
                    ("204 No Content", Vec::new())
                }
                (_, "PUT") => {
                    map.insert(path, body);
                    ("200 OK", Vec::new())
                }
                (_, "GET" | "HEAD") => match map.get(&path) {
                    Some(data) => ("200 OK", data.clone()),
                    None => ("404 Not Found", Vec::new()),
                },
                (_, "DELETE") => {
                    map.remove(&path);
                    ("204 No Content", Vec::new())
                }
                _ => ("405 Method Not Allowed", Vec::new()),
            };
            let mut socket = reader.into_inner();
            let etag = etag.map(|tag| format!("ETag: {tag}\r\n")).unwrap_or_default();
            let _ = write!(socket, "HTTP/1.1 {status}\r\n{etag}Content-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
            if method != "HEAD" {
                let _ = socket.write_all(&payload);
            }
        }
    });
    (port, store)
}

#[test]
fn s3_target_push_pull_delete_against_a_fake_server() {
    let (port, store) = fake_s3();
    let dir = temp("s3");
    let cfg = Config::parse(&format!(
        "kind = s3\nendpoint = http://127.0.0.1:{port}\nbucket = yedekler\nprefix = redox/\naccess_key = test-key\nsecret_key = test-secret\npassphrase = {PASS}\n"
    ))
    .unwrap();
    let local = dir.join("yerel/ct-200-5.tar.gz");
    let data = vec![9u8; 3000];
    std::fs::write(&local, &data).unwrap();

    assert_eq!(cfg.push(&local).unwrap(), encrypted_len(3000));
    let stored = store.lock().unwrap().get("/yedekler/redox/ct-200-5.tar.gz.enc").cloned().expect("nesne önekli yolda durur");
    assert!(stored.starts_with(MAGIC_BOUND) && !stored.windows(64).any(|w| w == &data[..64]));

    std::fs::remove_file(&local).unwrap();
    cfg.pull(&local).unwrap();
    assert_eq!(std::fs::read(&local).unwrap(), data);

    cfg.delete(&local).unwrap();
    assert!(store.lock().unwrap().is_empty());
    assert!(cfg.pull(&local).unwrap_err().contains("hedefte yok (404)"));
    assert_eq!(std::fs::read(&local).unwrap(), data, "başarısız indirme yerel dosyayı bozmaz");

    // Kapalı kapı: açık bir hata, takılma yok.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let down = Config::parse(&format!("kind = s3\nendpoint = http://127.0.0.1:{closed}\nbucket = yedekler\naccess_key = a\nsecret_key = b\n")).unwrap();
    assert!(down.push(&local).unwrap_err().contains("erişilemiyor"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn large_files_are_uploaded_to_s3_in_parts() {
    let (port, store) = fake_s3();
    let dir = temp("s3-parca");
    let mut cfg = Config::parse(&format!(
        "kind = s3\nendpoint = http://127.0.0.1:{port}\nbucket = yedekler\nprefix = redox/\naccess_key = test-key\nsecret_key = test-secret\npassphrase = {PASS}\n"
    ))
    .unwrap();
    assert_eq!(cfg.part_bytes, 64 * 1024 * 1024, "varsayılan parça 64 MiB");
    assert_eq!(cfg.keep_days, 0);
    cfg.part_bytes = 1000;
    let local = dir.join("yerel/vm-103-7.qcow2");
    let data: Vec<u8> = (0..2500u32).map(|i| (i * 7 % 251) as u8).collect();
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, &data).unwrap();

    // 2500 bayt şifrelenince 2547 bayt: 1000 + 1000 + 547 olarak üç parça.
    let sent = cfg.push(&local).unwrap();
    assert_eq!(sent, encrypted_len(2500));
    {
        let map = store.lock().unwrap();
        assert_eq!(map.keys().collect::<Vec<_>>(), ["/yedekler/redox/vm-103-7.qcow2.enc"], "parçalar birleşti, artık parça kalmadı");
        assert_eq!(map["/yedekler/redox/vm-103-7.qcow2.enc"].len() as u64, sent);
    }
    std::fs::remove_file(&local).unwrap();
    cfg.pull(&local).unwrap();
    assert_eq!(std::fs::read(&local).unwrap(), data, "parçalı yüklenen dosya bayt bayt geri gelir");
    // Sınırdaki dosya tek istekle gider (parçalı yükleme yalnızca sınır aşılınca).
    cfg.part_bytes = sent;
    cfg.delete(&local).unwrap();
    cfg.push(&local).unwrap();
    assert_eq!(store.lock().unwrap().len(), 1);

    // Bir parça gönderilemezse ya da birleştirme reddedilirse yükleme iptal edilir: hedefte yarım parça kalmaz.
    cfg.part_bytes = 1000;
    for (name, needle) in [("parca-hatasi.qcow2", "2. parça gönderilemedi"), ("birlesme-hatasi.qcow2", "birleştiremedi: InvalidPart")] {
        let bad = dir.join("yerel").join(name);
        std::fs::write(&bad, &data).unwrap();
        let err = cfg.push(&bad).unwrap_err();
        assert!(err.contains(needle), "{name}: {err}");
        let map = store.lock().unwrap();
        assert!(map.keys().all(|key| !key.contains(name)), "{name}: {:?}", map.keys().collect::<Vec<_>>());
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn named_files_live_in_subdirectories_and_cannot_escape_the_target() {
    use redox_core::Remote;
    for encrypted in [false, true] {
        let dir = temp(if encrypted { "ad-enc" } else { "ad" });
        let cfg = dir_config(&dir, encrypted);
        let local = dir.join("yerel/parca");
        std::fs::write(&local, b"parca verisi").unwrap();
        let name = "chunks/ab/abcdef0123";
        assert!(!cfg.exists(name).unwrap());
        cfg.put(name, &local).unwrap();
        assert!(cfg.exists(name).unwrap());
        let stored = dir.join("uzak/chunks/ab").join(if encrypted { "abcdef0123.enc" } else { "abcdef0123" });
        assert!(stored.is_file());
        assert_eq!(std::fs::read(&stored).unwrap() == b"parca verisi", !encrypted);
        assert_eq!(std::fs::read_dir(dir.join("uzak/chunks/ab")).unwrap().count(), 1, "geçici dosya kalmaz");
        let back = dir.join("yerel/.geri.tmp");
        cfg.get(name, &back).unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), b"parca verisi");
        Remote::delete(&cfg, name).unwrap();
        assert!(!cfg.exists(name).unwrap());
        Remote::delete(&cfg, name).unwrap();

        for bad in ["../x", "chunks/../x", "/etc/passwd", "a//b", "a/b/c/d", ".gizli", "chunks/.gizli", "bos luk", ""] {
            assert!(cfg.put(bad, &local).unwrap_err().contains("geçersiz yedek dosyası adı"), "{bad}");
            assert!(cfg.exists(bad).is_err() && cfg.get(bad, &back).is_err() && Remote::delete(&cfg, bad).is_err(), "{bad}");
        }
        // Hedef bağlı değilse "yok" denmez: hata verilir (yoksa bütün parçalar yeniden gönderilirdi).
        std::fs::remove_dir_all(dir.join("uzak")).unwrap();
        assert!(cfg.exists(name).unwrap_err().contains("bağlı değil"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn key_is_derived_once_per_run_but_every_file_gets_its_own_nonce() {
    let (a, b) = (encrypt_all(b"bir"), encrypt_all(b"iki"));
    assert_eq!(a[8..24], b[8..24], "tuz çalışma boyunca aynıdır (anahtar bir kez türetilir)");
    assert_ne!(a[24..31], b[24..31], "nonce öneki her dosyada yenilenir");
    // Başka bir tuzla (önceki çalışmadan kalma dosya) şifrelenmiş veri de çözülür.
    let mut old = MAGIC.to_vec();
    let (salt, prefix) = ([5u8; 16], [9u8; 7]);
    old.extend_from_slice(&salt);
    old.extend_from_slice(&prefix);
    let cipher = derive_key_uncached(PASS, &salt).unwrap();
    old.extend(cipher.encrypt(Nonce::from_slice(&nonce(&prefix, 0, true)), &b"eski dosya"[..]).unwrap());
    let mut plain = Vec::new();
    decrypt(&mut old.as_slice(), PASS, &mut plain, "herhangi-bir-ad.qcow2", false).unwrap();
    assert_eq!(plain, b"eski dosya", "eski biçimli kopyalar okunmaya devam eder");
    assert!(decrypt(&mut old.as_slice(), "yanlis-parola-123", &mut Vec::new(), NAME, false).is_err(), "önbellek parolayı karıştırmaz");
    // Adına bağlı gönderildiği bilinen bir yedeğin yerine konmuş eski biçimli dosya kabul edilmez.
    assert!(decrypt(&mut old.as_slice(), PASS, &mut Vec::new(), NAME, true).unwrap_err().contains("adına bağlı değil"));
}

#[test]
fn s3_target_reports_presence_and_tolerates_deleting_missing_objects() {
    use redox_core::Remote;
    let (port, store) = fake_s3();
    let dir = temp("s3-ad");
    let cfg = Config::parse(&format!("kind = s3\nendpoint = http://127.0.0.1:{port}\nbucket = yedekler\nprefix = redox/\naccess_key = test-key\nsecret_key = test-secret\n")).unwrap();
    let local = dir.join("yerel/parca");
    std::fs::write(&local, b"parca").unwrap();
    let name = "chunks/ab/abcdef0123";
    assert!(!cfg.exists(name).unwrap());
    cfg.put(name, &local).unwrap();
    assert!(cfg.exists(name).unwrap());
    assert_eq!(store.lock().unwrap().get("/yedekler/redox/chunks/ab/abcdef0123").map(Vec::as_slice), Some(&b"parca"[..]));
    let back = dir.join("yerel/geri");
    cfg.get(name, &back).unwrap();
    assert_eq!(std::fs::read(&back).unwrap(), b"parca");
    Remote::delete(&cfg, name).unwrap();
    assert!(!cfg.exists(name).unwrap());
    // Erişilemeyen hedef "yok" sayılmaz.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let down = Config::parse(&format!("kind = s3\nendpoint = http://127.0.0.1:{closed}\nbucket = yedekler\naccess_key = a\nsecret_key = b\n")).unwrap();
    assert!(down.exists(name).unwrap_err().contains("erişilemiyor"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn swapped_remote_copies_are_rejected_on_pull() {
    let dir = temp("bagli");
    let cfg = dir_config(&dir, true);
    let (a, b) = (dir.join("vm-103-1.qcow2"), dir.join("vm-104-2.qcow2"));
    std::fs::write(&a, b"yuz uc numarali konugun diski").unwrap();
    std::fs::write(&b, b"yuz dort numarali konugun diski").unwrap();
    cfg.push(&a).unwrap();
    cfg.push(&b).unwrap();
    // Hedefe yazabilen biri iki dosyanın yerini değiştirir.
    let remote = |name: &str| dir.join("uzak").join(format!("{name}.enc"));
    let hold = dir.join("uzak").join("gecici");
    std::fs::rename(remote("vm-103-1.qcow2"), &hold).unwrap();
    std::fs::rename(remote("vm-104-2.qcow2"), remote("vm-103-1.qcow2")).unwrap();
    std::fs::rename(&hold, remote("vm-104-2.qcow2")).unwrap();

    std::fs::remove_file(&a).unwrap();
    for require in [true, false] {
        let err = cfg.pull_bound(&a, require).unwrap_err();
        assert!(err.contains("başka bir yedeğe ait"), "{require}: {err}");
        assert!(!a.exists(), "çözülemeyen dosya yerel diske konmaz");
    }
    // Yerine konunca yeniden iner.
    std::fs::rename(remote("vm-103-1.qcow2"), &hold).unwrap();
    std::fs::rename(remote("vm-104-2.qcow2"), remote("vm-103-1.qcow2")).unwrap();
    std::fs::rename(&hold, remote("vm-104-2.qcow2")).unwrap();
    cfg.pull_bound(&a, true).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), b"yuz uc numarali konugun diski");
    // Şifresiz hedefte bağlama yoktur (doğrulama da yoktur); istek yok sayılır.
    let plain = dir_config(&dir.join("duz"), false);
    std::fs::create_dir_all(dir.join("duz").join("uzak")).unwrap();
    let c = dir.join("duz").join("vm-1-1.qcow2");
    std::fs::write(&c, b"duz").unwrap();
    plain.push(&c).unwrap();
    std::fs::remove_file(&c).unwrap();
    plain.pull_bound(&c, true).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
