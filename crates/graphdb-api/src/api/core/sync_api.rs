//! Sync Management API – Core Layer
//!
//! Provides transport layer independent sync system management operations.

use crate::sync::SyncManager;
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

    /// Get vector coordinator
    #[cfg(feature = "qdrant")]
    pub fn vector_coordinator(&self) -> Option<&Arc<crate::sync::VectorSyncCoordinator>> {
        self.sync_manager.vector_coordinator()
    }

    /// Get sync coordinator
    #[cfg(feature = "fulltext-search")]
    pub fn sync_coordinator(&self) -> &Arc<crate::sync::SyncCoordinator> {
        self.sync_manager.sync_coordinator()
    }
}
