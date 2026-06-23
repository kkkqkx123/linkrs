//! Vector Synchronization Coordinator
//!
//! Coordinates vector index updates with graph data changes.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::core::types::TransactionId;
use crate::core::{Value, Vertex};
use crate::sync::vector_error::{VectorCoordinatorError, VectorCoordinatorResult};

use vector_client::{
    EmbeddingService, FilterCondition, SearchQuery, SearchResult, VectorFilter, VectorManager,
    VectorPoint,
};

/// Vector point data for synchronization
#[derive(Debug, Clone)]
pub struct VectorPointData {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: HashMap<String, Value>,
}

/// Vector change type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum VectorChangeType {
    Insert,
    Delete,
}

impl From<crate::sync::types::ChangeType> for VectorChangeType {
    fn from(ct: crate::sync::types::ChangeType) -> Self {
        match ct {
            crate::sync::types::ChangeType::Insert | crate::sync::types::ChangeType::Update => {
                VectorChangeType::Insert
            }
            crate::sync::types::ChangeType::Delete => VectorChangeType::Delete,
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

const VECTOR_INDEX_PREFIX: &str = "space";

impl VectorIndexLocation {
    pub fn new(space_id: u64, tag_name: impl Into<String>, field_name: impl Into<String>) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
        }
    }

    /// Generate collection name from this index location.
    ///
    /// **ARCHITECTURAL NOTE**: This uses space-level collection granularity.
    /// All vector indexes within the same space share a single physical collection,
    /// with logical isolation via the `group_id` field in the payload.
    ///
    /// **Implications**:
    /// - Different (tag, field) combinations in the same space cannot have different
    ///   vector dimensions or distance metrics.
    /// - Deletion by vertex_id removes all vectors for that vertex across all
    ///   (tag, field) combinations in the space.
    /// - This is a deliberate design choice for resource efficiency. If finer
    ///   isolation is needed, change this to use (space_id, tag_name, field_name)
    ///   as the collection name.
    pub fn to_collection_name(&self) -> String {
        format!("{}_{}", VECTOR_INDEX_PREFIX, self.space_id)
    }

    /// Generate group ID for logical isolation within a space-level collection.
    /// This is used as a filter condition in vector searches.
    pub fn group_id(&self) -> String {
        format!("{}_{}", self.tag_name, self.field_name)
    }
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

/// Pending vector index update
#[derive(Debug, Clone)]
pub struct PendingVectorUpdate {
    pub txn_id: TransactionId,
    pub sequence: u64,
    pub context: VectorChangeContext,
}

impl PendingVectorUpdate {
    pub fn new(txn_id: TransactionId, sequence: u64, context: VectorChangeContext) -> Self {
        Self {
            txn_id,
            sequence,
            context,
        }
    }
}

/// Vector transaction buffer configuration
#[derive(Debug, Clone)]
pub struct VectorTransactionBufferConfig {
    pub max_buffer_size: usize,
    pub flush_timeout_ms: u64,
}

impl Default for VectorTransactionBufferConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 1000,
            flush_timeout_ms: 100,
        }
    }
}

/// Vector transaction buffer
pub struct VectorTransactionBuffer {
    buffers: DashMap<TransactionId, Vec<PendingVectorUpdate>>,
    config: VectorTransactionBufferConfig,
}

impl std::fmt::Debug for VectorTransactionBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorTransactionBuffer")
            .field("buffers", &self.buffers.len())
            .field("config", &self.config)
            .finish()
    }
}

impl VectorTransactionBuffer {
    pub fn new(config: VectorTransactionBufferConfig) -> Self {
        Self {
            buffers: DashMap::new(),
            config,
        }
    }

    pub fn config(&self) -> &VectorTransactionBufferConfig {
        &self.config
    }

    /// Add a pending vector update
    pub fn add_update(
        &self,
        txn_id: TransactionId,
        update: PendingVectorUpdate,
    ) -> Result<(), VectorBufferError> {
        let mut buffer = self.buffers.entry(txn_id).or_default();

        if buffer.len() >= self.config.max_buffer_size {
            return Err(VectorBufferError::BufferFull(format!(
                "Buffer full for transaction {:?}",
                txn_id
            )));
        }

        buffer.push(update);
        Ok(())
    }

    /// Peek at pending updates without removing them from the buffer
    pub fn peek_updates(&self, txn_id: TransactionId) -> Vec<PendingVectorUpdate> {
        self.buffers
            .get(&txn_id)
            .map(|entry| entry.value().clone())
            .unwrap_or_default()
    }

    /// Get and clear pending updates for a transaction
    pub fn take_updates(&self, txn_id: TransactionId) -> Vec<PendingVectorUpdate> {
        self.buffers
            .remove(&txn_id)
            .map(|(_, updates)| updates)
            .unwrap_or_default()
    }

    /// Get and remove updates with sequence number greater than the given sequence.
    /// This is used for rollback operations where you want to remove operations
    /// that occurred after a specific savepoint sequence.
    pub fn take_updates_after_sequence(
        &self,
        txn_id: TransactionId,
        sequence: u64,
    ) -> Vec<PendingVectorUpdate> {
        if let Some(mut buffer) = self.buffers.get_mut(&txn_id) {
            // Split into two groups: keep (<= sequence) and take (> sequence)
            let mut to_take = Vec::new();
            buffer.retain(|update| {
                if update.sequence > sequence {
                    to_take.push(update.clone());
                    false
                } else {
                    true
                }
            });
            return to_take;
        }
        Vec::new()
    }

    pub fn truncate_updates(&self, txn_id: TransactionId, sequence: u64) {
        if let Some(mut buffer) = self.buffers.get_mut(&txn_id) {
            buffer.retain(|update| update.sequence <= sequence);
        }
    }

    /// Check if there are pending updates
    pub fn has_pending_updates(&self, txn_id: TransactionId) -> bool {
        if let Some(buffer) = self.buffers.get(&txn_id) {
            !buffer.is_empty()
        } else {
            false
        }
    }

    /// Cleanup buffer for a transaction
    pub fn cleanup(&self, txn_id: TransactionId) {
        self.buffers.remove(&txn_id);
    }

    pub fn pending_sequence(&self, txn_id: TransactionId) -> u64 {
        self.buffers
            .get(&txn_id)
            .and_then(|buffer| buffer.iter().map(|update| update.sequence).max())
            .unwrap_or(0)
    }
}

/// Vector buffer error
#[derive(Debug, thiserror::Error)]
pub enum VectorBufferError {
    #[error("Buffer full: {0}")]
    BufferFull(String),

    #[error("Transaction not found: {0}")]
    TransactionNotFound(TransactionId),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Vector synchronization coordinator
pub struct VectorSyncCoordinator {
    vector_manager: Arc<VectorManager>,
    embedding_service: Option<Arc<EmbeddingService>>,
    transaction_buffer: Option<Arc<VectorTransactionBuffer>>,
    /// Tracks registered logical indexes by key "space_{space_id}_{tag}_{field}" -> metadata
    logical_indexes: DashMap<String, vector_client::manager::IndexMetadata>,
    /// Tokio runtime handle for blocking async operations from sync context.
    /// Using `Handle` instead of `&Runtime` avoids lifetime issues while allowing
    /// the caller (API layer or tests) to control the runtime lifecycle.
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for VectorSyncCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorSyncCoordinator")
            .field("vector_manager", &self.vector_manager)
            .field("embedding_service", &self.embedding_service.is_some())
            .field("logical_index_count", &self.logical_indexes.len())
            .finish()
    }
}

impl VectorSyncCoordinator {
    pub fn is_disabled_engine(&self) -> bool {
        self.vector_manager.engine().name() == "disabled"
    }

    /// Create a new vector sync coordinator with an explicit runtime handle.
    ///
    /// The caller is responsible for ensuring the runtime outlives the coordinator.
    /// In async contexts, use `Handle::current()` or `Runtime::handle()`.
    /// In sync contexts (e.g. tests), create a runtime and pass its handle.
    pub fn new(
        vector_manager: Arc<VectorManager>,
        embedding_service: Option<Arc<EmbeddingService>>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            vector_manager,
            embedding_service,
            transaction_buffer: None,
            logical_indexes: DashMap::new(),
            runtime,
        }
    }

    /// Create with transaction buffer support
    pub fn with_transaction_buffer(
        vector_manager: Arc<VectorManager>,
        embedding_service: Option<Arc<EmbeddingService>>,
        config: VectorTransactionBufferConfig,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            vector_manager,
            embedding_service,
            transaction_buffer: Some(Arc::new(VectorTransactionBuffer::new(config))),
            logical_indexes: DashMap::new(),
            runtime,
        }
    }

    /// Get the runtime handle for blocking async operations
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    fn logical_index_key(space_id: u64, tag_name: &str, field_name: &str) -> String {
        format!("space_idx_{}_{}_{}", space_id, tag_name, field_name)
    }

    /// Get the vector manager
    pub fn vector_manager(&self) -> &Arc<VectorManager> {
        &self.vector_manager
    }

    /// Get the embedding service
    pub fn embedding_service(&self) -> Option<&Arc<EmbeddingService>> {
        self.embedding_service.as_ref()
    }

    /// Get the transaction buffer
    pub fn transaction_buffer(&self) -> Option<&Arc<VectorTransactionBuffer>> {
        self.transaction_buffer.as_ref()
    }

    /// Create a vector index (logical index in shared collection)
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: vector_client::DistanceMetric,
    ) -> VectorCoordinatorResult<String> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        if self.is_disabled_engine() {
            let logical_key = Self::logical_index_key(space_id, tag_name, field_name);
            let meta = vector_client::manager::IndexMetadata::new(
                collection_name.clone(),
                vector_client::CollectionConfig::new(vector_size, distance),
            );
            self.logical_indexes.insert(logical_key, meta);
            info!(
                "Logical vector index created in disabled mode: space={} tag={} field={} in collection {}",
                space_id, tag_name, field_name, collection_name
            );
            return Ok(collection_name);
        }

        let hnsw_config = vector_client::HnswConfig::new(16, 100).with_payload_m(16);
        let config =
            vector_client::CollectionConfig::new(vector_size, distance).with_hnsw(hnsw_config);

        // Only create the physical collection if it doesn't exist yet
        if !self.vector_manager.index_exists(&collection_name) {
            self.vector_manager
                .create_index(&collection_name, config.clone())
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            // Create payload index for group_id filtering (best-effort, log on failure)
            if let Err(e) = self
                .vector_manager
                .engine()
                .create_payload_index(
                    &collection_name,
                    "group_id",
                    vector_client::types::PayloadSchemaType::Keyword,
                )
                .await
            {
                tracing::warn!(
                    "Failed to create payload index for group_id in collection '{}': {}",
                    collection_name,
                    e
                );
            }
        } else {
            if let Some(existing_meta) = self.vector_manager.get_index_metadata(&collection_name) {
                if existing_meta.config.vector_size != vector_size
                    || existing_meta.config.distance != distance
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing_meta.config.vector_size,
                        existing_dist: format!("{:?}", existing_meta.config.distance),
                        requested_size: vector_size,
                        requested_dist: format!("{:?}", distance),
                    });
                }
            }
        }

        // Register logical index with the actual config used
        let logical_key = Self::logical_index_key(space_id, tag_name, field_name);
        let meta = vector_client::manager::IndexMetadata::new(collection_name.clone(), config);
        self.logical_indexes.insert(logical_key, meta);

        info!(
            "Logical vector index created: space={} tag={} field={} in collection {}",
            space_id, tag_name, field_name, collection_name
        );
        Ok(collection_name)
    }

    /// Drop a vector index (remove logical index, physical collection remains)
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<()> {
        let logical_key = Self::logical_index_key(space_id, tag_name, field_name);
        self.logical_indexes.remove(&logical_key);

        // Don't delete the physical collection as other indexes may be using it
        // Just mark that this logical index no longer exists

        info!(
            "Logical vector index dropped: space={} tag={} field={}",
            space_id, tag_name, field_name
        );
        Ok(())
    }

    /// Handle vertex insertion
    pub async fn on_vertex_inserted(
        &self,
        space_id: u64,
        vertex: &Vertex,
    ) -> VectorCoordinatorResult<()> {
        self.upsert_vertex_vectors(space_id, vertex).await
    }

    /// Upsert vectors for a vertex
    async fn upsert_vertex_vectors(
        &self,
        space_id: u64,
        vertex: &Vertex,
    ) -> VectorCoordinatorResult<()> {
        let mut points_by_collection: HashMap<String, Vec<VectorPoint>> = HashMap::new();

        for tag in &vertex.tags {
            for (field_name, value) in &tag.properties {
                let collection_name =
                    VectorIndexLocation::new(space_id, &tag.name, field_name).to_collection_name();

                if self.vector_manager.index_exists(&collection_name) {
                    if let Some(vector) = value.as_vector() {
                        let point_id = format!("{}_{}_{}", vertex.vid, tag.name, field_name);
                        let mut payload = HashMap::new();
                        payload.insert(
                            "vertex_id".to_string(),
                            serde_json::to_value(vertex.vid).unwrap_or(serde_json::Value::Null),
                        );
                        payload.insert(
                            "group_id".to_string(),
                            serde_json::to_value(
                                VectorIndexLocation::new(space_id, &tag.name, field_name)
                                    .group_id(),
                            )
                            .unwrap_or(serde_json::Value::Null),
                        );
                        payload.insert(
                            "tags".to_string(),
                            serde_json::to_value(
                                vertex
                                    .tags
                                    .iter()
                                    .map(|t| t.name.clone())
                                    .collect::<Vec<_>>(),
                            )
                            .unwrap_or(serde_json::Value::Null),
                        );
                        payload.insert(
                            "field".to_string(),
                            serde_json::Value::String(field_name.clone()),
                        );

                        let point = VectorPoint::new(point_id.clone(), vector.clone())
                            .with_payload(payload);

                        points_by_collection
                            .entry(collection_name)
                            .or_default()
                            .push(point);
                    }
                }
            }
        }

        for (collection_name, points) in points_by_collection {
            let points_count = points.len();
            if points_count == 1 {
                self.vector_manager
                    .upsert(&collection_name, points.into_iter().next().unwrap())
                    .await?;
            } else if !points.is_empty() {
                self.vector_manager
                    .upsert_batch(&collection_name, points)
                    .await?;
                debug!(
                    "Batch upserted {} vectors for vertex {} in collection {}",
                    points_count, vertex.vid, collection_name
                );
            }
        }

        Ok(())
    }

    /// Handle vertex update
    pub async fn on_vertex_updated(
        &self,
        space_id: u64,
        vertex: &Vertex,
        changed_fields: &[String],
    ) -> VectorCoordinatorResult<()> {
        let mut points_to_upsert: HashMap<String, Vec<VectorPoint>> = HashMap::new();
        let mut points_to_delete: HashMap<String, Vec<String>> = HashMap::new();

        for tag in &vertex.tags {
            for field_name in changed_fields {
                if let Some(value) = tag.properties.get(field_name) {
                    let collection_name = VectorIndexLocation::new(space_id, &tag.name, field_name)
                        .to_collection_name();

                    if self.vector_manager.index_exists(&collection_name) {
                        let point_id = format!("{}_{}_{}", vertex.vid, tag.name, field_name);

                        if let Some(vector) = value.as_vector() {
                            let mut payload = HashMap::new();
                            payload.insert(
                                "vertex_id".to_string(),
                                serde_json::to_value(vertex.vid).unwrap_or(serde_json::Value::Null),
                            );
                            payload.insert(
                                "group_id".to_string(),
                                serde_json::to_value(
                                    VectorIndexLocation::new(space_id, &tag.name, field_name)
                                        .group_id(),
                                )
                                .unwrap_or(serde_json::Value::Null),
                            );
                            payload.insert(
                                "tags".to_string(),
                                serde_json::to_value(
                                    vertex
                                        .tags
                                        .iter()
                                        .map(|t| t.name.clone())
                                        .collect::<Vec<_>>(),
                                )
                                .unwrap_or(serde_json::Value::Null),
                            );
                            payload.insert(
                                "field".to_string(),
                                serde_json::Value::String(field_name.clone()),
                            );

                            let point = VectorPoint::new(point_id.clone(), vector.clone())
                                .with_payload(payload);

                            points_to_upsert
                                .entry(collection_name)
                                .or_default()
                                .push(point);
                        } else {
                            points_to_delete
                                .entry(collection_name)
                                .or_default()
                                .push(point_id);
                        }
                    }
                }
            }
        }

        for (collection_name, points) in points_to_upsert {
            let points_count = points.len();
            if points_count == 1 {
                self.vector_manager
                    .upsert(&collection_name, points.into_iter().next().unwrap())
                    .await?;
            } else if !points.is_empty() {
                self.vector_manager
                    .upsert_batch(&collection_name, points)
                    .await?;
                debug!(
                    "Batch updated {} vectors for vertex {} in collection {}",
                    points_count, vertex.vid, collection_name
                );
            }
        }

        for (collection_name, point_ids) in points_to_delete {
            let point_ids_count = point_ids.len();
            if point_ids_count == 1 {
                self.vector_manager
                    .delete(&collection_name, &point_ids[0])
                    .await?;
            } else if !point_ids.is_empty() {
                let refs: Vec<&str> = point_ids.iter().map(|s| s.as_str()).collect();
                self.vector_manager
                    .delete_batch(&collection_name, refs)
                    .await?;
                debug!(
                    "Batch deleted {} vectors for vertex {} from collection {}",
                    point_ids_count, vertex.vid, collection_name
                );
            }
        }

        Ok(())
    }

    /// Handle vertex deletion
    pub async fn on_vertex_deleted(
        &self,
        space_id: u64,
        _tag_name: &str,
        vertex_id: &Value,
    ) -> VectorCoordinatorResult<()> {
        let collection_name = VectorIndexLocation::new(space_id, "", "").to_collection_name();

        let filter = VectorFilter::new().must(FilterCondition::match_value(
            "vertex_id",
            format!("{}", vertex_id),
        ));

        self.vector_manager
            .delete_by_filter(&collection_name, filter)
            .await?;

        debug!(
            "Deleted vectors for vertex {} from collection {}",
            vertex_id, collection_name
        );
        Ok(())
    }

    /// Handle vector change (transaction mode - buffer the operation)
    pub fn buffer_vector_change(
        &self,
        txn_id: TransactionId,
        ctx: VectorChangeContext,
    ) -> Result<(), VectorCoordinatorError> {
        self.buffer_vector_change_with_sequence(txn_id, 0, ctx)
    }

    pub fn buffer_vector_change_with_sequence(
        &self,
        txn_id: TransactionId,
        sequence: u64,
        ctx: VectorChangeContext,
    ) -> Result<(), VectorCoordinatorError> {
        if let Some(ref buffer) = self.transaction_buffer {
            let update = PendingVectorUpdate::new(txn_id, sequence, ctx);
            buffer.add_update(txn_id, update).map_err(|e| {
                VectorCoordinatorError::BufferError(format!(
                    "Failed to buffer vector update: {}",
                    e
                ))
            })?;
            Ok(())
        } else {
            Err(VectorCoordinatorError::BufferError(
                "Transaction buffer not initialized".to_string(),
            ))
        }
    }

    /// Handle vector change (direct sync mode)
    pub async fn on_vector_change(&self, ctx: VectorChangeContext) -> VectorCoordinatorResult<()> {
        if self.is_disabled_engine() {
            return Ok(());
        }

        let collection_name = ctx.location.to_collection_name();
        let point_id = ctx.data.id.to_string();

        match ctx.change_type {
            VectorChangeType::Insert => {
                let vector = ctx.data.vector;
                let mut json_payload: HashMap<String, serde_json::Value> = ctx
                    .data
                    .payload
                    .into_iter()
                    .filter_map(|(k, v)| serde_json::to_value(&v).ok().map(|json| (k, json)))
                    .collect();

                json_payload.insert(
                    "group_id".to_string(),
                    serde_json::to_value(ctx.location.group_id())
                        .unwrap_or(serde_json::Value::Null),
                );

                let point = VectorPoint::new(point_id, vector).with_payload(json_payload);

                self.vector_manager.upsert(&collection_name, point).await?;
            }
            VectorChangeType::Delete => {
                self.vector_manager
                    .delete(&collection_name, &point_id)
                    .await?;
            }
        }

        Ok(())
    }

    /// Commit transaction: flush buffered vector updates
    pub async fn commit_transaction(&self, txn_id: TransactionId) -> VectorCoordinatorResult<()> {
        if let Some(ref buffer) = self.transaction_buffer {
            // Peek at updates without removing them from the buffer.
            // This prevents data loss: if batch processing fails partway,
            // the buffer still has all pending updates for retry.
            let updates = buffer.peek_updates(txn_id);

            if !updates.is_empty() {
                debug!(
                    "Committing {} vector updates for transaction {:?}",
                    updates.len(),
                    txn_id
                );

                // Use batch processing for efficiency (groups by collection)
                let contexts: Vec<VectorChangeContext> =
                    updates.into_iter().map(|u| u.context).collect();
                self.on_vector_change_batch(contexts).await?;

                // All operations succeeded — now safely remove from buffer
                buffer.take_updates(txn_id);
            }
        }

        Ok(())
    }

    /// Rollback transaction: clear buffered vector updates
    pub async fn rollback_transaction(&self, txn_id: TransactionId) {
        if let Some(ref buffer) = self.transaction_buffer {
            buffer.cleanup(txn_id);
            debug!("Rolled back vector updates for transaction {:?}", txn_id);
        }
    }

    pub fn truncate_transaction(
        &self,
        txn_id: TransactionId,
        sequence: u64,
    ) -> Result<(), VectorCoordinatorError> {
        if let Some(ref buffer) = self.transaction_buffer {
            buffer.truncate_updates(txn_id, sequence);
        }
        Ok(())
    }

    /// Handle batch vector changes
    pub async fn on_vector_change_batch(
        &self,
        contexts: Vec<VectorChangeContext>,
    ) -> VectorCoordinatorResult<()> {
        if self.is_disabled_engine() {
            return Ok(());
        }

        let mut upsert_by_collection: HashMap<String, Vec<VectorPoint>> = HashMap::new();
        let mut delete_by_collection: HashMap<String, Vec<String>> = HashMap::new();

        for ctx in contexts {
            let collection_name = ctx.location.to_collection_name();
            let point_id = ctx.data.id.to_string();

            match ctx.change_type {
                VectorChangeType::Insert => {
                    let vector = ctx.data.vector;
                    let mut json_payload: HashMap<String, serde_json::Value> = ctx
                        .data
                        .payload
                        .into_iter()
                        .filter_map(|(k, v)| serde_json::to_value(&v).ok().map(|json| (k, json)))
                        .collect();

                    json_payload.insert(
                        "group_id".to_string(),
                        serde_json::to_value(ctx.location.group_id())
                            .unwrap_or(serde_json::Value::Null),
                    );

                    let point = VectorPoint::new(point_id, vector).with_payload(json_payload);

                    upsert_by_collection
                        .entry(collection_name)
                        .or_default()
                        .push(point);
                }
                VectorChangeType::Delete => {
                    delete_by_collection
                        .entry(collection_name)
                        .or_default()
                        .push(point_id);
                }
            }
        }

        for (collection_name, points) in upsert_by_collection {
            let points_count = points.len();
            if points_count == 1 {
                self.vector_manager
                    .upsert(&collection_name, points.into_iter().next().unwrap())
                    .await?;
            } else if !points.is_empty() {
                self.vector_manager
                    .upsert_batch(&collection_name, points)
                    .await?;
                debug!(
                    "Batch upserted {} vectors to collection {}",
                    points_count, collection_name
                );
            }
        }

        for (collection_name, point_ids) in delete_by_collection {
            let point_ids_count = point_ids.len();
            if point_ids_count == 1 {
                self.vector_manager
                    .delete(&collection_name, &point_ids[0])
                    .await?;
            } else if !point_ids.is_empty() {
                let refs: Vec<&str> = point_ids.iter().map(|s| s.as_str()).collect();
                self.vector_manager
                    .delete_batch(&collection_name, refs)
                    .await?;
                debug!(
                    "Batch deleted {} vectors from collection {}",
                    point_ids_count, collection_name
                );
            }
        }

        Ok(())
    }

    /// Search for similar vectors
    pub async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Ok(Vec::new());
        }
        let results = self.vector_manager.search(collection, query).await?;
        Ok(results)
    }

    /// Search with options
    pub async fn search_with_options(
        &self,
        options: SearchOptions,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Ok(Vec::new());
        }
        let collection_name =
            VectorIndexLocation::new(options.space_id, &options.tag_name, &options.field_name)
                .to_collection_name();

        let mut query = SearchQuery::new(options.query_vector, options.limit);

        if let Some(threshold) = options.threshold {
            query = query.with_score_threshold(threshold);
        }

        // Inject group_id filter to scope search to the correct (tag, field) group
        let group_id = format!("{}_{}", options.tag_name, options.field_name);
        let mut filter = options.filter.unwrap_or_default();
        filter = filter.must(FilterCondition::match_value("group_id", group_id));
        query = query.with_filter(filter);

        let results = self.search(&collection_name, query).await?;
        Ok(results)
    }

    /// Search with space_id and tag/field names
    pub async fn search_by_location(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Ok(Vec::new());
        }
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        let filter = VectorFilter::new().must(FilterCondition::match_value(
            "group_id",
            format!("{}_{}", tag_name, field_name),
        ));
        let query = SearchQuery::new(query_vector, limit).with_filter(filter);
        self.search(&collection_name, query).await
    }

    /// Search with threshold
    pub async fn search_with_threshold(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Ok(Vec::new());
        }
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        let filter = VectorFilter::new().must(FilterCondition::match_value(
            "group_id",
            format!("{}_{}", tag_name, field_name),
        ));
        let query = SearchQuery::new(query_vector, limit)
            .with_score_threshold(threshold)
            .with_filter(filter);
        self.search(&collection_name, query).await
    }

    /// Search with filter
    pub async fn search_with_filter(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Ok(Vec::new());
        }
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        let group_id = format!("{}_{}", tag_name, field_name);
        let filter = filter.must(FilterCondition::match_value("group_id", group_id));
        let query = SearchQuery::new(query_vector, limit).with_filter(filter);
        self.search(&collection_name, query).await
    }

    /// Embed text to vector
    pub async fn embed_text(&self, text: &str) -> VectorCoordinatorResult<Vec<f32>> {
        if let Some(embedding) = &self.embedding_service {
            let vector = embedding
                .embed(text)
                .await
                .map_err(|e| VectorCoordinatorError::EmbeddingError(e.to_string()))?;
            Ok(vector)
        } else {
            Err(VectorCoordinatorError::EmbeddingError(
                "Embedding service not available".to_string(),
            ))
        }
    }

    /// Check if index exists (logical index)
    pub fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        let logical_key = Self::logical_index_key(space_id, tag_name, field_name);
        self.logical_indexes.contains_key(&logical_key)
    }

    /// List all indexes (logical indexes)
    pub fn list_indexes(&self) -> Vec<crate::sync::vector_sync::IndexMetadataWrapper> {
        self.logical_indexes
            .iter()
            .map(|pair| {
                let key = pair.key();
                let parts: Vec<&str> = key.split('_').collect();
                let (space_id, tag_name, field_name) =
                    if parts.len() >= 5 && parts[0] == "space" && parts[1] == "idx" {
                        let sid: u64 = parts[2].parse().unwrap_or(0);
                        let tag = parts[3..parts.len() - 1].join("_");
                        let field = parts[parts.len() - 1].to_string();
                        (sid, tag, field)
                    } else {
                        (0, String::new(), String::new())
                    };
                crate::sync::vector_sync::IndexMetadataWrapper {
                    collection_name: pair.value().name.clone(),
                    space_id,
                    tag_name,
                    field_name,
                    index_name: pair.value().index_name.clone(),
                }
            })
            .collect()
    }

    /// Register a logical index (for disabled-engine mode or external registration)
    pub fn register_logical_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        collection_name: String,
        config: vector_client::CollectionConfig,
        user_index_name: Option<String>,
    ) {
        let logical_key = Self::logical_index_key(space_id, tag_name, field_name);
        let meta = if let Some(idx_name) = user_index_name {
            vector_client::manager::IndexMetadata::with_index_name(
                collection_name,
                config,
                idx_name,
            )
        } else {
            vector_client::manager::IndexMetadata::new(collection_name, config)
        };
        self.logical_indexes.insert(logical_key, meta);
    }

    /// Create vector index with config (logical index in shared collection)
    pub async fn create_index_with_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        config: vector_client::CollectionConfig,
    ) -> VectorCoordinatorResult<String> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        if !self.vector_manager.index_exists(&collection_name) {
            self.vector_manager
                .create_index(&collection_name, config.clone())
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            // Create payload index for group_id filtering (best-effort, log on failure)
            if let Err(e) = self
                .vector_manager
                .engine()
                .create_payload_index(
                    &collection_name,
                    "group_id",
                    vector_client::types::PayloadSchemaType::Keyword,
                )
                .await
            {
                tracing::warn!(
                    "Failed to create payload index for group_id in collection '{}': {}",
                    collection_name,
                    e
                );
            }
        } else {
            if let Some(existing_meta) = self.vector_manager.get_index_metadata(&collection_name) {
                if existing_meta.config.vector_size != config.vector_size
                    || existing_meta.config.distance != config.distance
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing_meta.config.vector_size,
                        existing_dist: format!("{:?}", existing_meta.config.distance),
                        requested_size: config.vector_size,
                        requested_dist: format!("{:?}", config.distance),
                    });
                }
            }
        }

        let logical_key = Self::logical_index_key(space_id, tag_name, field_name);
        let meta = vector_client::manager::IndexMetadata::new(collection_name.clone(), config);
        self.logical_indexes.insert(logical_key, meta);

        info!(
            "Logical vector index created with config: space={} tag={} field={} in collection {}",
            space_id, tag_name, field_name, collection_name
        );
        Ok(collection_name)
    }

    /// Search with threshold and filter
    pub async fn search_with_threshold_and_filter(
        &self,
        mut options: SearchOptions,
        threshold: f32,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        options.threshold = Some(threshold);
        options.filter = Some(filter);
        self.search_with_options(options).await
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
