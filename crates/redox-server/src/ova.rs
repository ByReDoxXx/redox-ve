//! OVA (Open Virtual Appliance) içe ve dışa aktarma.
//!
//! OVA bir tar arşividir: bir `.ovf` tanım dosyası (XML) ve disk imajları (`.vmdk`). İçe aktarmada
//! arşivdeki üye adları güvenilmezdir ve hiçbir zaman dosya yolu olarak kullanılmaz: diskin baytları,
//! arşivdeki konumundan okunup bizim adlandırdığımız dosyaya yazılır. Dışa aktarmada arşiv diske
//! yazılmadan, parça parça üretilir.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const BLOCK: u64 = 512;
/// Arşivde aranacak en fazla üye (bozuk ya da kötü niyetli arşivde sonsuz döngüye karşı).
const MAX_MEMBERS: usize = 4096;
/// Tanım dosyası (OVF) için okunacak en fazla bayt.
const MAX_OVF: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    /// Üyenin verisinin arşivdeki başlangıcı.
    pub offset: u64,
    pub size: u64,
}

/// OVF tanımından okunan donanım önerisi.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct OvfInfo {
    pub name: Option<String>,
    pub cores: Option<u32>,
    pub memory_mb: Option<u64>,
    /// Tanımda anılan disk dosyaları (sırasıyla).
    pub disks: Vec<String>,
}

fn field(header: &[u8], range: std::ops::Range<usize>) -> &[u8] {
    let raw = &header[range];
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    &raw[..end]
}

/// Tar sayı alanı: sekizlik metin ya da (8 GB'ı aşan boyutlar için) ikili büyük-uçlu.
fn number(raw: &[u8]) -> Option<u64> {
    if raw.first().is_some_and(|b| b & 0x80 != 0) {
        let bytes = &raw[raw.len().saturating_sub(8)..];
        // İlk baytın işaret biti sayıya dahil değildir.
        let mut value = 0u64;
        for (i, b) in bytes.iter().enumerate() {
            let b = if raw.len() <= 8 && i == 0 { b & 0x7f } else { *b };
            value = value.checked_mul(256)?.checked_add(b as u64)?;
        }
        return Some(value);
    }
    let text = std::str::from_utf8(raw).ok()?.trim_matches(|c: char| c == ' ' || c == '\0');
    if text.is_empty() { Some(0) } else { u64::from_str_radix(text, 8).ok() }
}

/// Arşivin üyelerini (yalnızca düz dosyaları) listeler.
pub fn members(file: &mut std::fs::File) -> Result<Vec<Member>, String> {
    let io = |e: std::io::Error| format!("arşiv okunamadı: {e}");
    let total = file.metadata().map_err(io)?.len();
    let (mut found, mut offset, mut long_name): (Vec<Member>, u64, Option<String>) = (Vec::new(), 0, None);
    let mut header = [0u8; BLOCK as usize];
    for _ in 0..MAX_MEMBERS {
        if offset == total {
            break;
        }
        if offset + BLOCK > total {
            // Üstbilginin ortasında biten dosya: indirme ya da yükleme yarıda kalmış.
            return Err("arşiv eksik (kesilmiş dosya)".to_string());
        }
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        file.read_exact(&mut header).map_err(io)?;
        if header.iter().all(|b| *b == 0) {
            break;
        }
        if &header[257..262] != b"ustar" {
            return Err("dosya bir tar arşivi (OVA) değil".to_string());
        }
        let size = number(&header[124..136]).ok_or("arşivde geçersiz boyut alanı")?;
        let data = offset + BLOCK;
        let next = data.checked_add(size.div_ceil(BLOCK).checked_mul(BLOCK).ok_or("arşivde geçersiz boyut")?).ok_or("arşivde geçersiz boyut")?;
        if data + size > total {
            return Err("arşiv eksik (kesilmiş dosya)".to_string());
        }
        match header[156] {
            // GNU uzun ad: bir sonraki üyenin adı bu üyenin verisidir.
            b'L' => {
                let mut name = vec![0u8; size.min(4096) as usize];
                file.read_exact(&mut name).map_err(io)?;
                long_name = Some(String::from_utf8_lossy(&name).trim_end_matches('\0').to_string());
            }
            b'0' | 0 => {
                let prefix = String::from_utf8_lossy(field(&header, 345..500)).to_string();
                let short = String::from_utf8_lossy(field(&header, 0..100)).to_string();
                let name = long_name.take().unwrap_or(if prefix.is_empty() { short } else { format!("{prefix}/{short}") });
                found.push(Member { name, offset: data, size });
            }
            // Dizinler, bağlantılar, pax üstbilgileri: atlanır.
            _ => long_name = None,
        }
        offset = next;
    }
    Ok(found)
}

/// Etiket adının sonundaki metni bulur: `<önek:Ad ...>metin</...>`.
fn tag_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(at) = xml[from..].find(tag) {
        let start = from + at;
        let before = xml[..start].chars().next_back();
        let after = &xml[start + tag.len()..];
        // Etiket adı tam eşleşmeli: önünde `<` ya da `:` , ardında `>` ya da boşluk.
        if matches!(before, Some('<') | Some(':')) && after.starts_with(['>', ' ']) {
            let open_end = after.find('>')?;
            let body = &after[open_end + 1..];
            return Some(body[..body.find('<')?].trim());
        }
        from = start + tag.len();
    }
    None
}

fn attribute<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let at = element.find(&format!("{name}=\""))? + name.len() + 2;
    Some(&element[at..at + element[at..].find('"')?])
}

/// OVF tanımından adı, çekirdek sayısını, belleği ve disk dosyalarını çıkarır. Tam bir XML ayrıştırıcı
/// değildir; bulamadığı alanı boş bırakır (öneridir, zorunlu değildir).
pub fn parse_ovf(xml: &str) -> OvfInfo {
    let mut info = OvfInfo::default();
    // <File ovf:href="disk1.vmdk" .../>
    let mut rest = xml;
    while let Some(at) = rest.find("<File ").or_else(|| rest.find(":File ")) {
        let element = &rest[at..];
        let end = element.find('>').unwrap_or(element.len());
        if let Some(href) = attribute(&element[..end], "href") {
            info.disks.push(href.to_string());
        }
        rest = &element[end..];
    }
    // Donanım ögeleri: ResourceType 3 = işlemci, 4 = bellek.
    for item in xml.split("Item>").filter(|block| block.contains("ResourceType")) {
        let quantity = tag_text(item, "VirtualQuantity").and_then(|q| q.parse::<u64>().ok());
        match (tag_text(item, "ResourceType"), quantity) {
            (Some("3"), Some(n)) if info.cores.is_none() => info.cores = u32::try_from(n).ok(),
            (Some("4"), Some(n)) if info.memory_mb.is_none() => {
                let units = tag_text(item, "AllocationUnits").unwrap_or("byte * 2^20").replace(' ', "").to_ascii_lowercase();
                info.memory_mb = Some(match units.as_str() {
                    "byte*2^30" | "gigabytes" => n.saturating_mul(1024),
                    "byte*2^10" | "kilobytes" => n / 1024,
                    "byte" | "bytes" => n / (1024 * 1024),
                    _ => n,
                });
            }
            _ => {}
        }
    }
    if let Some(at) = xml.find("VirtualSystem ") {
        let element = &xml[at..];
        info.name = attribute(&element[..element.find('>').unwrap_or(element.len())], "id").map(str::to_string);
    }
    info
}

/// Arşivdeki tanımı (varsa) okur.
pub fn inspect(ova: &Path) -> Result<OvfInfo, String> {
    let mut file = std::fs::File::open(ova).map_err(|e| format!("{} açılamadı: {e}", ova.display()))?;
    let all = members(&mut file)?;
    let Some(ovf) = all.iter().find(|m| m.name.to_ascii_lowercase().ends_with(".ovf")) else {
        return Ok(OvfInfo::default());
    };
    let mut text = Vec::new();
    file.seek(SeekFrom::Start(ovf.offset)).map_err(|e| e.to_string())?;
    (&mut file).take(ovf.size.min(MAX_OVF)).read_to_end(&mut text).map_err(|e| e.to_string())?;
    Ok(parse_ovf(&String::from_utf8_lossy(&text)))
}

/// Arşivdeki (tanımın ilk andığı, yoksa ilk bulunan) diski `dest` dosyasına çıkarır; diskin arşivdeki
/// adını döndürür. Üye adı yalnızca seçim için kullanılır, yol olarak kullanılmaz.
pub fn extract_disk(ova: &Path, dest: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(ova).map_err(|e| format!("{} açılamadı: {e}", ova.display()))?;
    let all = members(&mut file)?;
    let is_disk = |m: &&Member| [".vmdk", ".qcow2", ".img", ".vhd", ".vhdx", ".vdi"].iter().any(|ext| m.name.to_ascii_lowercase().ends_with(ext));
    let wanted = inspect(ova).ok().and_then(|info| info.disks.into_iter().next());
    let disk = wanted
        .and_then(|name| all.iter().filter(is_disk).find(|m| m.name == name || m.name.rsplit('/').next() == Some(name.as_str())))
        .or_else(|| all.iter().find(is_disk))
        .ok_or("arşivde disk imajı (.vmdk) bulunamadı")?;
    let io = |e: std::io::Error| format!("disk arşivden çıkarılamadı: {e}");
    file.seek(SeekFrom::Start(disk.offset)).map_err(io)?;
    let mut out = std::fs::File::create(dest).map_err(io)?;
    let copied = std::io::copy(&mut (&mut file).take(disk.size), &mut out).map_err(io)?;
    if copied != disk.size {
        return Err("arşiv eksik (kesilmiş dosya)".to_string());
    }
    out.sync_all().map_err(io)?;
    Ok(disk.name.clone())
}

/// Tar (ustar) üstbilgisi: düz dosya, 0644.
pub fn tar_header(name: &str, size: u64) -> [u8; BLOCK as usize] {
    let mut h = [0u8; BLOCK as usize];
    let name = name.as_bytes();
    h[..name.len().min(100)].copy_from_slice(&name[..name.len().min(100)]);
    h[100..108].copy_from_slice(b"0000644\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    if size < 0o77777777777 {
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    } else {
        // 8 GB ve üzeri: ikili büyük-uçlu boyut (GNU uzantısı; yaygın araçlar okur).
        h[124] = 0x80;
        h[128..136].copy_from_slice(&size.to_be_bytes());
    }
    h[136..148].copy_from_slice(b"00000000000\0");
    h[148..156].fill(b' ');
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    h
}

/// Üyenin verisinden sonra 512'nin katına tamamlayan sıfır sayısı.
pub fn padding(size: u64) -> u64 {
    (BLOCK - size % BLOCK) % BLOCK
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Dışa aktarılan konuğun OVF tanımı. `disks`: (dosya adı, dosya boyutu, sanal kapasite) bayt cinsinden.
pub fn ovf_xml(name: &str, cores: u32, memory_mb: u64, disks: &[(String, u64, u64)]) -> String {
    let name = xml_escape(name);
    let mut files = String::new();
    let mut sections = String::new();
    let mut items = String::new();
    for (i, (file, size, capacity)) in disks.iter().enumerate() {
        let n = i + 1;
        files.push_str(&format!("    <File ovf:id=\"file{n}\" ovf:href=\"{}\" ovf:size=\"{size}\"/>\n", xml_escape(file)));
        sections.push_str(&format!(
            "    <Disk ovf:diskId=\"vmdisk{n}\" ovf:fileRef=\"file{n}\" ovf:capacity=\"{capacity}\" ovf:format=\"http://www.vmware.com/interfaces/specifications/vmdk.html#streamOptimized\"/>\n"
        ));
        items.push_str(&format!(
            "      <Item>\n        <rasd:AddressOnParent>{i}</rasd:AddressOnParent>\n        <rasd:ElementName>Disk {n}</rasd:ElementName>\n        <rasd:HostResource>ovf:/disk/vmdisk{n}</rasd:HostResource>\n        <rasd:InstanceID>{}</rasd:InstanceID>\n        <rasd:Parent>3</rasd:Parent>\n        <rasd:ResourceType>17</rasd:ResourceType>\n      </Item>\n",
            10 + n
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1" xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1" xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData" xmlns:vssd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_VirtualSystemSettingData">
  <References>
{files}  </References>
  <DiskSection>
    <Info>Sanal diskler</Info>
{sections}  </DiskSection>
  <NetworkSection>
    <Info>Ağlar</Info>
    <Network ovf:name="VM Network"><Description>VM Network</Description></Network>
  </NetworkSection>
  <VirtualSystem ovf:id="{name}">
    <Info>RedoX VE tarafından dışa aktarıldı</Info>
    <Name>{name}</Name>
    <OperatingSystemSection ovf:id="101"><Info>Konuk işletim sistemi</Info></OperatingSystemSection>
    <VirtualHardwareSection>
      <Info>Sanal donanım</Info>
      <System>
        <vssd:ElementName>Virtual Hardware Family</vssd:ElementName>
        <vssd:InstanceID>0</vssd:InstanceID>
        <vssd:VirtualSystemIdentifier>{name}</vssd:VirtualSystemIdentifier>
        <vssd:VirtualSystemType>vmx-13</vssd:VirtualSystemType>
      </System>
      <Item>
        <rasd:AllocationUnits>hertz * 10^6</rasd:AllocationUnits>
        <rasd:ElementName>{cores} sanal işlemci</rasd:ElementName>
        <rasd:InstanceID>1</rasd:InstanceID>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>{cores}</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits>
        <rasd:ElementName>{memory_mb} MB bellek</rasd:ElementName>
        <rasd:InstanceID>2</rasd:InstanceID>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>{memory_mb}</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:Address>0</rasd:Address>
        <rasd:ElementName>SCSI denetleyicisi</rasd:ElementName>
        <rasd:InstanceID>3</rasd:InstanceID>
        <rasd:ResourceSubType>lsilogic</rasd:ResourceSubType>
        <rasd:ResourceType>6</rasd:ResourceType>
      </Item>
{items}      <Item>
        <rasd:AutomaticAllocation>true</rasd:AutomaticAllocation>
        <rasd:Connection>VM Network</rasd:Connection>
        <rasd:ElementName>Ağ kartı</rasd:ElementName>
        <rasd:InstanceID>9</rasd:InstanceID>
        <rasd:ResourceSubType>E1000</rasd:ResourceSubType>
        <rasd:ResourceType>10</rasd:ResourceType>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>
"#
    )
}

/// Bir arşivi diske yazar (testler ve küçük arşivler için): (ad, içerik) üyeleri.
pub fn write_tar(dest: &Path, entries: &[(&str, &[u8])]) -> std::io::Result<()> {
    let mut out = std::fs::File::create(dest)?;
    for (name, data) in entries {
        out.write_all(&tar_header(name, data.len() as u64))?;
        out.write_all(data)?;
        out.write_all(&vec![0u8; padding(data.len() as u64) as usize])?;
    }
    out.write_all(&[0u8; 2 * BLOCK as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("redox-ova-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const VMWARE_OVF: &str = r#"<?xml version="1.0"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1" xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1" xmlns:rasd="x">
  <References>
    <File ovf:href="web-disk1.vmdk" ovf:id="file1" ovf:size="1234"/>
    <File ovf:id="file2" ovf:href="web-disk2.vmdk"/>
  </References>
  <VirtualSystem ovf:id="web-sunucu">
    <VirtualHardwareSection>
      <Item><rasd:ElementName>4 virtual CPU(s)</rasd:ElementName><rasd:ResourceType>3</rasd:ResourceType><rasd:VirtualQuantity>4</rasd:VirtualQuantity></Item>
      <Item><rasd:AllocationUnits>byte * 2^30</rasd:AllocationUnits><rasd:ResourceType>4</rasd:ResourceType><rasd:VirtualQuantity>8</rasd:VirtualQuantity></Item>
      <Item><rasd:ResourceType>17</rasd:ResourceType><rasd:HostResource>ovf:/disk/vmdisk1</rasd:HostResource></Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;

    #[test]
    fn reads_hardware_hints_from_an_ovf() {
        let info = parse_ovf(VMWARE_OVF);
        assert_eq!(info, OvfInfo { name: Some("web-sunucu".into()), cores: Some(4), memory_mb: Some(8192), disks: vec!["web-disk1.vmdk".into(), "web-disk2.vmdk".into()] });
        // Öneksiz etiketler (VirtualBox) ve megabayt birimi.
        let plain = "<VirtualSystem id=\"x\"><Item><ResourceType>3</ResourceType><VirtualQuantity>2</VirtualQuantity></Item><Item><AllocationUnits>MegaBytes</AllocationUnits><ResourceType>4</ResourceType><VirtualQuantity>2048</VirtualQuantity></Item></VirtualSystem>";
        let info = parse_ovf(plain);
        assert_eq!((info.cores, info.memory_mb), (Some(2), Some(2048)));
        // Anlamsız girdi çökertmez; yalnızca öneri vermez.
        assert_eq!(parse_ovf("<bozuk"), OvfInfo::default());
        assert_eq!(parse_ovf(""), OvfInfo::default());
    }

    #[test]
    fn our_own_ovf_round_trips() {
        let xml = ovf_xml("db & <arka>", 6, 4096, &[("db-disk1.vmdk".into(), 100, 10 << 30), ("db-disk2.vmdk".into(), 50, 5 << 30)]);
        assert!(xml.contains("db &amp; &lt;arka&gt;") && !xml.contains("<arka>"), "ad XML'e kaçırılarak yazılır");
        let info = parse_ovf(&xml);
        assert_eq!((info.cores, info.memory_mb, info.disks), (Some(6), Some(4096), vec!["db-disk1.vmdk".to_string(), "db-disk2.vmdk".to_string()]));
        assert!(xml.contains(&format!("ovf:capacity=\"{}\"", 10u64 << 30)));
    }

    #[test]
    fn extracts_the_disk_named_by_the_ovf() {
        let dir = temp("cikar");
        let ova = dir.join("web.ova");
        let disk2 = vec![7u8; 1500];
        write_tar(&ova, &[("web.ovf", VMWARE_OVF.as_bytes()), ("web-disk2.vmdk", &disk2), ("web-disk1.vmdk", b"birinci disk"), ("web.mf", b"SHA256(x)= 0")]).unwrap();
        let mut file = std::fs::File::open(&ova).unwrap();
        let all = members(&mut file).unwrap();
        assert_eq!(all.iter().map(|m| (m.name.as_str(), m.size)).collect::<Vec<_>>(), [("web.ovf", VMWARE_OVF.len() as u64), ("web-disk2.vmdk", 1500), ("web-disk1.vmdk", 12), ("web.mf", 12)]);
        assert_eq!(inspect(&ova).unwrap().cores, Some(4));
        let out = dir.join("disk.tmp");
        assert_eq!(extract_disk(&ova, &out).unwrap(), "web-disk1.vmdk", "tanımın ilk andığı disk seçilir");
        assert_eq!(std::fs::read(&out).unwrap(), b"birinci disk");

        // Tanımsız arşiv: ilk disk. Üye adı ne olursa olsun yalnızca hedef dosyaya yazılır.
        let hostile = dir.join("kotu.ova");
        write_tar(&hostile, &[("../../../etc/cron.d/kotu.vmdk", b"zararsiz veri")]).unwrap();
        assert_eq!(extract_disk(&hostile, &out).unwrap(), "../../../etc/cron.d/kotu.vmdk");
        assert_eq!(std::fs::read(&out).unwrap(), b"zararsiz veri");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3, "arşivdeki ad dosya yolu olarak kullanılmaz");

        // Disksiz, tar olmayan ve kesilmiş arşivler açık hatayla reddedilir.
        let empty = dir.join("bos.ova");
        write_tar(&empty, &[("x.ovf", b"<a/>")]).unwrap();
        assert!(extract_disk(&empty, &out).unwrap_err().contains("disk imajı"));
        std::fs::write(&empty, vec![b'x'; 2048]).unwrap();
        assert!(extract_disk(&empty, &out).unwrap_err().contains("tar arşivi"));
        let whole = std::fs::read(&ova).unwrap();
        std::fs::write(&empty, &whole[..1024 + 700]).unwrap();
        assert!(extract_disk(&empty, &out).unwrap_err().contains("eksik"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tar_headers_are_valid_and_sizes_are_padded() {
        let h = tar_header("a.vmdk", 1500);
        assert_eq!(&h[257..262], b"ustar");
        assert_eq!(number(&h[124..136]), Some(1500));
        let stored: u32 = u32::from_str_radix(std::str::from_utf8(&h[148..154]).unwrap(), 8).unwrap();
        let mut blank = h;
        blank[148..156].fill(b' ');
        assert_eq!(stored, blank.iter().map(|b| *b as u32).sum::<u32>(), "sağlama toplamı");
        assert_eq!((padding(0), padding(1), padding(512), padding(1500)), (0, 511, 0, 36));
        // 8 GB'ı aşan boyut ikili yazılır ve geri okunur.
        let big = 20u64 << 30;
        assert_eq!(number(&tar_header("buyuk.vmdk", big)[124..136]), Some(big));
        assert_eq!(number(b"00000002734\0"), Some(1500));
        assert_eq!(number(b"zzz"), None);
    }
}
