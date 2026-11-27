//! RedoX VE çekirdeği: sanallaştırma kümesinin alan modeli ve kuralları.
//!
//! Bu crate giriş/çıkış yapmaz; zaman dışarıdan `now` (epoch saniyesi) olarak verilir,
//! böylece bütün kurallar belirlenimci biçimde test edilebilir.

pub mod approvals;
pub mod cluster;
pub mod command;
pub mod driver;
pub mod error;
pub mod fmt;
pub mod hw;
pub mod jobs;
pub mod metrics;
pub mod mirrors;
pub mod model;
pub mod ops;
pub mod pools;
pub mod replicas;
pub mod retention;
pub mod seed;
pub mod usage;
pub mod vswitch;

pub use approvals::Approval;
pub use cluster::{Cluster, ClusterSummary};
pub use driver::{BackupSize, BalloonStats, FsUsage, Hypervisor, MirrorProbe, Probe, Remote, SimDriver, SnapshotOp};
pub use error::{Error, ErrorKind};
pub use hw::Hardware;
pub use ops::migrate::{migrate_blocker, valid_migrate_host, valid_migrate_key, MIGRATED_TAG};
pub use jobs::{BackupJob, JobStep, MAX_JOBS};
pub use mirrors::{mirror_blocker, Mirror, MirrorState, MirrorStore};
pub use pools::{Pool, PoolUsage, MAX_POOLS};
pub use replicas::Replica;
pub use retention::Gfs;
pub use usage::Usage;
pub use vswitch::VSwitch;
pub use model::*;
