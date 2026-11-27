//! Sunucu ağları: tanımları kaydetme ve kök yardımcısının okuyacağı dosyayı yazma.

use std::path::PathBuf;

use super::*;
use redox_core::hostnet::HostNet;

/// Bir sunucu ağının bu makinedeki durumu.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostNetReport {
    #[serde(flatten)]
    pub net: HostNet,
    /// Aygıt var mı? Gerçek olmayan sürücüde `None`.
    pub present: Option<bool>,
    /// Bu ağı kullanan konuklar.
    pub guests: Vec<VmId>,
}

impl AppState {
    pub fn set_hostnet_file(&self, path: PathBuf) -> Result<(), String> {
        let _ = self.0.hostnet_file.set(path);
        self.write_hostnet_file()
    }

    fn write_hostnet_file(&self) -> Result<(), String> {
        static WRITE: Mutex<()> = Mutex::new(());
        let _one = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        match self.0.hostnet_file.get() {
            Some(path) => crate::hostnet::write_spec(path, &self.snapshot().host_nets),
            None => Ok(()),
        }
    }

    pub fn host_net_report(&self) -> Vec<HostNetReport> {
        let real = self.driver().is_real();
        let c = self.snapshot();
        c.host_nets
            .iter()
            .map(|net| HostNetReport {
                net: net.clone(),
                present: real.then(|| std::path::Path::new("/sys/class/net").join(net.name()).exists()),
                guests: if net.is_bridge() { c.vswitch_users(net.name()) } else { Vec::new() },
            })
            .collect()
    }

    pub fn host_net_put(&self, net: HostNet, user: &str) -> Result<(), Error> {
        // Gerçek sunucuda, yardımcının reddedeceği tanım daha kaydedilmeden o anki ağa karşı denenir.
        if self.0.hostnet_file.get().is_some() {
            let mut next = self.snapshot();
            next.put_host_net(net.clone())?;
            if let Ok(observed) = crate::hostnet::observe("ip") {
                crate::hostnet::plan(&next.host_nets, &observed).map_err(Error::InvalidSpec)?;
            }
        }
        let name = net.name().to_string();
        self.mutate(|c| {
            c.put_host_net(net)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("hostnet set {name}"), &Ok(()), now(), None);
            Ok::<_, Error>(())
        })?;
        self.write_hostnet_file().map_err(Error::Driver)
    }

    pub fn host_net_remove(&self, name: &str, user: &str) -> Result<(), Error> {
        self.mutate(|c| {
            c.remove_host_net(name)?;
            let node = c.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
            c.log(&node, user, format!("hostnet remove {name}"), &Ok(()), now(), None);
            Ok::<_, Error>(())
        })?;
        self.write_hostnet_file().map_err(Error::Driver)
    }
}
