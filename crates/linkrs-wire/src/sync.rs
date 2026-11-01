//! Sync and outbox inspection DTOs.
//!
//! The durability plane already models these rows in `linkrs-sync`
//! (`DeadLetterRow`, `DegradedRangeRow`, target and index diagnostics).
//! This module mirrors those shapes with plain integers and strings so the
//! OpenAPI document carries named fields instead of generic JSON values.

use serde::{Deserialize, Serialize};

/// Health of one synchronization target.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TargetSyncDiagnostics {
    pub target: String,
    pub applied_lsn: u64,
    pub frontier_lag: u64,
    pub pending: u64,
    pub retrying: u64,
    pub leased: u64,
    pub dead_lettered: u64,
    #[serde(default)]
    pub oldest_event_age_ms: Option<u64>,
    pub degraded: bool,
}

/// Health of one secondary index generation.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IndexSyncDiagnostics {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub state: String,
    #[serde(default)]
    pub barrier_lsn: Option<u64>,
    pub applied_lsn: u64,
    pub frontier_lag: u64,
    pub degraded: bool,
}

/// Outbox diagnostics snapshot across targets and indexes.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SyncDiagnosticsResponse {
    pub materialized_lsn: u64,
    #[serde(default)]
    pub targets: Vec<TargetSyncDiagnostics>,
    #[serde(default)]
    pub indexes: Vec<IndexSyncDiagnostics>,
    #[serde(default)]
    pub vector_disabled_skips: u64,
}

/// One dead-letter row joined with its event metadata.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeadLetterRow {
    pub event_id: i64,
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub commit_lsn: u64,
    pub retry_count: u64,
    pub failed_at_ms: u64,
    #[serde(default)]
    pub error: String,
}

/// Dead-letter listing payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeadLetterListResponse {
    #[serde(default)]
    pub dead_letters: Vec<DeadLetterRow>,
}

/// Outbox retry receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RetryOutboxResponse {
    pub delivered: u64,
    pub status: String,
}

/// Dead-letter requeue receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequeueResponse {
    pub requeued: u64,
}

/// One degraded range entry.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DegradedRangeRow {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub start_lsn: u64,
    pub end_lsn: u64,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub created_at_ms: u64,
}

/// Degraded range listing payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DegradedRangeListResponse {
    #[serde(default)]
    pub degraded_ranges: Vec<DegradedRangeRow>,
}

/// Degraded range clear receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ClearDegradedResponse {
    pub cleared: bool,
}

/// Retention pass receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RetentionRunResponse {
    pub pruned: u64,
    pub archived: u64,
    pub retention_lsn: u64,
}

/// Retention watermark payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RetentionStatusResponse {
    pub retention_lsn: u64,
}
