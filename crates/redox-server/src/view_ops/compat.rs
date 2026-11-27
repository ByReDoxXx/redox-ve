//! Donanım sekmesinin uyumluluk alanları (disk denetleyicisi, ağ kartı modeli) ve çalışırken kalıp değiştirme.

use super::*;
use redox_core::hw::{DiskBus, NicModel};

pub(crate) fn disk_bus_label(bus: DiskBus) -> &'static str {
    match bus {
        DiskBus::Virtio => "virtio (en hızlı)",
        DiskBus::VirtioScsi => "virtio-scsi",
        DiskBus::Sata => "SATA (sürücü gerektirmez)",
    }
}

pub(crate) fn nic_model_label(model: NicModel) -> &'static str {
    match model {
        NicModel::Virtio => "virtio (en hızlı)",
        NicModel::E1000e => "Intel e1000e",
        NicModel::E1000 => "Intel e1000 (eski sistemler)",
    }
}

/// Disk denetleyicisi ve ağ kartı modeli seçimi. `form`: alanların bağlı olduğu formun kimliği (yeni konuk
/// formunda boş).
pub(crate) fn device_selects(form: Option<&str>, bus: &str, nic: &str, off: bool) -> Markup {
    let bus = DiskBus::parse(bus).unwrap_or_default();
    let nic = NicModel::parse(nic).unwrap_or_default();
    html! {
        div class="field" {
            label for="disk_bus" { (tx("Disk denetleyicisi")) }
            select id="disk_bus" name="disk_bus" form=[form] disabled[off] {
                @for b in DiskBus::ALL { option value=(b.as_str()) selected[b == bus] { (tx(disk_bus_label(b))) } }
            }
            span class="hint" tabindex="0" role="note" { (tx("Asıl diskin denetleyicisi; ek diskler virtio'dur. Sürücü yüklemeden Windows kurmak ya da başka sanallaştırıcıdan alınan diski açmak için SATA seçin. Kurulu sistem, denetleyici değişince açılmayabilir.")) }
        }
        div class="field" {
            label for="nic_model" { (tx("Ağ kartı modeli")) }
            select id="nic_model" name="nic_model" form=[form] disabled[off] {
                @for m in NicModel::ALL { option value=(m.as_str()) selected[m == nic] { (tx(nic_model_label(m))) } }
            }
            span class="hint" tabindex="0" role="note" { (tx("Bütün ağ kartlarına uygulanır. Konukta virtio sürücüsü yoksa Intel modelini seçin.")) }
        }
    }
}

/// Çalışan sanal makinede CD sürücülerindeki kalıbı değiştirme kartı.
pub(crate) fn media_card(p: &Page, g: &Guest, isos: &[String]) -> Markup {
    let drives = [(1u8, g.iso.as_deref()), (2u8, g.hw.iso2.as_deref())];
    html! {
        div class="card" {
            header { h2 { (tx("CD sürücüleri")) } (tip("Konuk çalışırken kalıbı değiştirir ya da çıkarır.")) }
            @for (drive, current) in drives {
                form class="inline" method="post" action={ "/guests/" (g.vmid) "/media" } {
                    input type="hidden" name="drive" value=(drive);
                    label for={ "media" (drive) } { (tx("Sürücü")) " " (drive) }
                    @if p.real {
                        select id={ "media" (drive) } name="iso" disabled[g.lock.is_some()] {
                            option value="" { (tx("(boş)")) }
                            @for i in isos { option value=(i) selected[Some(i.as_str()) == current] { (i) } }
                        }
                    } @else {
                        input id={ "media" (drive) } name="iso" type="text" value=(current.unwrap_or("")) placeholder="virtio-win.iso" disabled[g.lock.is_some()];
                    }
                    button class="btn sm" type="submit" disabled[g.lock.is_some()] { (icon("check")) (tx("Uygula")) }
                }
            }
            footer { (tx("Yalnızca konuk açılırken kalıp takılı olan sürücü değiştirilebilir. Kalıbı değişen konuk, bir kez kapatılıp açılana kadar canlı taşınmaz.")) }
        }
    }
}
