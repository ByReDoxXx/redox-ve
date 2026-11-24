use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redox_core::{Action, Backup, Cluster, Error, Guest, GuestPatch, GuestSpec, Hypervisor, SnapshotOp, TaskId, TaskStatus, VmId};

use crate::auth::{write_private, Auth};
use crate::host::HostSample;

/// Grafiklerde tutulan örnek sayısı (5 sn aralıkla 10 dakika).
pub const HISTORY_LEN: usize = 120;

pub type History = HashMap<String, Vec<f64>>;

/// Zamanlayıcının görev günlüğündeki adı.
pub const SCHEDULER: &str = "zamanlayıcı";
/// Sunucu açılışında konukları başlatan iç kullanıcı.
pub const BOOT: &str = "açılış";
/// Beklenmedik biçimde kapanan konukları yeniden başlatan iç kullanıcı.
pub const HA: &str = "gözcü";

/// Sunucunun yerel saatiyle bugünün tarihi (`YYYY-AA-GG`) ve gece yarısından itibaren dakika.
pub fn local_day_minute() -> (String, u16) {
    use chrono::Timelike;
    let t = chrono::Local::now();
    (t.format("%Y-%m-%d").to_string(), (t.hour() * 60 + t.minute()) as u16)
}

/// Yerel gün, haftanın günü (0 Pazartesi … 6 Pazar) ve gece yarısından beri geçen dakika.
pub fn local_now() -> (String, u8, u16) {
    use chrono::{Datelike, Timelike};
    let t = chrono::Local::now();
    (t.format("%Y-%m-%d").to_string(), t.weekday().num_days_from_monday() as u8, (t.hour() * 60 + t.minute()) as u16)
}

/// `export_backup`'ın bu çağrı için ürettiği geçici kopyayı ve (boşsa) dizinini siler.
pub fn discard_export(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sunucunun paylaşılan durumu: küme, sürücü, kimlik doğrulama ve kalıcılık ayarı.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    cluster: Mutex<Cluster>,
    driver: Box<dyn Hypervisor>,
    task_delay: Duration,
    state_file: Option<PathBuf>,
    auth: Auth,
    alerts: crate::alerts::Alerts,
    ha: Mutex<crate::ha::Watch>,
    peers: crate::peers::Peers,
    /// Yedeklerin sunucu dışı hedefi; yapılandırılmadıysa `None`.
    offsite: Mutex<Option<Arc<crate::offsite::Config>>>,
    /// Tek oturum açma; yapılandırılmadıysa `None`.
    oidc: Mutex<Option<Arc<crate::oidc::Oidc>>>,
    /// Gerçek sürücüden toplanan ölçüm geçmişi (anahtar: `node:cpu`, `guest:100:mem` …).
    history: Mutex<HashMap<String, VecDeque<f64>>>,
    /// Dakikalık ve saatlik ortalamalar (diske yazılır).
    long: Mutex<crate::history::Long>,
}

impl AppState {
    pub fn new(
        cluster: Cluster,
        driver: Box<dyn Hypervisor>,
        task_delay: Duration,
        state_file: Option<PathBuf>,
        auth: Auth,
    ) -> Self {
        AppState(Arc::new(Inner {
            cluster: Mutex::new(cluster),
            driver,
            task_delay,
            state_file,
            auth,
            alerts: crate::alerts::Alerts::default(),
            ha: Mutex::new(Default::default()),
            peers: crate::peers::Peers::default(),
            offsite: Mutex::new(None),
            oidc: Mutex::new(None),
            history: Mutex::new(HashMap::new()),
            long: Mutex::new(Default::default()),
        }))
    }

    pub fn auth(&self) -> &Auth {
        &self.0.auth
    }

    pub fn alerts(&self) -> &crate::alerts::Alerts {
        &self.0.alerts
    }

    pub fn set_oidc(&self, config: crate::oidc::Config) {
        *self.0.oidc.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(crate::oidc::Oidc::new(config)));
    }

    pub fn oidc(&self) -> Option<Arc<crate::oidc::Oidc>> {
        self.0.oidc.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_offsite(&self, config: crate::offsite::Config) {
        *self.0.offsite.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(config));
    }

    pub fn offsite(&self) -> Option<Arc<crate::offsite::Config>> {
        self.0.offsite.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Dış kopyası olan diğer yedeklerin kimlikleri.
    fn offsite_others(&self, id: &str) -> Vec<String> {
        self.lock().backups.iter().filter(|b| b.offsite && b.id != id).map(|b| b.id.clone()).collect()
    }

    /// Yedeği dış hedefe kopyalar. Hedef yapılandırılmadıysa `None`. Artımlı yedek parça parça gider
    /// (yalnızca hedefte olmayan kısım); diğerleri tek dosya olarak. `recheck`: diğer yedeklerden
    /// "hedefte vardır" diye bilinen parçalar da hedefe sorulur (elle yeniden gönderme).
    fn push_file(&self, id: &str, disks: &[u8], recheck: bool) -> Option<Result<(), String>> {
        let config = self.offsite()?;
        let known = if recheck { Vec::new() } else { self.offsite_others(id) };
        match self.driver().push_backup(id, &known, &*config) {
            Ok(Some(_)) => return Some(Ok(())),
            Ok(None) => {}
            Err(e) => return Some(Err(e)),
        }
        // Tam yedek: asıl disk ve her ek disk ayrı dosya olarak gider.
        let mut sent: Vec<std::path::PathBuf> = Vec::new();
        for disk in std::iter::once(0).chain(disks.iter().copied()) {
            let pushed = match self.driver().export_backup(id, disk) {
                Ok(Some((path, temporary))) => {
                    let pushed = config.push(&path).map(|_| ());
                    if temporary {
                        discard_export(&path);
                    }
                    pushed.map(|()| path)
                }
                Ok(None) => Err("bu sürücü yedek dosyası tutmuyor".to_string()),
                Err(e) => Err(e),
            };
            match pushed {
                Ok(path) => sent.push(path),
                Err(e) => {
                    // Yarım dış kopya bırakılmaz: gönderilmiş dosyalar geri alınır (kayıt "dış kopya yok" kalır).
                    for path in &sent {
                        let _ = config.delete(path);
                    }
                    return Some(Err(e));
                }
            }
        }
        Some(Ok(()))
    }

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
            me.mutate(|c| {
                let node = c.guest(backup.vmid).map(|g| g.node.clone()).ok().or_else(|| c.nodes.first().map(|n| n.id.clone())).unwrap_or_default();
                if outcome.is_ok() {
                    c.set_backup_offsite(&backup.id, true);
                }
                c.log(&node, &user, format!("backup push {}", backup.id), &outcome, now(), Some(backup.vmid));
            });
        });
        Ok(())
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
            let outcome = me.import_now(&peer, &peer_name, vmid, &image, &image_path, &user);
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

    fn import_now(&self, peer: &crate::peers::Peer, peer_name: &str, vmid: VmId, image: &str, image_path: &Path, user: &str) -> Result<VmId, String> {
        let remote = peer.guest(vmid)?;
        if remote.kind != redox_core::GuestKind::Vm {
            return Err("yalnızca sanal makineler taşınabilir (konteyner taşıma yok)".to_string());
        }
        if !remote.hw.disks.is_empty() {
            return Err("ek diskli sanal makine taşınamaz: taşıma yalnızca asıl diski kopyalar".to_string());
        }
        // Çalışan (ya da zaten askıda olan) konuk bellek durumuyla birlikte taşınır: kaynakta askıya alınır,
        // diski ve belleği kopyalanır, burada kaldığı yerden devam eder. Kesinti kopyalama süresi kadardır.
        let live = remote.status != redox_core::GuestStatus::Stopped;
        let carry_state = live || remote.hw.suspended;
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
        if live {
            peer.suspend_and_wait(vmid, Duration::from_secs(1800))?;
        }
        let backup = peer.backup_and_wait(vmid, Duration::from_secs(6 * 3600))?;
        if let Some(parent) = image_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{} oluşturulamadı: {e}", parent.display()))?;
        }
        let downloaded = peer.download_backup(&backup, image_path);
        // Eşte bıraktığımız geçici yedek her durumda silinir.
        let _ = peer.delete_backup(&backup);
        downloaded?;

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
        }
        hw.suspended = false;
        let spec = GuestSpec {
            // Bellek durumuyla gelen konuk, kimliği boşsa aynı kimliği alır: ağ kartının adresi kimlikten türetilir.
            vmid: Some(remote.vmid).filter(|id| carry_state && local.guest(*id).is_err()),
            name: remote.name.clone(),
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
        let id = self.create_guest(&spec, user).map_err(|e| e.to_string())?;
        // İmaj yalnızca taşıma içindi; kayıtta kalırsa silinmiş bir dosyayı gösterir.
        self.mutate(|c| {
            if let Ok(g) = c.guest_mut_for_import(id) {
                g.hw.image = None;
                g.tags = remote.tags.clone();
                g.tags.push(format!("tasindi-{peer_name}"));
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

    /// Kaynaktaki bellek durumunu indirir, yeni konuğun askı kaydı yapar ve (çalışırken taşınıyorsa) konuğu sürdürür.
    fn adopt_state(&self, peer: &crate::peers::Peer, remote_id: VmId, id: VmId, scratch: &Path, resume: bool, user: &str) -> Result<(), String> {
        let file = scratch.with_extension("bellek");
        let (running, layout) = peer.download_state(remote_id, &file)?;
        let guest = self.snapshot().guest(id).map_err(|e| e.to_string())?.clone();
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

    /// Uyarı turu: son turdan beri değişen koşulları ve olayları döndürür.
    pub fn alerts_tick(&self) -> Vec<crate::alerts::Event> {
        let snapshot = self.snapshot();
        self.0.alerts.update(&snapshot, now())
    }

    pub fn driver(&self) -> &dyn Hypervisor {
        self.0.driver.as_ref()
    }

    pub fn driver_name(&self) -> &'static str {
        self.0.driver.name()
    }

    fn lock(&self) -> MutexGuard<'_, Cluster> {
        // Zehirlenmiş kilit: veri hâlâ tutarlıdır (her değişiklik tek metotta biter).
        self.0.cluster.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> Cluster {
        self.lock().clone()
    }

    /// Konuğun etiketleri (yetki denetimi için; bütün kümeyi kopyalamadan).
    pub fn guest_tags(&self, vmid: VmId) -> Option<Vec<String>> {
        self.lock().guest(vmid).ok().map(|g| g.tags.clone())
    }

    /// Kümeyi değiştirir ve ardından diske yazar.
    pub fn mutate<R>(&self, f: impl FnOnce(&mut Cluster) -> R) -> R {
        let mut cluster = self.lock();
        let result = f(&mut cluster);
        self.persist(&cluster);
        result
    }

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
        if let Some(iso) = &spec.iso {
            if driver.is_real() && !driver.isos().iter().any(|i| i == iso) {
                return Err(Error::InvalidSpec(format!("ISO bulunamadı: {iso}")));
            }
        }
        if let Some(image) = &spec.hw.image {
            if driver.is_real() && !driver.images().iter().any(|i| i == image) {
                return Err(Error::InvalidSpec(format!("bulut imajı bulunamadı: {image}")));
            }
        }
        self.check_firewall(spec.hw.firewall)?;
        self.check_machine(spec.hw.firmware == redox_core::hw::Firmware::Uefi, spec.hw.tpm)?;
        if driver.is_real() {
            self.check_bridge(&spec.bridge)?;
            let templates = driver.templates();
            if spec.kind == redox_core::GuestKind::Ct && !templates.is_empty() && !templates.contains(&spec.os) {
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
        self.driver().destroy(&guest).map_err(Error::Driver)?;
        self.mutate(|c| c.delete_guest(vmid, user, now()))?;
        let _ = self.sync_firewall();
        Ok(())
    }

    /// Eylemi doğrular ve görevi başlatır. Gecikme sıfırsa (testler) görev dönmeden
    /// önce biter; değilse arka planda, küme kilidi tutulmadan tamamlanır.
    pub fn run_action(&self, vmid: VmId, action: Action, user: &str) -> Result<TaskId, Error> {
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

    fn finish(&self, task: TaskId, vmid: VmId, action: Action) {
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

    fn record(&self, key: String, value: f64) {
        let mut history = self.0.history.lock().unwrap_or_else(|e| e.into_inner());
        let series = history.entry(key).or_default();
        series.push_back(value);
        while series.len() > HISTORY_LEN {
            series.pop_front();
        }
    }

    /// Uzun süreli ölçümlerin durduğu dosya: durum dosyasının yanında.
    fn metrics_path(&self) -> Option<PathBuf> {
        self.0.state_file.as_ref().map(|p| p.with_file_name("metrics.json"))
    }

    /// Kayıtlı uzun süreli ölçümleri yükler (başlangıçta bir kez).
    pub fn load_metrics(&self) {
        if let Some(path) = self.metrics_path() {
            *self.0.long.lock().unwrap_or_else(|e| e.into_inner()) = crate::history::Long::load(&path);
        }
    }

    pub fn save_metrics(&self) {
        if let Some(path) = self.metrics_path() {
            if let Err(e) = self.0.long.lock().unwrap_or_else(|e| e.into_inner()).save(&path) {
                eprintln!("uyarı: {e}");
            }
        }
    }

    /// Seçilen aralığın ölçümleri: son 10 dakika ham örneklerden, daha uzunu ortalamalardan.
    pub fn history_for(&self, range: crate::history::Range) -> History {
        match range {
            crate::history::Range::Recent => self.history(),
            other => self.0.long.lock().unwrap_or_else(|e| e.into_inner()).series(other),
        }
    }

    /// Ölçüm turu: `refresh` gerçek saatle bunu çağırır; testler zamanı kendileri verir.
    pub fn record_long(&self, samples: &[(String, f64)], at: u64) {
        let save = {
            let mut long = self.0.long.lock().unwrap_or_else(|e| e.into_inner());
            let rolled = long.roll(at);
            for (key, value) in samples {
                long.push(key, *value);
            }
            let live: Vec<String> = samples.iter().map(|(k, _)| k.clone()).collect();
            long.retain(&live);
            // On dakikada bir diske yaz: her dakika yazmak diski boşuna yıpratır.
            rolled && (at / 60).is_multiple_of(10)
        };
        if save {
            self.save_metrics();
        }
    }

    pub fn history(&self) -> History {
        let history = self.0.history.lock().unwrap_or_else(|e| e.into_inner());
        history.iter().map(|(k, v)| (k.clone(), v.iter().copied().collect())).collect()
    }

    /// Gerçek sürücüden konukların, düğümün ve depolamanın güncel durumunu okur.
    /// Bloklayan G/Ç yapar; `spawn_blocking` içinden çağrılmalıdır.
    pub fn refresh(&self, host: Option<&HostSample>, disk_gb: Option<(u64, u64)>) {
        let driver = self.driver();
        if !driver.is_real() {
            return;
        }
        let snapshot = self.snapshot();
        let probes: Vec<_> = snapshot.guests.iter().filter_map(|g| driver.probe(g).map(|p| (g.vmid, p))).collect();
        // Adres yalnızca çalışan konukta anlamlıdır; duran konuğun eski adresi silinir.
        let ips: Vec<_> = probes
            .iter()
            .zip(&snapshot.guests)
            .map(|((vmid, p), g)| (*vmid, if p.status == redox_core::GuestStatus::Stopped { None } else { driver.guest_ip(g) }))
            .collect();
        let agents: Vec<_> = probes
            .iter()
            .zip(&snapshot.guests)
            .map(|((vmid, p), g)| (*vmid, if p.status == redox_core::GuestStatus::Stopped { None } else { driver.guest_agent(g) }))
            .collect();
        let node = snapshot.nodes.first().map(|n| n.id.clone()).unwrap_or_default();

        let mut cluster = self.lock();
        let mut changed = false;
        for (vmid, probe) in &probes {
            changed |= cluster.observe_guest(*vmid, probe, now());
        }
        for (vmid, ip) in ips {
            changed |= cluster.observe_ip(vmid, ip);
        }
        for (vmid, agent) in agents {
            changed |= cluster.observe_agent(vmid, agent);
        }
        if let Some(h) = host {
            cluster.observe_node(&node, h.cpu_pct, h.mem_used_mb, h.mem_total_mb, h.uptime_secs);
        }
        if let Some((used, total)) = disk_gb {
            cluster.observe_storage(crate::host::LOCAL_STORAGE, &node, used, total);
        }
        let samples: Vec<(String, f64)> = cluster
            .guests
            .iter()
            .flat_map(|g| {
                [
                    (format!("guest:{}:cpu", g.vmid), g.cpu_pct as f64),
                    (format!("guest:{}:mem", g.vmid), g.mem_pct() as f64),
                ]
            })
            .chain(cluster.nodes.first().into_iter().flat_map(|n| {
                [
                    ("node:cpu".to_string(), n.cpu_pct as f64),
                    ("node:mem".to_string(), redox_core::fmt::pct(n.mem_used_mb, n.mem_total_mb) as f64),
                ]
            }))
            .collect();
        // Yalnızca durum değiştiyse diske yaz; ölçümler her 5 saniyede dosyayı yıpratmasın.
        if changed {
            self.persist(&cluster);
        }
        drop(cluster);

        self.record_long(&samples, now());
        let live: Vec<String> = samples.iter().map(|(k, _)| k.clone()).collect();
        for (key, value) in samples {
            self.record(key, value);
        }
        // Silinen konukların geçmişi tutulmaz.
        self.0.history.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| live.contains(k));
    }

    fn persist(&self, cluster: &Cluster) {
        let Some(path) = &self.0.state_file else { return };
        if let Err(e) = write_atomically(path, cluster) {
            eprintln!("uyarı: durum {} dosyasına yazılamadı: {e}", path.display());
        }
    }
}

fn write_atomically(path: &Path, cluster: &Cluster) -> std::io::Result<()> {
    // Durum dosyası yalnızca hizmet kullanıcısınca okunabilir (0600).
    write_private(path, &serde_json::to_vec_pretty(cluster)?)
}

/// Durum dosyasını okur. Dosya yoksa `Ok(None)`; bozuksa hata döner (üzerine yazılmaz).
pub fn load_state(path: &Path) -> Result<Option<Cluster>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{} okunamadı: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} açılamadı: {e}", path.display())),
    }
}

/// Önceki süreçten yarım kalan görevleri başarısız sayar ve konukların kilidini açar.
/// Döndürdüğü sayı, kapatılan görev sayısıdır.
pub fn recover(cluster: &mut Cluster, now: u64) -> usize {
    let stale: Vec<TaskId> = cluster
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Running)
        .map(|t| t.id)
        .collect();
    for id in &stale {
        let _ = cluster.complete_task(*id, Err("sunucu yeniden başlatıldı; görev yarıda kaldı".into()), now);
    }
    stale.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Cost, UserFile};
    use redox_core::{GuestKind, GuestStatus, Probe, SimDriver};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn auth() -> Auth {
        Auth::new(UserFile::default(), None, false, Cost::Test)
    }

    fn spec(name: &str, kind: GuestKind) -> GuestSpec {
        GuestSpec {
            vmid: None,
            name: name.into(),
            kind,
            node: "pve-ist-01".into(),
            cores: 1,
            memory_mb: 512,
            disk_gb: 10,
            storage: "local-zfs".into(),
            os: "linux".into(),
            bridge: "nat".into(),
            onboot: false,
            iso: None,
            hw: Default::default(),
        }
    }

    /// Gerçek sürücü gibi davranan, ayarlanabilir sahte sürücü.
    #[derive(Default, Clone)]
    struct FakeReal {
        fail_create: bool,
        fail_destroy: bool,
        reported: Arc<Mutex<Option<Probe>>>,
        destroyed: Arc<AtomicBool>,
    }

    impl Hypervisor for FakeReal {
        fn name(&self) -> &'static str {
            "sahte"
        }
        fn is_real(&self) -> bool {
            true
        }
        fn supports(&self, kind: GuestKind) -> bool {
            kind == GuestKind::Vm
        }
        fn isos(&self) -> Vec<String> {
            vec!["debian.iso".into()]
        }
        fn bridges(&self) -> Vec<String> {
            vec!["vmbr0".into()]
        }
        fn templates(&self) -> Vec<String> {
            vec!["alpine-3.20".into()]
        }
        fn guest_ip(&self, g: &Guest) -> Option<String> {
            Some(format!("10.0.0.{}", g.vmid % 250))
        }
        fn configure(&self, _: &Guest) -> Result<(), String> {
            if self.fail_create { Err("yapılandırma yazılamadı".into()) } else { Ok(()) }
        }
        fn create(&self, _: &Guest) -> Result<(), String> {
            if self.fail_create { Err("disk yazılamadı".into()) } else { Ok(()) }
        }
        fn destroy(&self, _: &Guest) -> Result<(), String> {
            if self.fail_destroy {
                return Err("dosya kilitli".into());
            }
            self.destroyed.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
            Ok(())
        }
        fn probe(&self, _: &Guest) -> Option<Probe> {
            self.reported.lock().unwrap().clone()
        }
    }

    fn real(fake: FakeReal) -> (AppState, FakeReal) {
        let state = AppState::new(Cluster::demo(1000), Box::new(fake.clone()), Duration::ZERO, None, auth());
        (state, fake)
    }

    #[test]
    fn recover_fails_stale_tasks_and_unlocks_guests() {
        let mut c = Cluster::demo(1000);
        let t = c.begin_action(103, Action::Start, "u", 1000).unwrap();
        assert_eq!(recover(&mut c, 2000), 1);
        assert_eq!(c.task(t).unwrap().status, TaskStatus::Failed);
        let g = c.guest(103).unwrap();
        assert_eq!((g.status, g.lock), (GuestStatus::Stopped, None));
        assert_eq!(recover(&mut c, 2000), 0, "ikinci kez yapılacak iş kalmaz");
    }

    #[test]
    fn state_file_round_trip_and_errors() {
        let dir = std::env::temp_dir().join(format!("redox-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        assert_eq!(load_state(&path), Ok(None), "dosya yoksa hata değil");

        let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::default()), Duration::ZERO, Some(path.clone()), auth());
        state.run_action(103, Action::Start, "admin").unwrap();
        let loaded = load_state(&path).unwrap().expect("değişiklikten sonra dosya yazılmalı");
        assert_eq!(loaded, state.snapshot());
        assert_eq!(loaded.guest(103).unwrap().status, GuestStatus::Running);

        std::fs::write(&path, b"{ bozuk").unwrap();
        assert!(load_state(&path).is_err(), "bozuk dosya sessizce yok sayılmamalı");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn driver_failure_marks_task_failed() {
        let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::failing_on(103)), Duration::ZERO, None, auth());
        let t = state.run_action(103, Action::Start, "admin").unwrap();
        let c = state.snapshot();
        assert_eq!(c.task(t).unwrap().status, TaskStatus::Failed);
        assert_eq!(c.guest(103).unwrap().status, GuestStatus::Stopped);
    }

    #[test]
    fn create_checks_driver_support_and_iso() {
        let (state, _) = real(FakeReal::default());
        let before = state.snapshot();
        let err = state.create_guest(&spec("kutu", GuestKind::Ct), "admin").unwrap_err();
        assert!(matches!(&err, Error::InvalidSpec(m) if m.contains("konteyner türünü desteklemiyor")), "{err}");
        let missing = GuestSpec { iso: Some("yok.iso".into()), ..spec("vm", GuestKind::Vm) };
        assert_eq!(state.create_guest(&missing, "admin"), Err(Error::InvalidSpec("ISO bulunamadı: yok.iso".into())));
        assert_eq!(state.snapshot(), before);

        let ok = GuestSpec { iso: Some("debian.iso".into()), ..spec("vm", GuestKind::Vm) };
        let id = state.create_guest(&ok, "admin").unwrap();
        assert_eq!(state.snapshot().guest(id).unwrap().iso.as_deref(), Some("debian.iso"));
    }

    #[test]
    fn failed_disk_creation_is_rolled_back() {
        let (state, _) = real(FakeReal { fail_create: true, ..Default::default() });
        let storages = state.snapshot().storages;
        assert_eq!(state.create_guest(&spec("vm", GuestKind::Vm), "admin"), Err(Error::Driver("disk yazılamadı".into())));
        let c = state.snapshot();
        assert_eq!(c.guests.len(), 8, "yarım kalan konuk listede kalmamalı");
        assert_eq!(c.storages, storages);
        assert_eq!((c.tasks[0].status, c.tasks[0].message.as_deref()), (TaskStatus::Failed, Some("disk yazılamadı")));
    }

    #[test]
    fn delete_removes_driver_resources_only_when_allowed() {
        let (state, fake) = real(FakeReal::default());
        assert_eq!(state.delete_guest(100, "admin"), Err(Error::NotStopped(100)));
        assert!(!fake.destroyed.load(Ordering::SeqCst), "çalışan konuğun diski silinmemeli");
        state.delete_guest(103, "admin").unwrap();
        assert!(fake.destroyed.load(Ordering::SeqCst));
        assert!(state.snapshot().guest(103).is_err());

        let (state, _) = real(FakeReal { fail_destroy: true, ..Default::default() });
        assert_eq!(state.delete_guest(103, "admin"), Err(Error::Driver("dosya kilitli".into())));
        assert!(state.snapshot().guest(103).is_ok(), "sürücü silemediyse kayıt da kalır");
    }

    #[test]
    fn refresh_applies_probes_and_records_bounded_history() {
        let (state, fake) = real(FakeReal::default());
        *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Running, cpu_pct: Some(42), mem_used_mb: Some(256) });
        let host = HostSample { cpu_pct: 17, mem_used_mb: 4096, mem_total_mb: 8192, uptime_secs: 99 };

        state.refresh(Some(&host), Some((30, 100)));
        let c = state.snapshot();
        assert!(c.guests.iter().all(|g| g.status == GuestStatus::Running && g.cpu_pct == 42));
        let n = &c.nodes[0];
        assert_eq!((n.cpu_pct, n.mem_used_mb, n.mem_total_mb, n.uptime_secs), (17, 4096, 8192, 99));

        for _ in 0..HISTORY_LEN + 30 {
            state.refresh(Some(&host), None);
        }
        let h = state.history();
        assert_eq!(h["node:cpu"].len(), HISTORY_LEN, "geçmiş sınırlı kalmalı");
        assert_eq!(h["node:cpu"].last(), Some(&17.0));
        assert_eq!(h["node:mem"].last(), Some(&50.0));
        assert_eq!(h["guest:103:cpu"].last(), Some(&42.0));

        state.delete_guest(202, "admin").unwrap_err(); // çalışıyor göründüğü için silinemez
        *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None });
        state.refresh(None, None);
        state.delete_guest(202, "admin").unwrap();
        state.refresh(None, None);
        assert!(!state.history().contains_key("guest:202:cpu"), "silinen konuğun geçmişi atılır");
    }

    #[test]
    fn real_driver_validates_network_settings() {
        let (state, _) = real(FakeReal::default());
        let bad = GuestSpec { bridge: "br-yok".into(), ..spec("vm", GuestKind::Vm) };
        assert!(matches!(state.create_guest(&bad, "admin"), Err(Error::InvalidSpec(m)) if m.contains("adında bir köprü yok") && m.contains("vmbr0")));
        let id = state.create_guest(&GuestSpec { bridge: "vmbr0".into(), ..spec("vm", GuestKind::Vm) }, "admin").unwrap();
        assert_eq!(state.snapshot().guest(id).unwrap().bridge, "vmbr0");

        // Köprüdeki konukta port yönlendirme anlamsızdır; NAT'a geçince kabul edilir.
        let forwards = Some(vec![redox_core::PortForward { host_port: 8080, guest_port: 80 }]);
        let err = state.update_guest(id, &GuestPatch { forwards: forwards.clone(), ..Default::default() }, "admin").unwrap_err();
        assert!(matches!(&err, Error::InvalidSpec(m) if m.contains("yalnızca NAT")), "{err}");
        state.update_guest(id, &GuestPatch { bridge: Some("nat".into()), forwards, ..Default::default() }, "admin").unwrap();
        let err = state.update_guest(id, &GuestPatch { bridge: Some("vmbr0".into()), ..Default::default() }, "admin").unwrap_err();
        assert!(matches!(&err, Error::InvalidSpec(m) if m.contains("yalnızca NAT")), "yönlendirme varken köprüye geçilemez: {err}");
        assert!(matches!(state.update_guest(id, &GuestPatch { bridge: Some("br-yok".into()), ..Default::default() }, "admin"), Err(Error::InvalidSpec(_))));
    }

    #[test]
    fn refresh_tracks_addresses_of_running_guests_only() {
        let (state, fake) = real(FakeReal::default());
        *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Running, cpu_pct: None, mem_used_mb: None });
        state.refresh(None, None);
        assert_eq!(state.snapshot().guest(103).unwrap().ip.as_deref(), Some("10.0.0.103"));
        *fake.reported.lock().unwrap() = Some(Probe { status: GuestStatus::Stopped, cpu_pct: None, mem_used_mb: None });
        state.refresh(None, None);
        assert_eq!(state.snapshot().guest(103).unwrap().ip, None, "duran konuğun eski adresi silinir");
    }

    #[test]
    fn driver_reconfigure_failure_is_reported() {
        let (state, _) = real(FakeReal { fail_create: true, ..Default::default() });
        let err = state.update_guest(103, &GuestPatch { cores: Some(2), ..Default::default() }, "admin").unwrap_err();
        assert_eq!(err, Error::Driver("yapılandırma yazılamadı".into()));
    }

    #[test]
    fn exec_needs_a_running_guest_and_hides_the_command() {
        let (state, _) = real(FakeReal::default());
        assert!(matches!(state.exec(103, "id", "admin"), Err(Error::InvalidSpec(m)) if m.contains("çalışmıyor")));
        assert_eq!(state.exec(999, "id", "admin"), Err(Error::GuestNotFound(999)));
        // Sahte sürücü komut çalıştırmayı desteklemez: hata sürücüden gelir ve görev günlüğüne düşer.
        assert!(matches!(state.exec(200, "cat /root/gizli-parola", "ayse"), Err(Error::Driver(_))));
        let t = state.snapshot().tasks[0].clone();
        assert_eq!((t.command.as_str(), t.user.as_str(), t.status), ("ct exec 200", "ayse", TaskStatus::Failed));
        assert!(!format!("{:?}", state.snapshot().tasks).contains("gizli-parola"), "komut metni günlüğe yazılmaz");
    }

    #[test]
    fn refresh_is_a_no_op_for_the_simulator() {
        let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::default()), Duration::ZERO, None, auth());
        let before = state.snapshot();
        state.refresh(Some(&HostSample { cpu_pct: 99, mem_used_mb: 1, mem_total_mb: 2, uptime_secs: 3 }), Some((1, 2)));
        assert_eq!(state.snapshot(), before);
        assert!(state.history().is_empty());
    }
}

/// Yaşam döngüsü dışındaki konuk işlemleri: kurallar çekirdekte, dosya işleri sürücüde.
impl AppState {
    /// Uzun süren bir işlemi görev olarak yürütür: konuğu kilitler, sürücüyü kilit dışında
    /// çalıştırır, sonucu görev günlüğüne yazar. `done` başarıda kümeye değişikliği işler.
    #[allow(clippy::too_many_arguments)]
    fn run_op<T: Send + 'static>(
        &self,
        vmid: VmId,
        user: &str,
        command: String,
        need_stopped: Option<&'static str>,
        check: impl FnOnce(&Cluster) -> Result<(), Error>,
        work: impl FnOnce(&dyn Hypervisor, &Guest) -> Result<T, String> + Send + 'static,
        done: impl FnOnce(&mut Cluster, T) + Send + 'static,
    ) -> Result<TaskId, Error> {
        let (task, guest) = self.mutate(|c| {
            check(c)?;
            let task = c.begin_op(vmid, command, user, now(), need_stopped)?;
            Ok::<_, Error>((task, c.guest(vmid)?.clone()))
        })?;
        let me = self.clone();
        let job = move || {
            let result = work(me.driver(), &guest);
            me.mutate(|c| match result {
                Ok(value) => {
                    let _ = c.complete_task(task, Ok(()), now());
                    done(c, value);
                }
                Err(message) => {
                    let _ = c.complete_task(task, Err(message), now());
                }
            });
        };
        self.dispatch(job);
        Ok(task)
    }

    /// İşi hemen (testler) ya da arka planda, bloklayan iş parçacığında çalıştırır.
    fn dispatch(&self, job: impl FnOnce() + Send + 'static) {
        if self.0.task_delay.is_zero() {
            job();
        } else {
            let delay = self.0.task_delay;
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = tokio::task::spawn_blocking(job).await;
            });
        }
    }

    /// Ayarları değiştirir. Gerçek sürücüde takılacak ISO'nun var olduğu da denetlenir.
    pub fn update_guest(&self, vmid: VmId, patch: &GuestPatch, user: &str) -> Result<(), Error> {
        let driver = self.driver();
        if let Some(iso) = patch.iso.as_deref().filter(|i| !i.is_empty()) {
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
        if patch.shares.as_ref().is_some_and(|s| !s.is_empty()) && !driver.shared_folders() {
            return Err(Error::InvalidSpec("bu sunucudaki QEMU paylaşılan klasörü (9p) desteklemiyor".to_string()));
        }
        self.check_machine(patch.firmware == Some(redox_core::hw::Firmware::Uefi), patch.tpm == Some(true))?;
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

    /// Gerçek sürücüde UEFI ve TPM için sunucuda ilgili paketler kurulu olmalıdır.
    fn check_machine(&self, uefi: bool, tpm: bool) -> Result<(), Error> {
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

    /// Gerçek sürücüde güvenlik duvarı ancak sunucu `--firewall` ile başlatıldıysa açılabilir.
    fn check_firewall(&self, wanted: bool) -> Result<(), Error> {
        if wanted && self.driver().is_real() && !self.driver().firewall() {
            return Err(Error::InvalidSpec("konuk güvenlik duvarı kapalı: sunucuyu --firewall ile başlatın (nftables ve CAP_NET_ADMIN gerekir)".to_string()));
        }
        Ok(())
    }

    /// Bütün konukların güvenlik duvarı kurallarını yeniden yükler. Başarısızlık görev günlüğüne yazılır.
    pub fn sync_firewall(&self) -> Result<(), String> {
        if !self.driver().firewall() {
            return Ok(());
        }
        let guests = self.lock().guests.clone();
        let outcome = self.driver().apply_firewall(&guests);
        if outcome.is_err() {
            self.mutate(|c| {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, SCHEDULER, "firewall apply".to_string(), &outcome, now(), None);
            });
        }
        outcome
    }

    /// Gerçek sürücüde ağ ayarı `nat` ya da sunucuda var olan bir köprü olmalıdır.
    fn check_bridge(&self, bridge: &str) -> Result<(), Error> {
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

    pub fn snapshot_create(&self, vmid: VmId, name: &str, user: &str) -> Result<TaskId, Error> {
        let (kind, n1, n2, n3) = (self.kind_of(vmid)?, name.to_string(), name.to_string(), name.to_string());
        self.run_op(
            vmid,
            user,
            format!("{kind} snapshot {vmid} {name}"),
            self.stopped_unless_live(kind, "anlık görüntü almak"),
            move |c| c.not_template(vmid).and_then(|()| c.check_snapshot_new(vmid, &n1)),
            move |d, g| d.snapshot(g, SnapshotOp::Create(&n2)),
            move |c, ()| c.add_snapshot(vmid, &n3, now()),
        )
    }

    pub fn snapshot_rollback(&self, vmid: VmId, name: &str, user: &str) -> Result<TaskId, Error> {
        let (kind, n1, n2) = (self.kind_of(vmid)?, name.to_string(), name.to_string());
        self.run_op(
            vmid,
            user,
            format!("{kind} rollback {vmid} {name}"),
            self.stopped_unless_live(kind, "anlık görüntüye dönmek"),
            move |c| c.not_template(vmid).and_then(|()| c.not_suspended(vmid)).and_then(|()| c.check_snapshot(vmid, &n1)),
            move |d, g| d.snapshot(g, SnapshotOp::Rollback(&n2)),
            |_, ()| {},
        )
    }

    pub fn snapshot_delete(&self, vmid: VmId, name: &str, user: &str) -> Result<TaskId, Error> {
        let (kind, n1, n2, n3) = (self.kind_of(vmid)?, name.to_string(), name.to_string(), name.to_string());
        self.run_op(
            vmid,
            user,
            format!("{kind} delsnapshot {vmid} {name}"),
            self.stopped_unless_live(kind, "anlık görüntü silmek"),
            move |c| c.not_template(vmid).and_then(|()| c.check_snapshot(vmid, &n1)),
            move |d, g| d.snapshot(g, SnapshotOp::Delete(&n2)),
            move |c, ()| c.remove_snapshot(vmid, &n3),
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

    /// Diskin yedeğini alır; yedeğin kimliğini döndürür.
    pub fn backup_create(&self, vmid: VmId, user: &str) -> Result<String, Error> {
        self.backup_create_as(vmid, user, false)
    }

    /// Zamanlayıcının bir turu: saati gelen otomatik yedekleri başlatır ve saklama sayısını
    /// aşan eski otomatik yedekleri siler. Başlatılan yedek sayısını döndürür.
    pub fn scheduler_tick(&self, today: &str, minute_of_day: u16) -> usize {
        let due = self.lock().auto_backups_due(today, minute_of_day);
        for vmid in &due {
            // Önce işaretle: başarısız olsa da aynı gün her turda yeniden denenmesin.
            self.mutate(|c| c.mark_auto_backup(*vmid, today));
            if let Err(e) = self.backup_create_as(*vmid, SCHEDULER, true) {
                self.mutate(|c| {
                    if let Ok(g) = c.guest(*vmid).cloned() {
                        c.log(&g.node, SCHEDULER, format!("{} backup {vmid}", g.kind), &Err(e.to_string()), now(), Some(*vmid));
                    }
                });
            }
        }
        // Kilit döngüden önce bırakılmalı: silme işlemi kümeyi yeniden kilitler.
        let stale = self.lock().prunable_backups();
        for id in stale {
            let _ = self.backup_delete(&id, SCHEDULER);
        }
        due.len()
    }

    /// Yeniden başlatma turu: "yeniden başlat" işaretli, beklenmedik biçimde kapanmış konukları açar.
    /// Başlatılan konuk sayısını döndürür. Vazgeçilen konuk görev günlüğüne hata olarak yazılır.
    pub fn ha_tick(&self) -> usize {
        let snapshot = self.snapshot();
        let decisions = self.0.ha.lock().unwrap_or_else(|e| e.into_inner()).update(&snapshot, now());
        let mut started = 0;
        for decision in decisions {
            let (vmid, outcome) = match decision {
                crate::ha::Decision::Restart(vmid) => match self.run_action(vmid, Action::Start, HA) {
                    Ok(_) => {
                        started += 1;
                        continue;
                    }
                    Err(e) => (vmid, Err(e.to_string())),
                },
                crate::ha::Decision::GiveUp(vmid) => (
                    vmid,
                    Err(format!("konuk {} dakika içinde {} kez kendiliğinden kapandı; yeniden başlatmaktan vazgeçildi", crate::ha::RESTART_WINDOW_SECS / 60, crate::ha::MAX_RESTARTS + 1)),
                ),
            };
            self.mutate(|c| {
                if let Ok(g) = c.guest(vmid).cloned() {
                    c.log(&g.node, HA, format!("{} start {vmid}", g.kind), &outcome, now(), Some(vmid));
                }
            });
        }
        started
    }

    /// Ek depolama alanlarının doluluğunu günceller (`(ad, dolu GB, toplam GB)`); durum dosyası yazılmaz.
    pub fn observe_storages(&self, usage: &[(String, u64, u64)]) {
        let mut cluster = self.lock();
        let node = cluster.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        for (id, used, total) in usage {
            cluster.observe_storage(id, &node, *used, *total);
        }
    }

    /// Zamanlayıcının güç turu: saati gelen başlat, kapat ve yeniden başlat işlemlerini çalıştırır.
    /// Konuk zaten istenen durumdaysa işlem sessizce atlanır. Başlatılan görev sayısını döndürür.
    pub fn power_tick(&self, today: &str, weekday: u8, minute_of_day: u16) -> usize {
        let due = self.lock().schedules_due(today, weekday, minute_of_day);
        let mut started = 0;
        for (vmid, index, action) in due {
            // Önce işaretle: başarısız olsa da aynı gün her turda yeniden denenmesin.
            self.mutate(|c| c.mark_schedule(vmid, index, today));
            let Ok(guest) = self.lock().guest(vmid).cloned() else { continue };
            if action.target(guest.status).is_none() {
                continue;
            }
            match self.run_action(vmid, action, SCHEDULER) {
                Ok(_) => started += 1,
                Err(e) => self.mutate(|c| c.log(&guest.node, SCHEDULER, format!("{} {action} {vmid}", guest.kind), &Err(e.to_string()), now(), Some(vmid))),
            }
        }
        started
    }

    /// Sunucu açılışı: "açılışta başlat" işaretli, kapalı konukları sırasıyla başlatır. Her konuğun başlatma görevi
    /// bitince o konuğun gecikmesi kadar beklenir. `pause` beklemeyi yapar (testlerde kaydedilir).
    /// Başlatılan konuk sayısını döndürür; başlatılamayan konuk görev günlüğüne yazılır ve sıra sürer.
    pub fn autostart(&self, pause: &dyn Fn(Duration)) -> usize {
        let plan = self.lock().boot_plan();
        let mut started = 0;
        for (i, (vmid, delay)) in plan.iter().enumerate() {
            if let Err(e) = self.run_action(*vmid, Action::Start, BOOT) {
                self.mutate(|c| {
                    if let Ok(g) = c.guest(*vmid).cloned() {
                        c.log(&g.node, BOOT, format!("{} start {vmid}", g.kind), &Err(e.to_string()), now(), Some(*vmid));
                    }
                });
                continue;
            }
            // Başlatma görevi bitene kadar (en çok iki dakika) beklenir; gecikme ondan sonra sayılır.
            for _ in 0..600 {
                if self.lock().guest(*vmid).map(|g| g.lock.is_none()).unwrap_or(true) {
                    break;
                }
                pause(Duration::from_millis(200));
            }
            // Açılamayan konuk için beklenmez: görevi başarısız olarak günlükte durur.
            if !self.lock().guest(*vmid).is_ok_and(|g| g.status == redox_core::GuestStatus::Running) {
                continue;
            }
            started += 1;
            if *delay > 0 && i + 1 < plan.len() {
                pause(Duration::from_secs(u64::from(*delay)));
            }
        }
        started
    }

    fn backup_create_as(&self, vmid: VmId, user: &str, auto: bool) -> Result<String, Error> {
        let guest = self.lock().guest(vmid)?.clone();
        let created = now();
        let id = format!("{}-{vmid}-{created}", guest.kind);
        let (id1, id2, id3) = (id.clone(), id.clone(), id.clone());
        let (me, who, node) = (self.clone(), user.to_string(), guest.node.clone());
        self.run_op(
            vmid,
            user,
            format!("{} backup {vmid}", guest.kind),
            self.stopped_unless_live(guest.kind, "yedek almak"),
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
                d.backup(g, &id2).map(|size| (size, me.push_file(&id2, &ids, false), disks, disk_gb))
            },
            move |c, (size, pushed, disks, disk_gb)| {
                let offsite = matches!(pushed, Some(Ok(())));
                if let Some(Err(message)) = pushed {
                    c.log(&node, &who, format!("backup push {id3}"), &Err(message), now(), Some(vmid));
                }
                c.add_backup(Backup { id: id3, vmid, guest_name: guest.name, created, size_mb: size.size_mb, auto, offsite, new_mb: size.new_mb, disks, disk_gb, quiesced: size.quiesced, cbt: size.cbt, offsite_bound: offsite });
            },
        )?;
        Ok(id)
    }

    /// Yedeği, ait olduğu konuğun diskinin yerine koyar.
    pub fn backup_restore(&self, id: &str, user: &str) -> Result<TaskId, Error> {
        let backup = self.lock().backup(id)?.clone();
        let (kind, vmid, id1) = (self.kind_of(backup.vmid)?, backup.vmid, id.to_string());
        let remote = if backup.offsite { self.offsite() } else { None };
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
                // Yerel dosya yoksa (disk değişmiş olabilir) dış kopya önce geri indirilir.
                if let Some(config) = &remote {
                    let fetch = |disk: u8| match d.backup_disk_file(&id1, disk) {
                        Some(path) if !path.exists() => {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            // Bu sürümle gönderilmiş kopya adına bağlıdır: hedefte başka bir yedeğin dosyasıyla
                            // değiştirilmişse geri yükleme reddedilir.
                            config.pull_bound(&path, backup.offsite_bound)
                        }
                        _ => Ok(()),
                    };
                    if backup.new_mb.is_some() {
                        // Parçalı dış kopya: yalnızca yerelde eksik parçalar iner (bütün diskler tek dizindedir).
                        if !d.backup_present(&id1) && !d.pull_backup(&id1, &**config)? {
                            fetch(0)?;
                        }
                    } else {
                        // Tam yedek: yerelde eksik olan her disk dosyası indirilir.
                        for disk in std::iter::once(0).chain(disks.iter().copied()) {
                            fetch(disk)?;
                        }
                    }
                }
                match (d.restore(g, &id1, &disks), &remote) {
                    // Yerel parça bozuk ya da eksikse (geri yükleme hiçbir diske dokunmadan durmuştur) dış kopyadan
                    // onarılıp bir kez daha denenir.
                    (Err(first), Some(config)) if backup.new_mb.is_some() => match d.pull_backup(&id1, &**config) {
                        Ok(true) => d.restore(g, &id1, &disks),
                        _ => Err(first),
                    },
                    (outcome, _) => outcome,
                }
            },
            move |c, ()| c.after_restore(vmid, disk_gb),
        )
    }

    pub fn backup_delete(&self, id: &str, user: &str) -> Result<(), Error> {
        let backup = self.lock().backup(id)?.clone();
        let others = self.offsite_others(id);
        let outcome = self.driver().delete_backup(id);
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            if outcome.is_ok() {
                let _ = c.remove_backup(id);
            }
            c.log(&node, user, format!("backup delete {}", backup.id), &outcome, now(), None);
        });
        // Dış kopya arka planda silinir (süren bir göndermeyi beklemesi gerekebilir; istek bunu beklemez).
        // Silinemezse yerel silme yine geçerlidir ve hata görev günlüğüne yazılır.
        if let (true, Some(config)) = (outcome.is_ok() && backup.offsite, self.offsite()) {
            let (me, user) = (self.clone(), user.to_string());
            self.dispatch(move || {
                // Parçalı dış kopyada yalnızca bu yedeğe özgü parçalar silinir; yoksa disk dosyaları tek tek.
                let remote = match me.driver().unpush_backup(&backup.id, &others, &*config) {
                    Ok(true) => Ok(()),
                    Ok(false) => std::iter::once(0)
                        .chain(backup.disks.iter().map(|d| d.id))
                        .filter_map(|disk| me.driver().backup_disk_file(&backup.id, disk))
                        .try_for_each(|path| config.delete(&path)),
                    Err(e) => Err(e),
                };
                if remote.is_err() {
                    me.mutate(|c| {
                        let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                        c.log(&node, &user, format!("backup unpush {}", backup.id), &remote, now(), None);
                    });
                }
            });
        }
        outcome.map_err(Error::Driver)
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

    /// Sürücü canlı disk işlemi yapabiliyorsa konuğun durdurulması gerekmez.
    fn stopped_unless_live(&self, kind: redox_core::GuestKind, what: &'static str) -> Option<&'static str> {
        if self.driver().live_disk_ops(kind) { None } else { Some(what) }
    }

    fn kind_of(&self, vmid: VmId) -> Result<redox_core::GuestKind, Error> {
        Ok(self.lock().guest(vmid)?.kind)
    }
}

#[cfg(test)]
mod op_tests {
    use super::*;
    use crate::auth::{Cost, UserFile};
    use redox_core::{GuestStatus, PortForward, SimDriver};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Disk işlemleri istenince başarısız olabilen sürücü.
    #[derive(Default, Clone)]
    struct Flaky {
        fail: Arc<AtomicBool>,
    }

    impl Flaky {
        fn outcome<T: Default>(&self) -> Result<T, String> {
            if self.fail.load(Ordering::SeqCst) { Err("disk hatası".into()) } else { Ok(T::default()) }
        }
    }

    impl Hypervisor for Flaky {
        fn name(&self) -> &'static str {
            "oynak"
        }
        fn apply(&self, _: &Guest, _: Action) -> Result<(), String> {
            Ok(())
        }
        fn snapshot(&self, _: &Guest, _: SnapshotOp<'_>) -> Result<(), String> {
            self.outcome()
        }
        fn resize_disk(&self, _: &Guest, _: u64) -> Result<(), String> {
            self.outcome()
        }
        fn clone_disk(&self, _: &Guest, _: &Guest) -> Result<(), String> {
            self.outcome()
        }
        fn backup(&self, _: &Guest, _: &str) -> Result<redox_core::BackupSize, String> {
            self.outcome().map(|()| redox_core::BackupSize::full(42))
        }
        fn restore(&self, _: &Guest, _: &str, _: &[u8]) -> Result<(), String> {
            self.outcome()
        }
        fn delete_backup(&self, _: &str) -> Result<(), String> {
            self.outcome()
        }
    }

    fn setup() -> (AppState, Flaky) {
        let driver = Flaky::default();
        let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
        (AppState::new(Cluster::demo(1000), Box::new(driver.clone()), Duration::ZERO, None, auth), driver)
    }

    fn last_task(state: &AppState) -> (String, TaskStatus, Option<String>) {
        let t = state.snapshot().tasks[0].clone();
        (t.command, t.status, t.message)
    }

    #[test]
    fn snapshot_lifecycle_and_failures() {
        let (state, driver) = setup();
        assert!(matches!(state.snapshot_create(100, "x", "u"), Err(Error::RequiresStopped { .. })), "çalışan konukta alınmaz");
        assert!(matches!(state.snapshot_create(103, "Kötü", "u"), Err(Error::InvalidSpec(_))));

        state.snapshot_create(103, "temiz", "ayse").unwrap();
        assert_eq!(state.snapshot().guest(103).unwrap().snapshots[0].name, "temiz");
        assert_eq!(last_task(&state), ("vm snapshot 103 temiz".into(), TaskStatus::Ok, None));
        assert_eq!(state.snapshot_create(103, "temiz", "u"), Err(Error::SnapshotExists("temiz".into())));
        assert_eq!(state.snapshot_rollback(103, "yok", "u"), Err(Error::SnapshotNotFound("yok".into())));
        state.snapshot_rollback(103, "temiz", "u").unwrap();

        // Sürücü başarısız olursa kayıt değişmez, görev hatayı taşır, kilit açılır.
        driver.fail.store(true, Ordering::SeqCst);
        state.snapshot_create(103, "ikinci", "u").unwrap();
        state.snapshot_delete(103, "temiz", "u").unwrap();
        let g = state.snapshot().guest(103).unwrap().clone();
        assert_eq!((g.snapshots.len(), g.lock), (1, None));
        assert_eq!(last_task(&state), ("vm delsnapshot 103 temiz".into(), TaskStatus::Failed, Some("disk hatası".into())));

        driver.fail.store(false, Ordering::SeqCst);
        state.snapshot_delete(103, "temiz", "u").unwrap();
        assert!(state.snapshot().guest(103).unwrap().snapshots.is_empty());
    }

    #[test]
    fn resize_applies_only_on_success() {
        let (state, driver) = setup();
        assert!(matches!(state.resize_disk(103, 100, "u"), Err(Error::InvalidSpec(_))));
        driver.fail.store(true, Ordering::SeqCst);
        state.resize_disk(103, 300, "u").unwrap();
        assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 200, "başarısız büyütme kaydı değiştirmez");
        driver.fail.store(false, Ordering::SeqCst);
        state.resize_disk(103, 300, "u").unwrap();
        assert_eq!(state.snapshot().guest(103).unwrap().disk_gb, 300);
        assert_eq!(last_task(&state).0, "vm resize 103 300G");
    }

    #[test]
    fn backup_restore_and_delete() {
        let (state, driver) = setup();
        assert!(matches!(state.backup_create(100, "u"), Err(Error::RequiresStopped { .. })));
        state.snapshot_create(103, "s", "u").unwrap();

        let id = state.backup_create(103, "ayse").unwrap();
        assert!(id.starts_with("vm-103-"), "{id}");
        let c = state.snapshot();
        let b = c.backup(&id).unwrap();
        assert_eq!((b.vmid, b.guest_name.as_str(), b.size_mb), (103, "win-build", 42));
        assert!(matches!(state.backup_create(103, "u"), Err(Error::InvalidSpec(_))), "aynı saniyede ikinci yedek kimliği çakışır");

        assert_eq!(state.backup_restore("yok", "u"), Err(Error::BackupNotFound("yok".into())));
        state.backup_restore(&id, "u").unwrap();
        assert!(state.snapshot().guest(103).unwrap().snapshots.is_empty(), "geri yükleme anlık görüntü listesini sıfırlar");

        driver.fail.store(true, Ordering::SeqCst);
        assert_eq!(state.backup_delete(&id, "u"), Err(Error::Driver("disk hatası".into())));
        assert!(state.snapshot().backup(&id).is_ok(), "dosya silinemediyse kayıt da kalır");
        driver.fail.store(false, Ordering::SeqCst);
        state.backup_delete(&id, "u").unwrap();
        assert!(state.snapshot().backups.is_empty());
        assert_eq!(last_task(&state).0, format!("backup delete {id}"));
        assert_eq!(state.backup_delete(&id, "u"), Err(Error::BackupNotFound(id)));
    }

    #[test]
    fn verification_outcome_is_logged() {
        let (state, _) = setup();
        assert_eq!(state.backup_verify("yok", "u"), Err(Error::BackupNotFound("yok".into())));
        let id = state.backup_create(103, "ayse").unwrap();
        state.backup_verify(&id, "ayse").unwrap();
        let (command, status, message) = last_task(&state);
        // Bu sürücü doğrulamayı desteklemez: sessizce "sağlam" denmez, başarısız görev olarak yazılır.
        assert_eq!((command, status), (format!("backup verify {id}"), TaskStatus::Failed));
        assert!(message.unwrap().contains("doğrulama yok"));
    }

    #[test]
    fn scheduler_takes_daily_backups_and_prunes_old_ones() {
        let (state, driver) = setup();
        let schedule = GuestPatch { backup_time: Some("03:30".into()), backup_keep: Some(2), ..Default::default() };
        state.update_guest(103, &schedule, "admin").unwrap();

        assert_eq!(state.scheduler_tick("2026-11-24", 200), 0, "saati gelmedi");
        assert_eq!(state.scheduler_tick("2026-11-24", 215), 1);
        assert_eq!(state.scheduler_tick("2026-11-24", 216), 0, "aynı gün ikinci kez alınmaz");
        let c = state.snapshot();
        assert_eq!((c.backups.len(), c.backups[0].auto, c.backups[0].vmid), (1, true, 103));
        assert!(c.tasks.iter().any(|t| t.command == "vm backup 103" && t.user == SCHEDULER));

        // Elle alınan yedek saklama sayısına dahil değildir.
        std::thread::sleep(Duration::from_millis(1100));
        let manual = state.backup_create(103, "ayse").unwrap();
        for day in ["2026-11-25", "2026-11-26", "2026-11-27"] {
            std::thread::sleep(Duration::from_millis(1100));
            assert_eq!(state.scheduler_tick(day, 300), 1, "{day}");
        }
        state.scheduler_tick("2026-11-27", 301);
        let c = state.snapshot();
        let autos: Vec<&str> = c.backups.iter().filter(|b| b.auto).map(|b| b.id.as_str()).collect();
        assert_eq!(autos.len(), 2, "yalnızca en yeni iki otomatik yedek kalır: {autos:?}");
        assert!(c.backup(&manual).is_ok(), "elle alınan yedek silinmez");
        assert!(c.tasks.iter().any(|t| t.command.starts_with("backup delete vm-103-") && t.user == SCHEDULER));

        // Çalışan konuk (bu sürücüde canlı yedek yok): hata günlüğe düşer, gün içinde yinelenmez.
        state.update_guest(100, &schedule, "admin").unwrap();
        assert_eq!(state.scheduler_tick("2026-11-28", 300), 2);
        let c = state.snapshot();
        let failed = c.tasks.iter().find(|t| t.command == "vm backup 100").expect("başarısız deneme kaydedilmeli");
        assert_eq!((failed.status, failed.user.as_str()), (TaskStatus::Failed, SCHEDULER));
        assert!(failed.message.as_deref().unwrap().contains("önce konuğu kapatın"));
        assert_eq!(state.scheduler_tick("2026-11-28", 400), 0);

        // Sürücü hatasında da yeniden deneme fırtınası olmaz.
        driver.fail.store(true, Ordering::SeqCst);
        assert_eq!(state.scheduler_tick("2026-11-29", 300), 2);
        assert_eq!(state.scheduler_tick("2026-11-29", 301), 0);
    }

    #[test]
    fn local_time_helper_is_well_formed() {
        let (day, minute) = local_day_minute();
        assert_eq!(day.len(), 10);
        assert!(day.as_bytes()[4] == b'-' && day.as_bytes()[7] == b'-' && minute < 1440);
    }

    #[test]
    fn clone_creates_or_cleans_up() {
        let (state, driver) = setup();
        assert!(matches!(state.clone_guest(100, "kopya", "u"), Err(Error::RequiresStopped { .. })));
        assert_eq!(state.clone_guest(103, "Kötü", "u"), Err(Error::InvalidName("Kötü".into())));
        assert_eq!(state.snapshot().guest(103).unwrap().lock, None, "reddedilen klon kaynağı kilitli bırakmaz");

        let id = state.clone_guest(103, "win-build-2", "u").unwrap();
        let c = state.snapshot();
        assert_eq!((c.guest(id).unwrap().name.as_str(), c.guest(id).unwrap().lock, c.guest(103).unwrap().lock), ("win-build-2", None, None));

        driver.fail.store(true, Ordering::SeqCst);
        let storages = state.snapshot().storages;
        let failed = state.clone_guest(103, "ucuncu", "u").unwrap();
        let c = state.snapshot();
        assert!(c.guest(failed).is_err(), "disk kopyalanamadıysa hedef kayıt silinir");
        assert_eq!(c.storages, storages);
        assert_eq!(c.tasks[0].status, TaskStatus::Failed);
    }

    #[test]
    fn update_and_console_delegation() {
        let auth = Auth::new(UserFile::default(), None, false, Cost::Test);
        let state = AppState::new(Cluster::demo(1000), Box::new(SimDriver::default()), Duration::ZERO, None, auth);
        let patch = GuestPatch { tags: Some(vec!["ci".into()]), forwards: Some(vec![PortForward { host_port: 8080, guest_port: 80 }]), ..Default::default() };
        state.update_guest(103, &patch, "u").unwrap();
        let g = state.snapshot().guest(103).unwrap().clone();
        assert_eq!((g.tags, g.forwards.len(), g.status), (vec!["ci".to_string()], 1, GuestStatus::Stopped));
        // Simülasyon sürücüsünde konsol yoktur; hata sürücü hatası olarak döner.
        assert!(matches!(state.screenshot(103), Err(Error::Driver(_))));
        assert!(matches!(state.send_keys(103, &[]), Err(Error::Driver(_))));
        assert_eq!(state.screenshot(999), Err(Error::GuestNotFound(999)));
    }
}
