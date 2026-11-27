//! Yeni konuk formu.

use super::*;

/// Konuk oluşturma formunun (doğrulanmamış) alanları.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct NewGuestForm {
    pub kind: String,
    pub node: String,
    pub vmid: String,
    pub name: String,
    pub os: String,
    pub storage: String,
    pub disk_gb: String,
    pub cores: String,
    pub memory_mb: String,
    pub bridge: String,
    pub onboot: Option<String>,
    pub iso: String,
    pub iso2: String,
    pub disk_bus: String,
    pub nic_model: String,
    pub image: String,
    pub ci_user: String,
    pub ci_ssh_key: String,
    pub firmware: String,
    pub tpm: Option<String>,
}

impl NewGuestForm {
    pub fn defaults(kind: GuestKind, cluster: &Cluster) -> Self {
        let node = cluster.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        let storage = cluster
            .storages
            .iter()
            .find(|s| s.available_on(&node))
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let (disk, mem) = match kind {
            GuestKind::Vm => ("32", "2048"),
            GuestKind::Ct => ("8", "512"),
        };
        NewGuestForm {
            kind: kind.as_str().to_string(),
            node,
            vmid: String::new(),
            name: String::new(),
            os: "debian-12".to_string(),
            storage,
            disk_gb: disk.to_string(),
            cores: "2".to_string(),
            memory_mb: mem.to_string(),
            bridge: "vmbr0".to_string(),
            onboot: None,
            iso: String::new(),
            ..Default::default()
        }
    }

    pub fn kind(&self) -> GuestKind {
        self.kind.parse().unwrap_or(GuestKind::Vm)
    }
}

/// Formlardaki seçim kutularının sürücüden gelen seçenekleri.
#[derive(Debug, Clone, Default)]
pub struct FormOptions {
    pub isos: Vec<String>,
    pub bridges: Vec<String>,
    pub templates: Vec<String>,
    /// Hazır bulut imajları.
    pub images: Vec<String>,
    /// Görüntülenen konuğun canlı konsolu (VNC soketi) hazır mı?
    pub vnc: bool,
}

/// `(alan, ileti)`; alan boşsa hata forma geneldir.
pub type FieldErrors = Vec<(&'static str, String)>;

pub fn new_guest(p: &Page, f: &NewGuestForm, errors: &FieldErrors, opts: &FormOptions) -> String {
    let isos = &opts.isos;
    let c = p.cluster;
    let kind = f.kind();
    let title = match kind {
        GuestKind::Vm => (tx("Sanal Makine Oluştur")),
        GuestKind::Ct => (tx("Konteyner Oluştur")),
    };
    let cli = format!("{kind} create");
    let err = |name: &str| errors.iter().find(|(n, _)| *n == name).map(|(_, m)| m.as_str());
    let field = |name: &'static str, label: &str, required: bool, hint: &str, control: Markup| {
        let e = err(name);
        html! {
            div class={ "field" @if e.is_some() { " bad" } } {
                label for=(name) { (label) @if required { " " span class="req" aria-hidden="true" { "*" } } }
                (control)
                @if let Some(m) = e { span class="err" id={ (name) "-err" } { (m) } }
                @else if !hint.is_empty() { span class="hint" tabindex="0" role="note" { (hint) } }
            }
        }
    };
    let describe = |name: &str| err(name).map(|_| format!("{name}-err"));
    let mut storage_ids: Vec<&str> = c.storages.iter().map(|s| s.id.as_str()).collect();
    storage_ids.sort_unstable();
    storage_ids.dedup();

    let body = html! {
        (page_head(html! { (title) }, html! { span { (tx("Konuk durdurulmuş halde eklenir; sonra sayfasından başlatabilirsiniz.")) } }, html! {}, &cli, &[]))
        @if !errors.is_empty() {
            div class="errsum" role="alert" {
                h2 { (tx("Konuk oluşturulamadı")) }
                ul { @for (name, m) in errors {
                    li { @if name.is_empty() { (m) } @else { a href={ "#" (name) } { (m) } } }
                } }
            }
        }
        form method="post" action="/guests" novalidate {
            input type="hidden" name="kind" value=(kind.as_str());
            fieldset {
                legend { (tx("Genel")) }
                div class="form" {
                    (field("node", (tx("Düğüm")), true, "", html! {
                        select id="node" name="node" aria-describedby=[describe("node")] {
                            @for n in &c.nodes { option value=(n.id) selected[n.id == f.node] { (n.id) @if n.status == NodeStatus::Offline { (tx(" (çevrimdışı)")) } } }
                        }
                    }))
                    (field("vmid", (tx("Kimlik")), false, &tfmt(format!("Boş bırakırsanız {} atanır.", c.next_vmid())), html! {
                        input id="vmid" name="vmid" type="text" inputmode="numeric" value=(f.vmid) placeholder=(c.next_vmid()) aria-describedby=[describe("vmid")];
                    }))
                    (field("name", (tx("Ad")), true, (tx("Küçük harf, rakam ve tire; örn. web-prod-03.")), html! {
                        input id="name" name="name" type="text" value=(f.name) required autocomplete="off" aria-describedby=[describe("name")];
                    }))
                    @if kind == GuestKind::Ct && !opts.templates.is_empty() {
                        (field("os", (tx("Şablon")), true, (tx("Kök dosya sistemi bu dağıtımın hazır imajından indirilir.")), html! {
                            select id="os" name="os" { @for t in &opts.templates { option value=(t) selected[*t == f.os] { (t) } } }
                        }))
                    } @else {
                        (field("os", (tx("İşletim sistemi")), false, (tx("Bilgi amaçlı etiket.")), html! {
                            input id="os" name="os" type="text" value=(f.os);
                        }))
                    }
                    @if kind == GuestKind::Vm && p.real {
                        (field("iso", (tx("Kurulum kalıbı (ISO)")), false,
                            if isos.is_empty() { (tx("Kalıp yok. ISO dosyalarını Depolama sayfasından yükleyin ya da sunucuda veri dizininin iso/ klasörüne koyun.")) } else { (tx("İlk açılışta bu kalıptan başlar.")) },
                            html! {
                                select id="iso" name="iso" {
                                    option value="" { "(yok)" }
                                    @for i in isos { option value=(i) selected[*i == f.iso] { (i) } }
                                }
                            }))
                        (field("iso2", (tx("İkinci kalıp (sürücüler)")), false, (tx("İkinci CD sürücüsü. Windows kurarken virtio sürücü kalıbını buraya takın.")), html! {
                            select id="iso2" name="iso2" {
                                option value="" { "(yok)" }
                                @for i in isos { option value=(i) selected[*i == f.iso2] { (i) } }
                            }
                        }))
                    }
                }
            }
            @if kind == GuestKind::Vm && p.real {
                fieldset {
                    legend { (tx("Hazır imaj ve ilk kurulum (cloud-init)")) }
                    div class="form" {
                        (field("image", (tx("Disk imajı")), false,
                            if opts.images.is_empty() { (tx("İmaj yok. Bulut imajını (qcow2) ya da başka sanallaştırıcıdan aldığınız diski (vmdk, vdi, vhdx) Depolama sayfasından yükleyin ya da sunucuda veri dizininin cloud/ klasörüne koyun.")) } else { (tx("Disk bu imajdan kopyalanır (vmdk, vdi, vhdx qcow2'ye çevrilir) ve aşağıdaki boyuta büyütülür.")) },
                            html! {
                                select id="image" name="image" {
                                    option value="" { (tx("(boş disk)")) }
                                    @for i in &opts.images { option value=(i) selected[*i == f.image] { (i) } }
                                }
                            }))
                        (field("ci_user", (tx("Kullanıcı")), false, (tx("Boş bırakırsanız “redox”. Parolasız sudo yetkisiyle açılır.")), html! {
                            input id="ci_user" name="ci_user" type="text" value=(f.ci_user) autocomplete="off" spellcheck="false";
                        }))
                        div class="field full" {
                            label for="ci_ssh_key" { (tx("SSH açık anahtarı")) }
                            textarea id="ci_ssh_key" name="ci_ssh_key" rows="3" spellcheck="false" autocomplete="off" { (f.ci_ssh_key) }
                            span class="hint" tabindex="0" role="note" { (tx("Tek satır: ssh-ed25519 AAAA… Bulut imajlarında parola ile giriş kapalıdır; anahtar vermezseniz giriş yapamazsınız.")) }
                        }
                    }
                }
            }
            fieldset {
                legend { "Disk" }
                div class="form" {
                    (field("storage", (tx("Depolama")), true, (tx("Seçtiğiniz düğümde kullanılabilir olmalı.")), html! {
                        select id="storage" name="storage" aria-describedby=[describe("storage")] {
                            @for id in &storage_ids { option value=(id) selected[*id == f.storage] { (id) } }
                        }
                    }))
                    (field("disk_gb", (tx("Boyut (GB)")), true, "", html! {
                        input id="disk_gb" name="disk_gb" type="text" inputmode="numeric" value=(f.disk_gb) required aria-describedby=[describe("disk_gb")];
                    }))
                }
            }
            fieldset {
                legend { (tx("CPU ve Bellek")) }
                div class="form" {
                    (field("cores", (tx("Çekirdek")), true, "", html! {
                        input id="cores" name="cores" type="text" inputmode="numeric" value=(f.cores) required aria-describedby=[describe("cores")];
                    }))
                    (field("memory_mb", (tx("Bellek (MB)")), true, &tfmt(format!("En az {} MB.", kind.min_memory_mb())), html! {
                        input id="memory_mb" name="memory_mb" type="text" inputmode="numeric" value=(f.memory_mb) required aria-describedby=[describe("memory_mb")];
                    }))
                    @if kind == GuestKind::Vm {
                        (field("firmware", (tx("Donanım yazılımı")), false, (tx("Windows 11 için UEFI + güvenli önyükleme ve TPM gerekir.")), html! {
                            select id="firmware" name="firmware" {
                                option value="bios" selected[f.firmware == "bios"] { "BIOS (SeaBIOS)" }
                                option value="uefi" selected[f.firmware == "uefi"] { "UEFI" }
                                option value="uefi-secure" selected[f.firmware == "uefi-secure"] { (tx("UEFI + güvenli önyükleme")) }
                            }
                        }))
                        div class="field" {
                            span class="label" { (tx("Güvenlik yongası")) }
                            label class="check" { input type="checkbox" name="tpm" value="1" checked[f.tpm.is_some()]; (tx("Sanal TPM 2.0 ekle")) }
                            span class="hint" tabindex="0" role="note" { (tx("Sanal TPM 2.0. Windows 11 ve disk şifreleme (BitLocker) için gerekir.")) }
                        }
                        (crate::view_ops::device_selects(None, &f.disk_bus, &f.nic_model, false))
                    }
                }
            }
            fieldset {
                legend { (tx("Ağ ve Başlatma")) }
                div class="form" {
                    @if p.real {
                        (field("bridge", (tx("Ağ")), true, (tx("NAT: konuk internete çıkar, dışarıdan görünmez. Köprü: sunucunun ağına doğrudan bağlanır.")), html! {
                            select id="bridge" name="bridge" {
                                option value="nat" selected[f.bridge == "nat"] { "NAT" }
                                @for b in &opts.bridges { option value=(b) selected[*b == f.bridge] { (tx("Köprü: ")) (b) } }
                            }
                        }))
                    } @else {
                        (field("bridge", (tx("Köprü")), false, "", html! {
                            input id="bridge" name="bridge" type="text" value=(f.bridge);
                        }))
                    }
                    div class="field" {
                        span class="label" { (tx("Açılış")) }
                        label class="check" { input type="checkbox" name="onboot" value="1" checked[f.onboot.is_some()]; (tx("Düğüm açıldığında başlat")) }
                        span class="hint" tabindex="0" role="note" { (tx("Sunucu açıldığında konuk kendiliğinden başlatılır.")) }
                    }
                }
            }
            div class="savebar" {
                span class="note" { (tx("Konuk durdurulmuş halde eklenir.")) }
                a class="btn" href="/guests" { (tx("Vazgeç")) }
                button class="btn primary" type="submit" { (icon("plus")) (tx("Oluştur")) }
            }
        }
    };
    layout(p, title, &[((tx("Konuklar")), "/guests"), (title, "")], true, body)
}
