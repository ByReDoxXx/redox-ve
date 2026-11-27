//! OCI imajından konteyner: sahte bir kayıt sunucusu (belirteç, çok platformlu dizin, yönlendirme) ve gerçek
//! `redox oci-unpack` programı. `lxc-*` araçları sahtedir; konteynerin **başlatılması** burada sınanmaz.
#![cfg(unix)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use redox_core::{Cluster, Guest, GuestKind, GuestSpec, Hypervisor};
use redox_server::lxc::{LxcConfig, LxcDriver};
use redox_server::oci::registry::sha256_of;
use redox_server::oci::unpack::header;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rdx-lxcoci-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Sınama kullanıcısının kimliği: katman sahibi bu olursa kök yetkisi gerekmez.
fn me(dir: &Path) -> (u32, u32) {
    let meta = std::fs::metadata(dir).unwrap();
    (meta.uid(), meta.gid())
}

/// (ad, tür, bağ, içerik) listesinden tar katmanı.
fn layer(owner: (u32, u32), entries: &[(&str, u8, &str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, flag, link, data) in entries {
        out.extend_from_slice(&header(name, *flag, link, if *flag == b'5' { 0o755 } else { 0o644 }, owner.0, owner.1, data.len() as u64));
        out.extend_from_slice(data.as_bytes());
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut child = Command::new("gzip").arg("-c").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let data = data.to_vec();
    let feed = std::thread::spawn(move || input.write_all(&data).unwrap());
    let out = child.wait_with_output().unwrap();
    feed.join().unwrap();
    out.stdout
}

#[derive(Default)]
struct Registry {
    port: u16,
    /// Yol → (içerik türü, gövde).
    files: HashMap<String, Vec<u8>>,
    /// Gelen istekler: (yol, `Authorization` başlığı).
    seen: Mutex<Vec<(String, Option<String>)>>,
}

fn serve(build: impl FnOnce(&mut Registry)) -> Arc<Registry> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut registry = Registry { port: listener.local_addr().unwrap().port(), ..Default::default() };
    build(&mut registry);
    let registry = Arc::new(registry);
    let shared = registry.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let registry = shared.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut auth = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("authorization") {
                            auth = Some(value.trim().to_string());
                        }
                    }
                }
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                registry.seen.lock().unwrap().push((path.clone(), auth.clone()));
                let (status, extra, body) = registry.answer(&path, auth.as_deref());
                let mut stream = stream;
                let _ = write!(stream, "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            });
        }
    });
    registry
}

impl Registry {
    fn answer(&self, path: &str, auth: Option<&str>) -> (&'static str, String, Vec<u8>) {
        if path.starts_with("/token?") {
            let right = path.contains("scope=repository%3Atest%2Fapp%3Apull") && path.contains("service=reg");
            return if right { ("200 OK", String::new(), br#"{"token":"t0ken"}"#.to_vec()) } else { ("400 Bad Request", String::new(), Vec::new()) };
        }
        if let Some(blob) = path.strip_prefix("/cdn/") {
            return match self.files.get(&format!("blob/{blob}")) {
                Some(body) => ("200 OK", String::new(), body.clone()),
                None => ("404 Not Found", String::new(), Vec::new()),
            };
        }
        if auth != Some("Bearer t0ken") {
            return ("401 Unauthorized", format!("WWW-Authenticate: Bearer realm=\"http://127.0.0.1:{}/token\",service=\"reg\"\r\n", self.port), Vec::new());
        }
        if let Some(digest) = path.strip_prefix("/v2/test/app/blobs/") {
            // Parçalar başka bir adrese (başka kaynak) yönlendirilir; belirteç oraya gitmemelidir.
            return ("307 Temporary Redirect", format!("Location: http://localhost:{}/cdn/{digest}\r\n", self.port), Vec::new());
        }
        match path.strip_prefix("/v2/test/app/manifests/").and_then(|r| self.files.get(&format!("manifest/{r}"))) {
            Some(body) => ("200 OK", String::new(), body.clone()),
            None => ("404 Not Found", String::new(), Vec::new()),
        }
    }

    fn blob(&mut self, data: Vec<u8>) -> String {
        let digest = sha256_of(&data);
        self.files.insert(format!("blob/{digest}"), data);
        digest
    }

    /// `app:<tag>` imajını yayımlar.
    fn image(&mut self, tag: &str, config: serde_json::Value, layers: Vec<(&str, Vec<u8>)>) {
        let config = self.blob(serde_json::to_vec(&serde_json::json!({ "architecture": "amd64", "os": "linux", "config": config })).unwrap());
        let layers: Vec<_> = layers.into_iter().map(|(media, data)| serde_json::json!({ "mediaType": media, "digest": self.blob(data) })).collect();
        let manifest = serde_json::to_vec(&serde_json::json!({ "config": { "digest": config }, "layers": layers })).unwrap();
        let digest = sha256_of(&manifest);
        self.files.insert(format!("manifest/{digest}"), manifest);
        let index = serde_json::json!({ "manifests": [
            { "digest": format!("sha256:{}", "0".repeat(64)), "platform": { "os": "linux", "architecture": "arm64" } },
            { "digest": digest, "platform": { "os": "linux", "architecture": "amd64" } },
        ] });
        self.files.insert(format!("manifest/{tag}"), serde_json::to_vec(&index).unwrap());
    }
}

const TAR: &str = "application/vnd.oci.image.layer.v1.tar";
const TAR_GZIP: &str = "application/vnd.docker.image.rootfs.diff.tar.gzip";
const TAR_ZSTD: &str = "application/vnd.oci.image.layer.v1.tar+zstd";

fn driver(dir: &Path) -> LxcDriver {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let tool = bin.join("lxc-create");
    std::fs::write(&tool, "#!/bin/sh\ncase \"$*\" in *--version*) echo 6.0.0 ;; *) echo 'lxc-create çağrılmamalıydı' >&2; exit 1 ;; esac\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = LxcConfig::new(dir.join("lxc"));
    let userns = bin.join("lxc-usernsexec");
    std::fs::write(&userns, "#!/bin/sh
while [ \"$1\" != -- ]; do shift; done
shift
exec unshare -Ur \"$@\"
").unwrap();
    std::fs::set_permissions(&userns, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.bin_dir = Some(bin);
    cfg.idmap = Some((100_000, 65_536, 100_000, 65_536));
    cfg.extractor = Some(PathBuf::from(env!("CARGO_BIN_EXE_redox")));
    LxcDriver::new(cfg).unwrap()
}

fn guest(os: String) -> Guest {
    let mut c = Cluster::demo(0);
    let mut spec = GuestSpec {
        vmid: Some(260),
        name: "oci-kutu".into(),
        kind: GuestKind::Ct,
        node: "pve-ist-01".into(),
        cores: 1,
        memory_mb: 256,
        disk_gb: 1,
        storage: "local-zfs".into(),
        os,
        bridge: "nat".into(),
        onboot: false,
        iso: None,
        hw: Default::default(),
    };
    spec.hw.ct_address = Some("10.0.3.50/24".into());
    spec.hw.ct_gateway = Some("10.0.3.1".into());
    c.create_guest(&spec, "u", 0).unwrap();
    c.guest(260).unwrap().clone()
}

/// Kullanıcı ad alanı açılamayan sistemde imajdan kurulum sınanamaz.
fn namespaces() -> bool {
    let ok = Command::new("unshare").args(["-Ur", "true"]).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("ATLANDI: kullanıcı ad alanı açılamıyor");
    }
    ok
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("lxc")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
}

#[test]
fn a_container_is_built_from_an_image_in_three_compression_formats() {
    if !namespaces() {
        return;
    }
    let dir = temp();
    let owner = (0, 0);
    let canary = dir.join("kanarya");
    std::fs::create_dir_all(&canary).unwrap();
    let outside = canary.display().to_string();
    let registry = serve(|r| {
        let base = layer(owner, &[("etc/", b'5', "", ""), ("etc/passwd", b'0', "", "root:x:0:0::/root:/bin/sh\nweb:x:101:102::/:/bin/false\n"), ("app/silinecek", b'0', "", "1"), ("tuzak", b'2', &outside, "")]);
        let second = layer(owner, &[("app/.wh.silinecek", b'0', "", ""), ("app/sunucu", b'0', "", "ikili"), ("tuzak/sizinti", b'0', "", "kotu")]);
        let third = layer(owner, &[("app/ayar", b'0', "", "son")]);
        r.image(
            "1.0",
            serde_json::json!({ "Entrypoint": ["/app/sunucu"], "Cmd": ["--bicim", "uzun deger"], "Env": ["PATH=/usr/bin", "MOD=uretim"], "WorkingDir": "/app", "User": "web" }),
            vec![(TAR_GZIP, gzip(&base)), (TAR_ZSTD, zstd::stream::encode_all(second.as_slice(), 1).unwrap()), (TAR, third)],
        );
    });
    let driver = driver(&dir);
    let g = guest(format!("oci:127.0.0.1:{}/test/app:1.0", registry.port));
    driver.create(&g).unwrap_or_else(|e| panic!("{e}"));

    let container = dir.join("lxc/redox-260");
    let rootfs = container.join("rootfs");
    let read = |name: &str| std::fs::read_to_string(rootfs.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!((read("app/sunucu"), read("app/ayar"), read("etc/resolv.conf")), ("ikili".into(), "son".into(), "nameserver 10.0.3.1\n".into()));
    assert!(!rootfs.join("app/silinecek").exists());
    assert_eq!(std::fs::read_dir(&canary).unwrap().count(), 0, "imaj sunucuya yazamaz");

    let config = std::fs::read_to_string(container.join("config")).unwrap();
    for line in [
        "lxc.include = /usr/share/lxc/config/common.conf".to_string(),
        format!("lxc.rootfs.path = dir:{}", rootfs.display()),
        "lxc.init.cmd = /app/sunucu --bicim \"uzun deger\"".to_string(),
        "lxc.init.uid = 101".to_string(),
        "lxc.init.gid = 102".to_string(),
        "lxc.include = /usr/share/lxc/config/userns.conf".to_string(),
        "lxc.idmap = u 0 100000 65536".to_string(),
        "lxc.init.cwd = /app".to_string(),
        "lxc.environment = MOD=uretim".to_string(),
        "lxc.uts.name = oci-kutu".to_string(),
        "lxc.net.0.ipv4.address = 10.0.3.50/24".to_string(),
        "lxc.net.0.ipv4.gateway = 10.0.3.1".to_string(),
    ] {
        assert!(config.lines().any(|l| l == line), "eksik: {line}\n{config}");
    }
    assert_eq!(leftovers(&dir), ["redox-260"], "indirme önbelleği silinir");
    // Ayar değişince imajın satırları korunur.
    let mut bigger = g.clone();
    bigger.mem_total_mb = 512;
    driver.configure(&bigger).unwrap();
    let config = std::fs::read_to_string(container.join("config")).unwrap();
    assert!(config.contains("lxc.init.cmd = /app/sunucu") && config.contains("memory.max = 512M"));

    // Belirteç yalnızca kayıt sunucusuna gider; yönlendirilen adres onu görmez.
    let seen = registry.seen.lock().unwrap().clone();
    assert!(seen.iter().any(|(p, _)| p.starts_with("/token?")));
    assert!(seen.iter().filter(|(p, _)| p.starts_with("/cdn/")).count() >= 4);
    assert!(seen.iter().filter(|(p, _)| p.starts_with("/cdn/")).all(|(_, auth)| auth.is_none()), "{seen:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn failed_pulls_leave_nothing_behind() {
    if !namespaces() {
        return;
    }
    let dir = temp();
    let owner = (0, 0);
    let registry = serve(|r| {
        r.image("degisik", serde_json::json!({ "Cmd": ["/a"] }), vec![(TAR, layer(owner, &[("a", b'0', "", "1")]))]);
        // Kayıttaki katman özetinden farklı bir içerik sunulur.
        let tampered: Vec<String> = r.files.iter().filter(|(k, v)| k.starts_with("blob/") && v.len() > 512).map(|(k, _)| k.clone()).collect();
        assert_eq!(tampered.len(), 1, "bozulacak katman: {tampered:?}");
        for key in tampered {
            r.files.get_mut(&key).unwrap()[600] ^= 1;
        }
        r.image("kacak", serde_json::json!({ "Cmd": ["/a"] }), vec![(TAR, layer(owner, &[("../../kacak", b'0', "", "1")]))]);
        r.image("sahte-ayar", serde_json::json!({ "Cmd": ["/a\nlxc.hook.pre-start = /bin/x"] }), vec![(TAR, layer(owner, &[("ayri", b'0', "", "2")]))]);
        r.image("yabanci", serde_json::json!({ "Cmd": ["/a"] }), vec![("application/vnd.bilinmeyen", vec![0; 10])]);
    });
    let driver = driver(&dir);
    for (tag, expected) in [("degisik", "özetiyle uyuşmuyor"), ("kacak", "yolda \"..\" var"), ("sahte-ayar", "satır sonu"), ("yabanci", "desteklenmeyen katman türü"), ("yok", "imaj bulunamadı")] {
        let error = driver.create(&guest(format!("oci:127.0.0.1:{}/test/app:{tag}", registry.port))).err().unwrap_or_else(|| panic!("{tag}: reddedilmeliydi; istekler: {:?}", registry.seen.lock().unwrap()));
        assert!(error.contains(expected), "{tag}: {error}");
        assert_eq!(leftovers(&dir), Vec::<String>::new(), "{tag}");
    }
    assert!(!dir.join("kacak").exists() && !dir.join("lxc/kacak").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Yetkisiz kurulumda katmanlar bir kullanıcı ad alanında açılır: arşivdeki kök sahipliği (0:0) orada
/// uygulanabilmelidir. `lxc-usernsexec` yerine `unshare` kullanılır.
#[test]
fn unpacking_inside_a_user_namespace_applies_root_ownership() {
    if !Command::new("unshare").args(["-Ur", "true"]).status().is_ok_and(|s| s.success()) {
        eprintln!("ATLANDI: kullanıcı ad alanı açılamıyor");
        return;
    }
    let dir = temp();
    let root = dir.join("kok");
    std::fs::create_dir_all(&root).unwrap();
    let tar = dir.join("katman.tar");
    std::fs::write(&tar, layer((0, 0), &[("etc/", b'5', "", ""), ("etc/ad", b'0', "", "kutu")])).unwrap();
    let out = Command::new("unshare").arg("-Ur").arg(env!("CARGO_BIN_EXE_redox")).arg("oci-unpack").arg(&root).args(["tar", "1000"]).stdin(std::fs::File::open(&tar).unwrap()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "996");
    assert_eq!(std::fs::metadata(root.join("etc/ad")).unwrap().uid(), me(&dir).0, "ad alanının kökü bu kullanıcıdır");
    // Ad alanında karşılığı olmayan kimlik açık bir hatadır.
    std::fs::write(&tar, layer((1234, 1234), &[("baskasi", b'0', "", "1")])).unwrap();
    let out = Command::new("unshare").arg("-Ur").arg(env!("CARGO_BIN_EXE_redox")).arg("oci-unpack").arg(&root).args(["tar", "1000"]).stdin(std::fs::File::open(&tar).unwrap()).output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("sahipliği ayarlanamadı"));
    std::fs::remove_dir_all(&dir).unwrap();
}
