//! Anlık görüntüler ve üretim denetim noktaları.

use super::*;

impl AppState {
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

    /// Üretim denetim noktası: çalışan konuğun disklerinin, bellek durumu olmadan ve (konuk ajanı varsa) dosya
    /// sistemleri dondurularak alınan anlık görüntüsü. Bellekli görüntüye göre küçük ve hızlıdır; geri dönüşte
    /// konuk o andan devam etmez, o andaki diskle yeniden açılır.
    pub fn snapshot_create_production(&self, vmid: VmId, name: &str, user: &str) -> Result<TaskId, Error> {
        let (kind, n1, n2, n3) = (self.kind_of(vmid)?, name.to_string(), name.to_string(), name.to_string());
        if kind != redox_core::GuestKind::Vm {
            return Err(Error::InvalidSpec("üretim denetim noktası yalnızca sanal makinelerde alınır".to_string()));
        }
        self.run_op(
            vmid,
            user,
            format!("{kind} checkpoint {vmid} {name}"),
            self.stopped_unless_live(kind, "anlık görüntü almak"),
            move |c| c.not_template(vmid).and_then(|()| c.check_snapshot_new(vmid, &n1)),
            move |d, g| d.snapshot_disk_only(g, &n2),
            move |c, quiesced| c.add_snapshot_kind(vmid, &n3, now(), true, quiesced),
        )
    }

    pub fn snapshot_rollback(&self, vmid: VmId, name: &str, user: &str) -> Result<TaskId, Error> {
        let (kind, n1, n2) = (self.kind_of(vmid)?, name.to_string(), name.to_string());
        self.run_op(
            vmid,
            user,
            format!("{kind} rollback {vmid} {name}"),
            self.stopped_unless_live(kind, "anlık görüntüye dönmek"),
            move |c| c.not_template(vmid).and_then(|()| c.not_suspended(vmid)).and_then(|()| c.check_snapshot(vmid, &n1)).and_then(|()| c.check_rollback_state(vmid, &n1)),
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
}
