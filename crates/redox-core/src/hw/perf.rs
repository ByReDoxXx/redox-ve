//! Performans ayarları: sanal çekirdekleri sunucu çekirdeklerine sabitleme, belleği bir NUMA düğümüne bağlama
//! ve büyük bellek sayfaları (hugepages).

use super::*;

pub const MAX_HOST_CPU: u32 = 1023;
pub const MAX_NUMA_NODE: u32 = 63;

/// Çekirdek listesi: `0-3,8`. Sıralı ve tekrarsız numaralar döner.
pub fn parse_cpu_list(text: &str) -> Result<Vec<u32>, String> {
    let bad = || format!("geçersiz çekirdek listesi \"{text}\"; örnekler: 0-3, 2,4,6, 0-1,8-9");
    let mut cpus = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let number = |s: &str| s.trim().parse::<u32>().ok().filter(|n| *n <= MAX_HOST_CPU).ok_or_else(bad);
        let (a, b) = (number(a)?, number(b)?);
        if a > b {
            return Err(bad());
        }
        cpus.extend(a..=b);
    }
    cpus.sort_unstable();
    cpus.dedup();
    if cpus.is_empty() {
        return Err(bad());
    }
    Ok(cpus)
}

/// Listenin kısa yazımı: `[0,1,2,3,8]` → `0-3,8`.
pub fn format_cpu_list(cpus: &[u32]) -> String {
    let mut parts: Vec<(u32, u32)> = Vec::new();
    for cpu in cpus {
        match parts.last_mut() {
            Some((_, end)) if *end + 1 == *cpu => *end = *cpu,
            _ => parts.push((*cpu, *cpu)),
        }
    }
    parts.iter().map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") }).collect::<Vec<_>>().join(",")
}

impl Hardware {
    /// Belleğin nasıl ayrıldığını değiştiren bir ayar var mı?
    pub fn pinned_memory(&self) -> bool {
        self.hugepages || self.numa_node.is_some()
    }

    pub(super) fn check_performance(&self, kind: GuestKind) -> Result<(), String> {
        if (self.pinned_memory() || !self.cpu_affinity.is_empty()) && kind != GuestKind::Vm {
            return Err("çekirdek sabitleme, NUMA ve büyük sayfalar yalnızca sanal makinelerde kullanılır".to_string());
        }
        if !self.cpu_affinity.is_empty() {
            parse_cpu_list(&self.cpu_affinity)?;
        }
        if self.numa_node.is_some_and(|n| n > MAX_NUMA_NODE) {
            return Err(format!("NUMA düğümü 0 ile {MAX_NUMA_NODE} arasında olmalı"));
        }
        if self.pinned_memory() && self.max_mem_mb > 0 {
            return Err("NUMA düğümüne bağlı ya da büyük sayfalı konuğa çalışırken bellek eklenemez; \"en fazla bellek\" 0 olmalı".to_string());
        }
        if self.hugepages && self.balloon {
            return Err("büyük sayfalarla bellek balonu kullanılamaz: bellek konuk açılırken bütünüyle ayrılır".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists_parse_and_print_compactly() {
        assert_eq!(parse_cpu_list("0-3,8").unwrap(), [0, 1, 2, 3, 8]);
        assert_eq!(parse_cpu_list(" 5, 2 ,2-3").unwrap(), [2, 3, 5]);
        assert_eq!(format_cpu_list(&[0, 1, 2, 3, 8, 10, 11]), "0-3,8,10-11");
        for bad in ["", "a", "3-1", "1024", "0-", "-1", "0;1"] {
            assert!(parse_cpu_list(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn memory_settings_refuse_combinations_that_cannot_work() {
        let huge = Hardware { hugepages: true, ..Default::default() };
        assert!(huge.validate(GuestKind::Vm).is_ok());
        assert!(huge.validate(GuestKind::Ct).is_err());
        assert!(Hardware { balloon: true, ..huge.clone() }.validate(GuestKind::Vm).unwrap_err().contains("balonu"));
        assert!(Hardware { max_mem_mb: 4096, ..huge }.validate(GuestKind::Vm).unwrap_err().contains("bellek eklenemez"));
        let numa = Hardware { numa_node: Some(0), balloon: true, ..Default::default() };
        assert!(numa.validate(GuestKind::Vm).is_ok(), "NUMA bağı balonla birlikte olabilir");
        assert!(Hardware { numa_node: Some(64), ..Default::default() }.validate(GuestKind::Vm).is_err());
        assert!(Hardware { cpu_affinity: "9-1".into(), ..Default::default() }.validate(GuestKind::Vm).is_err());
    }
}
