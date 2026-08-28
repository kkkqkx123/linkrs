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
}
