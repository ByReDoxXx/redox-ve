//! OCI (Docker) imaj adları. Konteynerin şablonu `oci:<imaj>` ise kök dosya sistemi bir kayıt sunucusundan
//! çekilir ve konteyner imajın kendi programını çalıştırır.

/// Şablon alanındaki ön ek.
pub const OCI_PREFIX: &str = "oci:";
const MAX_REFERENCE: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    /// Kayıt sunucusu (`ad[:port]`).
    pub registry: String,
    pub repository: String,
    /// Etiket ya da `sha256:…` özeti.
    pub reference: String,
}

impl ImageRef {
    /// Docker'ın yazım kuralları: `nginx`, `grafana/grafana:11`, `ghcr.io/sahip/ad@sha256:…`.
    pub fn parse(text: &str) -> Result<ImageRef, String> {
        let bad = || format!("geçersiz imaj adı: {text} (örn. nginx:1.29, ghcr.io/sahip/ad:etiket)");
        if text.is_empty() || text.len() > MAX_REFERENCE {
            return Err(bad());
        }
        let (name, digest) = match text.split_once('@') {
            Some((name, digest)) if valid_digest(digest) => (name, Some(digest)),
            Some(_) => return Err(bad()),
            None => (text, None),
        };
        // İlk parça nokta ya da iki nokta içeriyorsa (ya da localhost ise) kayıt sunucusudur.
        let (registry, rest) = match name.split_once('/') {
            Some((first, rest)) if first.contains(['.', ':']) || first == "localhost" => (first, rest),
            _ => ("docker.io", name),
        };
        let (repository, tag) = match rest.rsplit_once(':') {
            Some((repository, tag)) => (repository, Some(tag)),
            None => (rest, None),
        };
        if !valid_registry(registry) || !valid_repository(repository) || tag.is_some_and(|t| !valid_tag_name(t)) {
            return Err(bad());
        }
        let (registry, repository) = match registry {
            "docker.io" | "index.docker.io" => ("registry-1.docker.io", if repository.contains('/') { repository.to_string() } else { format!("library/{repository}") }),
            other => (other, repository.to_string()),
        };
        Ok(ImageRef { registry: registry.to_string(), repository, reference: digest.or(tag).unwrap_or("latest").to_string() })
    }
}

pub fn valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn valid_registry(text: &str) -> bool {
    let (host, port) = match text.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (text, None),
    };
    port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok())
        && !host.is_empty()
        && host.split('.').all(|l| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

fn valid_repository(text: &str) -> bool {
    !text.is_empty()
        && text.split('/').all(|part| {
            let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
            part.bytes().next().is_some_and(edge) && part.bytes().last().is_some_and(edge) && part.bytes().all(|b| edge(b) || b"._-".contains(&b))
        })
}

fn valid_tag_name(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= 128 && !tag.starts_with(['.', '-']) && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// Şablon bir OCI imajı mı? Öyleyse imaj adı.
pub fn oci_reference(os: &str) -> Option<&str> {
    os.strip_prefix(OCI_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> (String, String, String) {
        let r = ImageRef::parse(text).unwrap_or_else(|e| panic!("{e}"));
        (r.registry, r.repository, r.reference)
    }

    #[test]
    fn docker_shorthand_expands_like_docker_does() {
        let owned = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
        assert_eq!(parts("nginx"), owned("registry-1.docker.io", "library/nginx", "latest"));
        assert_eq!(parts("grafana/grafana:11.2.0"), owned("registry-1.docker.io", "grafana/grafana", "11.2.0"));
        assert_eq!(parts("docker.io/library/redis:7-alpine"), owned("registry-1.docker.io", "library/redis", "7-alpine"));
        assert_eq!(parts("ghcr.io/sahip/ad:v1_2"), owned("ghcr.io", "sahip/ad", "v1_2"));
        assert_eq!(parts("localhost:5000/a/b/c"), owned("localhost:5000", "a/b/c", "latest"));
        let digest = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(parts(&format!("quay.io/a/b@{digest}")), owned("quay.io", "a/b", &digest));
    }

    #[test]
    fn names_that_could_change_the_request_are_refused() {
        let long = "a".repeat(300);
        for bad in ["", "Nginx", "a b", "nginx:", ":latest", "a//b", "../etc", "a/../b", "http://x.io/a", "x.io/a?b=1", "x.io/a#b", "nginx@sha256:12", "nginx@md5:abc", "x.io:99999/a", "x.io:/a", "-x.io/a", "nginx:a/b", "nginx\n", "kim:parola@x.io/a", long.as_str()] {
            assert!(ImageRef::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(oci_reference("oci:nginx"), Some("nginx"));
        assert_eq!(oci_reference("debian-12"), None);
    }
}
