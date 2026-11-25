//! Seçenekler sekmesi, zamanlama ve depolama kartları.

use super::*;

/// Seçenekler sekmesi: ad, etiketler, zamanlanmış yedek ve güvenlik duvarı. Konuk çalışırken de değişir.
pub fn options_tab(p: &Page, g: &Guest, f: &EditForm) -> Markup {
    let base = format!("/guests/{}", g.vmid);
    let off = !p.admin;
    let fields = html! {
            fieldset disabled[off] {
                legend { (tx("Genel")) }
                div class="form" {
                    div class="field" { label for="name" { (tx("Ad")) } input id="name" name="name" type="text" value=(f.name) required autocomplete="off"; }
                    div class="field" { label for="os" { (tx("İşletim sistemi")) } input id="os" name="os" type="text" value=(f.os); }
                    div class="field" {
                        label for="tags" { (tx("Etiketler")) }
                        input id="tags" name="tags" type="text" value=(f.tags) autocomplete="off";
                        span class="hint" { (tx("Virgülle ayırın; küçük harf, rakam ve tire. En fazla ")) (redox_core::MAX_TAGS) "." }
                    }
                    div class="field" {
                        label for="pool" { (tx("Kaynak havuzu")) }
                        select id="pool" name="pool" {
                            option value="" selected[f.pool.is_empty()] { (tx("Havuz yok")) }
                            @for pool in &p.cluster.pools { option value=(pool.name) selected[f.pool == pool.name] { (pool.name) } }
                        }
                        span class="hint" { (tx("Havuzun kotası bu konuğun çekirdek, bellek ve disklerini de sayar. Havuzlar ")) a class="link" href="/pools" { (tx("Havuzlar")) } (tx(" sayfasından tanımlanır.")) }
                    }
                }
            }
            fieldset disabled[off] {
                legend { (tx("Açılış ve yeniden başlatma")) }
                div class="form" {
                    div class="field" {
                        span class="label" { (tx("Açılış")) }
                        label class="check" { input type="checkbox" name="onboot" value="1" checked[f.onboot.is_some()]; (tx("Düğüm açıldığında başlat")) }
                        span class="hint" { (tx("Sunucu yeniden açıldığında, kapalıysa başlatılır. Hizmetin yeniden başlaması konuklara dokunmaz.")) }
                    }
                    div class="field" {
                        span class="label" { (tx("Beklenmedik kapanma")) }
                        label class="check" { input type="checkbox" name="restart" value="1" checked[f.restart.is_some()]; (tx("Kendiliğinden kapanırsa yeniden başlat")) }
                        span class="hint" { (tx("Konuk çökerse ya da içinden kapatılırsa yaklaşık 15 saniye içinde yeniden açılır (10 dakikada en çok ")) (crate::ha::MAX_RESTARTS) (tx(" kez). Kalıcı kapatmak için panelden kapatın.")) }
                    }
                    div class="field" {
                        label for="boot_order" { (tx("Başlatma sırası")) }
                        input id="boot_order" name="boot_order" type="text" inputmode="numeric" value=(f.boot_order);
                        span class="hint" { (tx("1 ilk başlar. 0 sırasızdır: sıralılardan sonra, kimlik sırasıyla başlar. En çok ")) (redox_core::hw::MAX_BOOT_ORDER) "." }
                    }
                    div class="field" {
                        label for="boot_delay" { (tx("Sonraki konuktan önce bekle (saniye)")) }
                        input id="boot_delay" name="boot_delay" type="text" inputmode="numeric" value=(f.boot_delay);
                        span class="hint" { (tx("Örneğin veritabanı açıldıktan 30 saniye sonra uygulama sunucusu başlasın. En çok ")) (redox_core::hw::MAX_BOOT_DELAY) "." }
                    }
                }
            }
            fieldset disabled[off] {
                legend { (tx("Zamanlanmış yedek")) }
                div class="form" {
                    div class="field" {
                        label for="backup_time" { (tx("Her gün saat")) }
                        input id="backup_time" name="backup_time" type="time" value=(f.backup_time);
                        span class="hint" { (tx("Sunucu saatiyle. Boş bırakırsanız otomatik yedek alınmaz.")) }
                    }
                    div class="field" {
                        label for="backup_keep" { (tx("Saklanacak otomatik yedek")) }
                        input id="backup_keep" name="backup_keep" type="text" inputmode="numeric" value=(f.backup_keep);
                    }
                    div class="field" {
                        label for="rpo_hours" { (tx("Yedek yaşı hedefi (RPO, saat)")) }
                        input id="rpo_hours" name="rpo_hours" type="text" inputmode="numeric" value=(f.rpo_hours);
                        span class="hint" { (tx("Son yedek bundan eskiyse konuk Yedekler sayfasında ve günlük raporda \"korumasız\" görünür. 0: sunucunun genel ayarı.")) }
                    }
                    div class="field" {
                        label for="backup_weekly" { (tx("Ek olarak sakla: haftalık / aylık / yıllık")) }
                        span class="inline" {
                            input id="backup_weekly" name="backup_weekly" type="text" inputmode="numeric" value=(f.backup_weekly) aria-label=(tx("Haftalık yedek sayısı")) style="max-width:5rem";
                            input name="backup_monthly" type="text" inputmode="numeric" value=(f.backup_monthly) aria-label=(tx("Aylık yedek sayısı")) style="max-width:5rem";
                            input name="backup_yearly" type="text" inputmode="numeric" value=(f.backup_yearly) aria-label=(tx("Yıllık yedek sayısı")) style="max-width:5rem";
                        }
                        span class="hint" { (tx("Son N yedeğe ek olarak, yedeği olan son N haftanın, ayın ve yılın en yeni otomatik yedeği saklanır. 0 kapalı.")) }
                    }
                    @if g.kind == GuestKind::Vm {
                        div class="field" {
                            label for="backup_pre" { (tx("Yedekten önce konukta çalıştır")) }
                            input id="backup_pre" name="backup_pre" type="text" value=(f.backup_pre) placeholder="/usr/local/bin/yedek-oncesi.sh" autocomplete="off" spellcheck="false";
                            span class="hint" { (tx("Konuk çalışırken alınan yedekten hemen önce, konuk ajanıyla çalışır (ör. veritabanını tutarlı duruma getirmek için). Başarısız olursa yedek alınmaz.")) }
                        }
                        div class="field" {
                            label for="backup_post" { (tx("Yedekten sonra konukta çalıştır")) }
                            input id="backup_post" name="backup_post" type="text" value=(f.backup_post) placeholder="/usr/local/bin/yedek-sonrasi.sh" autocomplete="off" spellcheck="false";
                            span class="hint" { (tx("Disk görüntüsü alınır alınmaz çalışır. Mutlak program yolu ve argümanları yazın; kabuk kullanılmaz. Konukta qemu-guest-agent gerekir.")) }
                        }
                        div class="field" {
                            label for="test_port" { (tx("Yedek sınamasında denetlenecek TCP kapısı")) }
                            input id="test_port" name="test_port" type="text" inputmode="numeric" value=(f.test_port);
                            span class="hint" { (tx("Yedek açılarak sınanırken bu kapıdaki hizmetten yanıt beklenir (ör. 22, 80, 3306). 0: denetlenmez. Veri göndermeyen hizmetler için aşağıdaki komutu kullanın.")) }
                        }
                        div class="field" {
                            label for="test_cmd" { (tx("Yedek sınamasında konukta çalıştır")) }
                            input id="test_cmd" name="test_cmd" type="text" value=(f.test_cmd) placeholder="/usr/local/bin/saglik.sh" autocomplete="off" spellcheck="false";
                            span class="hint" { (tx("Yedekten açılan geçici konukta konuk ajanıyla çalışır; 0 ile bitmezse sınama başarısız sayılır (ör. veritabanı tutarlılık denetimi).")) }
                        }
                        span class="hint" { (tx("Fazlası en eskiden başlayarak silinir. Elle aldıklarınıza dokunulmaz.")) }
                    }
                }
            }
            fieldset disabled[off] {
                legend { (tx("Güvenlik duvarı")) }
                div class="form" {
                    div class="field" {
                        span class="label" { (tx("Durum")) }
                        label class="check" { input type="checkbox" name="firewall" value="1" checked[f.firewall.is_some()]; (tx("Gelen trafiği kısıtla")) }
                        span class="hint" {
                            (tx("Açıkken yalnızca aşağıdaki kurallara uyan gelen bağlantılar geçer; konuğun başlattığı bağlantılar serbesttir. "))
                            @if g.kind == GuestKind::Vm { (tx("Yalnızca köprüye bağlı sanal makinede etkilidir; NAT'ta dışarıdan zaten yalnızca yönlendirilen kapılara erişilir.")) }
                        }
                    }
                    @if g.kind == GuestKind::Vm {
                        div class="field" {
                            span class="label" { (tx("Ağ kartı koruması")) }
                            label class="check" { input type="checkbox" name="nic_guard" value="1" checked[f.nic_guard.is_some()]; (tx("Sahte MAC, DHCP sunucusu ve yönlendirici duyurusunu engelle")) }
                            span class="hint" { (tx("Konuk yalnızca kendi MAC adresiyle konuşabilir; ağa DHCP yanıtı ya da IPv6 yönlendirici duyurusu (RA) gönderemez. Köprüye bağlı kartlarda etkilidir; çalışırken de açılıp kapanır (en geç 15 saniyede uygulanır).")) }
                        }
                    }
                    div class="field" {
                        label for="fw_rules" { (tx("İzin verilen gelen trafik")) }
                        textarea id="fw_rules" name="fw_rules" rows="4" spellcheck="false" autocomplete="off" placeholder="tcp/22 from 10.0.0.0/8" { (f.fw_rules) }
                        span class="hint" { (tx("Satır başına bir kural: tcp/22, tcp/80-443, udp/53 from 192.168.1.0/24, icmp. En fazla ")) (redox_core::hw::MAX_FW_RULES) "." }
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
                    span class="note" { (tx("Bu ayarlar konuk çalışırken de değişir.")) }
                    a class="btn" href={ (base) "?tab=secenekler" } { (tx("Vazgeç")) }
                    button class="btn primary" type="submit" { (icon("check")) (tx("Kaydet")) }
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
            header { h2 { (tx("Zamanlanmış güç işlemleri")) } span class="note" { "sunucu saatiyle · " (g.hw.schedules.len()) " / " (redox_core::hw::MAX_SCHEDULES) } }
            @if g.hw.schedules.is_empty() {
                p class="empty" { (tx("Zamanlanmış işlem yok. Konuğu belirli gün ve saatlerde başlatmak, kapatmak ya da yeniden başlatmak için ekleyin.")) }
            } @else {
                div class="tw" { table {
                    thead { tr { th { (tx("İşlem")) } th { (tx("Saat")) } th { (tx("Günler")) } th { (tx("Son çalışma")) } th { span class="sr" { (tx("Sil")) } } } }
                    tbody { @for (i, s) in g.hw.schedules.iter().enumerate() { tr {
                        td { (tx(s.action.label())) }
                        td class="mono" { (s.time) }
                        td { (tfmt(s.days_label())) }
                        td class="mono muted" { (s.last_run.as_deref().unwrap_or("-")) }
                        td class="act" {
                            @if p.admin { form method="post" action={ (base) "/schedules/" (i) "/delete" } { button class="btn sm danger" type="submit" aria-label={ (tx(s.action.label())) " " (s.time) (tx(" işlemini sil")) } { (icon("trash")) (tx("Sil")) } } }
                        }
                    } } }
                } }
            }
            @if p.admin {
                form class="inline" method="post" action={ (base) "/schedules" } {
                    label for="sch-action" { (tx("Yeni işlem")) }
                    select id="sch-action" name="action" disabled[full] {
                        @for a in [Action::Start, Action::Stop, Action::Reboot] { option value=(a.as_str()) { (tx(a.label())) } }
                    }
                    input name="time" type="time" required aria-label=(tx("Saat")) disabled[full];
                    @for (n, day) in [(tx("Pzt")), (tx("Sal")), (tx("Çar")), (tx("Per")), (tx("Cum")), (tx("Cmt")), (tx("Paz"))].iter().enumerate() {
                        label class="check" { input type="checkbox" name="day" value=(n + 1) checked disabled[full]; (day) }
                    }
                    button class="btn sm" type="submit" disabled[full] { (icon("plus")) (tx("Ekle")) }
                }
            }
            footer {
                (tx("Saati geldiğinde konuk zaten istenen durumdaysa işlem atlanır. Sunucu o saatte kapalıysa işlem, "))
                (redox_core::hw::SCHEDULE_GRACE_MIN) (tx(" dakika içinde açılırsa yine çalışır; sonrası o gün atlanır."))
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
            header { h2 { (tx("Depolama alanı")) } span class="note" { (tx("şimdi: ")) span class="mono" { (g.storage) } " · " (size) " GB" } }
            div class="op" {
                div {
                    h3 { (tx("Diskleri başka alana taşı")) }
                    p {
                        (tx("Konuğun bütün diskleri (ek diskler dahil) seçilen alana kopyalanır, kopya tamamlanınca eski yerlerinden silinir; anlık görüntüler disklerle birlikte gelir. "))
                        (tx("Süre disklerin dolu kısmıyla orantılıdır. Konuk kapalı olmalıdır; yarıda kalan taşıma konuğu eski alanında bırakır."))
                    }
                }
                @if others.is_empty() {
                    span class="muted" { (tx("Bu düğümde disk tutan başka alan yok.")) }
                } @else {
                    form method="post" action={ "/guests/" (g.vmid) "/move" } {
                        select name="storage" aria-label=(tx("Hedef depolama alanı")) disabled[!stopped] {
                            @for s in &others { option value=(s.id) disabled[s.free_gb() < size] { (s.id) " · " (s.free_gb()) (tx(" GB boş")) } }
                        }
                        button class="btn" type="submit" disabled[!stopped] { (icon("migrate")) (tx("Taşı")) }
                    }
                }
            }
        }
    }
}
