//! Recovery configuration and statistics model

use std::path::PathBuf;

use crate::wal::{Lsn, WalRecoveryMode};
use linkrs_core::types::Timestamp;

#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    pub wal_dir: PathBuf,
    pub data_dir: PathBuf,
    pub recovery_mode: WalRecoveryMode,
    pub parallel_recovery: bool,
    pub verify_checksum: bool,
    pub start_lsn: Option<Lsn>,
    /// Whether to throw on any WAL corruption (mirrors Ladybug's
    /// `throwOnWalReplayFailure`). When false, torn tails are truncated.
    pub throw_on_wal_replay_failure: bool,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            wal_dir: PathBuf::from("./data/wal"),
            data_dir: PathBuf::from("./data"),
            recovery_mode: WalRecoveryMode::default(),
            parallel_recovery: true,
            verify_checksum: true,
            start_lsn: None,
            throw_on_wal_replay_failure: false,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct RecoveryStats {
    pub wal_entries_replayed: usize,
    pub pages_restored: usize,
    pub checkpoints_processed: usize,
    pub recovery_time_ms: u64,
    pub errors_encountered: usize,
    pub last_lsn: Lsn,
    pub max_timestamp: Timestamp,
    pub max_transaction_id: u64,
}
