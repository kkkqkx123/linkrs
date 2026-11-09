//! Vector Synchronization Coordinator
//!
//! Coordinates vector index updates with graph data changes. Wraps a
//! [`VectorIndexManager`](crate::VectorIndexManager) for index lifecycle and
//! adds synchronization concerns: change batching, outbox integration and
//! disabled-engine accounting.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use linkrs_core::index_events::{RebuildPhase, RebuildProgress};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::backend::VectorBackend;
use crate::vector_error::{VectorCoordinatorError, VectorCoordinatorResult};
use crate::VectorIndexManager;
use linkrs_core::Value;

pub use simvec::types::{DistanceMetric, PointId, SearchQuery, SearchResult, VectorPoint};
use simvec::{CollectionConfig, IndexMetadata, VectorFilter};

// ── Types (kept here for backward compatibility) ──────────────────────────

/// Vector point data for synchronization
#[derive(Debug, Clone)]
pub struct VectorPointData {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: HashMap<Arc<str>, Value>,
}

/// Vector change type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum VectorChangeType {
    Insert,
    Delete,
}

impl From<crate::types::ChangeType> for VectorChangeType {
    fn from(ct: crate::types::ChangeType) -> Self {
        match ct {
            crate::types::ChangeType::Insert | crate::types::ChangeType::Update => {
                VectorChangeType::Insert
            }
            crate::types::ChangeType::Delete => VectorChangeType::Delete,
        }
    }
}

/// Search options for vector search
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub query_vector: Vec<f32>,
    pub limit: usize,
    pub threshold: Option<f32>,
    pub filter: Option<VectorFilter>,
}

impl SearchOptions {
    pub fn new(
        space_id: u64,
        tag_name: impl Into<String>,
        field_name: impl Into<String>,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
            query_vector,
            limit,
            threshold: None,
            filter: None,
        }
    }

    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = Some(threshold);
        self
    }

    pub fn with_filter(mut self, filter: VectorFilter) -> Self {
        self.filter = Some(filter);
        self
    }
}

/// Vector index location identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VectorIndexLocation {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
}

/// Collection name prefix mirrors the fulltext scheme (`space_ft`): one
/// distinct prefix per index subsystem keeps drop-space cleanup and startup
/// recovery from ever confusing the two naming domains.
const VECTOR_INDEX_PREFIX: &str = "space_vec";

/// Collection granularity shared with configuration so naming stays consistent.
pub use linkrs_config::VectorCollectionGranularity as CollectionGranularity;

impl VectorIndexLocation {
    pub fn new(space_id: u64, tag_name: impl Into<String>, field_name: impl Into<String>) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
        }
    }

    pub fn to_collection_name(&self) -> String {
        self.to_collection_name_with(CollectionGranularity::Space)
    }

    pub fn to_collection_name_with(&self, granularity: CollectionGranularity) -> String {
        match granularity {
            CollectionGranularity::Space => format!("{}_{}", VECTOR_INDEX_PREFIX, self.space_id),
            CollectionGranularity::Field => format!(
                "{}_{}_{}_{}",
                VECTOR_INDEX_PREFIX, self.space_id, self.tag_name, self.field_name
            ),
        }
    }

    pub fn group_id(&self) -> String {
        self.group_id_with(CollectionGranularity::Space)
            .unwrap_or_default()
    }

    pub fn group_id_with(&self, granularity: CollectionGranularity) -> Option<String> {
        match granularity {
            CollectionGranularity::Space => Some(format!("{}_{}", self.tag_name, self.field_name)),
            CollectionGranularity::Field => None,
        }
    }
}

/// Parsed vector index location from a collection name.
#[derive(Debug, Clone)]
pub struct IndexMetadataWrapper {
    pub collection_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub index_name: Option<String>,
}

/// Runtime state of the vector engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorEngineState {
    /// Engine is disabled: user-facing vector operations fail with
    /// [`VectorCoordinatorError::EngineDisabled`]; delivery-plane batches are
    /// skipped and counted. Logical index metadata is still tracked for
    /// schema correctness.
    Disabled,
    /// Engine is active; mutations and searches execute against the backend.
    Active,
}

/// Vector change context
#[derive(Debug, Clone)]
pub struct VectorChangeContext {
    pub location: VectorIndexLocation,
    pub change_type: VectorChangeType,
    pub data: VectorPointData,
}

impl VectorChangeContext {
    pub fn new(
        space_id: u64,
        tag_name: impl Into<String>,
        field_name: impl Into<String>,
        change_type: VectorChangeType,
        data: VectorPointData,
    ) -> Self {
        Self {
            location: VectorIndexLocation::new(space_id, tag_name, field_name),
            change_type,
            data,
        }
    }
}

// ── VectorSyncCoordinator ─────────────────────────────────────────────────

/// Vector synchronization coordinator
pub struct VectorSyncCoordinator {
    index_manager: Arc<VectorIndexManager>,
    /// Vector change items skipped because the engine is disabled (delivery
    /// plane). Observable accounting for silent degradation.
    disabled_skips: std::sync::atomic::AtomicU64,
    /// Tokio runtime handle for blocking async operations from sync context.
    runtime: tokio::runtime::Handle,
    /// Live rebuild progress by index; present only while a rebuild runs.
    rebuild_progress: DashMap<(u64, String, String), RebuildProgress>,
}

impl std::fmt::Debug for VectorSyncCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("VectorSyncCoordinator");
        debug.field("index_manager", &self.index_manager);
        debug.finish()
    }
}

impl VectorSyncCoordinator {
    pub fn is_disabled_engine(&self) -> bool {
        self.index_manager.is_disabled_engine()
    }

    /// Returns the runtime state of the underlying vector engine.
    pub fn engine_state(&self) -> crate::VectorEngineState {
        if self.is_disabled_engine() {
            crate::VectorEngineState::Disabled
        } else {
            crate::VectorEngineState::Active
        }
    }

    /// Total number of vector change items skipped because the engine was
    /// disabled.
    pub fn disabled_skip_count(&self) -> u64 {
        self.disabled_skips
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Create a new vector sync coordinator with an explicit runtime handle.
    pub fn new(backend: VectorBackend, runtime: tokio::runtime::Handle) -> Self {
        Self {
            index_manager: Arc::new(VectorIndexManager::new(backend)),
            disabled_skips: std::sync::atomic::AtomicU64::new(0),
            runtime,
            rebuild_progress: DashMap::new(),
        }
    }

    /// Convenience constructor without embedding service.
    pub fn new_without_embedding(backend: VectorBackend, runtime: tokio::runtime::Handle) -> Self {
        Self::new(backend, runtime)
    }

    /// Get a reference to the underlying index manager.
    pub fn index_manager(&self) -> &Arc<VectorIndexManager> {
        &self.index_manager
    }

    /// Get the runtime handle for blocking async operations.
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    /// Get the vector backend.
    pub fn backend(&self) -> &VectorBackend {
        self.index_manager.backend()
    }

    /// Publish fence for one vector index. Delivery applies hold the read
    /// guard; rebuild holds the write guard across its critical windows.
    /// Delegates to the index manager so explicit purges share the same lock.
    pub fn publish_fence_for(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Arc<tokio::sync::RwLock<()>> {
        self.index_manager
            .publish_fence_for(space_id, tag_name, field_name)
    }

    /// Latest rebuild progress snapshot for one vector index, if any.
    pub fn rebuild_progress(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<RebuildProgress> {
        self.rebuild_progress
            .get(&(space_id, tag_name.to_string(), field_name.to_string()))
            .map(|entry| entry.clone())
    }

    /// Replace the stored rebuild progress snapshot.
    pub fn update_rebuild_progress(&self, progress: RebuildProgress) {
        let key = (
            progress.space_id,
            progress.tag_name.clone(),
            progress.field_name.clone(),
        );
        self.rebuild_progress.insert(key, progress);
    }

    /// Advance the stored rebuild phase, if a snapshot is present.
    pub fn set_rebuild_phase(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        phase: RebuildPhase,
    ) {
        let key = (space_id, tag_name.to_string(), field_name.to_string());
        if let Some(mut entry) = self.rebuild_progress.get_mut(&key) {
            entry.phase = phase;
        }
    }

    /// Drop the stored rebuild progress snapshot.
    pub fn remove_rebuild_progress(&self, space_id: u64, tag_name: &str, field_name: &str) {
        self.rebuild_progress
            .remove(&(space_id, tag_name.to_string(), field_name.to_string()));
    }

    /// Vector indexes needing operator attention: rebuilds stuck in the
    /// `Failed` phase (live data still serves; retry the rebuild). Mirrors
    /// the fulltext `inconsistent` listing so both engines share one
    /// alerting shape.
    pub fn inconsistent_vector_indexes(&self) -> Vec<RebuildProgress> {
        self.rebuild_progress
            .iter()
            .filter(|entry| entry.value().phase == RebuildPhase::Failed)
            .map(|entry| entry.value().clone())
            .collect()
    }

    // ── Index lifecycle (delegated) ───────────────────────────────────

    /// Vertex-scoped index creation. The coordinator has no schema access:
    /// callers (query planner/executor, HTTP) must reject edge type names
    /// before reaching here; vector delivery and rebuild never produce
    /// points for edges.
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: DistanceMetric,
    ) -> VectorCoordinatorResult<String> {
        self.index_manager
            .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
            .await
    }

    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<()> {
        self.index_manager
            .drop_vector_index(space_id, tag_name, field_name)
            .await
    }

    pub fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        self.index_manager
            .index_exists(space_id, tag_name, field_name)
    }

    pub fn set_index_name(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        index_name: &str,
    ) {
        self.index_manager
            .set_index_name(space_id, tag_name, field_name, index_name);
    }

    pub fn index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexMetadata> {
        self.index_manager
            .index_info(space_id, tag_name, field_name)
    }

    pub fn list_indexes(&self) -> Vec<IndexMetadataWrapper> {
        self.index_manager.list_indexes()
    }

    pub fn register_logical_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        collection_name: String,
        config: CollectionConfig,
        user_index_name: Option<String>,
    ) {
        self.index_manager.register_logical_index(
            space_id,
            tag_name,
            field_name,
            collection_name,
            config,
            user_index_name,
        );
    }

    /// Vertex-scoped index creation with full config. Same contract as
    /// [`Self::create_vector_index`]: `tag_name` is always a vertex tag.
    pub async fn create_index_with_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        config: CollectionConfig,
    ) -> VectorCoordinatorResult<String> {
        self.index_manager
            .create_index_with_config(space_id, tag_name, field_name, config)
            .await
    }

    // ── Sync: change batch handling ───────────────────────────────────

    pub async fn on_vector_change_batch(
        &self,
        contexts: Vec<crate::VectorChangeContext>,
    ) -> VectorCoordinatorResult<()> {
        self.deliver_change_batch(None, contexts).await
    }

    /// Deliver a batch into one physical collection (rebuild temp). Shares
    /// the live grouping/`group_id` logic via
    /// `prepare_change_batch_for_collection`; temp writes need no publish
    /// fence (temps have no concurrent readers), bypass the disabled-engine
    /// skip accounting of live delivery, and still fail when the engine is
    /// disabled so rebuilds abort instead of silently diverging.
    pub async fn on_vector_change_batch_to(
        &self,
        collection: &str,
        contexts: Vec<crate::VectorChangeContext>,
    ) -> VectorCoordinatorResult<()> {
        self.deliver_change_batch(Some(collection), contexts).await
    }

    async fn deliver_change_batch(
        &self,
        collection_override: Option<&str>,
        contexts: Vec<crate::VectorChangeContext>,
    ) -> VectorCoordinatorResult<()> {
        if self.is_disabled_engine() {
            self.disabled_skips
                .fetch_add(contexts.len() as u64, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                count = contexts.len(),
                total_skipped = self
                    .disabled_skips
                    .load(std::sync::atomic::Ordering::Relaxed),
                "vector engine disabled: retaining {} vector changes for retry; \
                 vector data diverges until engine recovers",
                contexts.len()
            );
            return Err(VectorCoordinatorError::EngineDisabled);
        }

        let (upsert_by_collection, delete_by_collection) = self
            .index_manager
            .prepare_change_batch_for_collection(collection_override, contexts);
        let upsert_collections = upsert_by_collection.len();
        let delete_collections = delete_by_collection.len();
        self.backend()
            .apply_grouped_batch(upsert_by_collection, delete_by_collection)
            .await?;
        debug!(
            "Applied grouped vector batch to {} upsert and {} delete collections",
            upsert_collections, delete_collections
        );
        Ok(())
    }
}
