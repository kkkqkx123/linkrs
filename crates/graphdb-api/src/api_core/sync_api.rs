//! Sync Management API – Core Layer
//!
//! Provides transport layer independent sync system management operations.

use graphdb_sync::SyncManager;
use std::sync::Arc;

/// Sync Management API – Core Layer
pub struct SyncApi {
    sync_manager: Arc<SyncManager>,
}

impl SyncApi {
    /// Create a new SyncApi instance
    pub fn new(sync_manager: Arc<SyncManager>) -> Self {
        Self { sync_manager }
    }

    /// Get the sync manager
    pub fn sync_manager(&self) -> &Arc<SyncManager> {
        &self.sync_manager
    }

    /// Check if sync is running
    pub fn is_running(&self) -> bool {
        self.sync_manager.is_running()
    }

    /// Get dead letter queue size
    pub fn get_dlq_size(&self) -> usize {
        self.sync_manager.get_dlq_size()
    }

    /// Get unrecovered dead letter queue size
    pub fn get_unrecovered_dlq_size(&self) -> usize {
        self.sync_manager.get_unrecovered_dlq_size()
    }

    /// Get durable outbox delivery statistics.
    pub fn outbox_stats(&self) -> graphdb_sync::OutboxStats {
        self.sync_manager.outbox_stats()
    }

    /// Retry delivery of all pending durable outbox entries.
    pub fn retry_outbox_projection(&self) -> Result<usize, String> {
        self.sync_manager
            .retry_outbox_sync()
            .map_err(|error| error.to_string())
    }

    /// Get vector coordinator
    #[cfg(feature = "vector")]
    pub fn vector_coordinator(&self) -> Option<&Arc<graphdb_sync::VectorSyncCoordinator>> {
        self.sync_manager.vector_coordinator()
    }

    /// Get sync coordinator
    #[cfg(feature = "fulltext-search")]
    pub fn sync_coordinator(&self) -> &Arc<graphdb_sync::SyncCoordinator> {
        self.sync_manager.sync_coordinator()
    }

    pub fn sync_diagnostics(&self) -> Result<graphdb_sync::SyncDiagnostics, String> {
        self.sync_manager
            .sync_diagnostics()
            .map_err(|e| e.to_string())
    }

    pub fn list_dead_letters(
        &self,
        target: Option<&graphdb_core::types::TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<graphdb_sync::DeadLetterRow>, String> {
        self.sync_manager
            .list_dead_letters(target, index_id, generation, limit, offset)
            .map_err(|e| e.to_string())
    }

    pub fn requeue_dead_letter(&self, event_id: i64) -> Result<bool, String> {
        self.sync_manager
            .requeue_dead_letter(event_id)
            .map_err(|e| e.to_string())
    }

    pub fn requeue_dead_letters_batch(
        &self,
        target: Option<&graphdb_core::types::TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
    ) -> Result<usize, String> {
        self.sync_manager
            .requeue_dead_letters_batch(target, index_id, generation, limit)
            .map_err(|e| e.to_string())
    }

    pub fn list_degraded_ranges(
        &self,
        target: Option<&graphdb_core::types::TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
    ) -> Result<Vec<graphdb_sync::DegradedRangeRow>, String> {
        self.sync_manager
            .list_degraded_ranges(target, index_id, generation)
            .map_err(|e| e.to_string())
    }

    pub fn clear_degraded_range(
        &self,
        target: &graphdb_core::types::TargetId,
        index_id: u64,
        generation: u64,
        start_lsn: graphdb_core::types::CommitLsn,
        end_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<bool, String> {
        self.sync_manager
            .clear_degraded_range(target, index_id, generation, start_lsn, end_lsn)
            .map_err(|e| e.to_string())
    }

    pub fn retention_lsn(&self) -> Result<graphdb_core::types::CommitLsn, String> {
        self.sync_manager.retention_lsn().map_err(|e| e.to_string())
    }

    pub fn prune_applied_events(
        &self,
        retention_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<u64, String> {
        self.sync_manager
            .prune_applied_events(retention_lsn)
            .map_err(|e| e.to_string())
    }

    pub fn run_retention_once(
        &self,
        grace_lsn_distance: u64,
        max_age_ms: u64,
    ) -> Result<(u64, u64, u64), String> {
        self.sync_manager
            .run_retention_once(grace_lsn_distance, max_age_ms)
            .map_err(|e| e.to_string())
    }
}
