//! Sunucular arası taşıma, OVA dışa aktarma ve kurulum ortamları.

use super::*;

impl AppState {
    /// Kapalı sanal makineyi dışa aktarma için hazırlar: konuğun kaydı, geçici dizin ve vmdk diskleri.
    #[allow(clippy::type_complexity)]
    pub fn export_ova(&self, vmid: VmId) -> Result<(Guest, std::path::PathBuf, Vec<(String, std::path::PathBuf, u64)>), Error> {
        let guest = self.lock().guest(vmid)?.clone();
        if guest.kind != redox_core::GuestKind::Vm {
            return Err(Error::InvalidSpec("yalnızca sanal makineler OVA olarak dışa aktarılır".to_string()));
        }
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if guest.status != redox_core::GuestStatus::Stopped {
            return Err(Error::RequiresStopped { vmid, what: "dışa aktarmak".to_string() });
        }
        match self.driver().export_disks(&guest).map_err(Error::Driver)? {
            Some((dir, disks)) => Ok((guest, dir, disks)),
            None => Err(Error::InvalidSpec("bu sürücü disk dosyası tutmuyor (gerçek sürücü gerekir)".to_string())),
        }
    }

    /// Kalıbı ya da imajı bir adresten arka planda indirir; sonuç görev günlüğüne yazılır.
    /// Dosya adı verilmezse adresin son parçası kullanılır.
    pub fn media_fetch(&self, url: &str, name: &str, sha256: &str, user: &str) -> Result<String, Error> {
        let url = url.trim().to_string();
        let name = match name.trim() {
            "" => url.split(['?', '#']).next().unwrap_or_default().rsplit('/').next().unwrap_or_default().to_string(),
            given => given.to_string(),
        };
        let (dest, _) = crate::media::target(self, &name)?;
        if dest.exists() {
            return Err(Error::InvalidSpec(format!("{name} zaten var; önce onu silin ya da başka bir ad verin")));
        }
        // İstek hemen reddedilebilsin diye adres önce burada denetlenir; indirirken (yönlendirmelerde) yeniden denetlenir.
        crate::media::check_public_url(&url).map_err(Error::InvalidSpec)?;
        let (me, user, sha256, label) = (self.clone(), user.to_string(), sha256.trim().to_string(), name.clone());
        self.dispatch(move || {
            let outcome = crate::media::fetch(&url, &dest, Some(&sha256), &crate::media::check_public_url).map(|_| ());
            me.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, &user, format!("media fetch {label}"), &outcome, now(), None);
            });
        });
        Ok(name)
    }

    /// Katalogdaki hazır bulut imajını indirir; kaydedilen dosya adını döndürür.
    pub fn catalog_fetch(&self, key: &str, user: &str) -> Result<String, Error> {
        let (_, _, file, url) = crate::media::CATALOG.iter().find(|e| e.0 == key).ok_or_else(|| {
            Error::InvalidSpec(format!("katalogda \"{key}\" yok; olanlar: {}", crate::media::CATALOG.iter().map(|e| e.0).collect::<Vec<_>>().join(", ")))
        })?;
        self.media_fetch(url, file, "", user)
    }

    /// Yüklenen kalıbın ya da imajın kaydını görev günlüğüne düşer.
    pub fn media_uploaded(&self, name: &str, outcome: &Result<(), String>, user: &str) {
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("media upload {name}"), outcome, now(), None);
        });
    }

    /// Kalıbı ya da imajı siler. Bir konuk kullanıyorsa (takılı kalıp) silinmez.
    pub fn media_delete(&self, name: &str, user: &str) -> Result<(), Error> {
        let (path, iso) = crate::media::target(self, name)?;
        if iso {
            let users: Vec<String> = self.lock().guests.iter().filter(|g| g.iso.as_deref() == Some(name)).map(|g| g.vmid.to_string()).collect();
            if !users.is_empty() {
                return Err(Error::InvalidSpec(format!("{name} şu konuklara takılı: {}. Önce kalıbı onlardan çıkarın.", users.join(", "))));
            }
        }
        let outcome = match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::InvalidSpec(format!("{name} bulunamadı"))),
            Err(e) => Err(format!("{name} silinemedi: {e}")),
        };
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("media delete {name}"), &outcome, now(), None);
        });
        outcome.map_err(Error::Driver)
    }

    /// Eş sunucudaki kapalı sanal makineyi yedeği üzerinden bu sunucuya kopyalar (arka planda).
    /// Kaynak konuğa dokunulmaz: çalıştığını doğruladıktan sonra eşte silmek size kalır.
    pub fn import_from_peer(&self, peer_name: &str, vmid: VmId, user: &str) -> Result<(), Error> {
        let peer = self.peers().peer(peer_name).ok_or_else(|| Error::InvalidSpec(format!("\"{peer_name}\" adında bir eş sunucu tanımlı değil")))?;
        let image = format!("aktarim-{peer_name}-{vmid}.qcow2");
        let Some(image_path) = self.driver().image_file(&image) else {
            return Err(Error::InvalidSpec("bu sürücü disk imajı içe aktaramıyor (gerçek sürücü gerekir)".to_string()));
        };
        let (me, user, peer_name) = (self.clone(), user.to_string(), peer_name.to_string());
        self.dispatch(move || {
            let outcome = me.import_now(&peer, &peer_name, vmid, &image, &image_path, &user, false);
            let _ = std::fs::remove_file(&image_path);
            me.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                let (result, new_id) = match &outcome {
                    Ok(id) => (Ok(()), Some(*id)),
                    Err(e) => (Err(e.clone()), None),
                };
                c.log(&node, &user, format!("vm import {peer_name}/{vmid}"), &result, now(), new_id);
            });
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn import_now(&self, peer: &crate::peers::Peer, peer_name: &str, vmid: VmId, image: &str, image_path: &Path, user: &str, replica: bool) -> Result<VmId, String> {
        let remote = peer.guest(vmid)?;
        if remote.kind != redox_core::GuestKind::Vm {
            return self.import_container(peer, peer_name, &remote, user, replica);
        }
        // Çalışan (ya da zaten askıda olan) konuk bellek durumuyla birlikte taşınır: kaynakta askıya alınır,
        // diski ve belleği kopyalanır, burada kaldığı yerden devam eder. Kesinti kopyalama süresi kadardır.
        // Çoğaltmada kaynağa dokunulmaz: askıya alınmaz, bellek durumu taşınmaz; kopya kapalı bir konuktur.
        let live = !replica && remote.status != redox_core::GuestStatus::Stopped;
        let carry_state = !replica && (live || remote.hw.suspended);
        if carry_state {
            let (node_cores, node_mem) = self.snapshot().nodes.first().map(|n| (n.cores, n.mem_total_mb)).unwrap_or((0, 0));
            if remote.cores > node_cores || remote.mem_total_mb > node_mem {
                return Err(format!("çalışırken taşınan konuk aynı işlemci ve bellekle açılmalı ({} çekirdek, {} MB); bu sunucu yetmiyor. Kapatıp öyle taşıyın", remote.cores, remote.mem_total_mb));
            }
            if !remote.hw.usb.is_empty() || !remote.hw.pci.is_empty() {
                return Err("sunucu aygıtı aktarılmış konuk çalışırken taşınamaz (bellek durumu o aygıtları bekler); kapatıp öyle taşıyın".to_string());
            }
            if remote.lock.is_some() {
                return Err(format!("{vmid} üzerinde eş sunucuda bir işlem sürüyor; bitince yeniden deneyin"));
            }
        }
        // Ön kopya (isteğe bağlı): diskler konuk çalışırken bir kez indirilir; askıya alındıktan sonra yalnızca
        // o arada değişen bloklar gelir. Duraklama, diskin tamamı yerine değişen kısmın aktarımı kadar sürer.
        let warm = if live && self.move_precopy() { self.precopy(peer, vmid, &remote, image_path) } else { Vec::new() };
        let drop_warm = || warm.iter().for_each(|(_, raw)| drop(std::fs::remove_file(raw)));
        if live {
            peer.suspend_and_wait(vmid, Duration::from_secs(1800)).inspect_err(|_| drop_warm())?;
        }
        let paused = std::cell::Cell::new((0u64, 0u64));
        let backup = peer.backup_and_wait(vmid, Duration::from_secs(6 * 3600)).inspect_err(|_| drop_warm())?;
        if let Some(parent) = image_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{} oluşturulamadı: {e}", parent.display()))?;
        }
        // Asıl diskin yanına ek diskler ve makine durumu (UEFI değişkenleri, sanal TPM) da indirilir.
        let extra_path = |disk: u8| image_path.with_extension(format!("disk{disk}"));
        let machine_path = image_path.with_extension("machine");
        let wants_machine = remote.hw.firmware == redox_core::hw::Firmware::Uefi || remote.hw.tpm;
        let discard = || {
            for disk in &remote.hw.disks {
                let _ = std::fs::remove_file(extra_path(disk.id));
            }
            let _ = std::fs::remove_file(&machine_path);
        };
        // Çoğaltmada önceki yedek kopya duruyorsa yalnızca değişen bloklar indirilir; olmazsa tam kopyaya dönülür.
        let base = if replica { self.replica_base(peer_name, vmid) } else { None };
        if replica {
            self.mutate(|c| c.note_replica_delta(peer_name, vmid, 0, 0, true));
        }
        let fetch = |disk: u8, dest: &Path| -> Result<(), String> {
            if let Some((_, raw)) = warm.iter().find(|(id, _)| *id == disk) {
                match self.patch_to_image(peer, &backup, disk, raw, dest) {
                    Ok((fetched, total)) => {
                        paused.set((paused.get().0 + fetched, paused.get().1 + total));
                        return Ok(());
                    }
                    Err(e) => eprintln!("uyarı: {peer_name}/{vmid} disk {disk} ön kopyadan tamamlanamadı, tamamı indirilecek: {e}"),
                }
            }
            if let Some(old) = base.as_ref().filter(|old| disk == 0 || old.hw.disks.iter().any(|d| d.id == disk)) {
                match self.fetch_delta(peer, &backup, disk, old, dest) {
                    Ok((fetched, total)) => {
                        self.mutate(|c| c.note_replica_delta(peer_name, vmid, fetched, total, false));
                        return Ok(());
                    }
                    Err(e) => eprintln!("uyarı: {peer_name}/{vmid} disk {disk} artımlı aktarılamadı, tamamı indirilecek: {e}"),
                }
            }
            let size_gb = if disk == 0 { remote.disk_gb } else { remote.hw.disks.iter().find(|d| d.id == disk).map_or(0, |d| d.size_gb) };
            peer.download_backup_disk(&backup, disk, dest, crate::peers::download_limit(size_gb << 10)).map(|_| ())
        };
        let downloaded = (|| {
            fetch(0, image_path)?;
            for disk in &remote.hw.disks {
                fetch(disk.id, &extra_path(disk.id)).map_err(|e| format!("ek disk {} indirilemedi: {e}", disk.id))?;
            }
            if wants_machine { peer.download_machine(&backup, &machine_path) } else { Ok(false) }
        })();
        // Eşte bıraktığımız geçici yedek her durumda silinir.
        let _ = peer.delete_backup(&backup);
        drop_warm();
        if !warm.is_empty() {
            let (fetched, total) = paused.get();
            self.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, user, format!("vm precopy {peer_name}/{vmid} {fetched}/{total}"), &Ok(()), now(), None);
            });
        }
        let has_machine = downloaded.inspect_err(|_| discard())?;
        if remote.hw.tpm && !has_machine {
            discard();
            return Err("konuğun sanal TPM durumu eş sunucudan alınamadı (eş eski bir sürüm olabilir); TPM durumu olmadan taşınan konuk şifreli disklerini açamaz".to_string());
        }

        let local = self.snapshot();
        let node = local.nodes.first().ok_or("bu sunucuda düğüm yok")?;
        let storage = local.storages.iter().find(|s| s.available_on(&node.id)).ok_or("bu sunucuda depolama yok")?;
        // Sunucuya özgü ayarlar taşınmaz: köprü burada yoksa NAT'a düşülür, fiziksel aygıtlar bırakılır.
        let known = self.driver().bridges();
        let here = |bridge: &String| if known.contains(bridge) { bridge.clone() } else { redox_core::NAT.to_string() };
        let mut hw = remote.hw.clone();
        hw.image = Some(image.to_string());
        hw.usb.clear();
        hw.pci.clear();
        // Etiketli ek kart, köprüsü burada varsa etiketiyle kalır; yoksa NAT'a düşer.
        hw.nics = hw.nics.iter().map(|spec| match redox_core::hw::parse_nic(spec) {
            Some((bridge, _)) if known.iter().any(|k| k == bridge) => spec.clone(),
            _ => redox_core::NAT.to_string(),
        }).collect();
        if here(&remote.bridge) == redox_core::NAT {
            hw.vlan = 0;
        }
        if !self.driver().firewall() {
            hw.firewall = false;
            hw.nic_guard = false;
        }
        hw.suspended = false;
        // Ek diskler konuk oluşturulduktan sonra numaraları ve içerikleriyle eklenir.
        hw.disks.clear();
        hw.disk_seq = 0;
        let spec = GuestSpec {
            // Bellek durumuyla gelen konuk, kimliği boşsa aynı kimliği alır: ağ kartının adresi kimlikten türetilir.
            vmid: Some(remote.vmid).filter(|id| carry_state && local.guest(*id).is_err()),
            name: if replica { redox_core::replica_name(&remote.name) } else { remote.name.clone() },
            kind: remote.kind,
            node: node.id.clone(),
            cores: remote.cores.min(node.cores),
            memory_mb: remote.mem_total_mb.min(node.mem_total_mb),
            disk_gb: remote.disk_gb,
            storage: storage.id.clone(),
            os: remote.os.clone(),
            bridge: here(&remote.bridge),
            onboot: false,
            iso: None,
            hw,
        };
        let id = self.create_guest(&spec, user).map_err(|e| e.to_string()).inspect_err(|_| discard())?;
        let extras = (|| {
            for disk in &remote.hw.disks {
                let guest = self.mutate(|c| {
                    c.add_disk_exact(id, disk)?;
                    c.guest(id).cloned()
                })
                .map_err(|e| e.to_string())?;
                self.driver().import_disk(&guest, disk.id, &extra_path(disk.id)).map_err(|e| format!("ek disk {}: {e}", disk.id))?;
            }
            if has_machine {
                let guest = self.snapshot().guest(id).cloned().map_err(|e| e.to_string())?;
                self.driver().restore_machine(&guest, &machine_path).map_err(|e| format!("makine durumu (UEFI/TPM) uygulanamadı: {e}"))?;
            }
            Ok::<_, String>(())
        })();
        discard();
        if let Err(e) = extras {
            // Yarım kopya bırakılmaz.
            let _ = self.delete_guest(id, user);
            return Err(e);
        }
        // İmaj yalnızca taşıma içindi; kayıtta kalırsa silinmiş bir dosyayı gösterir.
        self.mutate(|c| {
            if let Ok(g) = c.guest_mut_for_import(id) {
                g.hw.image = None;
                g.tags = remote.tags.clone();
                g.tags.push(format!("{}-{peer_name}", if replica { "kopya" } else { "tasindi" }));
                if replica {
                    // Yedek kopya kendiliğinden açılmaz ve zamanlanmış işleri devralmaz.
                    g.hw.restart = false;
                    g.hw.schedules.clear();
                    g.backup_time = None;
                }
                g.tags.retain(|t| redox_core::valid_tag(t));
                g.tags.truncate(redox_core::MAX_TAGS);
            }
        });
        if carry_state {
            if let Err(e) = self.adopt_state(peer, vmid, id, image_path, live, user) {
                // Burada yarım kalan kopya bırakılmaz; kaynak askıda durur ve orada başlatılırsa kaldığı yerden sürer.
                let _ = self.delete_guest(id, user);
                return Err(format!("{e}. Konuk eş sunucuda askıda duruyor: orada başlatırsanız kaldığı yerden devam eder"));
            }
        }
        Ok(id)
    }

    /// Çoğaltmanın önceki yedek kopyası; artımlı aktarımın temeli olabilmesi için kapalı bir sanal makine olmalıdır.
    fn replica_base(&self, peer_name: &str, source: VmId) -> Option<Guest> {
        let c = self.lock();
        let standby = c.replicas.iter().find(|r| r.peer == peer_name && r.source == source)?.standby?;
        c.guest(standby).ok().filter(|g| g.kind == redox_core::GuestKind::Vm && g.status == redox_core::GuestStatus::Stopped && g.lock.is_none()).cloned()
    }

    /// Yedekteki diski, eski kopyanın aynı diskinden yola çıkarak `dest` imajına getirir: eski disk ham olarak
    /// dökülür, kaynakla özeti tutmayan bloklar indirilir, sonuç imaja çevrilir. (indirilen, toplam) blok döner.
    fn fetch_delta(&self, peer: &crate::peers::Peer, backup: &str, disk: u8, old: &Guest, dest: &Path) -> Result<(u64, u64), String> {
        let work = dest.with_extension(format!("ham{disk}"));
        let done = self.driver().disk_raw(old, disk, &work).and_then(|()| self.patch_to_image(peer, backup, disk, &work, dest));
        let _ = std::fs::remove_file(&work);
        done
    }

    /// `raw` dosyasını yedekteki diskin içeriğine getirir (yalnızca farklı bloklar indirilir) ve `dest` imajına çevirir.
    fn patch_to_image(&self, peer: &crate::peers::Peer, backup: &str, disk: u8, raw: &Path, dest: &Path) -> Result<(u64, u64), String> {
        let (size, hashes) = peer.backup_blocks(backup, disk)?;
        let counts = crate::delta::patch(raw, size, &hashes, &|index| peer.download_block(backup, disk, index))?;
        self.driver().raw_to_image(raw, dest)?;
        Ok(counts)
    }

    pub fn set_move_precopy(&self, on: bool) {
        self.0.move_precopy.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn move_precopy(&self) -> bool {
        self.0.move_precopy.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Ön kopya: konuk çalışırken eşte bir yedek alır ve disklerini ham dosyalara indirir. Başarılan disklerin
    /// (numara, dosya) listesini döndürür; başarısız olan disk sonradan olağan yolla, tam indirilir.
    fn precopy(&self, peer: &crate::peers::Peer, vmid: VmId, remote: &Guest, image_path: &Path) -> Vec<(u8, std::path::PathBuf)> {
        if let Some(parent) = image_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(backup) = peer.backup_and_wait(vmid, Duration::from_secs(6 * 3600)) else { return Vec::new() };
        let mut warm = Vec::new();
        for disk in std::iter::once(0).chain(remote.hw.disks.iter().map(|d| d.id)) {
            let raw = image_path.with_extension(format!("on{disk}"));
            let filled = std::fs::File::create(&raw)
                .map_err(|e| format!("{} oluşturulamadı: {e}", raw.display()))
                .and_then(|_| peer.backup_blocks(&backup, disk))
                .and_then(|(size, hashes)| crate::delta::patch(&raw, size, &hashes, &|index| peer.download_block(&backup, disk, index)));
            match filled {
                Ok(_) => warm.push((disk, raw)),
                Err(e) => {
                    let _ = std::fs::remove_file(&raw);
                    eprintln!("uyarı: {vmid} disk {disk} için ön kopya alınamadı: {e}");
                }
            }
        }
        let _ = peer.delete_backup(&backup);
        warm
    }

    /// Eş sunucudaki konteyneri yedeği üzerinden buraya kopyalar: aynı dağıtımın şablonundan boş bir konteyner
    /// oluşturulur, sonra kök dosya sistemi yedekle değiştirilir. Kaynak çalışıyorsa yedek sırasında kısa süre
    /// dondurulur ve çalışmaya devam eder; bellek durumu taşınmaz (kopya kapalı gelir).
    fn import_container(&self, peer: &crate::peers::Peer, peer_name: &str, remote: &Guest, user: &str, replica: bool) -> Result<VmId, String> {
        if remote.lock.is_some() {
            return Err(format!("{} üzerinde eş sunucuda bir işlem sürüyor; bitince yeniden deneyin", remote.vmid));
        }
        if !self.driver().supports(redox_core::GuestKind::Ct) {
            return Err("bu sunucuda konteyner desteği yok (LXC kurulu değil)".to_string());
        }
        let local = self.snapshot();
        let node = local.nodes.first().ok_or("bu sunucuda düğüm yok")?;
        let storage = local.storages.iter().find(|s| s.available_on(&node.id)).ok_or("bu sunucuda depolama yok")?;
        let known = self.driver().bridges();
        let mut hw = redox_core::Hardware { nics: remote.hw.nics.clone(), firewall: remote.hw.firewall && self.driver().firewall(), fw_rules: remote.hw.fw_rules.clone(), fw_out_restrict: remote.hw.fw_out_restrict, fw_out: remote.hw.fw_out.clone(), fw_groups: remote.hw.fw_groups.clone(), tun: remote.hw.tun, ..Default::default() };
        // Burada tanımı olmayan grup alınmaz; tanımsız adres kümesi ise kurallar uygulanırken hiçbir şeye izin vermez.
        hw.fw_groups.retain(|name| local.fw_group(name).is_some());
        hw.nics = hw.nics.iter().map(|spec| match redox_core::hw::parse_nic(spec) {
            Some((bridge, _)) if known.iter().any(|k| k == bridge) => spec.clone(),
            _ => redox_core::NAT.to_string(),
        }).collect();
        // Eşin bildirdiği OCI başvurusu izlenmez: yoksa eş, bu sunucuyu kendi seçtiği bir adresten (iç ağdaki bir
        // makine dahil) imaj indirmeye yollayabilirdi. İçerik zaten yedekten gelir; konteyner yerel bir şablonla açılır.
        let os = if redox_core::oci::oci_reference(&remote.os).is_some() {
            self.driver().templates().into_iter().next().unwrap_or_else(|| remote.os.clone())
        } else {
            remote.os.clone()
        };
        let spec = GuestSpec {
            vmid: None,
            name: if replica { redox_core::replica_name(&remote.name) } else { remote.name.clone() },
            kind: remote.kind,
            node: node.id.clone(),
            cores: remote.cores.min(node.cores),
            memory_mb: remote.mem_total_mb.min(node.mem_total_mb),
            disk_gb: remote.disk_gb,
            storage: storage.id.clone(),
            os,
            bridge: if known.contains(&remote.bridge) { remote.bridge.clone() } else { redox_core::NAT.to_string() },
            onboot: false,
            iso: None,
            hw,
        };
        if redox_core::oci::oci_reference(&spec.os).is_some() {
            return Err("eş sunucudaki konteyner bir OCI imajından oluşturulmuş; bu sunucuda onu açacak yerel şablon yok".to_string());
        }
        let backup = peer.backup_and_wait(remote.vmid, Duration::from_secs(6 * 3600))?;
        // Eşte bıraktığımız geçici yedek her durumda silinir.
        let made = (|| {
            let id = self.create_guest(&spec, user).map_err(|e| e.to_string())?;
            let filled = (|| {
                let temp = format!("ct-{id}-{}", now());
                let path = self.driver().backup_file(&temp).ok_or("bu sürücü yedek dosyası tutmuyor")?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("{} oluşturulamadı: {e}", parent.display()))?;
                }
                let restored = peer.download_backup(&backup, &path, crate::peers::download_limit(remote.disk_gb << 10)).and_then(|_| {
                    let guest = self.snapshot().guest(id).cloned().map_err(|e| e.to_string())?;
                    self.driver().restore(&guest, &temp, &[])
                });
                let _ = std::fs::remove_file(&path);
                restored
            })();
            match filled {
                Ok(()) => Ok(id),
                Err(e) => {
                    // Yarım kopya bırakılmaz.
                    let _ = self.delete_guest(id, user);
                    Err(e)
                }
            }
        })();
        let _ = peer.delete_backup(&backup);
        let id = made?;
        self.mutate(|c| {
            if let Ok(g) = c.guest_mut_for_import(id) {
                g.tags = remote.tags.clone();
                g.tags.push(format!("{}-{peer_name}", if replica { "kopya" } else { "tasindi" }));
                g.tags.retain(|t| redox_core::valid_tag(t));
                g.tags.truncate(redox_core::MAX_TAGS);
            }
        });
        Ok(id)
    }

    /// Kaynaktaki bellek durumunu indirir, yeni konuğun askı kaydı yapar ve (çalışırken taşınıyorsa) konuğu sürdürür.
    pub(super) fn adopt_state(&self, peer: &crate::peers::Peer, remote_id: VmId, id: VmId, scratch: &Path, resume: bool, user: &str) -> Result<(), String> {
        let file = scratch.with_extension("bellek");
        let guest = self.snapshot().guest(id).map_err(|e| e.to_string())?.clone();
        let (running, layout) = peer.download_state(remote_id, &file, crate::peers::download_limit(guest.mem_total_mb))?;
        let adopted = self.driver().adopt_suspend(&guest, &file, running, layout);
        let _ = std::fs::remove_file(&file);
        adopted?;
        self.mutate(|c| c.after_suspend(id));
        if !resume {
            return Ok(());
        }
        self.run_action(id, Action::Start, user).map_err(|e| e.to_string())?;
        // Başlatma görevi bitene kadar beklenir (bellek durumunun yüklenmesi uzun sürebilir).
        for _ in 0..7200 {
            let g = self.snapshot().guest(id).map_err(|e| e.to_string())?.clone();
            if g.lock.is_none() {
                return if g.status == redox_core::GuestStatus::Stopped {
                    let why = self.snapshot().tasks.iter().find(|t| t.vmid == Some(id) && t.status == TaskStatus::Failed).and_then(|t| t.message.clone()).unwrap_or_default();
                    Err(format!("konuk burada sürdürülemedi: {why}"))
                } else {
                    Ok(())
                };
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err("konuk burada zamanında sürdürülemedi".to_string())
    }

    /// Birleşik görünümdeki eş sunucular.
    pub fn peers(&self) -> &crate::peers::Peers {
        &self.0.peers
    }
}
