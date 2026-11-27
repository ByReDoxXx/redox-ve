//! Canlı taşıma: çalışan sanal makine, kapatılmadan bir eş sunucudan bu sunucuya geçer.
//!
//! Taşımayı **hedef** yönetir (öbür aktarımlar gibi çekme düzeni): eşteki konuğun kaydını okur, aynı kimlik ve
//! donanımla boş diskli bir konuk oluşturur, QEMU'sunu taşıma bekler halde açar ve eşe "bana gönder" der. Eş
//! (kaynak) disklerini aynalar, belleği taşır ve konuğu kendi tarafında kapatır; hedef konuğu sürdürür.
//! İki QEMU birbirine doğrudan bağlanır: hedefin `--migrate-ports` aralığı kaynaktan erişilebilir olmalıdır.
//! Bağlantılar o taşıma için üretilen, eşe yönetim bağlantısı üzerinden iletilen tek kullanımlık anahtarla şifrelenir.

use super::*;

/// Hedefin taşıma için açtığı kapıların varsayılan aralığı (her taşıma iki kapı kullanır).
pub const MIGRATE_PORTS: (u16, u16) = (49152, 49215);

/// Canlı taşıma ayarları (sunucu seçenekleri).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateSettings {
    /// Taşıma kapılarının aralığı (uçlar dahil).
    pub ports: (u16, u16),
    /// Kaynağın bu sunucuya bağlanacağı adres; verilmezse kaynak, isteğin geldiği adresi kullanır.
    pub host: Option<String>,
    /// Kapıların dinlediği adres.
    pub bind: String,
}

impl Default for MigrateSettings {
    fn default() -> Self {
        MigrateSettings { ports: MIGRATE_PORTS, host: None, bind: "0.0.0.0".to_string() }
    }
}

/// `--migrate-ports` değeri: `İLK-SON`, ayrıcalıksız kapılar, en az iki kapı.
pub fn parse_migrate_ports(text: &str) -> Result<(u16, u16), String> {
    let bad = || format!("geçersiz kapı aralığı \"{text}\": İLK-SON biçiminde, 1024-65535 arasında ve en az iki kapı olmalı (örn. 49152-49215)");
    let (first, last) = text.split_once('-').ok_or_else(bad)?;
    let (first, last): (u16, u16) = (first.trim().parse().map_err(|_| bad())?, last.trim().parse().map_err(|_| bad())?);
    if first < 1024 || last <= first {
        return Err(bad());
    }
    Ok((first, last))
}

/// Aralıktan şu an boş olan tek bir kapı seçer.
pub(super) fn free_port(range: (u16, u16), bind: &str) -> Result<u16, String> {
    (range.0..=range.1).find(|port| std::net::TcpListener::bind((bind, *port)).is_ok()).ok_or_else(|| format!("boş kapı yok ({}-{}); aralığı --migrate-ports ile genişletin", range.0, range.1))
}

/// Aralıktan şu an boş olan `count` ayrı kapı seçer.
pub(super) fn free_port_set(range: (u16, u16), bind: &str, count: usize) -> Result<Vec<u16>, String> {
    let found: Vec<u16> = (range.0..=range.1).filter(|port| std::net::TcpListener::bind((bind, *port)).is_ok()).take(count).collect();
    if found.len() == count {
        Ok(found)
    } else {
        Err(format!("yeterince boş kapı yok ({count} gerekli, {}-{}); aralığı --migrate-ports ile genişletin", range.0, range.1))
    }
}

/// Aralıktan şu an boş olan iki kapı seçer.
fn free_ports(range: (u16, u16), bind: &str) -> Result<(u16, u16), String> {
    let mut found = Vec::new();
    for port in range.0..=range.1 {
        if std::net::TcpListener::bind((bind, port)).is_ok() {
            found.push(port);
            if found.len() == 2 {
                return Ok((found[0], found[1]));
            }
        }
    }
    Err(format!("taşıma için boş kapı yok ({}-{}); süren taşımalar bitince yeniden deneyin ya da aralığı --migrate-ports ile genişletin", range.0, range.1))
}

/// Tek kullanımlık taşıma anahtarı.
pub(super) fn new_key() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("taşıma anahtarı üretilemedi: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Kapı seçimi ile QEMU'nun kapıları açması arasına ikinci bir taşıma girmesin.
pub(super) static PICK: Mutex<()> = Mutex::new(());

impl AppState {
    pub fn set_migrate(&self, settings: MigrateSettings) {
        *self.0.migrate.lock().unwrap_or_else(|e| e.into_inner()) = settings;
    }

    pub fn migrate_settings(&self) -> MigrateSettings {
        self.0.migrate.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Kaynak: konuk buradan canlı taşınabilir mi? Taşınabiliyorsa hedefin açması gereken donanım yerleşimi.
    pub fn migrate_prepare(&self, vmid: VmId) -> Result<u8, Error> {
        let guest = {
            let c = self.lock();
            c.check_migrate_out(vmid)?;
            c.guest(vmid)?.clone()
        };
        let driver = self.driver();
        if !driver.live_migration() {
            return Err(Error::InvalidSpec("bu sürücü canlı taşımayı desteklemiyor (gerçek sürücü gerekir)".to_string()));
        }
        driver.migrate_layout(&guest).ok_or_else(|| Error::InvalidSpec(format!("{vmid} çalışmıyor (QEMU süreci yok)")))
    }

    /// Kaynak: konuğu, taşımayı bekleyen hedefe gönderir (görev olarak). Bitince konuk burada kapalıdır.
    pub fn migrate_out(&self, vmid: VmId, host: &str, nbd_port: u16, ram_port: u16, key: &str, user: &str) -> Result<TaskId, Error> {
        self.send_guest(vmid, host, Some(nbd_port), ram_port, key, None, user)
    }

    /// Kaynak: diskleri ortak depoda olan konuğun yalnızca belleğini gönderir; bellek ulaşınca disk sahipliği
    /// `to` sunucusuna geçer.
    pub fn migrate_out_shared(&self, vmid: VmId, host: &str, ram_port: u16, key: &str, to: crate::shared::Owner, user: &str) -> Result<TaskId, Error> {
        if self.shared_offer(vmid)?.is_none() {
            return Err(Error::InvalidSpec(format!("{vmid} ortak depoda değil")));
        }
        if let Some(why) = redox_core::handover_blocker(self.lock().guest(vmid)?) {
            return Err(Error::InvalidSpec(why.to_string()));
        }
        self.send_guest(vmid, host, None, ram_port, key, Some(to), user)
    }

    #[allow(clippy::too_many_arguments)]
    fn send_guest(&self, vmid: VmId, host: &str, nbd_port: Option<u16>, ram_port: u16, key: &str, to: Option<crate::shared::Owner>, user: &str) -> Result<TaskId, Error> {
        if !redox_core::valid_migrate_host(host) {
            return Err(Error::InvalidSpec(format!("geçersiz hedef adresi: {host}")));
        }
        if !redox_core::valid_migrate_key(key) || ram_port < 1024 || nbd_port.is_some_and(|port| port < 1024 || port == ram_port) {
            return Err(Error::InvalidSpec("taşıma isteği eksik ya da geçersiz (anahtar ve iki ayrı kapı gerekir)".to_string()));
        }
        if self.snapshot().mirror(vmid).is_some() {
            return Err(Error::InvalidSpec(format!("{vmid} disk yansıtması açıkken canlı taşınamaz; önce yansıtmayı kapatın")));
        }
        if !self.driver().live_migration() {
            return Err(Error::InvalidSpec("bu sürücü canlı taşımayı desteklemiyor (gerçek sürücü gerekir)".to_string()));
        }
        let (host, key) = (host.to_string(), key.to_string());
        let (sent, handed) = (to.clone(), to);
        self.run_op(
            vmid,
            user,
            format!("vm migrate {vmid}"),
            None,
            move |c| c.check_migrate_out(vmid),
            move |d, g| match (&sent, nbd_port) {
                (Some(to), _) => d.migrate_send_shared(g, &host, ram_port, &key, (&to.server, &to.node)),
                (None, Some(nbd_port)) => d.migrate_send(g, &host, nbd_port, ram_port, &key),
                (None, None) => Err("taşıma isteği eksik: disk kapısı yok".to_string()),
            },
            move |c, _downtime_ms| {
                c.after_migrated_out(vmid);
                if let Some(to) = &handed {
                    c.mark_handed_over(vmid, &to.node);
                }
            },
        )
    }

    /// Hedef: eş sunucudaki çalışan sanal makineyi kapatmadan buraya taşır (arka planda). Sonuç görev günlüğüne
    /// yazılır. Konuğun eşteki kaydı ve diskleri silinmez: kapalı kalır, hedefte doğruladıktan sonra silmek size kalır.
    pub fn migrate_from_peer(&self, peer_name: &str, vmid: VmId, user: &str) -> Result<(), Error> {
        let peer = self.peers().peer(peer_name).ok_or_else(|| Error::InvalidSpec(format!("\"{peer_name}\" adında bir eş sunucu tanımlı değil")))?;
        if !self.driver().live_migration() {
            return Err(Error::InvalidSpec("bu sürücü canlı taşımayı desteklemiyor (gerçek sürücü gerekir)".to_string()));
        }
        let (me, user, peer_name) = (self.clone(), user.to_string(), peer_name.to_string());
        self.dispatch(move || {
            // Görev açılmadan önceki hatalar burada günlüğe düşer; sonrakiler görevin kendi kaydındadır.
            if let Err((message, false)) = me.migrate_now(&peer, &peer_name, vmid, &user) {
                me.mutate(|c| {
                    let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                    c.log(&node, &user, format!("vm migrate {peer_name}/{vmid}"), &Err(message), now(), None);
                });
            }
        });
        Ok(())
    }

    /// Başka sunucudaki bir konuk kaydından burada oluşturulacak konuğun tanımı (canlı taşıma ve devralma).
    /// Sunucuya özgü ayarlar uyarlanır; ek diskler sonradan `restore_hardware` ile eklenir.
    pub(super) fn local_spec(&self, remote: &Guest, vmid: Option<VmId>) -> Result<GuestSpec, Error> {
        let local = self.snapshot();
        let node = local.nodes.first().ok_or_else(|| Error::InvalidSpec("bu sunucuda düğüm yok".to_string()))?;
        let storage = local.storages.iter().find(|s| s.available_on(&node.id)).ok_or_else(|| Error::InvalidSpec("bu sunucuda depolama yok".to_string()))?;
        // Sunucuya özgü ayarlar taşınmaz: köprü burada yoksa NAT'a düşülür (konuğun kartı aynı kalır, bağlandığı ağ değişir).
        let known = self.driver().bridges();
        let here = |bridge: &String| if known.contains(bridge) { bridge.clone() } else { redox_core::NAT.to_string() };
        let mut hw = remote.hw.clone();
        hw.image = None;
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
        hw.usb.clear();
        hw.pci.clear();
        hw.shares.clear();
        // Ek diskler konuk oluşturulduktan sonra numaralarıyla, boş olarak eklenir.
        hw.disks.clear();
        hw.disk_seq = 0;
        Ok(GuestSpec {
            vmid,
            name: remote.name.clone(),
            kind: remote.kind,
            node: node.id.clone(),
            cores: remote.cores,
            memory_mb: remote.mem_total_mb,
            disk_gb: remote.disk_gb,
            storage: storage.id.clone(),
            os: remote.os.clone(),
            bridge: here(&remote.bridge),
            onboot: false,
            iso: None,
            hw,
        })
    }

    /// `local_spec` ile oluşturulan konuğa kaynağın ek disklerini, çekirdek ve bellek yapısını verir.
    pub(super) fn restore_hardware(&self, id: VmId, remote: &Guest) -> Result<Guest, String> {
        let driver = self.driver();
        for disk in &remote.hw.disks {
            let guest = self
                .mutate(|c| {
                    c.add_disk_exact(id, disk)?;
                    c.guest(id).cloned()
                })
                .map_err(|e| e.to_string())?;
            driver.add_disk(&guest, disk.id, disk.size_gb).map_err(|e| format!("ek disk {}: {e}", disk.id))?;
        }
        self.mutate(|c| {
            let g = c.guest_mut_for_import(id)?;
            (g.cores, g.mem_total_mb) = (remote.cores, remote.mem_total_mb);
            (g.hw.dimms, g.hw.max_cores, g.hw.max_mem_mb, g.hw.balloon_mb) = (remote.hw.dimms.clone(), remote.hw.max_cores, remote.hw.max_mem_mb, remote.hw.balloon_mb);
            Ok::<_, Error>(g.clone())
        })
        .map_err(|e| e.to_string())
    }

    /// Hata, görev günlüğüne yazıldıysa `true` ile döner.
    fn migrate_now(&self, peer: &crate::peers::Peer, peer_name: &str, vmid: VmId, user: &str) -> Result<(), (String, bool)> {
        let early = |message: String| (message, false);
        let remote = peer.guest(vmid).map_err(early)?;
        if let Some(why) = redox_core::migrate_blocker(&remote) {
            return Err(early(why.to_string()));
        }
        if remote.status != redox_core::GuestStatus::Running {
            return Err(early(format!("{vmid} eş sunucuda çalışmıyor; canlı taşıma çalışan konuk içindir (kapalı konuk \"Buraya Al\" ile kopyalanır)")));
        }
        if remote.lock.is_some() {
            return Err(early(format!("{vmid} üzerinde eş sunucuda bir işlem sürüyor; bitince yeniden deneyin")));
        }
        let local = self.snapshot();
        let node = local.nodes.first().ok_or_else(|| early("bu sunucuda düğüm yok".to_string()))?;
        if remote.cores.max(remote.hw.max_cores) > node.cores || remote.mem_total_mb.max(remote.hw.max_mem_mb) > node.mem_total_mb {
            return Err(early(format!("canlı taşınan konuk aynı işlemci ve bellekle açılmalı ({} çekirdek, {} MB); bu sunucu yetmiyor", remote.cores.max(remote.hw.max_cores), remote.mem_total_mb.max(remote.hw.max_mem_mb))));
        }
        // Aynı kimlik burada doluysa konuk gelemez; tek istisna, bu konuğun buradan taşınırken geride kalan
        // kapalı kopyasıdır: konuk geri geliyorsa bayat kopyanın yerini alır (yedekleri kimliğe bağlı kalır).
        let leftover = match local.guest(vmid) {
            Err(_) => false,
            Ok(here) if here.kind == redox_core::GuestKind::Vm && here.status == redox_core::GuestStatus::Stopped && here.lock.is_none() && here.tags.iter().any(|t| t == redox_core::MIGRATED_TAG) => true,
            Ok(_) => return Err(early(format!("bu sunucuda {vmid} kimliği dolu; canlı taşınan konuk aynı kimlikle gelir (ağ kartının adresi kimlikten türetilir)"))),
        };
        let layout = peer.migrate_prepare(vmid).map_err(early)?;
        // Eşin diskleri ortak depoda ve o depo burada da görünüyorsa yalnızca bellek taşınır.
        if redox_core::handover_blocker(&remote).is_none() {
            if let Some(offer) = peer.shared_offer(vmid).map_err(early)? {
                if let Some((storage, dir)) = self.shared_match(&offer, vmid) {
                    // Burada kalan eski kopya (devredilmemiş kayıt) paylaşımdaki dosyaları gösteriyor olabilir:
                    // yalnızca kaydı kaldırılır.
                    if leftover {
                        self.mutate(|c| {
                            c.mark_handed_over(vmid, &offer.server);
                        });
                    }
                    return self.migrate_shared_into(peer, peer_name, &remote, (&storage, &dir), layout, user);
                }
            }
        }
        if leftover {
            self.delete_guest(vmid, user).map_err(|e| early(format!("burada kalan eski kopya kaldırılamadı: {e}")))?;
        }
        let spec = self.local_spec(&remote, Some(vmid)).map_err(|e| early(e.to_string()))?;
        let id = self.create_guest(&spec, user).map_err(|e| early(e.to_string()))?;
        let moved = self.migrate_into(peer, peer_name, &remote, id, layout, user);
        if moved.is_err() {
            // Yarım kopya bırakılmaz; konuk kaynakta çalışmayı sürdürür (ya da orada kapalı durur).
            let _ = self.delete_guest(id, user);
        }
        moved
    }

    /// Diskleri ortak depoda duran konuğun canlı taşınması (hedef taraf). Hiçbir adımda disk oluşturulmaz ya
    /// da silinmez; başarısızlıkta yalnızca bekleyen QEMU kapatılır.
    fn migrate_shared_into(&self, peer: &crate::peers::Peer, peer_name: &str, remote: &Guest, share: (&str, &std::path::Path), layout: u8, user: &str) -> Result<(), (String, bool)> {
        let early = |message: String| (message, false);
        let vmid = remote.vmid;
        let me = self.driver().server_id();
        let command = format!("vm migrate {peer_name}/{vmid}");
        let (guest, task) = self.shared_record(remote, share.0, user, Some(&command)).map_err(|e| early(e.to_string()))?;
        let task = task.ok_or_else(|| early("taşıma görevi açılamadı".to_string()))?;
        let driver = self.driver();
        let owned = || crate::shared::require(share.1, vmid, &me).is_ok();
        let outcome = (|| -> Result<(), String> {
            let settings = self.migrate_settings();
            let key = new_key()?;
            let ram_port = {
                let _pick = PICK.lock().unwrap_or_else(|e| e.into_inner());
                let port = free_port(settings.ports, &settings.bind)?;
                driver.migrate_listen_shared(&guest, layout, &settings.bind, port, &key)?;
                port
            };
            let to = crate::shared::Owner { server: me.clone(), node: guest.node.clone() };
            let sent = peer.migrate_shared_and_wait(vmid, settings.host.as_deref(), ram_port, &key, &to, Duration::from_secs(72 * 3600));
            // Eş hata bildirse de (ör. yanıt yolda kaybolsa) sahiplik geçmişse konuk artık buradadır.
            if !owned() {
                driver.migrate_abort(&guest);
                return Err(sent.err().unwrap_or_else(|| "disk sahipliği bu sunucuya geçmedi".to_string()));
            }
            driver.migrate_finish_shared(&guest).map_err(|e| {
                driver.migrate_abort(&guest);
                format!("{e}. Diskler artık bu sunucunun: konuğu buradan başlatabilirsiniz")
            })
        })();
        let arrived = owned();
        self.mutate(|c| {
            let _ = c.complete_task(task, outcome.clone(), now());
            if arrived {
                if let Ok(g) = c.guest_mut_for_import(vmid) {
                    if g.tags.len() < redox_core::MAX_TAGS && redox_core::valid_tag(&format!("tasindi-{peer_name}")) {
                        g.tags.push(format!("tasindi-{peer_name}"));
                    }
                    (g.onboot, g.backup_time) = (remote.onboot, remote.backup_time.clone());
                }
            } else {
                // Diskler eşte: burada yalnızca yeni kurulan kayıt kaldırılır.
                let _ = c.delete_guest(vmid, user, now());
            }
        });
        outcome.map_err(|message| (message, true))?;
        let current = self.lock().guest(vmid).ok().cloned();
        if let Some(probe) = current.and_then(|g| driver.probe(&g)) {
            self.mutate(|c| {
                c.observe_guest(vmid, &probe, now());
            });
        }
        let _ = self.sync_firewall();
        Ok(())
    }

    fn migrate_into(&self, peer: &crate::peers::Peer, peer_name: &str, remote: &Guest, id: VmId, layout: u8, user: &str) -> Result<(), (String, bool)> {
        let early = |message: String| (message, false);
        let driver = self.driver();
        // Hedef QEMU'nun bulacağı kayıt, kaynağınkiyle aynı donanımı anlatmalıdır.
        let guest = self.restore_hardware(id, remote).map_err(early)?;
        let command = format!("vm migrate {peer_name}/{}", remote.vmid);
        let task = self.mutate(|c| c.begin_op(id, command, user, now(), None)).map_err(|e| early(e.to_string()))?;
        let outcome = (|| -> Result<(), String> {
            let settings = self.migrate_settings();
            let key = new_key()?;
            let (nbd_port, ram_port) = {
                let _pick = PICK.lock().unwrap_or_else(|e| e.into_inner());
                let ports = free_ports(settings.ports, &settings.bind)?;
                driver.migrate_listen(&guest, layout, &settings.bind, ports.0, ports.1, &key)?;
                ports
            };
            if let Err(e) = peer.migrate_and_wait(remote.vmid, settings.host.as_deref(), nbd_port, ram_port, &key, Duration::from_secs(72 * 3600)) {
                driver.migrate_abort(&guest);
                return Err(e);
            }
            driver.migrate_finish(&guest).map_err(|e| {
                driver.migrate_abort(&guest);
                format!("{e}. Konuk eş sunucuda kapalı duruyor: orada başlatılabilir")
            })
        })();
        self.mutate(|c| {
            let _ = c.complete_task(task, outcome.clone(), now());
            if outcome.is_ok() {
                if let Ok(g) = c.guest_mut_for_import(id) {
                    g.tags = remote.tags.clone();
                    g.tags.push(format!("tasindi-{peer_name}"));
                    g.tags.retain(|t| redox_core::valid_tag(t));
                    g.tags.truncate(redox_core::MAX_TAGS);
                    (g.onboot, g.backup_time) = (remote.onboot, remote.backup_time.clone());
                }
            }
        });
        outcome.map_err(|message| (message, true))?;
        // Kayıt, bir sonraki okuma turunu beklemeden konuğun çalıştığını göstersin.
        // Kilit, sürücü sorulmadan ve kayıt güncellenmeden önce bırakılır.
        let current = self.lock().guest(id).ok().cloned();
        if let Some(probe) = current.and_then(|g| driver.probe(&g)) {
            self.mutate(|c| {
                c.observe_guest(id, &probe, now());
            });
        }
        let _ = self.sync_firewall();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_ports_are_a_range_of_unprivileged_ports() {
        assert_eq!(parse_migrate_ports("49152-49215"), Ok((49152, 49215)));
        assert_eq!(parse_migrate_ports(" 5000 - 5001 "), Ok((5000, 5001)));
        for bad in ["", "49152", "80-90", "5000-5000", "6000-5000", "a-b", "5000-70000"] {
            assert!(parse_migrate_ports(bad).unwrap_err().contains("kapı aralığı"), "{bad}");
        }
        assert_eq!(MigrateSettings::default(), MigrateSettings { ports: MIGRATE_PORTS, host: None, bind: "0.0.0.0".into() });
    }

    #[test]
    fn two_free_ports_are_picked_and_busy_ones_are_skipped() {
        // Aralığın ilk kapısı doluyken sonraki iki boş kapı seçilir; boş kapı kalmayınca açık hata.
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let first = busy.local_addr().unwrap().port();
        if first > 65_000 {
            return;
        }
        let (a, b) = free_ports((first, first + 20), "127.0.0.1").unwrap();
        assert!(a > first && b > a, "{first} {a} {b}");
        assert!(free_ports((first, first), "127.0.0.1").unwrap_err().contains("boş kapı yok"));
    }

    #[test]
    fn every_migration_gets_a_fresh_key() {
        let (a, b) = (new_key().unwrap(), new_key().unwrap());
        assert!(redox_core::valid_migrate_key(&a) && redox_core::valid_migrate_key(&b) && a != b);
    }
}
