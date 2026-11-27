//! Çalışırken donanım: disk ve kart ekleme, balon ve dinamik bellek, depolama taşıma, kaynak ölçümü.

use super::*;

impl AppState {
    /// Kapalı sanal makineye boş bir veri diski ekler; diskin numarasını döndürür.
    pub fn disk_add(&self, vmid: VmId, size_gb: u64, user: &str) -> Result<u8, Error> {
        // Sürücü çalışırken takabiliyorsa çalışan sanal makine de kabul edilir.
        let hotplug = self.driver().hotplug(redox_core::GuestKind::Vm);
        let (id, guest) = self.mutate(|c| {
            let id = if hotplug { c.add_disk_hot(vmid, size_gb)? } else { c.add_disk(vmid, size_gb)? };
            Ok::<_, Error>((id, c.guest(vmid)?.clone()))
        })?;
        let hot = guest.status == redox_core::GuestStatus::Running;
        let driver = self.driver();
        let outcome = driver.add_disk(&guest, id, size_gb).and_then(|()| {
            if !hot {
                return Ok(());
            }
            // Takılamayan diskin dosyası bırakılmaz.
            driver.hot_add_disk(&guest, id).inspect_err(|_| {
                let _ = driver.remove_disk(&guest, id);
            })
        });
        self.mutate(|c| {
            if outcome.is_err() {
                // Dosya oluşmadıysa ya da disk takılamadıysa kayıt ve ayrılan yer geri alınır.
                let _ = c.forget_disk(vmid, id);
            }
            c.log(&guest.node, user, format!("vm disk add {vmid} {size_gb}G"), &outcome, now(), Some(vmid));
        });
        outcome.map(|()| id).map_err(Error::Driver)
    }

    /// Konuğa ek ağ kartı ekler; konuk çalışıyorsa (ve sürücü yapabiliyorsa) kart hemen takılır.
    /// Kartın sırasını döndürür (1 ilk ek kart).
    pub fn nic_add(&self, vmid: VmId, network: &str, user: &str) -> Result<u8, Error> {
        let driver = self.driver();
        if driver.is_real() {
            self.check_bridge(redox_core::hw::parse_nic(network).map(|(bridge, _)| bridge).unwrap_or(network))?;
        }
        let hotplug = driver.hotplug(redox_core::GuestKind::Vm);
        let (index, guest) = self.mutate(|c| {
            let index = c.add_nic(vmid, network, hotplug)?;
            Ok::<_, Error>((index, c.guest(vmid)?.clone()))
        })?;
        let outcome = if guest.status == redox_core::GuestStatus::Running { driver.hot_add_nic(&guest, index, network) } else { Ok(()) };
        self.mutate(|c| {
            if outcome.is_err() {
                c.forget_nic(vmid);
            }
            c.log(&guest.node, user, format!("vm nic add {vmid} {network}"), &outcome, now(), Some(vmid));
        });
        outcome.map(|()| index).map_err(Error::Driver)
    }

    /// Çalışan sanal makinenin çekirdek sayısını `target` değerine çıkarır. Çekirdekler tek tek eklenir ve her
    /// biri eklendikçe kayda işlenir: yarıda kalırsa kayıt konukta gerçekten olan sayıyı gösterir. Dönen değer,
    /// eklenen çekirdekleri konuk ajanının etkinleştirip etkinleştirmediğidir.
    pub fn cores_add(&self, vmid: VmId, target: u32, user: &str) -> Result<bool, Error> {
        let driver = self.driver();
        if !driver.hotplug(redox_core::GuestKind::Vm) {
            return Err(Error::InvalidSpec("bu sürücü çalışan konuğa çekirdek ekleyemez".to_string()));
        }
        let (from, mut guest) = self.mutate(|c| Ok::<_, Error>((c.check_core_add(vmid, target)?, c.guest(vmid)?.clone())))?;
        let mut outcome = Ok(());
        for _ in from..target {
            outcome = driver.hot_add_core(&guest);
            if outcome.is_err() {
                break;
            }
            guest.cores += 1;
            self.mutate(|c| c.note_core_added(vmid));
        }
        let online = guest.cores > from && driver.online_cores(&guest);
        self.mutate(|c| c.log(&guest.node, user, format!("vm cores {vmid} {target}"), &outcome, now(), Some(vmid)));
        outcome.map(|()| online).map_err(Error::Driver)
    }

    /// Çalışan sanal makinenin belleğini `target_mb` değerine çıkarır: aradaki fark tek bir parça olarak
    /// takılır. Dönen değer, konuğun eklenen belleği kullanmaya başladığının konuk ajanıyla doğrulanıp
    /// doğrulanmadığıdır.
    pub fn memory_add(&self, vmid: VmId, target_mb: u64, user: &str) -> Result<bool, Error> {
        let driver = self.driver();
        if !driver.hotplug(redox_core::GuestKind::Vm) {
            return Err(Error::InvalidSpec("bu sürücü çalışan konuğa bellek ekleyemez".to_string()));
        }
        let (add, guest) = self.mutate(|c| Ok::<_, Error>((c.check_memory_add(vmid, target_mb)?, c.guest(vmid)?.clone())))?;
        let outcome = driver.hot_add_memory(&guest, add);
        let mut online = false;
        if outcome.is_ok() {
            let after = self.mutate(|c| {
                c.note_memory_added(vmid, add);
                c.guest(vmid).ok().cloned()
            });
            online = after.is_some_and(|g| driver.online_memory(&g));
        }
        self.mutate(|c| c.log(&guest.node, user, format!("vm memory {vmid} {target_mb}"), &outcome, now(), Some(vmid)));
        outcome.map(|()| online).map_err(Error::Driver)
    }

    /// Bellek balonunun hedefini ayarlar (MB; 0: belleğin tamamı). Konuk çalışıyorsa hemen uygulanır,
    /// kapalıysa bir sonraki açılışta. Uygulanamazsa kayıt eski değerine döner.
    pub fn balloon_set(&self, vmid: VmId, target_mb: u64, user: &str) -> Result<(), Error> {
        let (before, guest) = self.mutate(|c| {
            let before = c.set_balloon_target(vmid, target_mb)?;
            Ok::<_, Error>((before, c.guest(vmid)?.clone()))
        })?;
        let live = matches!(guest.status, redox_core::GuestStatus::Running | redox_core::GuestStatus::Paused);
        let outcome = if live { self.driver().set_balloon(&guest, guest.hw.balloon_mb) } else { Ok(()) };
        self.mutate(|c| {
            if outcome.is_err() {
                c.restore_balloon_target(vmid, before);
            }
            c.log(&guest.node, user, format!("vm balloon {vmid} {target_mb}"), &outcome, now(), Some(vmid));
        });
        outcome.map_err(Error::Driver)
    }

    /// Konuğun kaynak ölçümü sayaçlarını sıfırlar (fatura dönemi kapanınca).
    pub fn usage_reset(&self, vmid: VmId, user: &str) -> Result<(), Error> {
        self.mutate(|c| {
            c.reset_usage(vmid, now())?;
            let (node, kind) = c.guest(vmid).map(|g| (g.node.clone(), g.kind))?;
            c.log(&node, user, format!("{kind} usage-reset {vmid}"), &Ok(()), now(), Some(vmid));
            Ok(())
        })
    }

    /// Dinamik belleği açar (alt sınır MB) ya da kapatır (0). Kapatılınca balon boşaltılır: konuk belleğinin
    /// tamamını geri alır.
    pub fn dynamic_memory_set(&self, vmid: VmId, min_mb: u64, user: &str) -> Result<(), Error> {
        let guest = self.mutate(|c| {
            c.set_dynamic_memory(vmid, min_mb)?;
            if min_mb == 0 {
                c.restore_balloon_target(vmid, 0);
            }
            Ok::<_, Error>(c.guest(vmid)?.clone())
        })?;
        let live = matches!(guest.status, redox_core::GuestStatus::Running | redox_core::GuestStatus::Paused);
        let outcome = if live && min_mb == 0 { self.driver().set_balloon(&guest, 0) } else { Ok(()) };
        self.mutate(|c| c.log(&guest.node, user, format!("vm balloon {vmid} auto {min_mb}"), &outcome, now(), Some(vmid)));
        outcome.map_err(Error::Driver)
    }

    /// Dinamik belleği açık olan çalışan konukların bellek hedefini kullanımlarına göre ayarlar; değişen
    /// konuk sayısını döndürür. Ölçüm vermeyen (balon sürücüsü olmayan) konuklara dokunulmaz.
    pub fn dynamic_memory_tick(&self) -> usize {
        let guests: Vec<_> = self
            .snapshot()
            .guests
            .iter()
            .filter(|g| g.kind == redox_core::GuestKind::Vm && g.status == redox_core::GuestStatus::Running && g.hw.balloon && g.hw.mem_min_mb > 0 && g.lock.is_none())
            .cloned()
            .collect();
        let mut changed = 0;
        for g in guests {
            let Some(stats) = self.driver().balloon_stats(&g) else { continue };
            let Some(target) = redox_core::hw::dynamic_memory_target(g.mem_total_mb, g.hw.mem_min_mb, g.hw.balloon_mb, stats) else { continue };
            match self.driver().set_balloon(&g, target) {
                Ok(()) => {
                    self.mutate(|c| c.set_dynamic_target(g.vmid, target));
                    changed += 1;
                }
                Err(e) => eprintln!("uyarı: konuk {} için dinamik bellek hedefi uygulanamadı: {e}", g.vmid),
            }
        }
        changed
    }

    /// Ek diski verileriyle birlikte siler.
    pub fn disk_remove(&self, vmid: VmId, id: u8, user: &str) -> Result<(), Error> {
        let guest = self.mutate(|c| {
            c.remove_disk(vmid, id)?;
            c.guest(vmid).cloned()
        })?;
        let outcome = self.driver().remove_disk(&guest, id);
        self.mutate(|c| c.log(&guest.node, user, format!("vm disk remove {vmid} {id}"), &outcome, now(), Some(vmid)));
        outcome.map_err(Error::Driver)
    }

    /// Konuğun disklerini başka bir depolama alanına taşır. Kapalı konukta dosyalar kopyalanır; çalışan sanal
    /// makinede (sürücü yapabiliyorsa) diskler konuk durdurulmadan aynalanıp yeni yerine geçirilir.
    pub fn storage_move(&self, vmid: VmId, target: &str, user: &str) -> Result<TaskId, Error> {
        let kind = self.kind_of(vmid)?;
        if !self.driver().storage_supports(target, kind) {
            return Err(Error::InvalidSpec("konteynerler ek depolama alanlarında tutulamaz; yalnızca sanal makine diskleri taşınır".to_string()));
        }
        let (to, to2, to3) = (target.to_string(), target.to_string(), target.to_string());
        let running = self.lock().guest(vmid)?.status != redox_core::GuestStatus::Stopped;
        if running && kind == redox_core::GuestKind::Vm && self.driver().hotplug(redox_core::GuestKind::Vm) {
            return self.run_op(
                vmid,
                user,
                format!("{kind} move {vmid} {target}"),
                None,
                move |c| c.check_storage_move_live(vmid, &to),
                move |d, g| d.move_storage_live(g, &to2),
                move |c, ()| c.apply_storage_move(vmid, &to3),
            );
        }
        self.run_op(
            vmid,
            user,
            format!("{kind} move {vmid} {target}"),
            Some("depolama alanını değiştirmek"),
            move |c| c.check_storage_move(vmid, &to),
            move |d, g| d.move_storage(g, &to2),
            move |c, ()| c.apply_storage_move(vmid, &to3),
        )
    }

    pub fn resize_disk(&self, vmid: VmId, new_gb: u64, user: &str) -> Result<TaskId, Error> {
        let kind = self.kind_of(vmid)?;
        self.run_op(
            vmid,
            user,
            format!("{kind} resize {vmid} {new_gb}G"),
            Some("diski büyütmek"),
            move |c| c.check_grow(vmid, new_gb),
            move |d, g| d.resize_disk(g, new_gb),
            move |c, ()| c.apply_grow(vmid, new_gb),
        )
    }
}
