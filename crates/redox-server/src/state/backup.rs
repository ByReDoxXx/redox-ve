//! Yedekler: alma, geri yükleme, yeni konuk, anında kurtarma, sınama, kilit, silme, arşiv ve rapor.

use super::*;

impl AppState {
    pub fn set_backup_age_days(&self, days: u32) {
        *self.0.backup_age_days.lock().unwrap_or_else(|e| e.into_inner()) = days;
    }

    pub fn backup_age_days(&self) -> u32 {
        *self.0.backup_age_days.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Günlük yedek raporunun saatini ayarlar (gece yarısından dakika).
    pub fn set_backup_report(&self, minute_of_day: u16) {
        self.0.report.lock().unwrap_or_else(|e| e.into_inner()).0 = Some(minute_of_day);
    }

    /// Şimdiki yedek raporu.
    pub fn backup_report(&self) -> crate::report::Report {
        crate::report::compose(&self.snapshot(), now(), self.backup_age_days())
    }

    /// Saati geldiyse günlük yedek raporunu e-postayla gönderir (günde bir kez). Gönderildiyse sonucu döndürür.
    /// Sunucu o saatte kapalıysa rapor, gün bitmeden açılınca gönderilir.
    pub fn report_tick(&self, today: &str, minute_of_day: u16) -> Option<Result<(), String>> {
        {
            let mut report = self.0.report.lock().unwrap_or_else(|e| e.into_inner());
            let due = report.0.is_some_and(|at| at <= minute_of_day) && report.1.as_deref() != Some(today);
            if !due {
                return None;
            }
            // Önce işaretle: gönderim başarısız olsa da aynı gün her turda yeniden denenmesin.
            report.1 = Some(today.to_string());
        }
        let report = self.backup_report();
        let outcome = match self.alerts().email() {
            Some(config) => config.send(&report.subject, &report.body),
            None => Err("e-posta ayarı yok (--alert-email)".to_string()),
        };
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, SCHEDULER, "backup report".to_string(), &outcome, now(), None);
        });
        Some(outcome)
    }

    pub fn set_boot_test_wait(&self, wait: Duration) {
        *self.0.boot_test_wait.lock().unwrap_or_else(|e| e.into_inner()) = wait;
    }

    /// Yedeğin içini okumak için gereken imaj dosyası ve silinmesi gereken geçici bir kopya olup olmadığı.
    /// Artımlı yedek tek dosya olmadığı için önce geçici bir imaja dökülür (uzun sürebilir).
    pub fn backup_image(&self, id: &str, disk: u8) -> Result<(String, std::path::PathBuf, bool), Error> {
        let bin = self.guestfish().ok_or_else(|| Error::InvalidSpec("dosya düzeyinde geri yükleme için sunucuda guestfish gerekir (sudo apt install libguestfs-tools)".to_string()))?;
        let backup = self.lock().backup(id)?.clone();
        if !backup.id.starts_with("vm-") {
            return Err(Error::InvalidSpec("konteyner yedeği bir tar arşividir: yedeği indirip tar ile açın".to_string()));
        }
        if disk != 0 && !backup.disks.iter().any(|d| d.id == disk) {
            return Err(Error::InvalidSpec(format!("bu yedekte {disk} numaralı ek disk yok")));
        }
        match self.driver().export_backup(id, disk).map_err(Error::Driver)? {
            Some((path, temporary)) if path.is_file() => Ok((bin, path, temporary)),
            Some(_) => Err(Error::InvalidSpec("yedek dosyası yerelde yok; önce geri yükleyerek ya da yeniden göndererek dış kopyadan indirin".to_string())),
            None => Err(Error::InvalidSpec("bu sürücü yedek dosyası tutmuyor (gerçek sürücü gerekir)".to_string())),
        }
    }

    /// Yedeğin içindeki bir dosyayı (ya da `tar` ile bir klasörü), yedeğin alındığı **çalışan** konuğun içine
    /// geri yükler (konuk ajanıyla). Arka planda çalışır; sonuç görev günlüğüne yazılır. `target` boşsa dosya
    /// yedekteki yoluna yazılır.
    pub fn backup_put(&self, id: &str, disk: u8, path: &str, tar: bool, target: &str, user: &str) -> Result<VmId, Error> {
        use std::io::Read;
        if !crate::guestfs::valid_path(path) {
            return Err(Error::InvalidSpec(format!("geçersiz yol: {path}")));
        }
        let path = crate::guestfs::normalize(path);
        if path == "/" {
            return Err(Error::InvalidSpec("kök dizinin tamamı konuğa geri yüklenmez; bir dosya ya da klasör seçin".to_string()));
        }
        let target = if target.trim().is_empty() { path.clone() } else { target.trim().to_string() };
        if !crate::guestfs::valid_target(&target) {
            return Err(Error::InvalidSpec(format!("geçersiz hedef yol: {target}")));
        }
        let backup = self.lock().backup(id)?.clone();
        if self.guestfish().is_none() {
            return Err(Error::InvalidSpec("dosya düzeyinde geri yükleme için sunucuda guestfish gerekir (sudo apt install libguestfs-tools)".to_string()));
        }
        let vmid = backup.vmid;
        let guest = self.lock().guest(vmid).cloned().map_err(|_| Error::InvalidSpec("yedeğin alındığı konuk artık yok; dosyayı indirip elle kopyalayın".to_string()))?;
        if guest.kind != redox_core::GuestKind::Vm || guest.status != redox_core::GuestStatus::Running {
            return Err(Error::InvalidSpec(format!("{vmid} çalışmıyor; dosya yalnızca çalışan sanal makinenin içine geri yüklenir")));
        }
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if self.driver().guest_agent(&guest).is_none() {
            return Err(Error::InvalidSpec("konuk ajanı yanıt vermiyor; konukta qemu-guest-agent kurulu ve çalışıyor olmalı".to_string()));
        }
        let (me, user, id) = (self.clone(), user.to_string(), id.to_string());
        self.dispatch(move || {
            let outcome = (|| -> Result<(), String> {
                let (bin, image, temporary) = me.backup_image(&id, disk).map_err(|e| e.to_string())?;
                let cleanup: Option<Box<dyn FnOnce() + Send>> = if temporary {
                    let image = image.clone();
                    Some(Box::new(move || crate::state::discard_export(&image)))
                } else {
                    None
                };
                let mut stream = crate::guestfs::open(&bin, &image, &path, tar, cleanup)?;
                // İlk parça konuğa dokunmadan önce okunur: yedekte olmayan bir dosya, konuktaki dosyayı boşaltmaz.
                let mut first = vec![0u8; 64 * 1024];
                let n = stream.read(&mut first).map_err(|e| format!("yedekteki dosya okunamadı: {e}"))?;
                if n == 0 {
                    return Err("dosya yedekte yok, boş ya da okunamıyor; konuğa dokunulmadı".to_string());
                }
                first.truncate(n);
                let mut data = std::io::Cursor::new(first).chain(stream);
                me.driver().guest_put(&guest, &target, tar, &mut data).map(|_| ())
            })();
            me.mutate(|c| c.log(&guest.node, &user, format!("backup put {id} {path} {target}"), &outcome, now(), Some(vmid)));
        });
        Ok(vmid)
    }

    /// Yedeğin bozulmamış olduğunu arka planda denetler; sonuç görev günlüğüne yazılır.
    pub fn backup_verify(&self, id: &str, user: &str) -> Result<(), Error> {
        let backup = self.lock().backup(id)?.clone();
        let (me, user) = (self.clone(), user.to_string());
        self.dispatch(move || {
            let disks: Vec<u8> = backup.disks.iter().map(|d| d.id).collect();
            let outcome = me.driver().verify_backup(&backup.id, &disks).map(|_| ());
            me.mutate(|c| {
                let node = c.guest(backup.vmid).map(|g| g.node.clone()).ok().or_else(|| c.nodes.first().map(|n| n.id.clone())).unwrap_or_default();
                c.log(&node, &user, format!("backup verify {}", backup.id), &outcome, now(), Some(backup.vmid));
            });
        });
        Ok(())
    }

    /// Var olan bir yedeği dış hedefe (yeniden) gönderir; sonuç görev günlüğüne yazılır.
    /// Artımlı yedekte bütün parçalar hedefe sorulur; eksik olanlar yeniden gönderilir.
    pub fn backup_push(&self, id: &str, user: &str) -> Result<(), Error> {
        let backup = self.lock().backup(id)?.clone();
        if self.offsite().is_none() {
            return Err(Error::InvalidSpec("dış yedek hedefi yapılandırılmamış (redox serve --offsite DOSYA)".to_string()));
        }
        let (me, user) = (self.clone(), user.to_string());
        self.dispatch(move || {
            let disks: Vec<u8> = backup.disks.iter().map(|d| d.id).collect();
            let outcome = me.push_file(&backup.id, &disks, true).unwrap_or(Ok(()));
            let copies = if outcome.is_ok() { me.push_copies(&backup.id, &disks, Some(backup.vmid)) } else { backup.copies };
            me.mutate(|c| {
                c.set_backup_copies(&backup.id, copies);
                let node = c.guest(backup.vmid).map(|g| g.node.clone()).ok().or_else(|| c.nodes.first().map(|n| n.id.clone())).unwrap_or_default();
                if outcome.is_ok() {
                    c.set_backup_offsite(&backup.id, true);
                }
                c.log(&node, &user, format!("backup push {}", backup.id), &outcome, now(), Some(backup.vmid));
            });
        });
        Ok(())
    }

    /// Diskin yedeğini alır; yedeğin kimliğini döndürür.
    pub fn backup_create(&self, vmid: VmId, user: &str) -> Result<String, Error> {
        self.backup_create_as(vmid, user, false, None)
    }

    pub(super) fn backup_create_as(&self, vmid: VmId, user: &str, auto: bool, job: Option<String>) -> Result<String, Error> {
        let guest = self.lock().guest(vmid)?.clone();
        let created = now();
        let id = format!("{}-{vmid}-{created}", guest.kind);
        let (id1, id2, id3) = (id.clone(), id.clone(), id.clone());
        let (me, who, node) = (self.clone(), user.to_string(), guest.node.clone());
        self.run_op(
            vmid,
            user,
            format!("{} backup {vmid}", guest.kind),
            if self.driver().live_backup(guest.kind) { None } else { Some("yedek almak") },
            move |c| match c.backup(&id1) {
                Ok(_) => Err(Error::InvalidSpec("bu konuğun bu saniyede alınmış bir yedeği var; bir saniye sonra yeniden deneyin".to_string())),
                Err(_) => Ok(()),
            },
            // Yerel yedek tamamlanınca dış hedefe de kopyalanır; kopya başarısız olsa da yerel yedek geçerlidir.
            move |d, g| {
                // Kayda, sürücünün gerçekten yedeklediği diskler yazılır: iş başlarken kilitlenen konuğun diskleri
                // (konteynerlerde ek disk yoktur).
                let disks = if g.kind == redox_core::GuestKind::Vm { g.hw.disks.clone() } else { Vec::new() };
                let ids: Vec<u8> = disks.iter().map(|disk| disk.id).collect();
                let disk_gb = g.disk_gb;
                d.backup(g, &id2).map(|size| {
                    let spec = Some(Box::new(g.to_spec()));
                    let machine = d.backup_machine_file(&id2).is_some_and(|path| path.is_file());
                    let pushed = me.push_file(&id2, &ids, false);
                    // Asıl hedefe giden yedek, ek kopya hedeflerine de gider.
                    let copies = if matches!(pushed, Some(Ok(()))) { me.push_copies(&id2, &ids, Some(vmid)) } else { 0 };
                    (size, pushed, disks, disk_gb, machine, spec, copies)
                })
            },
            move |c, (size, pushed, disks, disk_gb, machine, spec, copies)| {
                let offsite = matches!(pushed, Some(Ok(())));
                if let Some(Err(message)) = pushed {
                    c.log(&node, &who, format!("backup push {id3}"), &Err(message), now(), Some(vmid));
                }
                // Yedeği bir iş aldıysa ve işin kilit süresi varsa yedek o süre boyunca silinemez.
                let locked_until = job.as_deref().and_then(|name| c.job(name).ok()).filter(|j| j.lock_days > 0).map(|j| created + u64::from(j.lock_days) * redox_core::retention::DAY_SECS);
                let taken = id3.clone();
                c.add_backup(Backup { id: id3, vmid, guest_name: guest.name, created, size_mb: size.size_mb, auto, offsite, new_mb: size.new_mb, disks, disk_gb, quiesced: size.quiesced, cbt: size.cbt, offsite_bound: offsite, job, machine, spec, suspicious: None, locked_until, tested: None, copies });
                // Olağan dışı değişim başarısız bir görev olarak da kaydedilir: uyarı kanallarına (web kancası,
                // e-posta) böyle ulaşır.
                if let Some(note) = c.backups.iter().find(|b| b.id == taken).and_then(|b| b.suspicious.clone()) {
                    c.log(&node, &who, format!("backup anomaly {taken}"), &Err(note), now(), Some(vmid));
                }
            },
        )?;
        Ok(id)
    }

    /// Yedeği, ait olduğu konuğun diskinin yerine koyar.
    pub fn backup_restore(&self, id: &str, user: &str) -> Result<TaskId, Error> {
        let backup = self.lock().backup(id)?.clone();
        let (kind, vmid) = (self.kind_of(backup.vmid)?, backup.vmid);
        let remotes = self.remotes_for(&backup);
        // Yedekteki ek disklerden konuğa hâlâ takılı olanlar da geri yüklenir. Liste, konuk kilitlenirken
        // (aynı kilit altında) belirlenir: arada disk eklenip çıkarılamaz.
        let chosen = Arc::new(Mutex::new(Vec::<u8>::new()));
        let (pick, wanted, disk_gb) = (chosen.clone(), id.to_string(), backup.disk_gb);
        self.run_op(
            vmid,
            user,
            format!("{kind} restore {vmid} {id}"),
            Some("yedeği geri yüklemek"),
            move |c| {
                c.not_template(vmid)?;
                c.not_suspended(vmid)?;
                *pick.lock().unwrap_or_else(|e| e.into_inner()) = c.restore_disks(&wanted)?;
                Ok(())
            },
            move |d, g| {
                let disks = chosen.lock().unwrap_or_else(|e| e.into_inner()).clone();
                restore_from_any(d, g, &backup, &disks, &remotes)
            },
            move |c, ()| c.after_restore(vmid, disk_gb),
        )
    }

    /// Yedekten yeni bir sanal makine oluşturur: özgün konuğa dokunulmaz (silinmiş de olabilir). Konuğun tanımı
    /// yedekle birlikte saklanan kayıttan, o yoksa (eski yedek) şimdiki konuktan alınır. Yeni konuğun kimliğini döndürür.
    pub fn backup_restore_new(&self, id: &str, name: &str, user: &str) -> Result<VmId, Error> {
        self.restore_new(id, name, user, false)
    }

    /// Anında kurtarma: yedekten, diskleri yedeğin üzerine kurulu yeni bir konuk oluşturur. Kopyalama olmadığı için
    /// konuk hemen başlatılabilir; yedek, konuk ondan ayrılana kadar (`guest_detach`) silinemez.
    pub fn backup_instant(&self, id: &str, name: &str, user: &str) -> Result<VmId, Error> {
        let backup = self.lock().backup(id)?.clone();
        if backup.new_mb.is_some() {
            return Err(Error::InvalidSpec("artımlı yedek parça parça durur; anında açılamaz. \"Yeni konuk olarak aç\" ile geri yükleyin".to_string()));
        }
        let (new, ids) = self.prepare_from_backup(&backup, name, user, false, true)?;
        let guest = self.snapshot().guest(new)?.clone();
        let built = self.driver().instant_from_backup(&guest, &backup.id, &ids);
        self.mutate(|c| c.log(&guest.node, user, format!("vm instant {new} {id}"), &built, now(), Some(new)));
        if let Err(e) = built {
            let _ = self.delete_guest(new, user);
            return Err(Error::Driver(e));
        }
        Ok(new)
    }

    /// Anında kurtarılmış konuğu yedekten bağımsızlaştırır; konuk çalışırken de yapılır.
    pub fn guest_detach(&self, vmid: VmId, user: &str) -> Result<TaskId, Error> {
        let kind = self.kind_of(vmid)?;
        self.run_op(vmid, user, format!("{kind} detach {vmid}"), None, move |c| c.check_detach(vmid), |d, g| d.detach_backing(g), move |c, ()| c.apply_detach(vmid))
    }

    /// Yedeği **açarak** sınar: yedekten ağdan yalıtılmış geçici bir konuk oluşturur, başlatır, bir süre çalışır
    /// halde kaldığını (ve konuk ajanı varsa işletim sisteminin yanıt verdiğini) denetler, sonra konuğu siler.
    /// Sonuç yedeğe ve görev günlüğüne yazılır. Özet doğrulamasından farkı: yalnızca verinin bozulmadığını değil,
    /// yedeğin gerçekten açılan bir konuk verdiğini gösterir.
    pub fn backup_test(&self, id: &str, user: &str) -> Result<(), Error> {
        let backup = self.lock().backup(id)?.clone();
        if !backup.id.starts_with("vm-") {
            return Err(Error::InvalidSpec("yalnızca sanal makine yedekleri açılarak sınanır".to_string()));
        }
        let (me, user) = (self.clone(), user.to_string());
        let wait = *self.0.boot_test_wait.lock().unwrap_or_else(|e| e.into_inner());
        self.dispatch(move || {
            let mut temp: Option<VmId> = None;
            let outcome = (|| -> Result<String, String> {
                let name = format!("sinama-{}", backup.vmid);
                let new = me.restore_new(&backup.id, &name, &user, true).map_err(|e| format!("geçici konuk oluşturulamadı: {e}"))?;
                temp = Some(new);
                me.wait_unlocked(new, Duration::from_secs(6 * 3600))?;
                if me.snapshot().tasks.iter().any(|t| t.vmid == Some(new) && t.command.starts_with("vm restore-new") && t.status == TaskStatus::Failed) {
                    return Err(format!("yedek geri yüklenemedi: {}", me.last_failure(new)));
                }
                // Kapı denetimi istenmişse sunucuda boş bir kapı seçilir ve konuğun kapısına yönlendirilir.
                let wants_port = me.snapshot().guest(new).is_ok_and(|g| g.hw.test_port > 0);
                if wants_port {
                    let free = std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()).map(|a| a.port()).map_err(|e| format!("sınama için boş kapı bulunamadı: {e}"))?;
                    me.mutate(|c| c.set_test_forward(new, free));
                }
                me.run_action(new, Action::Start, &user).map_err(|e| format!("geçici konuk başlatılamadı: {e}"))?;
                if me.wait_unlocked(new, Duration::from_secs(600))?.status != redox_core::GuestStatus::Running {
                    return Err(format!("yedekten açılan konuk başlamadı: {}", me.last_failure(new)));
                }
                std::thread::sleep(wait);
                let guest = me.snapshot().guest(new).map_err(|e| e.to_string())?.clone();
                let alive = me.driver().probe(&guest).is_none_or(|p| p.status == redox_core::GuestStatus::Running);
                let agent = me.driver().guest_agent(&guest);
                if !alive {
                    return Err(format!("yedekten açılan konuk {} saniye içinde kendiliğinden kapandı", wait.as_secs()));
                }
                let mut note = match agent {
                    Some(os) => format!("konuk açıldı ve işletim sistemi yanıt verdi ({os})"),
                    None => format!("konuk {} sn çalıştı; konuk ajanı olmadığı için işletim sisteminin açıldığı doğrulanamadı", wait.as_secs()),
                };
                // İstenen denetimler: hizmet gerçekten ayağa kalktı mı?
                if guest.hw.test_port > 0 {
                    probe_tcp(guest.hw.test_fwd, wait.max(Duration::from_secs(1))).map_err(|e| format!("konuk açıldı ama TCP {} kapısındaki hizmet doğrulanamadı: {e}", guest.hw.test_port))?;
                    note.push_str(&format!("; TCP {} kapısındaki hizmet yanıt verdi", guest.hw.test_port));
                }
                if !guest.hw.test_cmd.is_empty() {
                    me.driver().guest_run(&guest, &guest.hw.test_cmd).map_err(|e| format!("konuk açıldı ama sınama komutu başarısız: {e}"))?;
                    note.push_str("; sınama komutu başarıyla bitti");
                }
                Ok(note)
            })();
            // Geçici konuk her durumda kaldırılır.
            if let Some(new) = temp {
                if me.snapshot().guest(new).is_ok_and(|g| g.status != redox_core::GuestStatus::Stopped) {
                    let _ = me.run_action(new, Action::Stop, &user);
                }
                let _ = me.wait_unlocked(new, Duration::from_secs(600));
                me.refresh(None, None);
                if let Err(e) = me.delete_guest(new, &user) {
                    eprintln!("uyarı: sınama konuğu {new} silinemedi: {e}");
                }
            }
            me.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                let (ok, note) = match &outcome {
                    Ok(note) => (true, note.clone()),
                    Err(why) => (false, why.clone()),
                };
                c.set_backup_test(&backup.id, redox_core::BackupTest { at: now(), ok, note });
                c.log(&node, &user, format!("backup test {}", backup.id), &outcome.map(|_| ()), now(), Some(backup.vmid));
            });
        });
        Ok(())
    }

    /// `isolated`: kopya ağdan yalıtılır ve sunucuya özgü hiçbir şeye (köprü, paylaşım, hız sınırı) bağlanmaz.
    pub(super) fn restore_new(&self, id: &str, name: &str, user: &str, isolated: bool) -> Result<VmId, Error> {
        let backup = self.lock().backup(id)?.clone();
        let (new, ids) = self.prepare_from_backup(&backup, name, user, isolated, false)?;
        let remotes = self.remotes_for(&backup);
        let command = format!("vm restore-new {new} {id}");
        match self.run_op(new, user, command, Some("yedeği geri yüklemek"), |_| Ok(()), move |d, g| restore_from_any(d, g, &backup, &ids, &remotes), |_, ()| {}) {
            Ok(_) => Ok(new),
            Err(e) => {
                // Yarım konuk bırakılmaz.
                let _ = self.delete_guest(new, user);
                Err(e)
            }
        }
    }

    /// Yedekteki tanımdan yeni bir konuk kaydı ve boş diskler hazırlar; yeni kimliği ve yedekteki ek disk
    /// numaralarını döndürür. `instant`: diskler yedeğin üzerine kurulacak (boş ek disk dosyası açılmaz).
    pub(super) fn prepare_from_backup(&self, backup: &Backup, name: &str, user: &str, isolated: bool, instant: bool) -> Result<(VmId, Vec<u8>), Error> {
        if !backup.id.starts_with("vm-") {
            return Err(Error::InvalidSpec("yalnızca sanal makine yedekleri yeni konuk olarak açılır".to_string()));
        }
        let c = self.snapshot();
        let mut spec = match (&backup.spec, c.guest(backup.vmid)) {
            (Some(saved), _) => (**saved).clone(),
            // Eski yedekte tanım yoktur: şimdiki konuğun ayarları kullanılır, ama yalnızca gerçekten aynı konuksa.
            // Silinen konuğun kimliği sonradan başka bir konuğa verilmiş olabilir; ad tutmuyorsa onunki alınmaz.
            (None, Ok(guest)) if guest.name == backup.guest_name => guest.to_spec(),
            (None, _) => {
                return Err(Error::InvalidSpec(
                    "bu yedek eski biçimde alınmış ve konuğu silinmiş: konuğun ayarları bilinmiyor. Aynı ayarlarla bir konuk oluşturup yedeği ona geri yükleyin".to_string(),
                ))
            }
        };
        (spec.vmid, spec.name, spec.onboot, spec.iso) = (None, name.trim().to_string(), false, None);
        if backup.disk_gb > 0 {
            spec.disk_gb = backup.disk_gb;
        }
        // Düğüm ya da depolama alanı artık yoksa bu sunucudaki ilk uygun olan kullanılır.
        if c.node(&spec.node).is_err() {
            spec.node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        }
        if !c.storages.iter().any(|s| s.id == spec.storage && s.available_on(&spec.node)) {
            spec.storage = c.storages.iter().find(|s| s.available_on(&spec.node) && s.content.iter().any(|k| k == "disk")).map(|s| s.id.clone()).unwrap_or_default();
        }
        // Kopya; şablonluk, askı durumu, zamanlama ve ek diskler özgün konuktan devralınmaz (diskler aşağıda eklenir).
        let hw = &mut spec.hw;
        (hw.image, hw.template, hw.suspended, hw.base, hw.restart, hw.disk_seq) = (None, false, false, None, false, 0);
        hw.disks.clear();
        hw.schedules.clear();
        // Kopya, sunucunun fiziksel aygıtlarını kendiliğinden üstlenmez (özgün konuk onları kullanıyor olabilir).
        hw.usb.clear();
        hw.pci.clear();
        if isolated {
            hw.isolated = true;
            hw.nics = hw.nics.iter().map(|_| redox_core::NAT.to_string()).collect();
            (hw.vlan, hw.net_mbps, hw.firewall, hw.balloon_mb) = (0, 0, false, 0);
            hw.shares.clear();
            hw.fw_rules.clear();
            spec.bridge = redox_core::NAT.to_string();
        }
        if instant {
            spec.hw.instant = Some(backup.id.clone());
        }
        let new = self.create_guest(&spec, user)?;
        let prepared = backup.disks.iter().try_for_each(|disk| {
            let guest = self.mutate(|c| {
                c.add_disk_exact(new, disk)?;
                c.guest(new).cloned()
            })?;
            if instant { Ok(()) } else { self.driver().add_disk(&guest, disk.id, disk.size_gb).map_err(Error::Driver) }
        });
        match prepared {
            Ok(()) => Ok((new, backup.disks.iter().map(|d| d.id).collect())),
            Err(e) => {
                let _ = self.delete_guest(new, user);
                Err(e)
            }
        }
    }

    /// Yedeği `days` gün boyunca silinemez yapar (kilit yalnızca uzatılabilir).
    pub fn backup_lock(&self, id: &str, days: u32, user: &str) -> Result<u64, Error> {
        self.mutate(|c| {
            let outcome = c.lock_backup(id, days, now());
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("backup lock {id} {days}"), &outcome.as_ref().map(|_| ()).map_err(|e| e.to_string()), now(), None);
            outcome
        })
    }

    /// Yedekteki diskin ham içeriğinin önbellek dosyası: artımlı çoğaltmada bloklar buradan okunur. İlk
    /// istekte üretilir, yedek silinince kaldırılır.
    pub fn backup_raw_cache(&self, id: &str, disk: u8) -> Result<std::path::PathBuf, Error> {
        let backup = self.lock().backup(id)?.clone();
        if self.driver().backups_sealed() {
            // Ham önbellek, şifreli deponun yanında yedeğin açık bir kopyasını bırakırdı.
            return Err(Error::InvalidSpec("yedekler şifreli saklanıyor; artımlı aktarım için açık kopya tutulmaz".to_string()));
        }
        if disk != 0 && !backup.disks.iter().any(|d| d.id == disk) {
            return Err(Error::InvalidSpec(format!("bu yedekte {disk} numaralı ek disk yok")));
        }
        let cache = self.raw_cache_path(id, disk).ok_or_else(|| Error::InvalidSpec("bu sürücü yedek dosyası tutmuyor (gerçek sürücü gerekir)".to_string()))?;
        if !cache.is_file() {
            let building = cache.with_extension("hazirlaniyor");
            self.driver().backup_raw(id, disk, &building).and_then(|()| std::fs::rename(&building, &cache).map_err(|e| format!("{} yerine konamadı: {e}", cache.display()))).map_err(|e| {
                let _ = std::fs::remove_file(&building);
                Error::Driver(e)
            })?;
        }
        Ok(cache)
    }

    fn raw_cache_path(&self, id: &str, disk: u8) -> Option<std::path::PathBuf> {
        let base = self.driver().backup_file(id)?;
        Some(base.parent()?.join(format!(".{id}.disk{disk}.ham")))
    }

    /// Yedekteki olağan dışı değişim uyarısını kapatır; kimin kapattığı görev günlüğüne yazılır.
    pub fn backup_ack(&self, id: &str, user: &str) -> Result<(), Error> {
        self.mutate(|c| {
            c.clear_backup_suspicion(id)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("backup ack {id}"), &Ok(()), now(), None);
            Ok(())
        })
    }

    /// Yedekleri kendiliğinden açarak sınamayı ayarlar: her konuğun en yeni yedeği `every_days` günde bir,
    /// `minute_of_day` saatinden başlayarak sırayla sınanır. 0 gün kapatır.
    pub fn set_auto_test(&self, every_days: u32, minute_of_day: u16) {
        *self.0.auto_test.lock().unwrap_or_else(|e| e.into_inner()) = (every_days, minute_of_day);
    }

    /// Zamanlanmış sınama turu (dakikada bir çağrılır): başlama saatinden sonraki dört saat içinde, o an
    /// süren bir sınama yoksa sırası gelen bir yedeği sınamaya başlar. Başlattığı yedeğin adını döndürür.
    pub fn auto_test_tick(&self, minute_of_day: u16) -> Option<String> {
        const WINDOW_MIN: u16 = 240;
        if self.in_maintenance() {
            return None;
        }
        let (every, start) = *self.0.auto_test.lock().unwrap_or_else(|e| e.into_inner());
        if every == 0 || minute_of_day < start || minute_of_day >= start.saturating_add(WINDOW_MIN) {
            return None;
        }
        let c = self.snapshot();
        // Sınama konukları yalıtılmış açılır; biri duruyorsa sınama sürüyordur (aynı anda tek sınama).
        if c.guests.iter().any(|g| g.hw.isolated) {
            return None;
        }
        let id = c.next_backup_to_test(now(), every)?;
        match self.backup_test(&id, SCHEDULER) {
            Ok(()) => Some(id),
            Err(e) => {
                eprintln!("zamanlanmış yedek sınaması başlatılamadı ({id}): {e}");
                None
            }
        }
    }

    pub fn backup_delete(&self, id: &str, user: &str) -> Result<(), Error> {
        // Kilitli yedek, süresi dolana kadar kimse tarafından silinemez (zamanlayıcının budaması dahil).
        self.lock().backup_deletable(id, now())?;
        let backup = self.lock().backup(id)?.clone();
        let others = self.offsite_others(id);
        let outcome = self.driver().delete_backup(id);
        // Artımlı çoğaltma için üretilmiş ham önbellekler yedekle birlikte gider.
        for disk in std::iter::once(0).chain(backup.disks.iter().map(|d| d.id)) {
            if let Some(cache) = self.raw_cache_path(id, disk) {
                let _ = std::fs::remove_file(cache);
            }
        }
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            if outcome.is_ok() {
                let _ = c.remove_backup(id);
            }
            c.log(&node, user, format!("backup delete {}", backup.id), &outcome, now(), None);
        });
        // Dış kopya arka planda silinir (süren bir göndermeyi beklemesi gerekebilir; istek bunu beklemez).
        // Silinemezse yerel silme yine geçerlidir ve hata görev günlüğüne yazılır.
        let targets = if outcome.is_ok() { self.remotes_for(&backup) } else { Vec::new() };
        let primary = self.offsite();
        let archived: Vec<String> = self.lock().archive.iter().map(|b| b.id.clone()).collect();
        let mut kept = false;
        for config in targets {
            let copy = !primary.as_ref().is_some_and(|p| Arc::ptr_eq(p, &config));
            // Saklama süresi olan kopya hedefi yedeği tutar: yedek arşive alınır, süresi dolunca oradan da silinir.
            if copy && config.keeps(backup.created, now()) {
                kept = true;
                continue;
            }
            // Kopya hedefinde arşivdeki yedeklerin parçaları da durmalıdır.
            let mut others = others.clone();
            if copy {
                others.extend(archived.iter().cloned());
            }
            self.unpush_from(config, backup.clone(), others, user.to_string());
        }
        if kept {
            self.mutate(|c| c.archive_backup(backup.clone()));
        }
        outcome.map_err(Error::Driver)
    }

    /// Yedeğin bir dış hedefteki kopyasını arka planda siler (süren bir göndermeyi beklemesi gerekebilir).
    /// Parçalı dış kopyada yalnızca bu yedeğe özgü parçalar silinir (`others`: parçaları durması gereken
    /// öbür yedekler); yoksa disk dosyaları tek tek. Silinemezse hata görev günlüğüne yazılır.
    pub(super) fn unpush_from(&self, config: Arc<crate::offsite::Config>, backup: Backup, others: Vec<String>, user: String) {
        let me = self.clone();
        self.dispatch(move || {
            let remote = match me.driver().unpush_backup(&backup.id, &others, &*config) {
                Ok(true) => Ok(()),
                Ok(false) => std::iter::once(0)
                    .chain(backup.disks.iter().map(|d| d.id))
                    .filter_map(|disk| me.driver().backup_disk_file(&backup.id, disk))
                    .try_for_each(|path| config.delete(&path)),
                Err(e) => Err(e),
            };
            let remote = remote.and_then(|()| match me.driver().backup_machine_file(&backup.id) {
                Some(path) if backup.machine => config.delete(&path),
                _ => Ok(()),
            });
            if remote.is_err() {
                me.mutate(|c| {
                    let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                    c.log(&node, &user, format!("backup unpush {}", backup.id), &remote, now(), None);
                });
            }
        });
    }

    /// Arşivde saklama süresi dolan yedekleri kopya hedeflerinden siler; silinen kayıt sayısını döndürür.
    /// Düzenli aralıkla çağrılır; süresi dolan yoksa hedefe hiç gidilmez.
    pub fn archive_tick(&self) -> usize {
        let copies = self.offsite_copies();
        let expired: Vec<Backup> = self.lock().archive.iter().filter(|b| !copies.iter().any(|c| c.keeps(b.created, now()))).cloned().collect();
        for backup in &expired {
            self.mutate(|c| {
                c.drop_archived(&backup.id);
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, SCHEDULER, format!("backup archive-expire {}", backup.id), &Ok(()), now(), None);
            });
            let mut others = self.offsite_others(&backup.id);
            others.extend(self.lock().archive.iter().map(|b| b.id.clone()));
            for config in &copies {
                self.unpush_from(config.clone(), backup.clone(), others.clone(), SCHEDULER.to_string());
            }
        }
        expired.len()
    }

    /// Arşivdeki yedeği yedek listesine geri getirir. Dosyaları kopya hedefinde durur; geri yükleme ya da
    /// yedekten yeni konuk açma sırasında oradan indirilir.
    pub fn backup_recover(&self, id: &str, user: &str) -> Result<(), Error> {
        if self.offsite_copies().is_empty() {
            return Err(Error::InvalidSpec("kopya hedefi tanımlı değil: arşivdeki yedek indirilemez (--offsite-copy)".to_string()));
        }
        self.mutate(|c| {
            c.unarchive_backup(id)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("backup recover {id}"), &Ok(()), now(), None);
            Ok(())
        })
    }
}
