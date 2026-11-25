use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::fmt::pct;
use crate::model::*;

mod lifecycle;
mod observe;

/// Görev günlüğünde tutulan en fazla kayıt.
const MAX_TASKS: usize = 200;

/// Bir kümenin bütün durumu. Değişiklikler yalnızca buradaki metotlarla yapılır;
/// her biri kuralları doğrular ve görev günlüğüne kayıt düşer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    pub name: String,
    pub nodes: Vec<Node>,
    /// Kimliğe göre sıralı tutulur.
    pub guests: Vec<Guest>,
    pub storages: Vec<Storage>,
    /// En yeni görev başta.
    pub tasks: Vec<Task>,
    /// En yeni yedek başta.
    #[serde(default)]
    pub backups: Vec<Backup>,
    /// Yedek işleri (ada göre sıralı).
    #[serde(default)]
    pub jobs: Vec<crate::jobs::BackupJob>,
    /// Kaynak havuzları (ada göre sıralı).
    #[serde(default)]
    pub pools: Vec<crate::pools::Pool>,
    /// Arşiv: yerelde silinmiş ama saklama süresi dolmadığı için kopya hedefinde duran yedekler (en yeni başta).
    #[serde(default)]
    pub archive: Vec<Backup>,
    /// Dört göz ilkesi: ikinci yöneticinin onayını bekleyen istekler ve son verilen sıra numarası.
    #[serde(default)]
    pub approvals: Vec<crate::approvals::Approval>,
    #[serde(default)]
    pub(crate) approval_seq: u64,
    /// Konukların içinden okunan dosya sistemi dolulukları (yalnızca çalışan ve ajanı yanıt veren konuklar).
    #[serde(default)]
    pub guest_fs: Vec<(VmId, Vec<crate::driver::FsUsage>)>,
    /// Konuk başına birikimli kaynak tüketimi (kimliğe göre sıralı).
    #[serde(default)]
    pub usage: Vec<crate::usage::Usage>,
    next_task_id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClusterSummary {
    pub name: String,
    pub nodes_online: usize,
    pub nodes_total: usize,
    pub quorate: bool,
    pub cores: u32,
    pub cpu_pct: u8,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub mem_pct: u8,
    pub storage_used_gb: u64,
    pub storage_total_gb: u64,
    pub storage_pct: u8,
    pub guests_running: usize,
    pub guests_total: usize,
}

impl Cluster {
    pub fn new(name: impl Into<String>) -> Self {
        Cluster {
            name: name.into(),
            nodes: Vec::new(),
            guests: Vec::new(),
            storages: Vec::new(),
            tasks: Vec::new(),
            backups: Vec::new(),
            jobs: Vec::new(),
            pools: Vec::new(),
            usage: Vec::new(),
            archive: Vec::new(),
            guest_fs: Vec::new(),
            approvals: Vec::new(),
            approval_seq: 0,
            next_task_id: 1,
        }
    }

    pub fn node(&self, id: &str) -> Result<&Node, Error> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or_else(|| Error::NodeNotFound(id.to_string()))
    }

    pub fn guest(&self, vmid: VmId) -> Result<&Guest, Error> {
        self.guests
            .iter()
            .find(|g| g.vmid == vmid)
            .ok_or(Error::GuestNotFound(vmid))
    }

    pub(crate) fn guest_mut(&mut self, vmid: VmId) -> Result<&mut Guest, Error> {
        self.guests
            .iter_mut()
            .find(|g| g.vmid == vmid)
            .ok_or(Error::GuestNotFound(vmid))
    }

    pub fn task(&self, id: TaskId) -> Result<&Task, Error> {
        self.tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or(Error::TaskNotFound(id))
    }

    pub fn guests_on<'a>(&'a self, node: &'a str) -> impl Iterator<Item = &'a Guest> {
        self.guests.iter().filter(move |g| g.node == node)
    }

    /// `node` üzerinde kullanılabilen `id` kimlikli depolamanın dizini.
    pub(crate) fn storage_index(&self, id: &str, node: &str) -> Result<usize, Error> {
        if let Some(i) = self
            .storages
            .iter()
            .position(|s| s.id == id && s.available_on(node))
        {
            return Ok(i);
        }
        if self.storages.iter().any(|s| s.id == id) {
            Err(Error::StorageNotOnNode {
                storage: id.to_string(),
                node: node.to_string(),
            })
        } else {
            Err(Error::StorageNotFound(id.to_string()))
        }
    }

    pub fn storage_for(&self, id: &str, node: &str) -> Result<&Storage, Error> {
        self.storage_index(id, node).map(|i| &self.storages[i])
    }

    /// Tek düğümlü, konuksuz küme: gerçek sürücüyle bu makineyi yönetmek için.
    pub fn local(node: Node, storage: Storage) -> Self {
        let mut c = Cluster::new(node.id.clone());
        c.nodes.push(node);
        c.storages.push(storage);
        c
    }

    /// Kullanılmayan en küçük geçerli kimlik.
    pub fn next_vmid(&self) -> VmId {
        let mut candidate = VMID_MIN;
        for g in &self.guests {
            if g.vmid == candidate {
                candidate += 1;
            } else if g.vmid > candidate {
                break;
            }
        }
        candidate
    }

    pub fn has_running_tasks(&self) -> bool {
        self.tasks.iter().any(|t| t.status == TaskStatus::Running)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_task(
        &mut self,
        node: &str,
        user: &str,
        command: String,
        status: TaskStatus,
        now: u64,
        vmid: Option<VmId>,
        action: Option<Action>,
    ) -> TaskId {
        let id = self.next_task_id;
        self.next_task_id += 1;
        self.tasks.insert(
            0,
            Task {
                id,
                started: now,
                ended: (status != TaskStatus::Running).then_some(now),
                node: node.to_string(),
                user: user.to_string(),
                command,
                status,
                message: None,
                vmid,
                action,
            },
        );
        // Süren görevler asla atılmaz; yalnızca bitmiş en eski kayıtlar düşer.
        while self.tasks.len() > MAX_TASKS {
            match self.tasks.iter().rposition(|t| t.status != TaskStatus::Running) {
                Some(i) => {
                    self.tasks.remove(i);
                }
                None => break,
            }
        }
        id
    }
}

#[cfg(test)]
mod tests;
