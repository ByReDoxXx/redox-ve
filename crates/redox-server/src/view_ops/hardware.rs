//! Ayar formunun alanları ve donanım sekmesi.

use super::*;

/// Düzenleme formunun (doğrulanmamış) alanları.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct EditForm {
    /// Formun hangi sekmeden geldiği: `donanim`, `secenekler` ya da boş (bütün alanlar).
    pub part: String,
    pub name: String,
    pub os: String,
    pub tags: String,
    pub pool: String,
    pub cores: String,
    pub memory_mb: String,
    pub iso: String,
    pub iso2: String,
    pub cpu_affinity: String,
    pub numa_node: String,
    pub hugepages: Option<String>,
    pub disk_bus: String,
    pub nic_model: String,
    pub forwards: String,
    pub bridge: String,
    pub onboot: Option<String>,
    pub boot_order: String,
    pub boot_delay: String,
    pub restart: Option<String>,
    pub cpu_shares: String,
    pub mem_reserve_mb: String,
    pub disk_mbps: String,
    pub disk_iops: String,
    pub net_mbps: String,
    pub vlan: String,
    pub shares: String,
    pub backup_time: String,
    pub backup_keep: String,
    pub rpo_hours: String,
    pub backup_weekly: String,
    pub backup_monthly: String,
    pub backup_yearly: String,
    pub firewall: Option<String>,
    pub nic_guard: Option<String>,
    pub fw_rules: String,
    pub fw_out_restrict: Option<String>,
    pub fw_out: String,
    pub fw_groups: String,
    pub backup_pre: String,
    pub test_port: String,
    pub test_cmd: String,
    pub backup_post: String,
    pub firmware: String,
    pub tpm: Option<String>,
    pub nics: String,
    pub usb: String,
    pub pci: String,
    pub cpu_type: String,
    pub balloon: Option<String>,
    pub sound: Option<String>,
    pub max_cores: String,
    pub max_memory_mb: String,
    pub nested: Option<String>,
    pub ci_user: String,
    pub ci_ssh_key: String,
    pub ci_ip: String,
    pub ci_gw: String,
    pub ci_dns: String,
}

impl EditForm {
    pub fn from_guest(g: &Guest) -> Self {
        EditForm {
            part: String::new(),
            name: g.name.clone(),
            os: g.os.clone(),
            tags: g.tags.join(", "),
            pool: g.pool.clone().unwrap_or_default(),
            cores: g.cores.to_string(),
            memory_mb: g.mem_total_mb.to_string(),
            iso: g.iso.clone().unwrap_or_default(),
            iso2: g.hw.iso2.clone().unwrap_or_default(),
            cpu_affinity: g.hw.cpu_affinity.clone(),
            numa_node: g.hw.numa_node.map(|n| n.to_string()).unwrap_or_default(),
            hugepages: g.hw.hugepages.then(|| "1".to_string()),
            disk_bus: g.hw.disk_bus.as_str().to_string(),
            nic_model: g.hw.nic_model.as_str().to_string(),
            forwards: g.forwards.iter().map(|f| format!("{}:{}", f.host_port, f.guest_port)).collect::<Vec<_>>().join(", "),
            bridge: g.bridge.clone(),
            backup_time: g.backup_time.clone().unwrap_or_default(),
            backup_keep: g.backup_keep.to_string(),
            rpo_hours: g.hw.rpo_hours.to_string(),
            backup_weekly: g.backup_gfs.weekly.to_string(),
            backup_monthly: g.backup_gfs.monthly.to_string(),
            backup_yearly: g.backup_gfs.yearly.to_string(),
            onboot: g.onboot.then(|| "1".to_string()),
            restart: g.hw.restart.then(|| "1".to_string()),
            cpu_shares: g.hw.cpu_shares.to_string(),
            mem_reserve_mb: g.hw.mem_reserve_mb.to_string(),
            boot_order: g.hw.boot_order.to_string(),
            boot_delay: g.hw.boot_delay.to_string(),
            disk_mbps: g.hw.disk_mbps.to_string(),
            disk_iops: g.hw.disk_iops.to_string(),
            net_mbps: g.hw.net_mbps.to_string(),
            vlan: g.hw.vlan.to_string(),
            shares: g.hw.shares.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
            firewall: g.hw.firewall.then(|| "1".to_string()),
            nic_guard: g.hw.nic_guard.then(|| "1".to_string()),
            firmware: g.hw.firmware_choice().to_string(),
            tpm: g.hw.tpm.then(|| "1".to_string()),
            nics: g.hw.nics.join(", "),
            usb: g.hw.usb.join(", "),
            pci: g.hw.pci.join(", "),
            cpu_type: g.hw.cpu_type.clone(),
            balloon: g.hw.balloon.then(|| "1".to_string()),
            sound: g.hw.sound.then(|| "1".to_string()),
            max_cores: g.hw.max_cores.to_string(),
            max_memory_mb: g.hw.max_mem_mb.to_string(),
            nested: g.hw.nested.then(|| "1".to_string()),
            ci_user: g.hw.ci_user.clone().unwrap_or_default(),
            ci_ssh_key: g.hw.ci_ssh_key.clone().unwrap_or_default(),
            ci_ip: g.hw.ci_ip.clone().unwrap_or_default(),
            ci_gw: g.hw.ci_gw.clone().unwrap_or_default(),
            ci_dns: g.hw.ci_dns.join(", "),
            fw_rules: g.hw.fw_rules.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
            fw_out_restrict: g.hw.fw_out_restrict.then(|| "1".to_string()),
            fw_out: g.hw.fw_out.iter().map(redox_core::hw::FwRule::outbound).collect::<Vec<_>>().join("\n"),
            fw_groups: g.hw.fw_groups.join(", "),
            backup_pre: g.hw.backup_pre.clone(),
            test_port: g.hw.test_port.to_string(),
            test_cmd: g.hw.test_cmd.clone(),
            backup_post: g.hw.backup_post.clone(),
        }
    }
}

/// Donanım sekmesi. Alanlar `form="hw"` ile tek forma bağlanır; diskler kendi formlarıyla araya girebilir.
pub fn hardware_tab(p: &Page, g: &Guest, f: &EditForm, opts: &FormOptions) -> Markup {
    let isos = &opts.isos;
    let disk_ops = !(p.real && g.kind == GuestKind::Ct);
    let base = format!("/guests/{}", g.vmid);
    let stopped = g.status == GuestStatus::Stopped && g.lock.is_none();
    let off = !(p.admin && stopped);
    let vm = g.kind == GuestKind::Vm;
    let total = g.disk_gb + g.hw.disks.iter().map(|d| d.size_gb).sum::<u64>();
    // Çalışırken takma: disk ve ağ kartı eklemek, konuk çalışırken de açıktır (duraklatılmışken değil).
    let hot = p.admin && vm && p.hotplug && g.status == GuestStatus::Running && g.lock.is_none();
    let add_off = off && !hot;
    html! {
        @if p.admin {
            form id="hw" method="post" action={ (base) "/edit" } novalidate { input type="hidden" name="part" value="donanim"; }
        }
        @if !stopped {
            div class="notice" role="status" { (icon("alert")) span { (tx("Donanım ayarları yalnızca konuk kapalıyken değişir.")) @if hot { (tx(" Disk ve ağ kartı çalışırken de eklenebilir.")) } } }
        }
        div class="stack" {
            div class="card" {
                header { h2 { @if vm { (tx("İşlemci, bellek ve donanım yazılımı")) } @else { (tx("İşlemci ve bellek")) } } }
                div class="form" {
                    div class="field" { label for="cores" { (tx("Çekirdek")) } input id="cores" form="hw" name="cores" type="text" inputmode="numeric" value=(f.cores) disabled[off]; span class="hint" tabindex="0" role="note" { (tx("Konuğa verilen sanal işlemci çekirdeği sayısı.")) } }
                    div class="field" { label for="memory_mb" { (tx("Bellek (MB)")) } input id="memory_mb" form="hw" name="memory_mb" type="text" inputmode="numeric" value=(f.memory_mb) disabled[off]; span class="hint" tabindex="0" role="note" { (tx("Konuğa ayrılan bellek.")) } }
                    @if vm {
                        div class="field" {
                            label for="firmware" { (tx("Donanım yazılımı")) }
                            select id="firmware" form="hw" name="firmware" disabled[off] {
                                option value="bios" selected[f.firmware == "bios"] { "BIOS (SeaBIOS)" }
                                option value="uefi" selected[f.firmware == "uefi"] { "UEFI" }
                                option value="uefi-secure" selected[f.firmware == "uefi-secure"] { (tx("UEFI + güvenli önyükleme")) }
                            }
                            span class="hint" tabindex="0" role="note" { (tx("Kurulu bir işletim sistemi, donanım yazılımı değişince açılmayabilir.")) }
                        }
                        div class="field" {
                            label for="numa_node" { (tx("NUMA düğümü")) }
                            input id="numa_node" form="hw" name="numa_node" type="text" inputmode="numeric" value=(f.numa_node) disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Belleği ve (çekirdek listesi boşsa) sanal çekirdekleri sunucunun bu NUMA düğümüne bağlar. Boş: bağ yok.")) }
                        }
                        div class="field" {
                            span class="label" { (tx("Büyük sayfalar")) }
                            label class="check" { input type="checkbox" form="hw" name="hugepages" value="1" checked[f.hugepages.is_some()] disabled[off]; (tx("Belleği 2 MB'lık sayfalardan ayır")) }
                            span class="hint" tabindex="0" role="note" { (tx("Sunucuda önceden boş büyük sayfa ayrılmış olmalıdır (vm.nr_hugepages). Bellek balonu ve çalışırken bellek ekleme ile birlikte kullanılamaz.")) }
                        }
                        div class="field" {
                            span class="label" { (tx("Güvenlik yongası")) }
                            label class="check" { input type="checkbox" form="hw" name="tpm" value="1" checked[f.tpm.is_some()] disabled[off]; (tx("Sanal TPM 2.0")) }
                            span class="hint" tabindex="0" role="note" { (tx("Sanal TPM 2.0. Windows 11 ve disk şifreleme (BitLocker) için gerekir.")) }
                        }
                        div class="field" {
                            label for="cpu_type" { (tx("İşlemci türü")) }
                            select id="cpu_type" form="hw" name="cpu_type" disabled[off] {
                                option value="" selected[f.cpu_type.is_empty()] { (tx("Sunucunun işlemcisi (host)")) }
                                @for model in redox_core::hw::CPU_TYPES { option value=(model) selected[f.cpu_type == model] { (model) } }
                            }
                            span class="hint" tabindex="0" role="note" { (tx("En hızlısı sunucunun kendi işlemcisidir. Konuğu farklı işlemcili bir sunucuya taşıyacaksanız ikisinin de desteklediği ortak bir model seçin. Başka üreticinin modeli (Intel sunucuda EPYC gibi) kabul edilmez.")) }
                        }
                        div class="field" {
                            span class="label" { (tx("İç içe sanallaştırma")) }
                            label class="check" { input type="checkbox" form="hw" name="nested" value="1" checked[f.nested.is_some()] disabled[off]; (tx("Konuğa VT-x / AMD-V göster")) }
                            span class="hint" tabindex="0" role="note" { (tx("Konuk kendi sanal makinelerini çalıştırabilir (KVM, Hyper-V, WSL2, Docker Desktop). Sunucunun KVM modülünde iç içe sanallaştırma açık olmalıdır; qemu64 ve kvm64 modelleriyle kullanılamaz.")) }
                        }
                        div class="field" {
                            label for="max_cores" { (tx("En fazla çekirdek")) }
                            input id="max_cores" form="hw" name="max_cores" type="text" inputmode="numeric" value=(f.max_cores) disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Çalışırken çıkılabilecek çekirdek sayısı; 0 ise çalışan konuğa çekirdek eklenmez. Konuk bu kadar çekirdek yuvasıyla açılır, kullanılmayan yuvalar işlemci harcamaz.")) }
                        }
                        div class="field" {
                            label for="max_memory_mb" { (tx("En fazla bellek (MB)")) }
                            input id="max_memory_mb" form="hw" name="max_memory_mb" type="text" inputmode="numeric" value=(f.max_memory_mb) disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Çalışırken çıkılabilecek bellek; 0 ise çalışan konuğa bellek eklenmez. Konuk boş bellek yuvalarıyla açılır, eklenmemiş bellek sunucuda yer tutmaz.")) }
                        }
                        div class="field" {
                            span class="label" { (tx("Ses")) }
                            label class="check" { input type="checkbox" form="hw" name="sound" value="1" checked[f.sound.is_some()] disabled[off]; (tx("Ses kartı tak")) }
                            span class="hint" tabindex="0" role="note" { (tx("Konuğa Intel HD Audio kartı takılır. Ses canlı konsoldan “Sesi Aç” ile dinlenir; mikrofon taşınmaz.")) }
                        }
                        div class="field" {
                            span class="label" { (tx("Bellek balonu")) }
                            label class="check" { input type="checkbox" form="hw" name="balloon" value="1" checked[f.balloon.is_some()] disabled[off]; (tx("Balon aygıtını tak")) }
                            span class="hint" tabindex="0" role="note" { (tx("Konuğun belleği çalışırken küçültülüp büyütülebilir. Konukta virtio balon sürücüsü gerekir (Linux'ta hazır; Windows'ta virtio-win).")) }
                        }
                    }
                }
            }
            @if disk_ops {
                div class="card" {
                    header { h2 { (tx("Diskler")) } (tip("Konuğun asıl diski ve ek diskleri. Disk yalnızca büyütülür; ek disk çalışırken de eklenir.")) span class="note" { "toplam " (total) " GB · " (g.storage) } }
                    div class="tw" { table {
                        thead { tr { th { "Disk" } th class="num" { (tx("Boyut")) } th { span class="sr" { (tx("İşlem")) } } } }
                        tbody {
                            tr { td class="mono" { "disk 0 " span class="tag" { (tx("asıl")) } } td class="num mono" { (g.disk_gb) " GB" } td class="act" {} }
                            @for d in &g.hw.disks { tr {
                                td class="mono" { "disk " (d.id) span class="sub" { "vm-" (g.vmid) "-disk-" (d.id) } }
                                td class="num mono" { (d.size_gb) " GB" }
                                td class="act" { @if !off { a class="btn sm danger" href={ (base) "/confirm?op=deldisk&name=" (d.id) } { (icon("trash")) (tx("Sil")) } } }
                            } }
                        }
                    } }
                    @if p.admin {
                        form class="inline" method="post" action={ (base) "/resize" } {
                            label for="disk_gb" { (tx("Yeni boyut (GB), şimdi ")) (g.disk_gb) }
                            input id="disk_gb" name="disk_gb" type="text" inputmode="numeric" size="6" value=(g.disk_gb + 10) disabled[off];
                            button class="btn sm" type="submit" disabled[off] { (icon("disk")) (tx("Büyüt")) }
                            span class="hint" tabindex="0" role="note" { (tx("Disk yalnızca büyütülür. Konuğun içindeki dosya sistemini ayrıca genişletmeniz gerekir.")) }
                        }
                        @if vm {
                            form class="inline" method="post" action={ (base) "/disks" } {
                                label for="new_disk_gb" { (tx("Yeni disk (GB)")) }
                                input id="new_disk_gb" name="disk_gb" type="text" inputmode="numeric" size="6" value="10" disabled[add_off];
                                button class="btn sm" type="submit" disabled[add_off] { (icon("plus")) (tx("Disk Ekle")) }
                                span class="hint" tabindex="0" role="note" { (tx("Ek diskler konuğa boş takılır. Anlık görüntülere, klonlara ve yedeklere dahildir.")) }
                            }
                        }
                    }
                }
            }
            div class="card" {
                header { h2 { (tx("Ağ")) } (tip("Konuğun ağ kartları: NAT ya da köprü, VLAN etiketi ve ek kartlar.")) }
                div class="form" {
                    div class="field" {
                        label for="bridge" { (tx("Ağ")) }
                        @if p.real {
                            select id="bridge" form="hw" name="bridge" disabled[off] {
                                option value="nat" selected[f.bridge == "nat"] { "NAT" }
                                @for b in &opts.bridges { option value=(b) selected[*b == f.bridge] { (tx("Köprü: ")) (b) } }
                            }
                        } @else {
                            input id="bridge" form="hw" name="bridge" type="text" value=(f.bridge) disabled[off];
                        }
                        span class="hint" tabindex="0" role="note" { (tx("Köprüde konuk sunucunun ağından adres alır; port yönlendirme gerekmez.")) }
                    }
                    @if vm {
                        div class="field" {
                            label for="vlan" { (tx("VLAN etiketi")) }
                            input id="vlan" form="hw" name="vlan" type="text" inputmode="numeric" value=(f.vlan) disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Birinci kartın etiketi (1-")) (redox_core::hw::MAX_VLAN) (tx("); 0 etiketsiz. Yalnızca köprüde. Köprüde VLAN süzme açık, hizmette CAP_NET_ADMIN yetkisi olmalı; kurulamazsa konuk başlatılmaz.")) }
                        }
                    }
                    div class="field" {
                        label for="nics" { (tx("Ek ağ kartları")) }
                        input id="nics" form="hw" name="nics" type="text" value=(f.nics) placeholder="vmbr1, nat" autocomplete="off" disabled[off];
                        span class="hint" tabindex="0" role="note" { (tx("Virgülle ayırın: köprü adı ya da nat. En fazla ")) (redox_core::hw::MAX_EXTRA_NICS) (tx(". VLAN etiketi için köprü:etiket yazın, örn. vmbr1:20.")) }
                    }
                    div class="field" {
                        label for="forwards" { (tx("Port yönlendirme")) }
                        input id="forwards" form="hw" name="forwards" type="text" value=(f.forwards) placeholder="8080:80, 2222:22" autocomplete="off" disabled[off];
                        span class="hint" tabindex="0" role="note" { (tx("sunucu-portu:konuk-portu (TCP). Sunucu portu 1024 ve üzeri olmalı; güvenlik duvarında ayrıca açın.")) }
                    }
                }
                @if p.admin && vm {
                    @let full = g.hw.nics.len() >= redox_core::hw::MAX_EXTRA_NICS;
                    form class="inline" method="post" action={ (base) "/nics" } {
                        label for="new_nic" { (tx("Ağ kartı ekle")) }
                        @if p.real {
                            select id="new_nic" name="network" disabled[add_off || full] {
                                option value="nat" { "NAT" }
                                @for b in &opts.bridges { option value=(b) { (tx("Köprü: ")) (b) } }
                            }
                        } @else {
                            input id="new_nic" name="network" type="text" value="vmbr1" autocomplete="off" disabled[add_off || full];
                        }
                        input name="vlan" type="text" inputmode="numeric" size="5" placeholder="VLAN" aria-label=(tx("VLAN etiketi (isteğe bağlı)")) disabled[add_off || full];
                        button class="btn sm" type="submit" disabled[add_off || full] { (icon("plus")) (tx("Kart Ekle")) }
                        span class="hint" tabindex="0" role="note" { @if p.hotplug { (tx("Kart, konuk çalışırken de takılır; çıkarmak için konuğu kapatıp üstteki listeden silin.")) } @else { (tx("Kart, konuk kapalıyken eklenir.")) } }
                    }
                }
            }
            @if vm {
                div class="card" {
                    header { h2 { (tx("Hız sınırları")) } (tip("Konuğun disk ve ağ kullanımına üst sınır koyar; gürültülü bir konuk öbürlerini yavaşlatmasın.")) span class="note" { (tx("0 sınırsız")) } }
                    div class="form" {
                        div class="field" {
                            label for="disk_mbps" { (tx("Disk (MB/sn)")) }
                            input id="disk_mbps" form="hw" name="disk_mbps" type="text" inputmode="numeric" value=(f.disk_mbps) disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Okuma ve yazmanın toplamı; her diske ayrı uygulanır. En çok ")) (redox_core::hw::MAX_DISK_MBPS) "." }
                        }
                        div class="field" {
                            label for="disk_iops" { (tx("Disk (işlem/sn)")) }
                            input id="disk_iops" form="hw" name="disk_iops" type="text" inputmode="numeric" value=(f.disk_iops) disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Saniyedeki okuma ve yazma işlemi; her diske ayrı. En çok ")) (redox_core::hw::MAX_DISK_IOPS) "." }
                        }
                        div class="field" {
                            label for="net_mbps" { (tx("Ağ (Mbit/sn)")) }
                            input id="net_mbps" form="hw" name="net_mbps" type="text" inputmode="numeric" value=(f.net_mbps) disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Her yönde, köprüye bağlı her karta ayrı. NAT kartına uygulanamaz. Sunucuda iproute2 ve hizmette CAP_NET_ADMIN yetkisi gerekir; uygulanamazsa konuk başlatılmaz.")) }
                        }
                    }
                }
                @if vm && g.hw.max_cores > 0 {
                    div class="card" {
                        header { h2 { (tx("Çalışırken çekirdek")) } (tip("Çalışan konuğa, kapatmadan işlemci çekirdeği ekler.")) span class="note" { (g.cores) " / " (g.hw.max_cores) } }
                        @if p.admin {
                            form class="inline" method="post" action={ (base) "/cores" } {
                                label for="hot_cores" { (tx("Çekirdek")) }
                                input id="hot_cores" name="cores" type="text" inputmode="numeric" value=((g.cores + 1).min(g.hw.max_cores)) required disabled[!hot];
                                button class="btn sm" type="submit" disabled[!hot] { (icon("check")) (tx("Uygula")) }
                            }
                        }
                        footer {
                            (tx("Yalnızca konuk çalışırken. Daha küçük bir sayı yazarsanız çekirdek çıkarılır; konuk işletim sistemi çekirdeği bırakmalıdır. "))
                            (tx("Konukta ajan (qemu-guest-agent) çalışıyorsa yeni çekirdekler kendiliğinden etkinleşir; yoksa konuğun içinde etkinleştirmeniz gerekebilir."))
                        }
                    }
                }
                @if vm && g.hw.max_mem_mb > 0 {
                    @let step = redox_core::hw::MEM_STEP_MB;
                    @let room = g.hw.max_mem_mb.saturating_sub(g.mem_total_mb) / step * step;
                    @let full = room == 0 || g.hw.dimms.len() >= redox_core::hw::MEM_SLOTS;
                    div class="card" {
                        header { h2 { (tx("Çalışırken bellek")) } (tip("Çalışan konuğa, kapatmadan bellek ekler.")) span class="note" { (g.mem_total_mb) " / " (g.hw.max_mem_mb) " MB" } }
                        @if p.admin {
                            form class="inline" method="post" action={ (base) "/memory" } {
                                label for="hot_memory" { (tx("Bellek (MB)")) }
                                input id="hot_memory" name="memory_mb" type="text" inputmode="numeric" value=(g.mem_total_mb + room.min(1024)) required disabled[!hot || (full && g.hw.dimms.is_empty())];
                                button class="btn sm" type="submit" disabled[!hot || (full && g.hw.dimms.is_empty())] { (icon("check")) (tx("Uygula")) }
                            }
                        }
                        footer {
                            (tx("Yalnızca konuk çalışırken ve 128 MB'lık adımlarla. Yalnızca sonradan eklenen parçalar, sondan başlayarak çıkarılabilir; Linux konukta güvenilir çıkarma için yeni bellek “taşınabilir” açılmalıdır (çekirdek seçeneği memhp_default_state=online_movable). "))
                            (tx("Linux konuklar yeni belleği çoğunlukla kendiliğinden kullanır; kullanmıyorsa konuk ajanı (qemu-guest-agent) etkinleştirir."))
                        }
                    }
                }
                @if p.admin && g.status == GuestStatus::Running && (g.iso.is_some() || g.hw.iso2.is_some() || g.hw.media_changed) {
                    (media_card(p, g, isos))
                }
                @if g.hw.balloon {
                    div class="card" {
                        header { h2 { (tx("Bellek hedefi")) } (tip("Balon aygıtıyla konuğun kullandığı belleği çalışırken küçültür ya da büyütür.")) span class="note" { @if g.hw.balloon_mb == 0 { (tx("tamamı (")) (g.mem_total_mb) " MB)" } @else { (g.hw.balloon_mb) " / " (g.mem_total_mb) " MB" } } }
                        @if p.admin {
                            form class="inline" method="post" action={ (base) "/balloon" } {
                                label for="balloon_mb" { (tx("Hedef (MB)")) }
                                input id="balloon_mb" name="mb" type="text" inputmode="numeric" value=(g.hw.balloon_mb) required disabled[g.lock.is_some() || g.hw.mem_min_mb > 0];
                                button class="btn sm" type="submit" disabled[g.lock.is_some() || g.hw.mem_min_mb > 0] { (icon("check")) (tx("Uygula")) }
                            }
                            form class="inline" method="post" action={ (base) "/balloon" } {
                                label for="mem_min" { (tx("Dinamik bellek: en az (MB)")) }
                                input id="mem_min" name="min" type="text" inputmode="numeric" value=(g.hw.mem_min_mb) required disabled[g.lock.is_some()];
                                button class="btn sm" type="submit" disabled[g.lock.is_some()] { (icon("check")) (tx("Uygula")) }
                            }
                        }
                        footer {
                            (tx("Konuk çalışırken de değişir. ")) (redox_core::hw::MIN_BALLOON_MB) (tx(" MB ile konuğun belleği arasında bir değer yazın; 0 balonu boşaltır. "))
                            (tx("Konuk belleği hemen bırakmayabilir: balon sürücüsü kullanılmayan belleği verir, gerekirse konuk takas alanına başvurur. "))
                            (tx("Dinamik bellekte hedefi RedoX belirler: konuğun kullandığı bellek + %")) (redox_core::hw::DYNAMIC_BUFFER_PCT) (tx(" pay, alt sınır ile belleğin tamamı arasında. 0 kapatır."))
                        }
                    }
                }
                div class="card" {
                    header { h2 { "cloud-init" } span class="note" { @if g.hw.cloud_init() { (tx("açık · sürüm ")) (g.hw.ci_gen) } @else { (tx("kapalı")) } } }
                    div class="form" {
                        div class="field" {
                            label for="ci_user" { (tx("Kullanıcı")) }
                            input id="ci_user" form="hw" name="ci_user" type="text" value=(f.ci_user) placeholder=(redox_core::hw::DEFAULT_CI_USER) autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Parolasız sudo yetkisiyle açılır; parolayla giriş kapalıdır.")) }
                        }
                        div class="field full" {
                            label for="ci_ssh_key" { (tx("SSH açık anahtarı")) }
                            input id="ci_ssh_key" form="hw" name="ci_ssh_key" type="text" value=(f.ci_ssh_key) placeholder=(tx("ssh-ed25519 AAAA… ad@makine")) autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Doluysa cloud-init açıktır; boşaltırsanız kapanır ve aşağıdaki ayarlar da silinir.")) }
                        }
                        div class="field" {
                            label for="ci_ip" { (tx("Sabit adres")) }
                            input id="ci_ip" form="hw" name="ci_ip" type="text" value=(f.ci_ip) placeholder="10.0.0.5/24" autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Birinci kart için IPv4 adresi ve önek. Boşsa konuk DHCP kullanır.")) }
                        }
                        div class="field" {
                            label for="ci_gw" { (tx("Ağ geçidi")) }
                            input id="ci_gw" form="hw" name="ci_gw" type="text" value=(f.ci_gw) placeholder="10.0.0.1" autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Konuğun varsayılan ağ geçidi; yalnızca sabit adresle.")) }
                        }
                        div class="field" {
                            label for="ci_dns" { (tx("Ad sunucuları")) }
                            input id="ci_dns" form="hw" name="ci_dns" type="text" value=(f.ci_dns) placeholder="1.1.1.1, 9.9.9.9" autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" tabindex="0" role="note" { (tx("Virgülle ayırın; en fazla ")) (redox_core::hw::MAX_CI_DNS) (tx(". Yalnızca sabit adresle.")) }
                        }
                    }
                    footer {
                        (tx("Konuğun işletim sisteminde cloud-init kurulu olmalıdır (bulut imajlarında hazır gelir). Ayarlar değişince konuk bir sonraki açılışta "))
                        (tx("bunları yeniden uygular: yeni kullanıcı ve anahtar eklenir, makinenin SSH sunucu anahtarları yenilenebilir; eski kullanıcı silinmez."))
                    }
                }
                div class="card" {
                    header { h2 { (tx("Paylaşılan klasörler")) } (tip("Sunucudaki bir klasörü konuğa bağlar; birden çok konuk aynı klasörü görebilir.")) span class="note" { (g.hw.shares.len()) " / " (redox_core::hw::MAX_SHARES) } }
                    div class="form" {
                        div class="field full" {
                            label for="shares" { (tx("Paylaşımlar")) }
                            input id="shares" form="hw" name="shares" type="text" value=(f.shares) placeholder=(tx("ortak, veri:ro")) autocomplete="off" spellcheck="false" disabled[off];
                            span class="hint" tabindex="0" role="note" {
                                (tx("Virgülle ayırın; salt okunur için ad:ro. Her ad, sunucuda veri dizinindeki ")) span class="mono" { "shares/<ad>" }
                                (tx(" klasörüdür; aynı adı kullanan konuklar aynı klasörü görür. Konukta bağlamak için: "))
                                span class="mono" { "mount -t 9p -o trans=virtio,version=9p2000.L ortak /mnt/ortak" }
                                (tx(". Paylaşım konukta bağlıyken konuk askıya alınamaz, çalışırken anlık görüntüsü alınamaz ve çalışırken taşınamaz."))
                            }
                        }
                    }
                }
                div class="card" {
                    header { h2 { (tx("Kurulum kalıbı ve aygıt aktarımı")) } (tip("Takılı ISO kalıbı ve sunucudaki USB / PCI aygıtlarını konuğa doğrudan verme.")) }
                    div class="form" {
                        div class="field" {
                            label for="iso" { (tx("Kurulum kalıbı (ISO)")) }
                            @if p.real {
                                select id="iso" form="hw" name="iso" disabled[off] {
                                    option value="" { "(yok)" }
                                    @for i in isos { option value=(i) selected[*i == f.iso] { (i) } }
                                }
                            } @else {
                                input id="iso" form="hw" name="iso" type="text" value=(f.iso) placeholder="debian-12.iso" disabled[off];
                            }
                            span class="hint" tabindex="0" role="note" { (tx("Takılıysa ilk açılış bu kalıptan yapılır. Kalıpları ")) a class="link" href="/storage" { (tx("Depolama")) } (tx(" sayfasından yüklersiniz.")) }
                        }
                        div class="field" {
                            label for="iso2" { (tx("İkinci kalıp (sürücüler)")) }
                            @if p.real {
                                select id="iso2" form="hw" name="iso2" disabled[off] {
                                    option value="" { "(yok)" }
                                    @for i in isos { option value=(i) selected[*i == f.iso2] { (i) } }
                                }
                            } @else {
                                input id="iso2" form="hw" name="iso2" type="text" value=(f.iso2) placeholder="virtio-win.iso" disabled[off];
                            }
                            span class="hint" tabindex="0" role="note" { (tx("İkinci CD sürücüsü. Windows kurarken virtio sürücü kalıbını buraya takın.")) }
                        }
                        (device_selects(Some("hw"), &f.disk_bus, &f.nic_model, off))
                        div class="field" {
                            label for="usb" { (tx("USB aygıtları")) }
                            input id="usb" form="hw" name="usb" type="text" value=(f.usb) placeholder="046d:c52b" autocomplete="off" spellcheck="false" disabled[off];
                            (device_tags(&f.usb))
                            span class="hint" tabindex="0" role="note" { (tx("Sunucuya takılı aygıtın lsusb çıktısındaki kimliği. Hizmet kullanıcısının aygıta erişim izni olmalıdır.")) }
                        }
                        div class="field" {
                            label for="pci" { (tx("PCI aygıtları ve ekran kartı dilimleri")) }
                            input id="pci" form="hw" name="pci" type="text" value=(f.pci) placeholder="0000:01:00.0" autocomplete="off" spellcheck="false" disabled[off];
                            (device_tags(&f.pci))
                            span class="hint" tabindex="0" role="note" { (tx("lspci -D çıktısındaki adres. Sunucuda IOMMU açık ve aygıt vfio-pci sürücüsüne bağlı olmalıdır. Paylaşılan ekran kartı (vGPU) için adres yerine aracılı aygıtın kimliğini (mdev UUID) yazın; aynı kart böylece birkaç konuğa bölünür.")) }
                        }
                    }
                }
            }
        }
        @if p.admin {
            div class="savebar" {
                span class="note" { @if stopped { (tx("Değişiklikler kaydedince uygulanır; konuk bir sonraki açılışta yeni donanımla başlar.")) } @else { (tx("Kaydetmek için önce konuğu kapatın.")) } }
                a class="btn" href={ (base) "?tab=donanim" } { (tx("Vazgeç")) }
                button class="btn primary" type="submit" form="hw" disabled[off] { (icon("check")) (tx("Donanımı Kaydet")) }
            }
        }
    }
}

/// Aktarılan aygıt alanının altında, girilen her değerin ne olduğunu söyleyen etiketler (USB, PCI, vGPU).
/// Tanınmayan değer etiketlenmez; kaydederken zaten reddedilir.
fn device_tags(value: &str) -> Markup {
    let known: Vec<(&str, bool, String)> = value
        .split(',')
        .map(str::trim)
        .filter_map(|item| {
            if redox_core::hw::valid_mdev_uuid(item) {
                Some(("vGPU", true, format!("{}…{}", &item[..8], &item[item.len() - 4..])))
            } else if redox_core::hw::valid_pci_addr(item) {
                Some(("PCI", false, item.to_string()))
            } else if redox_core::hw::valid_usb_id(item) {
                Some(("USB", false, item.to_string()))
            } else {
                None
            }
        })
        .collect();
    html! {
        @if !known.is_empty() {
            div class="devs" { @for (kind, shared, text) in &known { span class=(if *shared { "tag gold" } else { "tag" }) { b { (kind) } (text) } } }
        }
    }
}
