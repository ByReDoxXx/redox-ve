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
pub mod model;
pub mod ops;
pub mod pools;
pub mod retention;
pub mod seed;
pub mod usage;

pub use approvals::Approval;
pub use cluster::{Cluster, ClusterSummary};
pub use driver::{BackupSize, BalloonStats, Hypervisor, Probe, Remote, SimDriver, SnapshotOp};
pub use error::{Error, ErrorKind};
pub use hw::Hardware;
pub use jobs::{BackupJob, JobStep, MAX_JOBS};
pub use pools::{Pool, PoolUsage, MAX_POOLS};
pub use retention::Gfs;
pub use usage::Usage;
pub use model::*;
