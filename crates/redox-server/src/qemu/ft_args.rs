//! Hata toleransı (COLO) için QEMU argümanları. Saf işlevlerdir.
//!
//! Birincil, olağan konuk gibi açılır; yalnızca diski `quorum` sarmalındadır, böylece ikizin diski çalışırken
//! ikinci çocuk olarak eklenebilir. İkizin diski üç katmanlıdır (asıl kopya, gizli ve etkin katman) ve ağ
//! trafiği süzgeçlerle birincile bağlanır.

use std::path::Path;

use super::args::opt;

/// Birincil ve ikizde asıl diskin QEMU'daki adı.
pub const DISK_ID: &str = "colo-disk0";
/// İkizde, birincilin yazdığı asıl kopyanın adı (NBD ile sunulur).
pub const PARENT_ID: &str = "parent0";

/// Hata toleransına uygun açılan konuğun asıl disk argümanı.
pub fn primary_drive(disk: &Path) -> String {
    format!("if=virtio,id={DISK_ID},driver=quorum,read-pattern=fifo,vote-threshold=1,children.0.file.filename={},children.0.driver=qcow2", opt(disk))
}

/// İkizin, birincile bağlandığı iki kapı: giden paketlerin kopyası ve karşılaştırmaya dönen paketler.
#[derive(Debug, Clone, Copy)]
pub struct PrimaryLink<'a> {
    pub host: &'a str,
    pub mirror_port: u16,
    pub compare_port: u16,
    /// Paket kapılarının şifreleme anahtarını tutan dizin (`keys.psk`).
    pub tls_dir: &'a Path,
}

/// İkizde, paket kapılarına bağlanırken kullanılan şifreleme nesnesinin adı.
pub const PACKET_TLS_ID: &str = "fttls-paket";

/// Olağan argümanlardaki asıl diski ikizin disk zinciriyle değiştirir ve ağ süzgeçlerini ekler.
/// `files`: `(asıl kopya, etkin katman, gizli katman)`.
pub fn secondary_args(mut args: Vec<String>, disk: &Path, files: (&Path, &Path, &Path), link: PrimaryLink<'_>) -> Result<Vec<String>, String> {
    let wanted = primary_drive(disk);
    let at = args.iter().position(|a| *a == wanted).filter(|at| *at > 0 && args[at - 1] == "-drive").ok_or("konuk hata toleransına uygun açılmıyor (asıl disk sarılı değil)")?;
    let (parent, active, hidden) = files;
    let chain = [
        "-drive".to_string(),
        format!("if=none,id={PARENT_ID},file.filename={},driver=qcow2", opt(parent)),
        "-drive".to_string(),
        format!(
            "if=none,id=childs0,driver=replication,mode=secondary,file.driver=qcow2,top-id={DISK_ID},file.file.filename={},file.backing.driver=qcow2,file.backing.file.filename={},file.backing.backing={PARENT_ID}",
            opt(active),
            opt(hidden)
        ),
        "-drive".to_string(),
        format!("if=virtio,id={DISK_ID},driver=quorum,read-pattern=fifo,vote-threshold=1,children.0=childs0"),
    ];
    args.splice(at - 1..=at, chain);
    // Paket kapıları da disk ve bellek kapıları gibi tek kullanımlık anahtarla şifrelenir: anahtarı bilmeyen biri
    // konuğun ağ trafiğini okuyamaz, ikizin yerine geçemez.
    args.extend(["-object".to_string(), format!("tls-creds-psk,id={PACKET_TLS_ID},endpoint=client,dir={}", opt(link.tls_dir))]);
    for (id, port) in [("red0", link.mirror_port), ("red1", link.compare_port)] {
        args.extend(["-chardev".to_string(), format!("socket,id={id},host={},port={port},reconnect-ms=1000,tls-creds={PACKET_TLS_ID}", link.host)]);
    }
    // Süzgeçler ağ ucundan sonra gelmelidir; argüman sırasının sonuna eklenmeleri yeter.
    for object in [
        "filter-redirector,id=f1,netdev=net0,queue=tx,indev=red0,vnet_hdr_support=on",
        "filter-redirector,id=f2,netdev=net0,queue=rx,outdev=red1,vnet_hdr_support=on",
        "filter-rewriter,id=rew0,netdev=net0,queue=all,vnet_hdr_support=on",
    ] {
        args.extend(["-object".to_string(), object.to_string()]);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standby_gets_the_replication_chain_and_the_network_filters() {
        let disk = Path::new("/d/vm-100.qcow2");
        let base: Vec<String> = ["-name", "x", "-drive", &primary_drive(disk), "-netdev", "bridge,id=net0,br=vds-a"].iter().map(|s| s.to_string()).collect();
        let args = secondary_args(base.clone(), disk, (Path::new("/f/p.qcow2"), Path::new("/f/a.qcow2"), Path::new("/f/h,x.qcow2")), PrimaryLink { host: "10.0.0.1", mirror_port: 49152, compare_port: 49153, tls_dir: Path::new("/run/100.ftp") }).unwrap();
        let text = args.join(" ");
        assert!(!text.contains("vm-100.qcow2"), "ikiz birincilin dosyasını açmaz: {text}");
        for part in [
            "-name x -drive if=none,id=parent0,file.filename=/f/p.qcow2,driver=qcow2",
            "driver=replication,mode=secondary",
            "file.backing.file.filename=/f/h,,x.qcow2,file.backing.backing=parent0",
            "children.0=childs0",
            "-object tls-creds-psk,id=fttls-paket,endpoint=client,dir=/run/100.ftp -chardev socket,id=red0",
            "socket,id=red0,host=10.0.0.1,port=49152,reconnect-ms=1000,tls-creds=fttls-paket",
            "socket,id=red1,host=10.0.0.1,port=49153,reconnect-ms=1000,tls-creds=fttls-paket",
            "filter-rewriter,id=rew0,netdev=net0",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        assert!(text.find("-netdev").unwrap() < text.find("filter-redirector").unwrap());
        // Sarılmamış diskten ikiz kurulmaz.
        let plain: Vec<String> = ["-drive", "file=/d/vm-100.qcow2,if=virtio,format=qcow2"].iter().map(|s| s.to_string()).collect();
        assert!(secondary_args(plain, disk, (disk, disk, disk), PrimaryLink { host: "h", mirror_port: 1, compare_port: 2, tls_dir: disk }).unwrap_err().contains("uygun açılmıyor"));
    }
}
