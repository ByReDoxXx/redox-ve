//! Konuk yaşam döngüsü: oluşturma, silme, güç işlemleri, askı, ayar değişikliği, klon ve konsol.

use super::*;

impl AppState {
    /// Konuğu doğrular, ekler ve sürücüye kaynaklarını (disk imajı) hazırlatır.
    /// Sürücü başarısız olursa ekleme geri alınır.
    pub fn create_guest(&self, spec: &GuestSpec, user: &str) -> Result<VmId, Error> {
        let driver = self.driver();
        if !driver.supports(spec.kind) {
            return Err(Error::InvalidSpec(format!(
                "{} sürücüsü {} türünü desteklemiyor",
                driver.name(),
                spec.kind.long_label().to_lowercase()
            )));
        }
        if !driver.storage_supports(&spec.storage, spec.kind) {
            return Err(Error::InvalidSpec(format!("{} depolama alanı yalnızca sanal makine diski tutar; konteyner için {} seçin", spec.storage, crate::host::LOCAL_STORAGE)));
        }
        for iso in spec.iso.iter().chain(spec.hw.iso2.iter()) {
            if driver.is_real() && !driver.isos().iter().any(|i| i == iso) {
                return Err(Error::InvalidSpec(format!("ISO bulunamadı: {iso}")));
            }
        }
        if let Some(image) = &spec.hw.image {
            if driver.is_real() && !driver.images().iter().any(|i| i == image) {
                return Err(Error::InvalidSpec(format!("bulut imajı bulunamadı: {image}")));
            }
        }
        if spec.hw.encrypted && driver.is_real() && !driver.disk_encryption() {
            return Err(Error::InvalidSpec("disk şifreleme kapalı: sunucuyu --disk-key-file ile başlatın".to_string()));
        }
        if let Some(image) = redox_core::oci::oci_reference(&spec.os) {
            if spec.kind != redox_core::GuestKind::Ct {
                return Err(Error::InvalidSpec("OCI imajı yalnızca konteynerlerde kullanılır".to_string()));
            }
            redox_core::oci::ImageRef::parse(image).map_err(Error::InvalidSpec)?;
        }
        self.check_firewall(spec.hw.firewall)?;
        self.check_machine(spec.hw.firmware == redox_core::hw::Firmware::Uefi, spec.hw.tpm)?;
        if driver.is_real() {
            self.check_bridge(&spec.bridge)?;
            let templates = driver.templates();
            let image = redox_core::oci::oci_reference(&spec.os);
            if image.is_some() && !driver.oci_images() {
                return Err(Error::InvalidSpec("bu sunucu OCI imajından konteyner oluşturamıyor (LXC gerekir)".to_string()));
            }
            if spec.kind == redox_core::GuestKind::Ct && image.is_none() && !templates.is_empty() && !templates.contains(&spec.os) {
                return Err(Error::InvalidSpec(format!("bilinmeyen konteyner şablonu: {}; seçenekler: {}", spec.os, templates.join(", "))));
            }
        }
        let guest = self.mutate(|c| {
            let vmid = c.create_guest(spec, user, now())?;
            c.guest(vmid).cloned()
        })?;
        if let Err(message) = driver.create(&guest) {
            self.mutate(|c| c.abort_create(guest.vmid, &message, now()));
            return Err(Error::Driver(message));
        }
        let _ = self.sync_firewall();
        Ok(guest.vmid)
    }

    /// Önce silinebilirliği denetler, sonra sürücüye kaynakları sildirir, en son kaydı kaldırır.
    pub fn delete_guest(&self, vmid: VmId, user: &str) -> Result<(), Error> {
        let guest = self.lock().check_deletable(vmid)?.clone();
        // Diskleri başka sunucuya geçen konukta silinen yalnızca kayıttır.
        if guest.hw.handed_to.is_none() {
            self.driver().destroy(&guest).map_err(Error::Driver)?;
        }
        self.mutate(|c| c.delete_guest(vmid, user, now()))?;
        let _ = self.sync_firewall();
        Ok(())
    }

    /// Eylemi doğrular ve görevi başlatır. Gecikme sıfırsa (testler) görev dönmeden
    /// önce biter; değilse arka planda, küme kilidi tutulmadan tamamlanır.
    pub fn run_action(&self, vmid: VmId, action: Action, user: &str) -> Result<TaskId, Error> {
        if action == Action::Start {
            self.lock().check_not_in_maintenance()?;
        }
        // Korunan konukta, ikizin aynı konuğu ayrıca çalıştırmayacağından emin olunmadan eylem yapılmaz.
        self.ft_before_action(vmid, action)?;
        let task = self.mutate(|c| c.begin_action(vmid, action, user, now()))?;
        if self.0.task_delay.is_zero() {
            self.finish(task, vmid, action);
        } else {
            let (me, delay) = (self.clone(), self.0.task_delay);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                // Sürücü çağrısı bloklayabilir (konuğun kapanmasını beklemek gibi).
                let _ = tokio::task::spawn_blocking(move || me.finish(task, vmid, action)).await;
            });
        }
        Ok(task)
    }

    pub(super) fn finish(&self, task: TaskId, vmid: VmId, action: Action) {
        // Sürücü kilit dışında çalışır: uzun süren bir kapatma diğer istekleri bekletmez.
        let guest = self.lock().guest(vmid).cloned();
        let outcome = match guest {
            Ok(guest) => self.0.driver.apply(&guest, action),
            Err(e) => Err(e.to_string()),
        };
        // Görev zaten kapatılmışsa (ör. kurtarma) yapılacak bir şey yok.
        self.mutate(|c| {
            if action == Action::Start && outcome.is_ok() {
                // Askıdaki konuk kaldığı yerden devam etti: kayıtlı durum kullanıldı.
                c.clear_suspended(vmid);
            }
            let _ = c.complete_task(task, outcome, now());
        });
        // Askıdan dönen konuk duraklatılmış olabilir (duraklatılmışken askıya alındıysa): kayıt, bir sonraki
        // okuma turunu beklemeden gerçek durumu göstersin.
        if action == Action::Start {
            let probed = self.lock().guest(vmid).ok().cloned().and_then(|g| self.0.driver.probe(&g));
            if let Some(probe) = probed {
                self.mutate(|c| {
                    c.observe_guest(vmid, &probe, now());
                });
            }
        }
    }

    /// Çalışan sanal makineyi askıya alır: bellek durumu diske yazılır, konuk kapanır; başlatılınca kaldığı yerden sürer.
    pub fn suspend(&self, vmid: VmId, user: &str) -> Result<TaskId, Error> {
        // Bellek durumu, kayıttaki makineyle aynı olmalıdır.
        self.reconcile_unplug(vmid);
        let guest = self.lock().guest(vmid)?.clone();
        if guest.kind != redox_core::GuestKind::Vm {
            return Err(Error::InvalidSpec("yalnızca sanal makineler askıya alınır".to_string()));
        }
        if guest.status == redox_core::GuestStatus::Stopped {
            return Err(Error::InvalidSpec(format!("{vmid} çalışmıyor; askıya almak için çalışıyor olmalı")));
        }
        self.run_op(vmid, user, format!("vm suspend {vmid}"), None, |_| Ok(()), |d, g| d.suspend(g), move |c, ()| c.after_suspend(vmid))
    }

    /// Askıdaki konuğun kayıtlı bellek durumunu atar: konuk bir sonraki başlatmada sıfırdan açılır.
    pub fn discard_suspend(&self, vmid: VmId, user: &str) -> Result<(), Error> {
        let guest = self.lock().guest(vmid)?.clone();
        if !guest.hw.suspended {
            return Err(Error::InvalidSpec(format!("{vmid} askıda değil")));
        }
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        let outcome = self.driver().discard_suspend(&guest);
        self.mutate(|c| {
            if outcome.is_ok() {
                c.clear_suspended(vmid);
            }
            c.log(&guest.node, user, format!("vm discard-suspend {vmid}"), &outcome, now(), Some(vmid));
        });
        outcome.map_err(Error::Driver)
    }

    /// Ayarları değiştirir. Gerçek sürücüde takılacak ISO'nun var olduğu da denetlenir.
    pub fn update_guest(&self, vmid: VmId, patch: &GuestPatch, user: &str) -> Result<(), Error> {
        let driver = self.driver();
        for iso in patch.iso.iter().chain(patch.iso2.iter()).filter(|i| !i.is_empty()) {
            if driver.is_real() && !driver.isos().iter().any(|i| i == iso) {
                return Err(Error::InvalidSpec(format!("ISO bulunamadı: {iso}")));
            }
        }
        if driver.is_real() {
            let current = self.lock().guest(vmid)?.clone();
            if let Some(bridge) = &patch.bridge {
                self.check_bridge(bridge)?;
            }
            let bridge = patch.bridge.as_deref().unwrap_or(&current.bridge);
            let forwards = patch.forwards.as_ref().unwrap_or(&current.forwards);
            if !forwards.is_empty() && (bridge != redox_core::NAT || current.kind == redox_core::GuestKind::Ct) {
                return Err(Error::InvalidSpec(
                    "port yönlendirme yalnızca NAT ağındaki sanal makinelerde kullanılır; köprüdeki konuğa doğrudan adresinden erişilir".to_string(),
                ));
            }
        }
        if patch.firewall == Some(true) {
            self.check_firewall(true)?;
        }
        if patch.nested == Some(true) && driver.is_real() && !driver.nested_virt() {
            return Err(Error::InvalidSpec(crate::qemu::NESTED_HINT.to_string()));
        }
        if patch.nic_guard == Some(true) && driver.is_real() && !driver.firewall() {
            return Err(Error::InvalidSpec("ağ kartı koruması kapalı: sunucuyu --firewall ile başlatın (nftables ve CAP_NET_ADMIN gerekir)".to_string()));
        }
        if patch.shares.as_ref().is_some_and(|s| !s.is_empty()) && !driver.shared_folders() {
            return Err(Error::InvalidSpec("bu sunucudaki QEMU paylaşılan klasörü (9p) desteklemiyor".to_string()));
        }
        self.check_machine(patch.firmware == Some(redox_core::hw::Firmware::Uefi), patch.tpm == Some(true))?;
        if let Some(model) = &patch.cpu_type {
            redox_core::hw::check_cpu_vendor(model, driver.host_cpu_vendor()).map_err(Error::InvalidSpec)?;
        }
        if driver.is_real() {
            for network in patch.nics.iter().flatten() {
                self.check_bridge(redox_core::hw::parse_nic(network).map(|(bridge, _)| bridge).unwrap_or(network))?;
            }
        }
        let guest = self.mutate(|c| {
            c.update_guest(vmid, patch, user, now())?;
            c.guest(vmid).cloned()
        })?;
        driver.configure(&guest).map_err(Error::Driver)?;
        // Ağ ya da kural değişmiş olabilir; yüklenemezse kullanıcı hemen öğrenmeli.
        self.sync_firewall().map_err(Error::Driver)
    }

    /// Gerçek sürücüde UEFI ve TPM için sunucuda ilgili paketler kurulu olmalıdır.
    pub(super) fn check_machine(&self, uefi: bool, tpm: bool) -> Result<(), Error> {
        let driver = self.driver();
        if !driver.is_real() {
            return Ok(());
        }
        if uefi && !driver.uefi() {
            return Err(Error::InvalidSpec("UEFI donanım yazılımı sunucuda kurulu değil: sudo apt install ovmf".to_string()));
        }
        if tpm && !driver.tpm() {
            return Err(Error::InvalidSpec("sanal TPM sunucuda kurulu değil: sudo apt install swtpm".to_string()));
        }
        Ok(())
    }

    /// Gerçek sürücüde ağ ayarı `nat` ya da sunucuda var olan bir köprü olmalıdır.
    pub(super) fn check_bridge(&self, bridge: &str) -> Result<(), Error> {
        let known = self.driver().bridges();
        if bridge == redox_core::NAT || known.iter().any(|b| b == bridge) {
            return Ok(());
        }
        Err(Error::InvalidSpec(format!(
            "\"{bridge}\" adında bir köprü yok; seçenekler: nat{}",
            known.iter().map(|b| format!(", {b}")).collect::<String>()
        )))
    }

    /// Çalışan konteynerin içinde komut çalıştırır. Komut metni günlüğe yazılmaz (gizli bilgi içerebilir).
    pub fn exec(&self, vmid: VmId, command: &str, user: &str) -> Result<String, Error> {
        let guest = self.lock().guest(vmid)?.clone();
        if guest.status != redox_core::GuestStatus::Running {
            return Err(Error::InvalidSpec(format!("{vmid} çalışmıyor; komut çalıştırmak için başlatın")));
        }
        let result = self.driver().exec(&guest, command);
        let outcome = result.as_ref().map(|_| ()).map_err(|e| e.lines().next().unwrap_or_default().to_string());
        self.mutate(|c| c.log(&guest.node, user, format!("{} exec {vmid}", guest.kind), &outcome, now(), Some(vmid)));
        result.map_err(Error::Driver)
    }

    /// Durdurulmuş konuğu yeni bir kimlikle kopyalar; yeni konuğun kimliğini döndürür.
    pub fn clone_guest(&self, vmid: VmId, name: &str, user: &str) -> Result<VmId, Error> {
        self.clone_guest_as(vmid, name, false, user)
    }

    /// Kapalı sanal makineyi şablona çevirir ya da şablonluktan çıkarır.
    pub fn set_template(&self, vmid: VmId, on: bool, user: &str) -> Result<(), Error> {
        self.mutate(|c| {
            let node = c.guest(vmid)?.node.clone();
            let outcome = c.set_template(vmid, on);
            if outcome.is_ok() {
                c.log(&node, user, format!("vm template {vmid} {}", if on { "on" } else { "off" }), &Ok(()), now(), Some(vmid));
            }
            outcome
        })
    }

    /// `linked`: şablondan bağlı klon (disk kopyalanmaz, yalnızca farklar tutulur).
    pub fn clone_guest_as(&self, vmid: VmId, name: &str, linked: bool, user: &str) -> Result<VmId, Error> {
        let (task, source, target) = self.mutate(|c| {
            let kind = c.guest(vmid)?.kind;
            let verb = if linked { "linkclone" } else { "clone" };
            let task = c.begin_op(vmid, format!("{kind} {verb} {vmid} {name}"), user, now(), Some("klonlamak"))?;
            match c.clone_guest_as(vmid, name, linked) {
                Ok(new_id) => {
                    c.lock_with(new_id, task);
                    Ok((task, c.guest(vmid)?.clone(), c.guest(new_id)?.clone()))
                }
                Err(e) => {
                    let _ = c.complete_task(task, Err(e.to_string()), now());
                    Err(e)
                }
            }
        })?;
        let (me, new_id) = (self.clone(), target.vmid);
        self.dispatch(move || {
            let result = if linked { me.driver().link_disk(&source, &target) } else { me.driver().clone_disk(&source, &target) };
            me.mutate(|c| {
                if result.is_ok() {
                    c.unlock(new_id, task);
                } else {
                    c.discard_clone(new_id);
                }
                let _ = c.complete_task(task, result, now());
            });
            // Klon kaynağın güvenlik duvarı ayarını devralır; kendi adresiyle yüklenmeli.
            let _ = me.sync_firewall();
        });
        Ok(new_id)
    }

    pub fn screenshot(&self, vmid: VmId) -> Result<Vec<u8>, Error> {
        let guest = self.lock().guest(vmid)?.clone();
        self.driver().screenshot(&guest).map_err(Error::Driver)
    }

    pub fn send_keys(&self, vmid: VmId, chords: &[Vec<String>]) -> Result<(), Error> {
        let guest = self.lock().guest(vmid)?.clone();
        self.driver().send_keys(&guest, chords).map_err(Error::Driver)
    }
}
