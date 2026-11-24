use crate::model::{Action, GuestStatus, TaskId, VmId};

/// Hatanın genel sınıfı; HTTP durum koduna çevirmek için kullanılır.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Conflict,
    Invalid,
    /// Hipervizör sürücüsü işlemi yapamadı.
    Driver,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("{0} kimlikli konuk bulunamadı")]
    GuestNotFound(VmId),
    #[error("düğüm bulunamadı: {0}")]
    NodeNotFound(String),
    #[error("depolama bulunamadı: {0}")]
    StorageNotFound(String),
    #[error("görev bulunamadı: {0}")]
    TaskNotFound(TaskId),
    #[error("{0} kimliği zaten kullanılıyor")]
    VmidTaken(VmId),
    #[error("geçersiz kimlik {0}: 100 ile 999999999 arasında olmalı")]
    InvalidVmid(VmId),
    #[error("geçersiz ad \"{0}\": 1-63 karakter olmalı; küçük harf, rakam ve tire kullanın, tire ile başlatmayın ya da bitirmeyin")]
    InvalidName(String),
    #[error("{0}")]
    InvalidSpec(String),
    #[error("{vmid} şu anda {status}; \"{action}\" uygulanamaz")]
    InvalidTransition {
        vmid: VmId,
        action: Action,
        status: GuestStatus,
    },
    #[error("{vmid} üzerinde başka bir işlem sürüyor (görev {task}); bitmesini bekleyin")]
    Locked { vmid: VmId, task: TaskId },
    #[error("{0} silinmeden önce kapatılmalı")]
    NotStopped(VmId),
    #[error("{storage} deposunda yeterli alan yok: {needed_gb} GB gerekli, {free_gb} GB boş")]
    InsufficientStorage {
        storage: String,
        needed_gb: u64,
        free_gb: u64,
    },
    #[error("{storage} deposu {node} düğümünde kullanılamıyor")]
    StorageNotOnNode { storage: String, node: String },
    #[error("{0} düğümü çevrimdışı")]
    NodeOffline(String),
    #[error("sürücü hatası: {0}")]
    Driver(String),
    #[error("{vmid} çalışırken yapılamaz: {what} için önce konuğu kapatın")]
    RequiresStopped { vmid: VmId, what: String },
    #[error("\"{0}\" adlı bir anlık görüntü zaten var")]
    SnapshotExists(String),
    #[error("anlık görüntü bulunamadı: {0}")]
    SnapshotNotFound(String),
    #[error("yedek bulunamadı: {0}")]
    BackupNotFound(String),
    #[error("{0} numaralı sunucu portu başka bir yönlendirmede kullanılıyor")]
    PortTaken(u16),
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::GuestNotFound(_)
            | Error::NodeNotFound(_)
            | Error::StorageNotFound(_)
            | Error::TaskNotFound(_)
            | Error::SnapshotNotFound(_)
            | Error::BackupNotFound(_) => ErrorKind::NotFound,
            Error::VmidTaken(_)
            | Error::InvalidTransition { .. }
            | Error::Locked { .. }
            | Error::NotStopped(_)
            | Error::NodeOffline(_)
            | Error::RequiresStopped { .. }
            | Error::SnapshotExists(_)
            | Error::PortTaken(_) => ErrorKind::Conflict,
            Error::InvalidVmid(_)
            | Error::InvalidName(_)
            | Error::InvalidSpec(_)
            | Error::InsufficientStorage { .. }
            | Error::StorageNotOnNode { .. } => ErrorKind::Invalid,
            Error::Driver(_) => ErrorKind::Driver,
        }
    }

    /// Makinece okunur kısa kod (API yanıtlarında).
    pub fn code(&self) -> &'static str {
        match self {
            Error::GuestNotFound(_) => "guest_not_found",
            Error::NodeNotFound(_) => "node_not_found",
            Error::StorageNotFound(_) => "storage_not_found",
            Error::TaskNotFound(_) => "task_not_found",
            Error::VmidTaken(_) => "vmid_taken",
            Error::InvalidVmid(_) => "invalid_vmid",
            Error::InvalidName(_) => "invalid_name",
            Error::InvalidSpec(_) => "invalid_spec",
            Error::InvalidTransition { .. } => "invalid_transition",
            Error::Locked { .. } => "locked",
            Error::NotStopped(_) => "not_stopped",
            Error::InsufficientStorage { .. } => "insufficient_storage",
            Error::StorageNotOnNode { .. } => "storage_not_on_node",
            Error::NodeOffline(_) => "node_offline",
            Error::Driver(_) => "driver_error",
            Error::RequiresStopped { .. } => "requires_stopped",
            Error::SnapshotExists(_) => "snapshot_exists",
            Error::SnapshotNotFound(_) => "snapshot_not_found",
            Error::BackupNotFound(_) => "backup_not_found",
            Error::PortTaken(_) => "port_taken",
        }
    }
}
