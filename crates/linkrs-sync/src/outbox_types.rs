use linkrs_core::types::CommitLsn;
use linkrs_core::types::LeaseEpoch;
use linkrs_core::wal::IndexMutation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedEvent {
    pub event_id: i64,
    pub commit_lsn: CommitLsn,
    pub intent_sequence: u32,
    pub mutation: IndexMutation,
    pub lease_owner: String,
    pub lease_epoch: LeaseEpoch,
}

#[derive(Debug, Clone)]
pub struct RebuildEvent {
    pub commit_lsn: CommitLsn,
    pub mutation: IndexMutation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxSnapshot {
    pub path: std::path::PathBuf,
    pub size_bytes: u64,
    pub checksum: u32,
    pub materialized_lsn: CommitLsn,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncDiagnostics {
    pub materialized_lsn: CommitLsn,
    pub targets: Vec<TargetSyncDiagnostics>,
    pub indexes: Vec<IndexSyncDiagnostics>,
    pub vector_disabled_skips: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TargetSyncDiagnostics {
    pub target: String,
    pub applied_lsn: CommitLsn,
    pub frontier_lag: u64,
    pub pending: u64,
    pub retrying: u64,
    pub leased: u64,
    pub dead_lettered: u64,
    pub oldest_event_age_ms: Option<u64>,
    pub degraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndexSyncDiagnostics {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub state: String,
    pub barrier_lsn: Option<CommitLsn>,
    pub applied_lsn: CommitLsn,
    pub frontier_lag: u64,
    pub degraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeadLetterRow {
    pub event_id: i64,
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub commit_lsn: CommitLsn,
    pub retry_count: u64,
    pub failed_at_ms: u64,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DegradedRangeRow {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub start_lsn: CommitLsn,
    pub end_lsn: CommitLsn,
    pub reason: String,
    pub created_at_ms: u64,
}
