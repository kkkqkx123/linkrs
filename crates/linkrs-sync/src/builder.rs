#[cfg(any(feature = "fulltext", feature = "vector"))]
use std::sync::Arc;

#[cfg(feature = "fulltext")]
use crate::fulltext_sync::FulltextSyncCoordinator;
#[cfg(feature = "vector")]
use crate::vector_sync::VectorSyncCoordinator;
use crate::SyncManager;

pub struct SyncManagerBuilder {
    #[cfg(feature = "fulltext")]
    sync_coordinator: Option<Arc<FulltextSyncCoordinator>>,
    #[cfg(feature = "vector")]
    vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    outbox_path: Option<std::path::PathBuf>,
}

impl Default for SyncManagerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncManagerBuilder {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "fulltext")]
            sync_coordinator: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
            outbox_path: None,
        }
    }

    #[cfg(feature = "fulltext")]
    pub fn with_sync_coordinator(mut self, coordinator: Arc<FulltextSyncCoordinator>) -> Self {
        self.sync_coordinator = Some(coordinator);
        self
    }

    #[cfg(feature = "vector")]
    pub fn with_vector_coordinator(mut self, coordinator: Arc<VectorSyncCoordinator>) -> Self {
        self.vector_coordinator = Some(coordinator);
        self
    }

    pub fn with_outbox_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.outbox_path = Some(path.into());
        self
    }

    pub fn build(self) -> Result<SyncManager, crate::manager::SyncError> {
        let mut manager = SyncManager::new_without_fulltext();

        #[cfg(feature = "fulltext")]
        if let Some(coordinator) = self.sync_coordinator {
            manager = SyncManager::new(coordinator);
        }

        #[cfg(feature = "vector")]
        if let Some(vector_coordinator) = self.vector_coordinator {
            manager = manager.with_vector_coordinator(vector_coordinator);
        }

        if let Some(path) = self.outbox_path {
            manager.configure_outbox(path)?;
        }

        Ok(manager)
    }
}

#[cfg(feature = "fulltext")]
pub struct FulltextSyncCoordinatorBuilder {
    fulltext_manager: Option<Arc<linkrs_fulltext::manager::FulltextIndexManager>>,
    stats_manager: Option<Arc<linkrs_metrics::StatsManager>>,
}

#[cfg(feature = "fulltext")]
impl Default for FulltextSyncCoordinatorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "fulltext")]
impl FulltextSyncCoordinatorBuilder {
    pub fn new() -> Self {
        Self {
            fulltext_manager: None,
            stats_manager: None,
        }
    }

    pub fn with_fulltext_manager(
        mut self,
        manager: Arc<linkrs_fulltext::manager::FulltextIndexManager>,
    ) -> Self {
        self.fulltext_manager = Some(manager);
        self
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<linkrs_metrics::StatsManager>) -> Self {
        self.stats_manager = Some(stats_manager);
        self
    }

    pub fn build(self) -> Result<Arc<FulltextSyncCoordinator>, crate::SyncError> {
        let manager = self.fulltext_manager.ok_or_else(|| {
            crate::SyncError::Internal("FulltextIndexManager is required".to_string())
        })?;

        let mut coordinator = FulltextSyncCoordinator::new(manager);

        if let Some(stats) = self.stats_manager {
            coordinator = coordinator.with_stats_manager(stats);
        }

        Ok(Arc::new(coordinator))
    }
}

#[cfg(feature = "vector")]
pub struct VectorCoordinatorBuilder {
    backend: Option<crate::backend::VectorBackend>,
    #[cfg(feature = "vector-qdrant")]
    vector_manager: Option<Arc<vector_client::VectorManager>>,
    runtime_handle: Option<tokio::runtime::Handle>,
}

#[cfg(feature = "vector")]
impl Default for VectorCoordinatorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "vector")]
impl VectorCoordinatorBuilder {
    pub fn new() -> Self {
        Self {
            backend: None,
            #[cfg(feature = "vector-qdrant")]
            vector_manager: None,
            runtime_handle: None,
        }
    }

    pub fn with_backend(mut self, backend: crate::backend::VectorBackend) -> Self {
        self.backend = Some(backend);
        self
    }

    #[cfg(feature = "vector-qdrant")]
    pub fn with_vector_manager(mut self, manager: Arc<vector_client::VectorManager>) -> Self {
        self.vector_manager = Some(manager);
        self
    }

    pub fn with_runtime_handle(mut self, handle: tokio::runtime::Handle) -> Self {
        self.runtime_handle = Some(handle);
        self
    }

    pub fn build(self) -> Result<Arc<VectorSyncCoordinator>, crate::SyncError> {
        let backend = if let Some(backend) = self.backend {
            backend
        } else {
            #[cfg(feature = "vector-qdrant")]
            if let Some(manager) = self.vector_manager {
                crate::backend::VectorBackend::qdrant(manager)
            } else {
                return Err(crate::SyncError::Internal(
                    "VectorBackend or VectorManager is required".to_string(),
                ));
            }
            #[cfg(not(feature = "vector-qdrant"))]
            return Err(crate::SyncError::Internal(
                "VectorBackend is required".to_string(),
            ));
        };
        let Some(handle) = self
            .runtime_handle
            .or_else(|| tokio::runtime::Handle::try_current().ok())
        else {
            return Err(crate::SyncError::Internal(
                "VectorCoordinatorBuilder requires an explicit runtime handle outside a tokio runtime"
                    .to_string(),
            ));
        };
        Ok(Arc::new(VectorSyncCoordinator::new(backend, handle)))
    }
}
