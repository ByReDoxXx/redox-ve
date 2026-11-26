//! Konuk üzerinde yaşam döngüsü dışındaki işlemler: düzenleme, anlık görüntü,
//! disk büyütme, klonlama ve yedekleme. Kurallar burada; dosya işleri sürücüde.

use crate::cluster::Cluster;
use crate::error::Error;
use crate::model::*;

mod backups;
mod clone;
mod cpu;
mod disks;
mod edit;
mod memory;
pub mod migrate;
mod schedule;
mod snapshots;

impl Cluster {
    /// Konuğu kilitleyip süren bir görev açar (anlık görüntü, yedek, klon gibi uzun işler için).
    /// `need_stopped` doluysa konuk durdurulmuş olmalıdır; değer hata iletisinde işlemi adlandırır.
    /// Görev `complete_task` ile kapatılır.
    pub fn begin_op(
        &mut self,
        vmid: VmId,
        command: String,
        user: &str,
        now: u64,
        need_stopped: Option<&str>,
    ) -> Result<TaskId, Error> {
        let guest = self.guest(vmid)?;
        if let Some(task) = guest.lock {
            return Err(Error::Locked { vmid, task });
        }
        if let Some(what) = need_stopped {
            if guest.status != GuestStatus::Stopped {
                return Err(Error::RequiresStopped { vmid, what: what.to_string() });
            }
        }
        let node = guest.node.clone();
        if self.node(&node)?.status != NodeStatus::Online {
            return Err(Error::NodeOffline(node));
        }
        let id = self.push_task(&node, user, command, TaskStatus::Running, now, Some(vmid), None);
        self.guest_mut(vmid)?.lock = Some(id);
        Ok(id)
    }

    /// İkincil bir konuğu (klon hedefi) aynı görevle kilitler.
    pub fn lock_with(&mut self, vmid: VmId, task: TaskId) {
        if let Ok(g) = self.guest_mut(vmid) {
            g.lock = Some(task);
        }
    }

    /// `task` tarafından kilitlenmiş konuğun kilidini açar (klon hedefi gibi ikincil konuklar için).
    pub fn unlock(&mut self, vmid: VmId, task: TaskId) {
        if let Ok(g) = self.guest_mut(vmid) {
            if g.lock == Some(task) {
                g.lock = None;
            }
        }
    }

    /// Anında biten bir işlemi görev günlüğüne yazar.
    pub fn log(&mut self, node: &str, user: &str, command: String, outcome: &Result<(), String>, now: u64, vmid: Option<VmId>) {
        let status = if outcome.is_ok() { TaskStatus::Ok } else { TaskStatus::Failed };
        let id = self.push_task(node, user, command, status, now, vmid, None);
        if let (Err(message), Some(task)) = (outcome, self.tasks.iter_mut().find(|t| t.id == id)) {
            task.message = Some(message.clone());
        }
    }
}

#[cfg(test)]
mod tests;
