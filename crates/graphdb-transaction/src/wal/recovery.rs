//! WAL recovery: orchestration, consistency filtering, and replay dispatch
//!
//! Configuration and statistics types live in [`config`], the orchestration
//! skeleton and lifecycle interface in [`manager`], dry-run filtering and
//! transaction boundary computation in [`filter`], and per-operation replay
//! dispatch in [`replay`].

mod config;
mod filter;
mod manager;
mod replay;
pub use config::{RecoveryConfig, RecoveryStats};
pub use graphdb_core::wal::traits::RecoveryApplier;
pub use manager::RecoveryManager;
