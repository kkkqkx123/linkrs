use std::sync::Arc;

#[cfg(feature = "fulltext-search")]
use crate::sync::coordinator::SyncCoordinator;
#[cfg(feature = "qdrant")]
use crate::sync::vector_sync::VectorSyncCoordinator;
use crate::sync::DeadLetterQueue;
use crate::sync::SyncManager;

pub struct SyncManagerBuilder {
    #[cfg(feature = "fulltext-search")]
    sync_coordinator: Option<Arc<SyncCoordinator>>,
    #[cfg(feature = "qdrant")]
    vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    dead_letter_queue: Option<Arc<DeadLetterQueue>>,
}

impl Default for SyncManagerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncManagerBuilder {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "fulltext-search")]
            sync_coordinator: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            dead_letter_queue: None,
        }
    }

    #[cfg(feature = "fulltext-search")]
    pub fn with_sync_coordinator(mut self, coordinator: Arc<SyncCoordinator>) -> Self {
        self.sync_coordinator = Some(coordinator);
        self
    }

    #[cfg(feature = "qdrant")]
    pub fn with_vector_coordinator(mut self, coordinator: Arc<VectorSyncCoordinator>) -> Self {
        self.vector_coordinator = Some(coordinator);
        self
    }

    pub fn with_dead_letter_queue(mut self, dlq: Arc<DeadLetterQueue>) -> Self {
        self.dead_letter_queue = Some(dlq);
        self
    }

    pub fn build(self) -> SyncManager {
        let mut manager = SyncManager::new_without_fulltext();

        #[cfg(feature = "fulltext-search")]
        if let Some(coordinator) = self.sync_coordinator {
            manager = SyncManager::new(coordinator);
        }

        #[cfg(feature = "qdrant")]
        if let Some(vector_coordinator) = self.vector_coordinator {
            manager = manager.with_vector_coordinator(vector_coordinator);
        }

        if let Some(dlq) = self.dead_letter_queue {
            manager = manager.with_dead_letter_queue(dlq);
        }

        manager
    }
}

#[cfg(feature = "fulltext-search")]
pub struct SyncCoordinatorBuilder {
    fulltext_manager: Option<Arc<crate::search::manager::FulltextIndexManager>>,
    config: Option<crate::sync::batch::BatchConfig>,
    stats_manager: Option<Arc<crate::core::stats::StatsManager>>,
}

#[cfg(feature = "fulltext-search")]
impl Default for SyncCoordinatorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "fulltext-search")]
impl SyncCoordinatorBuilder {
    pub fn new() -> Self {
        Self {
            fulltext_manager: None,
            config: None,
            stats_manager: None,
        }
    }

    pub fn with_fulltext_manager(
        mut self,
        manager: Arc<crate::search::manager::FulltextIndexManager>,
    ) -> Self {
        self.fulltext_manager = Some(manager);
        self
    }

    pub fn with_config(mut self, config: crate::sync::batch::BatchConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_stats_manager(
        mut self,
        stats_manager: Arc<crate::core::stats::StatsManager>,
    ) -> Self {
        self.stats_manager = Some(stats_manager);
        self
    }

    pub fn build(self) -> Result<Arc<SyncCoordinator>, crate::sync::SyncError> {
        let manager = self.fulltext_manager.ok_or_else(|| {
            crate::sync::SyncError::Internal("FulltextIndexManager is required".to_string())
        })?;
        let config = self.config.unwrap_or_default();

        let mut coordinator = SyncCoordinator::new(manager, config);

        if let Some(stats) = self.stats_manager {
            coordinator = coordinator.with_stats_manager(stats);
        }

        Ok(Arc::new(coordinator))
    }
}

#[cfg(feature = "qdrant")]
pub struct VectorCoordinatorBuilder {
    vector_manager: Option<Arc<vector_client::VectorManager>>,
    embedding_service: Option<Arc<vector_client::EmbeddingService>>,
    runtime_handle: Option<tokio::runtime::Handle>,
}

#[cfg(feature = "qdrant")]
impl Default for VectorCoordinatorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "qdrant")]
impl VectorCoordinatorBuilder {
    pub fn new() -> Self {
        Self {
            vector_manager: None,
            embedding_service: None,
            runtime_handle: None,
        }
    }

    pub fn with_vector_manager(mut self, manager: Arc<vector_client::VectorManager>) -> Self {
        self.vector_manager = Some(manager);
        self
    }

    pub fn with_embedding_service(mut self, service: Arc<vector_client::EmbeddingService>) -> Self {
        self.embedding_service = Some(service);
        self
    }

    pub fn with_runtime_handle(mut self, handle: tokio::runtime::Handle) -> Self {
        self.runtime_handle = Some(handle);
        self
    }

    pub fn build(self) -> Result<Arc<VectorSyncCoordinator>, crate::sync::SyncError> {
        let manager = self.vector_manager.ok_or_else(|| {
            crate::sync::SyncError::Internal("VectorManager is required".to_string())
        })?;
        let handle = self.runtime_handle.unwrap_or_else(|| {
            tokio::runtime::Handle::try_current().expect("No tokio runtime available")
        });

        Ok(Arc::new(VectorSyncCoordinator::new(
            manager,
            self.embedding_service,
            handle,
        )))
    }
}
