//! İşlemci modelleri, üretici denetimi ve iç içe sanallaştırma.

/// Seçilebilen işlemci modelleri (QEMU adlarıyla), eskiden yeniye. Boş değer sunucunun kendi işlemcisidir.
pub const CPU_TYPES: [&str; 17] = [
    "qemu64", "kvm64", "Nehalem", "Westmere", "SandyBridge", "IvyBridge", "Haswell", "Broadwell", "Skylake-Client", "Skylake-Server",
    "Cascadelake-Server", "Icelake-Server", "EPYC", "EPYC-Rome", "EPYC-Milan", "EPYC-Genoa", "max",
];

/// Modelin üreticisi: `intel`, `amd` ya da üreticiden bağımsız modellerde (`qemu64`, `kvm64`, `max`, boş) `None`.
pub fn cpu_vendor_of(model: &str) -> Option<&'static str> {
    match model {
        "" | "qemu64" | "kvm64" | "max" => None,
        m if m.starts_with("EPYC") => Some("amd"),
        m if CPU_TYPES.contains(&m) => Some("intel"),
        _ => None,
    }
}

/// `/proc/cpuinfo` içeriğinden sunucunun işlemci üreticisi.
pub fn parse_cpu_vendor(cpuinfo: &str) -> Option<&'static str> {
    let vendor = cpuinfo.lines().find_map(|l| l.strip_prefix("vendor_id")).and_then(|v| v.split_once(':')).map(|(_, v)| v.trim())?;
    match vendor {
        "GenuineIntel" => Some("intel"),
        "AuthenticAMD" | "HygonGenuine" => Some("amd"),
        _ => None,
    }
}

/// QEMU'nun `-cpu` değeri. `model` çözülmüş addır (`host`, `max` ya da bir model). İç içe sanallaştırmada
/// adı belli modellere sanallaştırma uzantısı eklenir; `host` ve `max` onu zaten taşır.
pub fn cpu_arg(model: &str, nested: bool) -> String {
    match cpu_vendor_of(model).filter(|_| nested) {
        Some("amd") => format!("{model},+svm"),
        Some(_) => format!("{model},+vmx"),
        None => model.to_string(),
    }
}

/// Seçilen model bu sunucunun işlemcisinde çalıştırılabilir mi? Üretici bilinmiyorsa engellenmez.
pub fn check_cpu_vendor(model: &str, host: Option<&str>) -> Result<(), String> {
    match (cpu_vendor_of(model), host) {
        (Some(wanted), Some(have)) if wanted != have => Err(format!(
            "{model} bir {} modeli, bu sunucunun işlemcisi {}: konuk eksik özelliklerle açılır ya da hiç açılmaz. Sunucunun kendi üreticisinden bir model ya da host seçin",
            vendor_label(wanted),
            vendor_label(have)
        )),
        _ => Ok(()),
    }
}

pub(super) fn vendor_label(vendor: &str) -> &'static str {
    if vendor == "amd" { "AMD" } else { "Intel" }
}

pub fn valid_cpu_type(name: &str) -> bool {
    name.is_empty() || CPU_TYPES.contains(&name)
}
