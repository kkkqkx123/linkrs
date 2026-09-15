//! Read-only accessors, rebuild status and index facades.
use super::*;
#[cfg(feature = "fulltext")]
use crate::coordinator::SyncCoordinator;
#[cfg(any(feature = "fulltext", feature = "vector"))]
use crate::sqlite_outbox::SqliteOutbox;
#[cfg(feature = "vector")]
use crate::vector_sync::VectorSyncCoordinator;
#[cfg(any(feature = "fulltext", feature = "vector"))]
use graphdb_metrics::StatsManager;
impl super::SyncManager {
    /// Admission lock for one index rebuild. The winner holds the guard for
    /// the whole rebuild; concurrent attempts fail fast with
    /// `SyncError::RebuildBusy` instead of interleaving generations on the
    /// same outbox `tag_index_id`.
    #[cfg(any(feature = "fulltext", feature = "vector"))]
    pub(crate) fn rebuild_lock_for(
        &self,
        target: &str,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        let key = format!("{target}:{space_id}:{tag_name}:{field_name}");
        self.rebuild_locks
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    #[cfg(feature = "fulltext")]
    pub fn sync_coordinator(&self) -> &Arc<SyncCoordinator> {
        self.sync_coordinator
            .as_ref()
            .expect("SyncCoordinator not available without fulltext feature")
    }

    #[cfg(feature = "fulltext")]
    pub(crate) fn sync_coordinator_opt(&self) -> Option<&Arc<SyncCoordinator>> {
        self.sync_coordinator.as_ref()
    }

    #[cfg(any(feature = "fulltext", feature = "vector"))]
    pub(crate) fn sqlite_outbox_opt(&self) -> Option<&SqliteOutbox> {
        self.sqlite_outbox.as_deref()
    }

    #[cfg(feature = "vector")]
    pub fn vector_coordinator(&self) -> Option<&Arc<VectorSyncCoordinator>> {
        self.vector_coordinator.as_ref()
    }

    #[cfg(feature = "fulltext")]
    pub fn fulltext_manager(&self) -> Arc<graphdb_fulltext::manager::FulltextIndexManager> {
        self.sync_coordinator
            .as_ref()
            .expect("SyncCoordinator not available without fulltext feature")
            .fulltext_manager()
            .clone()
    }

    /// Non-panicking fulltext manager access for read-only diagnostics and
    /// maintenance endpoints. `None` when the fulltext target is not
    /// configured.
    #[cfg(feature = "fulltext")]
    pub fn fulltext_manager_opt(
        &self,
    ) -> Option<Arc<graphdb_fulltext::manager::FulltextIndexManager>> {
        self.sync_coordinator
            .as_ref()
            .map(|coordinator| coordinator.fulltext_manager().clone())
    }

    #[cfg(any(feature = "fulltext", feature = "vector"))]
    pub(crate) fn stats_manager_opt(&self) -> Option<Arc<StatsManager>> {
        self.stats_manager.clone()
    }

    /// Fulltext indexes whose engine is in the `Inconsistent` state. These
    /// reject writes and need operator attention (online rebuild or drop and
    /// recreate). Empty when fulltext is not configured.
    #[cfg(feature = "fulltext")]
    pub fn inconsistent_fulltext_indexes(&self) -> Vec<graphdb_fulltext::IndexMetadata> {
        self.fulltext_manager_opt()
            .map(|manager| manager.get_inconsistent_indexes())
            .unwrap_or_default()
    }

    /// Unified rebuild progress for one logical index.
    ///
    /// Fulltext and vector rebuilds share the same `RebuildProgress` shape but
    /// store it in different managers. This helper checks the fulltext manager
    /// first and then the vector coordinator, so operators poll a single
    /// endpoint instead of knowing which engine backs `(space, tag, field)`.
    /// Returns `None` when neither target has ever run a rebuild for the key.
    #[cfg(any(feature = "fulltext", feature = "vector"))]
    pub fn rebuild_progress(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<graphdb_fulltext::RebuildProgress> {
        #[cfg(feature = "fulltext")]
        if let Some(manager) = self.fulltext_manager_opt() {
            if let Some(progress) = manager.rebuild_progress(space_id, tag_name, field_name) {
                return Some(progress);
            }
        }
        #[cfg(feature = "vector")]
        if let Some(coordinator) = self.vector_coordinator.as_ref() {
            if let Some(progress) = coordinator.rebuild_progress(space_id, tag_name, field_name) {
                return Some(progress);
            }
        }
        None
    }

    /// No rebuild engines are compiled in, so no progress can exist.
    #[cfg(not(any(feature = "fulltext", feature = "vector")))]
    pub fn rebuild_progress(
        &self,
        _space_id: u64,
        _tag_name: &str,
        _field_name: &str,
    ) -> Option<graphdb_fulltext::RebuildProgress> {
        None
    }

    /// Fail non-terminal rebuild generations stranded by crashed attempts for
    /// every configured target, and collect fulltext rebuild scratch state.
    /// Idempotent; safe to run at startup before serving traffic. Missing
    /// targets count as zero instead of failing startup.
    pub async fn recover_stale_rebuilds(&self) -> Result<usize, SyncError> {
        let fulltext_failed = self.recover_stale_fulltext_rebuilds_tolerant().await?;
        let vector_failed = self.recover_stale_vector_rebuilds_tolerant().await?;
        Ok(fulltext_failed + vector_failed)
    }

    #[cfg(feature = "fulltext")]
    async fn recover_stale_fulltext_rebuilds_tolerant(&self) -> Result<usize, SyncError> {
        match self.recover_stale_fulltext_rebuilds().await {
            Ok(count) => Ok(count),
            Err(SyncError::PersistenceError(_)) => Ok(0),
            Err(error) => Err(error),
        }
    }

    #[cfg(not(feature = "fulltext"))]
    async fn recover_stale_fulltext_rebuilds_tolerant(&self) -> Result<usize, SyncError> {
        Ok(0)
    }

    #[cfg(feature = "vector")]
    async fn recover_stale_vector_rebuilds_tolerant(&self) -> Result<usize, SyncError> {
        match self.recover_stale_vector_rebuilds().await {
            Ok(count) => Ok(count),
            Err(SyncError::PersistenceError(_)) => Ok(0),
            Err(error) => Err(error),
        }
    }

    #[cfg(not(feature = "vector"))]
    async fn recover_stale_vector_rebuilds_tolerant(&self) -> Result<usize, SyncError> {
        Ok(0)
    }

    /// Sync wrapper around [`SyncManager::recover_stale_rebuilds`] for
    /// startup paths without an async context.
    pub fn recover_stale_rebuilds_sync(&self) -> Result<usize, SyncError> {
        self.execute_sync(|| async { self.recover_stale_rebuilds().await })
    }

    pub fn get_dead_letter_entries(&self) -> Vec<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_all()
        } else {
            vec![]
        }
    }

    pub fn get_unrecovered_entries(&self) -> Vec<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_unrecovered()
        } else {
            vec![]
        }
    }

    pub fn get_old_dead_letter_entries(
        &self,
        age: std::time::Duration,
    ) -> Vec<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_old_entries(age)
        } else {
            vec![]
        }
    }

    pub fn remove_dead_letter_entry(&self, index: usize) -> Option<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.remove(index)
        } else {
            None
        }
    }

    pub fn get_dlq_size(&self) -> usize {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_all().len()
        } else {
            0
        }
    }

    pub fn get_unrecovered_dlq_size(&self) -> usize {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_unrecovered().len()
        } else {
            0
        }
    }

    #[cfg(feature = "vector")]
    pub fn vector_index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord.index_exists(space_id, tag_name, field_name)
        } else {
            false
        }
    }

    #[cfg(feature = "vector")]
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: vector_search::DistanceMetric,
    ) -> Result<String, SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }

    #[cfg(feature = "vector")]
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .drop_vector_index(space_id, tag_name, field_name)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }

    #[cfg(feature = "vector")]
    pub async fn search_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>, SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            let options = crate::vector_sync::SearchOptions::new(
                space_id,
                tag_name,
                field_name,
                vector.to_vec(),
                top_k,
            );
            vector_coord
                .search_with_options(options)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }
}
