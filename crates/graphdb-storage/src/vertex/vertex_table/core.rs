//! Vertex Table Core
//!
//! Main vertex storage with columnar layout.
//! Combines ID indexing, column storage, and timestamp tracking.
//!
//! # Concurrency Note
//!
//! Point paths are `&self`: the identity step (duplicate check, id
//! allocation, timestamp publish) serializes on the timestamp latch while
//! the data step (column segment writes) uses the column segment latches,
//! so point writes to different rows of one table proceed concurrently.
//! `IdIndexer` carries its own mutex; `ColumnStore` and `Column` are
//! internally locked. Schema, pending schema changes and the property
//! index cache mutate only under table-exclusive operations (schema
//! evolution, load, offline rebuild) and are read lock-free on point
//! paths. Exclusive operations keep their table-wide semantics by holding
//! the shard write guard across the same identity-plus-segments sequence.
//!
//! Lock order inside one table: identity (timestamp latch) before column
//! segments before overflow. Point paths never hold a segment latch while
//! acquiring the identity latch.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::super::vertex_timestamp::IdentityLatch;
use super::super::{ColumnStore, IdIndexer, LabelId, Timestamp, VertexSchema, VertexTimestamp};
use crate::encoding::EncodingSelector;
use crate::schema::{LabelVersionHistory, SchemaObjectType};
use graphdb_core::{StorageError, StorageResult};

#[derive(Debug, Clone)]
pub struct VertexTableConfig {
    pub initial_capacity: usize,
    /// Payload size above which strings spill to the per-column overflow
    /// file. `usize::MAX` disables overflow routing (inline storage).
    pub string_overflow_threshold: usize,
    /// Rows per chunk for chunk-local encodings and update overlays.
    pub chunk_capacity: usize,
}

impl Default for VertexTableConfig {
    fn default() -> Self {
        Self {
            initial_capacity: 4096,
            string_overflow_threshold: crate::vertex::column::overflow::DEFAULT_OVERFLOW_THRESHOLD,
            chunk_capacity: crate::vertex::column::chunk::DEFAULT_CHUNK_ROWS,
        }
    }
}

#[derive(Debug)]
pub struct VertexTable {
    pub(super) label: LabelId,
    pub(super) label_name: String,
    pub(super) schema: VertexSchema,
    pub(super) id_indexer: IdIndexer,
    pub(super) columns: ColumnStore,
    /// Identity latch: duplicate check, id allocation and timestamp publish
    /// hold this across the identity step so concurrent point inserts of one
    /// table allocate each key exactly once. The data step runs after the
    /// guard is released. `IdIndexer` serializes its own map internally;
    /// this latch orders the map-plus-timestamp pair.
    pub(super) timestamps: IdentityLatch,
    pub(super) is_open: AtomicBool,
    /// Cache for property name → index mapping to avoid O(n) schema lookups.
    /// Invalidated whenever schema changes.
    pub(super) property_index_cache: HashMap<String, usize>,
    /// Version history tracking for schema changes
    pub(super) version_history: Arc<Mutex<LabelVersionHistory>>,
    /// Persistent encoding selector with accumulated compression feedback.
    /// Feedback is gathered across flushes so that `should_reencode` can
    /// detect when a column's compression ratio degrades and recommend
    /// re-evaluating the encoding choice.
    pub(super) encoding_selector: EncodingSelector,
    /// Payload size above which new string/blob columns spill to the
    /// per-column overflow file (applied when columns are created).
    pub(super) string_overflow_threshold: usize,
    /// Rows per chunk for chunk-local encodings (applied when columns are
    /// created).
    pub(super) chunk_capacity: usize,
    /// One in-flight staged schema change (prepare/fill/publish/abort).
    /// Pending state lives only in memory; publishing is the visibility
    /// boundary. While set, `set_schema` is rejected so the staged change
    /// cannot be silently discarded.
    pub(super) pending_schema_change: Option<super::staged_schema::PendingVertexSchemaChange>,
    /// Checkpoint-epoch floor for attribute time travel.
    ///
    /// Version chains stay memory-only, so a load drops every before-image.
    /// Reads at timestamps below this floor may need dropped history and
    /// fail closed on the strict paths; reads at or above it serve from
    /// live chains. Recomputed on every load as the maximum creation
    /// stamp; zero on fresh tables, disabling the fence.
    pub(super) history_floor: Timestamp,
}

mod maintenance;
mod mutations;
mod reads;
mod writes;

impl VertexTable {
    /// Per-row version-chain length above which `gc` emits a pressure
    /// warning. Chains grow without bound while the GC watermark is pinned,
    /// so crossing this threshold points at a stuck snapshot, not a hot row.
    /// Shared with the GC manager, which triggers bounded cold-chunk
    /// eviction at the same threshold instead of only warning.
    pub(crate) const VERSION_CHAIN_PRESSURE_WARN_LEN: usize = 1024;

    pub fn with_config(
        label: LabelId,
        label_name: String,
        schema: VertexSchema,
        config: VertexTableConfig,
    ) -> Self {
        let columns = ColumnStore::with_capacity(schema.properties.len());

        for prop in &schema.properties {
            columns.add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);
            if let Some(col) = columns.get_column(&prop.name) {
                col.set_chunk_capacity(config.chunk_capacity);
            }
            if matches!(
                prop.data_type,
                graphdb_core::DataType::String | graphdb_core::DataType::Blob
            ) {
                if let Some(col) = columns.get_column(&prop.name) {
                    col.set_overflow_threshold(config.string_overflow_threshold);
                }
            }
        }

        let mut property_index_cache = HashMap::new();
        for (idx, prop) in schema.properties.iter().enumerate() {
            property_index_cache.insert(prop.name.clone(), idx);
        }

        let version_history = Arc::new(Mutex::new(LabelVersionHistory::new(
            label,
            label_name.clone(),
            SchemaObjectType::Vertex,
        )));

        Self {
            label,
            label_name,
            schema,
            id_indexer: IdIndexer::with_capacity(config.initial_capacity),
            columns,
            timestamps: IdentityLatch::new(VertexTimestamp::with_capacity(config.initial_capacity)),
            is_open: AtomicBool::new(true),
            property_index_cache,
            version_history,
            encoding_selector: EncodingSelector::default(),
            string_overflow_threshold: config.string_overflow_threshold,
            chunk_capacity: config.chunk_capacity,
            pending_schema_change: None,
            history_floor: 0,
        }
    }

    pub fn schema(&self) -> &VertexSchema {
        &self.schema
    }

    /// Replace the published schema without staging.
    ///
    /// Recovery and undo compensation only. Live evolution must go through
    /// prepare/fill/publish/abort; while a staged change is pending this
    /// is rejected.
    pub fn set_schema(&mut self, schema: VertexSchema) -> StorageResult<()> {
        if self.pending_schema_change.is_some() {
            return Err(StorageError::invalid_operation(
                "set_schema rejected while a staged vertex schema change is pending".to_string(),
            ));
        }
        self.schema = schema;

        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in self.schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        Ok(())
    }

    /// Get reference to version history Arc for shared access
    pub fn version_history_ref(&self) -> Arc<Mutex<LabelVersionHistory>> {
        Arc::clone(&self.version_history)
    }
}

#[cfg(test)]
mod tests;
