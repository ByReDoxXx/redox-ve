//! RedoX VE çekirdeği: sanallaştırma kümesinin alan modeli ve kuralları.
//!
//! Bu crate giriş/çıkış yapmaz; zaman dışarıdan `now` (epoch saniyesi) olarak verilir,
//! böylece bütün kurallar belirlenimci biçimde test edilebilir.

pub mod cluster;
pub mod command;
pub mod driver;
pub mod error;
pub mod fmt;
pub mod hw;
pub mod metrics;
pub mod model;
pub mod ops;
pub mod seed;

pub use cluster::{Cluster, ClusterSummary};
pub use driver::{BackupSize, Hypervisor, Probe, Remote, SimDriver, SnapshotOp};
pub use error::{Error, ErrorKind};
pub use hw::Hardware;
pub use model::*;
