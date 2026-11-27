//! Dağıtık anahtarların yönetimi: tanımı saklar, ağ yardımcısının okuyacağı dosyayı yazar, eşlere gönderir ve
//! eşlerdeki tanımlarla karşılaştırır.

use super::*;
use crate::peers::PeerView;
use crate::vswitch::status::Local;
use redox_core::VSwitch;
use serde::Serialize;
use std::path::PathBuf;

/// Eşlerle yapılan bir işin eş eş sonucu: `(eş, sonuç)`.
pub type PeerResults = Vec<(String, Result<(), String>)>;

/// Bir eşteki tanımın buradakine göre durumu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerState {
    Same,
    Different,
    Missing,
    /// Eşe ulaşılamıyor ya da eş dağıtık anahtarı bilmeyen bir sürüm.
    Unknown,
}

impl PeerState {
    pub fn label(self) -> &'static str {
        match self {
            PeerState::Same => "aynı",
            PeerState::Different => "tanımı farklı",
            PeerState::Missing => "tanımlı değil",
            PeerState::Unknown => "bilinmiyor",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SwitchReport {
    #[serde(flatten)]
    pub switch: VSwitch,
    /// Bu sunucudaki ağın durumu; benzetim sürücüsünde ağ kurulmadığı için `None`.
    pub local: Option<Local>,
    pub guests: Vec<VmId>,
    pub peers: Vec<(String, PeerState)>,
}

/// Saf karşılaştırma: eşlerin son okunan tanımlarına göre bu anahtarın durumu.
pub fn compare(switch: &VSwitch, peers: &[PeerView]) -> Vec<(String, PeerState)> {
    peers
        .iter()
        .map(|view| {
            let state = match view.vswitches.as_ref().filter(|_| view.online()) {
                None => PeerState::Unknown,
                Some(all) => match all.iter().find(|s| s.name == switch.name) {
                    None => PeerState::Missing,
                    Some(theirs) if theirs == switch => PeerState::Same,
                    Some(_) => PeerState::Different,
                },
            };
            (view.name.clone(), state)
        })
        .collect()
}

impl AppState {
    /// Ağ yardımcısının okuyacağı dosya. Ayarlandığında o anki tanımlar hemen yazılır.
    pub fn set_vswitch_file(&self, path: PathBuf) -> Result<(), String> {
        let _ = self.0.vswitch_file.set(path);
        self.write_vswitch_file()
    }

    fn write_vswitch_file(&self) -> Result<(), String> {
        // Dosyayı aynı anda iki istek yazmasın: son yazan, en yeni tanımları yazmış olur.
        static WRITE: Mutex<()> = Mutex::new(());
        let _one = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        match self.0.vswitch_file.get() {
            Some(path) => crate::vswitch::write_spec(path, &self.snapshot().vswitches),
            None => Ok(()),
        }
    }

    pub fn vswitch_put(&self, switch: VSwitch, user: &str) -> Result<(), Error> {
        let name = switch.name.clone();
        let vni = switch.vni;
        self.mutate(|c| {
            if c.put_vswitch(switch)? {
                let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
                c.log(&node, user, format!("switch set {name} {vni}"), &Ok(()), now(), None);
            }
            Ok::<_, Error>(())
        })?;
        self.write_vswitch_file().map_err(Error::Driver)
    }

    pub fn vswitch_remove(&self, name: &str, user: &str) -> Result<(), Error> {
        self.mutate(|c| {
            c.remove_vswitch(name)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("switch remove {name}"), &Ok(()), now(), None);
            Ok::<_, Error>(())
        })?;
        self.write_vswitch_file().map_err(Error::Driver)
    }

    pub fn vswitch_report(&self) -> Vec<SwitchReport> {
        let c = self.snapshot();
        let views = self.peers().views();
        let real = self.driver().is_real();
        c.vswitches
            .iter()
            .map(|switch| SwitchReport { local: real.then(|| crate::vswitch::status::local_status(switch)), guests: c.vswitch_users(&switch.name), peers: compare(switch, &views), switch: switch.clone() })
            .collect()
    }

    fn vswitch_named(&self, name: &str) -> Result<VSwitch, Error> {
        self.snapshot().vswitches.into_iter().find(|s| s.name == name).ok_or_else(|| Error::InvalidSpec(format!("\"{name}\" adında bir dağıtık anahtar yok")))
    }

    /// Anahtarın tanımını bütün eşlere gönderir: `(eş, sonuç)`. Eşlerle konuştuğu için istek iş parçacığında çağrılmaz.
    pub fn vswitch_push(&self, name: &str, user: &str) -> Result<PeerResults, Error> {
        let switch = self.vswitch_named(name)?;
        let body = serde_json::to_value(&switch).map_err(|e| Error::Driver(e.to_string()))?;
        let results = self.on_every_peer(|peer| peer.vswitch_put(&body))?;
        self.log_everywhere(format!("switch push {name}"), &results, user);
        Ok(results)
    }

    /// Anahtarı önce eşlerden, hepsi başarılıysa buradan siler. Bir eş silemezse burada da kalır ki yeniden
    /// denenebilsin.
    pub fn vswitch_remove_everywhere(&self, name: &str, user: &str) -> Result<PeerResults, Error> {
        self.vswitch_named(name)?;
        let users = self.snapshot().vswitch_users(name);
        if !users.is_empty() {
            // Eşlerde silmeye başlamadan buradaki engel bildirilir.
            return self.vswitch_remove(name, user).map(|()| Vec::new());
        }
        let results = self.on_every_peer(|peer| peer.vswitch_delete(name))?;
        if results.iter().all(|(_, r)| r.is_ok()) {
            self.vswitch_remove(name, user)?;
        }
        self.log_everywhere(format!("switch remove-everywhere {name}"), &results, user);
        Ok(results)
    }

    fn on_every_peer(&self, work: impl Fn(&crate::peers::Peer) -> Result<(), String>) -> Result<PeerResults, Error> {
        let views = self.peers().views();
        if views.is_empty() {
            return Err(Error::InvalidSpec("tanımlı eş sunucu yok (--peers)".to_string()));
        }
        Ok(views.into_iter().filter_map(|v| self.peers().peer(&v.name).map(|peer| (v.name, work(&peer)))).collect())
    }

    fn log_everywhere(&self, command: String, results: &[(String, Result<(), String>)], user: &str) {
        let failed: Vec<String> = results.iter().filter_map(|(peer, r)| r.as_ref().err().map(|e| format!("{peer}: {e}"))).collect();
        let outcome = if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) };
        self.mutate(|c| {
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, command, &outcome, now(), None);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_definitions_are_compared_by_content() {
        let switch = VSwitch { name: "vds-a".to_string(), vni: 7, mtu: 1450, members: vec!["10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap()] };
        let view = |name: &str, switches: Option<Vec<VSwitch>>| PeerView { name: name.to_string(), checked: Some(1), vswitches: switches, ..Default::default() };
        let mut other = switch.clone();
        other.mtu = 1400;
        let mut offline = view("kapali", Some(vec![switch.clone()]));
        offline.error = Some("erişilemiyor".to_string());
        let states = compare(&switch, &[view("ayni", Some(vec![switch.clone()])), view("farkli", Some(vec![other])), view("yok", Some(Vec::new())), view("eski", None), offline]);
        assert_eq!(states.iter().map(|(_, s)| *s).collect::<Vec<_>>(), [PeerState::Same, PeerState::Different, PeerState::Missing, PeerState::Unknown, PeerState::Unknown]);
    }
}
