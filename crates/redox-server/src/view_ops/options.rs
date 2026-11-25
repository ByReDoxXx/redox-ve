//! Seçenekler sekmesi, zamanlama ve depolama kartları.

use super::*;

/// Seçenekler sekmesi: ad, etiketler, zamanlanmış yedek ve güvenlik duvarı. Konuk çalışırken de değişir.
pub fn options_tab(p: &Page, g: &Guest, f: &EditForm) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let off = !p.admin;
    let fields = html! {
            fieldset disabled[off] {
                legend { "Genel" }
                div class="form" {
                    div class="field" { label for="name" { "Ad" } input id="name" name="name" type="text" value=(f.name) required autocomplete="off"; }
                    div class="field" { label for="os" { "İşletim sistemi" } input id="os" name="os" type="text" value=(f.os); }
                    div class="field" {
                        label for="tags" { "Etiketler" }
                        input id="tags" name="tags" type="text" value=(f.tags) autocomplete="off";
                        span class="hint" { "Virgülle ayırın; küçük harf, rakam ve tire. En fazla " (redox_core::MAX_TAGS) "." }
                    }
                    div class="field" {
                        label for="pool" { "Kaynak havuzu" }
                        select id="pool" name="pool" {
                            option value="" selected[f.pool.is_empty()] { "Havuz yok" }
                            @for pool in &p.cluster.pools { option value=(pool.name) selected[f.pool == pool.name] { (pool.name) } }
                        }
                        span class="hint" { "Havuzun kotası bu konuğun çekirdek, bellek ve disklerini de sayar. Havuzlar " a class="link" href="/pools" { "Havuzlar" } " sayfasından tanımlanır." }
                    }
                }
            }
            fieldset disabled[off] {
                legend { "Açılış ve yeniden başlatma" }
                div class="form" {
                    div class="field" {
                        span class="label" { "Açılış" }
                        label class="check" { input type="checkbox" name="onboot" value="1" checked[f.onboot.is_some()]; "Düğüm açıldığında başlat" }
                        span class="hint" { "Sunucu yeniden açıldığında, kapalıysa başlatılır. Hizmetin yeniden başlaması konuklara dokunmaz." }
                    }
                    div class="field" {
                        span class="label" { "Beklenmedik kapanma" }
                        label class="check" { input type="checkbox" name="restart" value="1" checked[f.restart.is_some()]; "Kendiliğinden kapanırsa yeniden başlat" }
                        span class="hint" { "Konuk çökerse ya da içinden kapatılırsa yaklaşık 15 saniye içinde yeniden açılır (10 dakikada en çok " (crate::ha::MAX_RESTARTS) " kez). Kalıcı kapatmak için panelden kapatın." }
                    }
                    div class="field" {
                        label for="boot_order" { "Başlatma sırası" }
                        input id="boot_order" name="boot_order" type="text" inputmode="numeric" value=(f.boot_order);
                        span class="hint" { "1 ilk başlar. 0 sırasızdır: sıralılardan sonra, kimlik sırasıyla başlar. En çok " (redox_core::hw::MAX_BOOT_ORDER) "." }
                    }
                    div class="field" {
                        label for="boot_delay" { "Sonraki konuktan önce bekle (saniye)" }
                        input id="boot_delay" name="boot_delay" type="text" inputmode="numeric" value=(f.boot_delay);
                        span class="hint" { "Örneğin veritabanı açıldıktan 30 saniye sonra uygulama sunucusu başlasın. En çok " (redox_core::hw::MAX_BOOT_DELAY) "." }
                    }
                }
            }
            fieldset disabled[off] {
                legend { "Zamanlanmış yedek" }
                div class="form" {
                    div class="field" {
                        label for="backup_time" { "Her gün saat" }
                        input id="backup_time" name="backup_time" type="time" value=(f.backup_time);
                        span class="hint" { "Sunucu saatiyle. Boş bırakırsanız otomatik yedek alınmaz." }
                    }
                    div class="field" {
                        label for="backup_keep" { "Saklanacak otomatik yedek" }
                        input id="backup_keep" name="backup_keep" type="text" inputmode="numeric" value=(f.backup_keep);
                    }
                    div class="field" {
                        label for="rpo_hours" { "Yedek yaşı hedefi (RPO, saat)" }
                        input id="rpo_hours" name="rpo_hours" type="text" inputmode="numeric" value=(f.rpo_hours);
                        span class="hint" { "Son yedek bundan eskiyse konuk Yedekler sayfasında ve günlük raporda \"korumasız\" görünür. 0: sunucunun genel ayarı." }
                    }
                    div class="field" {
                        label for="backup_weekly" { "Ek olarak sakla: haftalık / aylık / yıllık" }
                        span class="inline" {
                            input id="backup_weekly" name="backup_weekly" type="text" inputmode="numeric" value=(f.backup_weekly) aria-label="Haftalık yedek sayısı" style="max-width:5rem";
                            input name="backup_monthly" type="text" inputmode="numeric" value=(f.backup_monthly) aria-label="Aylık yedek sayısı" style="max-width:5rem";
                            input name="backup_yearly" type="text" inputmode="numeric" value=(f.backup_yearly) aria-label="Yıllık yedek sayısı" style="max-width:5rem";
                        }
                        span class="hint" { "Son N yedeğe ek olarak, yedeği olan son N haftanın, ayın ve yılın en yeni otomatik yedeği saklanır. 0 kapalı." }
                    }
                    @if g.kind == GuestKind::Vm {
                        div class="field" {
                            label for="backup_pre" { "Yedekten önce konukta çalıştır" }
                            input id="backup_pre" name="backup_pre" type="text" value=(f.backup_pre) placeholder="/usr/local/bin/yedek-oncesi.sh" autocomplete="off" spellcheck="false";
                            span class="hint" { "Konuk çalışırken alınan yedekten hemen önce, konuk ajanıyla çalışır (ör. veritabanını tutarlı duruma getirmek için). Başarısız olursa yedek alınmaz." }
                        }
                        div class="field" {
                            label for="backup_post" { "Yedekten sonra konukta çalıştır" }
                            input id="backup_post" name="backup_post" type="text" value=(f.backup_post) placeholder="/usr/local/bin/yedek-sonrasi.sh" autocomplete="off" spellcheck="false";
                            span class="hint" { "Disk görüntüsü alınır alınmaz çalışır. Mutlak program yolu ve argümanları yazın; kabuk kullanılmaz. Konukta qemu-guest-agent gerekir." }
                        }
                        div class="field" {
                            label for="test_port" { "Yedek sınamasında denetlenecek TCP kapısı" }
                            input id="test_port" name="test_port" type="text" inputmode="numeric" value=(f.test_port);
                            span class="hint" { "Yedek açılarak sınanırken bu kapıdaki hizmetten yanıt beklenir (ör. 22, 80, 3306). 0: denetlenmez. Veri göndermeyen hizmetler için aşağıdaki komutu kullanın." }
                        }
                        div class="field" {
                            label for="test_cmd" { "Yedek sınamasında konukta çalıştır" }
                            input id="test_cmd" name="test_cmd" type="text" value=(f.test_cmd) placeholder="/usr/local/bin/saglik.sh" autocomplete="off" spellcheck="false";
                            span class="hint" { "Yedekten açılan geçici konukta konuk ajanıyla çalışır; 0 ile bitmezse sınama başarısız sayılır (ör. veritabanı tutarlılık denetimi)." }
                        }
                        span class="hint" { "Fazlası en eskiden başlayarak silinir. Elle aldıklarınıza dokunulmaz." }
                    }
                }
            }
            fieldset disabled[off] {
                legend { "Güvenlik duvarı" }
                div class="form" {
                    div class="field" {
                        span class="label" { "Durum" }
                        label class="check" { input type="checkbox" name="firewall" value="1" checked[f.firewall.is_some()]; "Gelen trafiği kısıtla" }
                        span class="hint" {
                            "Açıkken yalnızca aşağıdaki kurallara uyan gelen bağlantılar geçer; konuğun başlattığı bağlantılar serbesttir. "
                            @if g.kind == GuestKind::Vm { "Yalnızca köprüye bağlı sanal makinede etkilidir; NAT'ta dışarıdan zaten yalnızca yönlendirilen kapılara erişilir." }
                        }
                    }
                    @if g.kind == GuestKind::Vm {
                        div class="field" {
                            span class="label" { "Ağ kartı koruması" }
                            label class="check" { input type="checkbox" name="nic_guard" value="1" checked[f.nic_guard.is_some()]; "Sahte MAC, DHCP sunucusu ve yönlendirici duyurusunu engelle" }
                            span class="hint" { "Konuk yalnızca kendi MAC adresiyle konuşabilir; ağa DHCP yanıtı ya da IPv6 yönlendirici duyurusu (RA) gönderemez. Köprüye bağlı kartlarda etkilidir; çalışırken de açılıp kapanır (en geç 15 saniyede uygulanır)." }
                        }
                    }
                    div class="field" {
                        label for="fw_rules" { "İzin verilen gelen trafik" }
                        textarea id="fw_rules" name="fw_rules" rows="4" spellcheck="false" autocomplete="off" placeholder="tcp/22 from 10.0.0.0/8" { (f.fw_rules) }
                        span class="hint" { "Satır başına bir kural: tcp/22, tcp/80-443, udp/53 from 192.168.1.0/24, icmp. En fazla " (redox_core::hw::MAX_FW_RULES) "." }
                    }
                }
            }
    };
    html! {
        @if p.admin {
            form method="post" action={ (base) "/edit" } novalidate {
                input type="hidden" name="part" value="secenekler";
                (fields)
                div class="savebar" {
                    span class="note" { "Bu ayarlar konuk çalışırken de değişir." }
                    a class="btn" href={ (base) "?tab=secenekler" } { "Vazgeç" }
                    button class="btn primary" type="submit" { (icon("check")) "Kaydet" }
                }
            }
        } @else {
            (fields)
        }
        (schedules_card(p, g))
    }
}

/// Zamanlanmış güç işlemleri: liste, ekleme formu ve silme düğmeleri.
pub(super) fn schedules_card(p: &Page, g: &Guest) -> Markup {
    use redox_core::Action;
    let base = format!("/guests/{}", g.vmid);
    let full = g.hw.schedules.len() >= redox_core::hw::MAX_SCHEDULES;
    html! {
        div class="card" style="margin-top:var(--s4)" {
            header { h2 { "Zamanlanmış güç işlemleri" } span class="note" { "sunucu saatiyle · " (g.hw.schedules.len()) " / " (redox_core::hw::MAX_SCHEDULES) } }
            @if g.hw.schedules.is_empty() {
                p class="empty" { "Zamanlanmış işlem yok. Konuğu belirli gün ve saatlerde başlatmak, kapatmak ya da yeniden başlatmak için ekleyin." }
            } @else {
                div class="tw" { table {
                    thead { tr { th { "İşlem" } th { "Saat" } th { "Günler" } th { "Son çalışma" } th { span class="sr" { "Sil" } } } }
                    tbody { @for (i, s) in g.hw.schedules.iter().enumerate() { tr {
                        td { (s.action.label()) }
                        td class="mono" { (s.time) }
                        td { (s.days_label()) }
                        td class="mono muted" { (s.last_run.as_deref().unwrap_or("-")) }
                        td class="act" {
                            @if p.admin { form method="post" action={ (base) "/schedules/" (i) "/delete" } { button class="btn sm danger" type="submit" aria-label={ (s.action.label()) " " (s.time) " işlemini sil" } { (icon("trash")) "Sil" } } }
                        }
                    } } }
                } }
            }
            @if p.admin {
                form class="inline" method="post" action={ (base) "/schedules" } {
                    label for="sch-action" { "Yeni işlem" }
                    select id="sch-action" name="action" disabled[full] {
                        @for a in [Action::Start, Action::Stop, Action::Reboot] { option value=(a.as_str()) { (a.label()) } }
                    }
                    input name="time" type="time" required aria-label="Saat" disabled[full];
                    @for (n, day) in ["Pzt", "Sal", "Çar", "Per", "Cum", "Cmt", "Paz"].iter().enumerate() {
                        label class="check" { input type="checkbox" name="day" value=(n + 1) checked disabled[full]; (day) }
                    }
                    button class="btn sm" type="submit" disabled[full] { (icon("plus")) "Ekle" }
                }
            }
            footer {
                "Saati geldiğinde konuk zaten istenen durumdaysa işlem atlanır. Sunucu o saatte kapalıysa işlem, "
                (redox_core::hw::SCHEDULE_GRACE_MIN) " dakika içinde açılırsa yine çalışır; sonrası o gün atlanır."
            }
        }
    }
}

/// Konuğun disklerini aynı düğümdeki başka bir depolama alanına taşıma formu.
pub(super) fn storage_card(p: &Page, g: &Guest, stopped: bool) -> Markup {
    let others: Vec<_> = p.cluster.storages.iter().filter(|s| s.available_on(&g.node) && s.id != g.storage && s.content.iter().any(|c| c == "disk")).collect();
    let size = g.total_disk_gb();
    html! {
        div class="card" {
            header { h2 { "Depolama alanı" } span class="note" { "şimdi: " span class="mono" { (g.storage) } " · " (size) " GB" } }
            div class="op" {
                div {
                    h3 { "Diskleri başka alana taşı" }
                    p {
                        "Konuğun bütün diskleri (ek diskler dahil) seçilen alana kopyalanır, kopya tamamlanınca eski yerlerinden silinir; anlık görüntüler disklerle birlikte gelir. "
                        "Süre disklerin dolu kısmıyla orantılıdır. Konuk kapalı olmalıdır; yarıda kalan taşıma konuğu eski alanında bırakır."
                    }
                }
                @if others.is_empty() {
                    span class="muted" { "Bu düğümde disk tutan başka alan yok." }
                } @else {
                    form method="post" action={ "/guests/" (g.vmid) "/move" } {
                        select name="storage" aria-label="Hedef depolama alanı" disabled[!stopped] {
                            @for s in &others { option value=(s.id) disabled[s.free_gb() < size] { (s.id) " · " (s.free_gb()) " GB boş" } }
                        }
                        button class="btn" type="submit" disabled[!stopped] { (icon("migrate")) "Taşı" }
                    }
                }
            }
        }
    }
}
