//! Vertex Table Core
//!
//! Main vertex storage with columnar layout.
//! Combines ID indexing, column storage, and timestamp tracking.
//!
//! # Concurrency Note
//!
//! `VertexTable` is NOT thread-safe. Multiple threads must not call mutable methods (`insert`, `delete`,
//! `update_property`, etc.) concurrently. `IdIndexer` provides concurrent-safe lookups via `parking_lot::Mutex`,
//! but the overall table state (columns, timestamps, schema) requires external synchronization.
//!
//! For multi-threaded access, use `ShardedVertexTable` which wraps `VertexTable` with per-shard
//! `parking_lot::Mutex` provides shard-level concurrency.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::{
    ColumnStore, IdIndexer, IdKey, LabelId, Timestamp, VertexId, VertexRecord, VertexSchema,
    VertexTimestamp,
};
use graphdb_core::{StorageError, StorageResult, Value};
use crate::encoding::EncodingSelector;
use crate::mvcc::SnapshotHandle;
use crate::schema::{LabelVersionHistory, SchemaObjectType};

#[derive(Debug, Clone)]
pub struct VertexTableConfig {
    pub initial_capacity: usize,
}

impl Default for VertexTableConfig {
    fn default() -> Self {
        Self {
            initial_capacity: 4096,
        }
    }
}

/// MVCC snapshot tracking for VertexTable
#[derive(Debug)]
pub struct VertexMVCC {
    /// Maps timestamp → count of active snapshots at that timestamp
    active_snapshots: HashMap<Timestamp, usize>,
    /// Minimum timestamp among all active snapshots
    min_active_snapshot_ts: Timestamp,
    /// Counter for generating unique snapshot IDs
    handle_counter: u64,
}

#[derive(Debug)]
pub struct VertexTable {
    pub(super) label: LabelId,
    pub(super) label_name: String,
    pub(super) schema: VertexSchema,
    pub(super) id_indexer: IdIndexer,
    pub(super) columns: ColumnStore,
    pub(super) timestamps: VertexTimestamp,
    pub(super) is_open: bool,
    /// Cache for property name → index mapping to avoid O(n) schema lookups.
    /// Invalidated whenever schema changes.
    pub(super) property_index_cache: HashMap<String, usize>,
    /// Version history tracking for schema changes
    pub(super) version_history: Arc<Mutex<LabelVersionHistory>>,
    /// MVCC snapshot tracking for snapshot isolation
    pub(super) mvcc: VertexMVCC,
    /// Persistent encoding selector with accumulated compression feedback.
    /// Feedback is gathered across flushes so that `should_reencode` can
    /// detect when a column's compression ratio degrades and recommend
    /// re-evaluating the encoding choice.
    pub(super) encoding_selector: EncodingSelector,
}

impl VertexTable {
    pub fn with_config(
        label: LabelId,
        label_name: String,
        schema: VertexSchema,
        config: VertexTableConfig,
    ) -> Self {
        let mut columns = ColumnStore::with_capacity(schema.properties.len());

        for prop in &schema.properties {
            columns.add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);
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
            timestamps: VertexTimestamp::with_capacity(config.initial_capacity),
            is_open: true,
            property_index_cache,
            version_history,
            mvcc: VertexMVCC {
                active_snapshots: HashMap::new(),
                min_active_snapshot_ts: Timestamp::MAX,
                handle_counter: 0,
            },
            encoding_selector: EncodingSelector::default(),
        }
    }

    pub fn insert(
        &mut self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        self.insert_by_key(IdKey::Text(external_id.to_string()), properties, ts)
    }

    pub fn insert_by_i64(
        &mut self,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        self.insert_by_key(IdKey::Int(external_id), properties, ts)
    }

    fn insert_by_key(
        &mut self,
        key: IdKey,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let mut converted: Vec<(String, Value)> = Vec::with_capacity(properties.len());
        for (name, value) in properties {
            // Use cached index lookup instead of O(n) schema search
            let prop_idx = self
                .property_index_cache
                .get(name)
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
            let prop_def = &self.schema.properties[*prop_idx];

            if value.data_type() != prop_def.data_type {
                let converted_val = value.try_cast_to(&prop_def.data_type)?;
                converted.push((name.clone(), converted_val));
            } else {
                converted.push((name.clone(), value.clone()));
            }
        }

        if self.id_indexer.contains(&key) {
            let internal_id = self
                .id_indexer
                .get_index(&key)
                .ok_or(StorageError::vertex_not_found())?;

            if self.timestamps.is_valid(internal_id, ts) {
                return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
            }

            // Re-insert after deletion: the vertex id stays allocated, so
            // re-open its lifetime window at `ts` (revert_remove alone would
            // require ts <= deletion ts and is only valid for transaction
            // rollbacks, not for a plain INSERT after DELETE).
            self.timestamps.insert(internal_id, ts);
            self.columns
                .set_versioned(internal_id as usize, &converted, ts)?;
            return Ok(internal_id);
        }

        let internal_id = self.id_indexer.insert(key)?;
        self.timestamps.insert(internal_id, ts);
        self.columns
            .set_versioned(internal_id as usize, &converted, ts)?;

        Ok(internal_id)
    }

    pub fn get_by_internal_id(&self, internal_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        self.get_projected_by_internal_id(internal_id, ts, None)
    }

    /// Live internal IDs (excludes vertices deleted at or before `ts`-visible
    /// state), in allocation order. Used by lazy paginated scans.
    pub fn live_ids(&self) -> Vec<u32> {
        self.id_indexer.live_ids()
    }

    /// Batch variant of [`get_projected_by_internal_id`].
    ///
    /// Validity is checked once per id, then all requested rows are decoded
    /// column-at-a-time in a single pass. The output is aligned with the
    /// input order; invalid or missing ids yield `None`.
    pub fn get_projected_batch(
        &self,
        internal_ids: &[u32],
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<Option<VertexRecord>> {
        if !self.is_open {
            return internal_ids.iter().map(|_| None).collect();
        }

        let mut positions: Vec<(usize, u32)> = Vec::with_capacity(internal_ids.len());
        for (pos, &id) in internal_ids.iter().enumerate() {
            if self.timestamps.is_valid(id, ts) {
                positions.push((pos, id));
            }
        }

        let mut out: Vec<Option<VertexRecord>> = internal_ids.iter().map(|_| None).collect();
        if positions.is_empty() {
            return out;
        }

        let row_indices: Vec<usize> = positions.iter().map(|&(_, id)| id as usize).collect();
        let props = match projection {
            Some(names) => self
                .columns
                .get_projected_batch_at_ts(&row_indices, names, ts),
            None => self.columns.get_batch_at_ts(&row_indices, ts),
        };

        for ((pos, id), prop_row) in positions.into_iter().zip(props) {
            let key = match self.id_indexer.get_key(id) {
                Some(key) => key,
                None => continue,
            };
            let vid = match key {
                IdKey::Int(i) => VertexId::from_int64(i),
                IdKey::Text(s) => VertexId::from_string(&s),
            };
            let properties: Vec<(String, Value)> = prop_row
                .into_iter()
                .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
                .collect();
            out[pos] = Some(VertexRecord {
                vid,
                internal_id: id,
                properties,
            });
        }
        out
    }

    /// Resolve the external vertex IDs of `internal_ids` that are valid at
    /// `ts`.  The output is aligned with the input; invalid ids yield `None`.
    pub fn resolve_valid_ids(&self, internal_ids: &[u32], ts: Timestamp) -> Vec<Option<VertexId>> {
        if !self.is_open {
            return internal_ids.iter().map(|_| None).collect();
        }
        internal_ids
            .iter()
            .map(|&id| {
                if !self.timestamps.is_valid(id, ts) {
                    return None;
                }
                match self.id_indexer.get_key(id) {
                    Some(IdKey::Int(i)) => Some(VertexId::from_int64(i)),
                    Some(IdKey::Text(s)) => Some(VertexId::from_string(&s)),
                    None => None,
                }
            })
            .collect()
    }

    /// Column-major batch decode (A1).  Decodes the requested columns for
    /// `internal_ids` column-at-a-time into typed [`ColumnValues`] arrays.
    /// The ids must already be valid at `ts`; validity is not re-checked here.
    pub fn get_projected_columns(
        &self,
        internal_ids: &[u32],
        ts: Timestamp,
        names: &[String],
    ) -> Vec<(String, crate::cursor::ColumnValues)> {
        if !self.is_open {
            return names
                .iter()
                .map(|n| {
                    (
                        n.clone(),
                        crate::cursor::ColumnValues::General(vec![
                            None;
                            internal_ids.len()
                        ]),
                    )
                })
                .collect();
        }
        let row_indices: Vec<usize> = internal_ids.iter().map(|&id| id as usize).collect();
        self.columns
            .get_projected_columns_at_ts(&row_indices, names, ts)
    }

    pub fn get_projected_by_internal_id(
        &self,
        internal_id: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<VertexRecord> {
        if !self.is_open {
            return None;
        }

        if !self.timestamps.is_valid(internal_id, ts) {
            return None;
        }

        let external_id = self.id_indexer.get_key(internal_id)?;
        let props = projection.map_or_else(
            || self.columns.get_at_ts(internal_id as usize, ts),
            |names| {
                self.columns
                    .get_projected_at_ts(internal_id as usize, names, ts)
            },
        );
        let properties: Vec<(String, Value)> = props
            .into_iter()
            .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
            .collect();

        let vid = match external_id {
            IdKey::Int(i) => VertexId::from_int64(i),
            IdKey::Text(s) => VertexId::from_string(&s),
        };

        Some(VertexRecord {
            vid,
            internal_id,
            properties,
        })
    }

    pub fn update_property(
        &mut self,
        internal_id: u32,
        col_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if !self.timestamps.is_valid(internal_id, ts) {
            return Err(StorageError::vertex_not_found());
        }

        // Use cached index lookup
        let prop_idx = self
            .property_index_cache
            .get(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;
        let prop_def = &self.schema.properties[*prop_idx];

        let converted_value = if value.data_type() != prop_def.data_type {
            value.try_cast_to(&prop_def.data_type)?
        } else {
            value.clone()
        };

        self.columns.set_property_versioned(
            internal_id as usize,
            col_name,
            Some(&converted_value),
            ts,
        )
    }

    pub fn update_property_by_id(
        &mut self,
        internal_id: u32,
        col_id: i32,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if !self.timestamps.is_valid(internal_id, ts) {
            return Err(StorageError::vertex_not_found());
        }

        let col = self
            .columns
            .get_column_by_id(col_id)
            .ok_or_else(|| StorageError::column_not_found(format!("col_id={}", col_id)))?;

        let converted_value = if value.data_type() != col.data_type {
            value.try_cast_to(&col.data_type)?
        } else {
            value.clone()
        };

        let col = self
            .columns
            .get_column_by_id_mut(col_id)
            .ok_or_else(|| StorageError::column_not_found(format!("col_id={}", col_id)))?;
        col.set_versioned(internal_id as usize, Some(&converted_value), ts)
    }

    pub fn delete(&mut self, external_id: &str, ts: Timestamp) -> StorageResult<()> {
        self.delete_by_key(&IdKey::Text(external_id.to_string()), ts)
    }

    pub fn delete_by_i64(&mut self, external_id: i64, ts: Timestamp) -> StorageResult<()> {
        self.delete_by_key(&IdKey::Int(external_id), ts)
    }

    fn delete_by_key(&mut self, key: &IdKey, ts: Timestamp) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let internal_id = self
            .id_indexer
            .get_index(key)
            .ok_or(StorageError::vertex_not_found())?;

        self.timestamps.remove(internal_id, ts);
        Ok(())
    }

    pub fn delete_by_internal_id(&mut self, internal_id: u32, ts: Timestamp) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        self.timestamps.remove(internal_id, ts);
        Ok(())
    }

    pub fn revert_delete(&mut self, internal_id: u32, ts: Timestamp) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if !self.timestamps.revert_remove(internal_id, ts) {
            return Err(StorageError::invalid_operation(format!(
                "Cannot revert deletion of vertex {}: invalid timestamp",
                internal_id
            )));
        }
        Ok(())
    }

    /// Batch delete multiple vertices by external ID.
    /// Returns count of successfully deleted vertices.
    pub fn batch_delete(&mut self, external_ids: &[&str], ts: Timestamp) -> StorageResult<usize> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let mut deleted_count = 0;

        for external_id in external_ids {
            match self.delete_by_key(&IdKey::Text(external_id.to_string()), ts) {
                Ok(_) => {
                    deleted_count += 1;
                }
                Err(e) => {
                    // Skip this vertex and continue with others
                    eprintln!("Failed to delete vertex {}: {}", external_id, e);
                }
            }
        }

        Ok(deleted_count)
    }

    /// Batch delete multiple vertices by i64 external ID.
    /// Returns count of successfully deleted vertices.
    pub fn batch_delete_i64(
        &mut self,
        external_ids: &[i64],
        ts: Timestamp,
    ) -> StorageResult<usize> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let mut deleted_count = 0;

        for external_id in external_ids {
            match self.delete_by_key(&IdKey::Int(*external_id), ts) {
                Ok(_) => {
                    deleted_count += 1;
                }
                Err(e) => {
                    eprintln!("Failed to delete vertex {}: {}", external_id, e);
                }
            }
        }

        Ok(deleted_count)
    }

    pub fn get_internal_id(&self, external_id: &str, ts: Timestamp) -> Option<u32> {
        if !self.is_open {
            return None;
        }

        let internal_id = self
            .id_indexer
            .get_index(&IdKey::Text(external_id.to_string()))?;
        if self.timestamps.is_valid(internal_id, ts) {
            Some(internal_id)
        } else {
            None
        }
    }

    pub fn get_internal_id_by_i64(&self, external_id: i64, ts: Timestamp) -> Option<u32> {
        if !self.is_open {
            return None;
        }

        let internal_id = self.id_indexer.get_index(&IdKey::Int(external_id))?;
        if self.timestamps.is_valid(internal_id, ts) {
            Some(internal_id)
        } else {
            None
        }
    }

    /// Lookup internal ID from external i64 without timestamp check.
    /// Returns Some(internal_id) even for deleted vertices.
    pub fn get_internal_id_by_i64_raw(&self, external_id: i64) -> Option<u32> {
        if !self.is_open {
            return None;
        }
        self.id_indexer.get_index(&IdKey::Int(external_id))
    }

    /// Lookup internal ID from external string without timestamp check.
    /// Returns Some(internal_id) even for deleted vertices.
    pub fn get_internal_id_raw(&self, external_id: &str) -> Option<u32> {
        if !self.is_open {
            return None;
        }
        self.id_indexer
            .get_index(&IdKey::Text(external_id.to_string()))
    }

    pub fn get_external_id(&self, internal_id: u32, ts: Timestamp) -> Option<IdKey> {
        if !self.is_open || !self.timestamps.is_valid(internal_id, ts) {
            return None;
        }
        self.id_indexer.get_key(internal_id)
    }

    /// Lookup external ID from internal ID without timestamp check.
    /// Returns the external ID even for deleted vertices.
    pub fn get_external_id_raw(&self, internal_id: u32) -> Option<IdKey> {
        if !self.is_open {
            return None;
        }
        self.id_indexer.get_key(internal_id)
    }

    /// Declared data type of the column `name`, if it exists in the schema.
    pub fn data_type_of(&self, name: &str) -> Option<graphdb_core::types::DataType> {
        self.columns.data_type_of(name)
    }

    pub fn total_count(&self) -> usize {
        self.id_indexer.len()
    }

    /// Next free local id within this table: local ids are never reused, so
    /// this is the highest id ever allocated plus one.
    pub fn next_local_id(&self) -> u32 {
        self.id_indexer.next_index()
    }

    /// Live vertex count at `ts` (excludes vertices deleted at or before
    /// `ts`) and total allocated local IDs (the high-water mark, never
    /// reused until compaction). The gap `allocated - live` is the number of
    /// slots reclaimable by a compaction at `ts`.
    pub fn id_hole_stats(&self, ts: Timestamp) -> (usize, usize) {
        let allocated = self.next_local_id() as usize;
        let deleted = self.timestamps.iter_deleted(ts).count();
        (allocated.saturating_sub(deleted), allocated)
    }

    /// Pre-allocate capacity for `additional` more vertices in the ID indexer,
    /// the column buffers, and the timestamp vectors.
    ///
    /// Without column/timestamp reservation, every appended row in a large
    /// batch resizes the backing `Vec`s one element at a time, making bulk
    /// inserts quadratic in the table size.
    pub fn reserve_id_capacity(&mut self, additional: usize) {
        self.id_indexer.reserve(additional);
        self.columns.reserve(additional);
        self.timestamps.reserve(additional);
    }

    pub fn scan(&self, ts: Timestamp) -> VertexIterator<'_> {
        VertexIterator::new(self, ts)
    }

    pub fn schema(&self) -> &VertexSchema {
        &self.schema
    }

    pub fn set_schema(&mut self, schema: VertexSchema) {
        self.schema = schema;

        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in self.schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
    }

    /// Get reference to version history Arc for shared access
    pub fn version_history_ref(&self) -> Arc<Mutex<LabelVersionHistory>> {
        Arc::clone(&self.version_history)
    }

    pub fn memory_size(&self) -> usize {
        let mut total = 0;

        total += self.id_indexer.memory_size();
        total += self.columns.memory_size();
        total += self.timestamps.memory_size();

        // Account for label_name string (content only)
        total += self.label_name.len();

        // Account for property_index_cache HashMap (actual entries, not capacity)
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        total += std::mem::size_of::<Self>();

        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;

        let active_count = self.id_indexer.len();
        total += active_count * std::mem::size_of::<(String, u32)>();

        total += self.columns.used_memory_size();

        total += self.timestamps.size() * std::mem::size_of::<Timestamp>();

        // Account for actual label_name usage
        total += self.label_name.len();

        // Account for property_index_cache actual entries
        total += self.property_index_cache.len() * (24 + std::mem::size_of::<usize>()); // String overhead + usize

        total
    }

    // ==================== MVCC Methods ====================

    /// Register a new snapshot at the given timestamp
    ///
    /// Increments the reference count for this timestamp and tracks it in active_snapshots.
    /// Returns a unique SnapshotHandle that must be used to unregister later.
    pub fn register_snapshot(&mut self, ts: Timestamp) -> StorageResult<SnapshotHandle> {
        *self.mvcc.active_snapshots.entry(ts).or_insert(0) += 1;
        // Incremental min maintenance: a new snapshot can only lower the
        // minimum, so compare against the current value instead of rescanning
        // the whole map (which made transaction begin O(active snapshots)).
        if ts < self.mvcc.min_active_snapshot_ts {
            self.mvcc.min_active_snapshot_ts = ts;
        }

        self.mvcc.handle_counter += 1;
        Ok(SnapshotHandle::new(ts, self.mvcc.handle_counter))
    }

    /// Unregister a snapshot, allowing GC of related version data
    ///
    /// Decrements the reference count for the snapshot's timestamp.
    /// When the count reaches 0, the timestamp is removed from tracking.
    pub fn unregister_snapshot(&mut self, handle: SnapshotHandle) -> StorageResult<()> {
        if let Some(count) = self.mvcc.active_snapshots.get_mut(&handle.ts) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.mvcc.active_snapshots.remove(&handle.ts);
                // Only rescan when the removed timestamp was the current
                // minimum; otherwise the min is unchanged.
                if handle.ts == self.mvcc.min_active_snapshot_ts {
                    self.mvcc.min_active_snapshot_ts = self
                        .mvcc
                        .active_snapshots
                        .keys()
                        .min()
                        .copied()
                        .unwrap_or(Timestamp::MAX);
                }
            }
        }

        Ok(())
    }

    /// Unregister all snapshots with the given timestamp.
    /// Used by lazy registration cleanup on transaction finalize.
    pub fn unregister_snapshot_by_timestamp(&mut self, ts: Timestamp) -> StorageResult<()> {
        if let Some(count) = self.mvcc.active_snapshots.get_mut(&ts) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.mvcc.active_snapshots.remove(&ts);
                // Only rescan when the removed timestamp was the current
                // minimum; otherwise the min is unchanged.
                if ts == self.mvcc.min_active_snapshot_ts {
                    self.mvcc.min_active_snapshot_ts = self
                        .mvcc
                        .active_snapshots
                        .keys()
                        .min()
                        .copied()
                        .unwrap_or(Timestamp::MAX);
                }
            }
        }

        Ok(())
    }

    /// Get the count of currently active snapshots
    pub fn active_snapshot_count(&self) -> usize {
        self.mvcc.active_snapshots.len()
    }

    /// Get the minimum timestamp among all active snapshots
    pub fn min_active_snapshot_ts(&self) -> Timestamp {
        self.mvcc.min_active_snapshot_ts
    }

    /// Perform garbage collection on version data older than min_ts
    ///
    /// Reclaims deleted vertices (from the id indexer / timestamps) and drops
    /// property version-chain entries that no active snapshot can observe.
    ///
    /// Returns the number of version entries cleaned up.
    pub fn gc(&mut self, min_ts: Timestamp) -> StorageResult<usize> {
        // Property version-chain GC runs every pass regardless of deleted
        // vertices so before-images of overwritten properties are reclaimed.
        let version_removed = self.columns.gc_versions(min_ts);

        // Collect all vertices deleted before min_ts
        let deleted_ids: Vec<u32> = self.timestamps.iter_deleted(min_ts).collect();

        if deleted_ids.is_empty() {
            return Ok(version_removed);
        }

        let count = deleted_ids.len();

        // Remove from id_indexer
        for id in &deleted_ids {
            if let Some(key) = self.id_indexer.get_key(*id) {
                self.id_indexer.remove(&key);
            }
        }

        // Compact to reclaim space
        self.compact_coordinated()?;

        // If no snapshots are active, also compact timestamps to remove
        // any entries with end_ts != MAX_TIMESTAMP. This is safe because
        // without active snapshots there are no readers that need those
        // version records.
        if self.min_active_snapshot_ts() == crate::vertex::MAX_TIMESTAMP {
            self.compact_timestamps();
        }
        // Read active snapshot count for diagnostics — ensures the method
        // is exercised even when there are no snapshots to clean.
        let _active_count = self.active_snapshot_count();

        Ok(count)
    }

    /// Compact timestamps independently of id_indexer and columns.
    ///
    /// Removes all temporally-deleted entries (those with `end_ts != MAX_TIMESTAMP`)
    /// and returns the old_id → new_id mapping for entries that moved.
    /// This is a standalone operation — use it when only timestamp cleanup is
    /// needed without full table compaction.
    ///
    /// # Safety
    ///
    /// Caller must ensure no active snapshots reference the removed entries.
    /// When in doubt, use `gc()` instead, which coordinates all three structures
    /// and respects snapshot isolation boundaries.
    pub fn compact_timestamps(&mut self) -> std::collections::HashMap<u32, u32> {
        self.timestamps.compact()
    }
}

pub struct VertexIterator<'a> {
    table: &'a VertexTable,
    ts: Timestamp,
    live_ids: std::vec::IntoIter<u32>,
}

impl<'a> VertexIterator<'a> {
    pub fn new(table: &'a VertexTable, ts: Timestamp) -> Self {
        Self {
            table,
            ts,
            live_ids: table.id_indexer.live_ids().into_iter(),
        }
    }
}

impl<'a> Iterator for VertexIterator<'a> {
    type Item = VertexRecord;

    fn next(&mut self) -> Option<Self::Item> {
        for id in self.live_ids.by_ref() {
            if let Some(record) = self.table.get_by_internal_id(id, self.ts) {
                return Some(record);
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
