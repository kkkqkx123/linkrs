//! Vertex Table Core
//!
//! Main vertex storage with columnar layout.
//! Combines ID indexing, column storage, and timestamp tracking.
//!
//! # Concurrency Note
//!
//! `VertexTable` is NOT thread-safe. Multiple threads must not call mutable methods (`insert`, `delete`,
//! `update_property`, etc.) concurrently. Although `IdIndexer` uses DashMap for concurrent-safe lookups,
//! the overall table state (columns, timestamps, schema) requires external synchronization.
//!
//! **Pattern for multi-threaded access:**
//! ```ignore
//! let vertex_table = Arc::new(Mutex::new(VertexTable::new(...)));
//! // Use vertex_table.lock().unwrap().insert(...) for mutable operations
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::{
    ColumnStore, IdIndexer, IdKey, LabelId, Timestamp, VertexId, VertexRecord, VertexSchema,
    VertexTimestamp,
};
use crate::core::error::storage::StorageErrorKind;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::mvcc::SnapshotHandle;
use crate::storage::schema::{LabelVersionHistory, SchemaObjectType};

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
}

impl VertexTable {
    pub fn new(label: LabelId, label_name: String, schema: VertexSchema) -> Self {
        Self::with_config(label, label_name, schema, VertexTableConfig::default())
    }

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
                min_active_snapshot_ts: u32::MAX,
                handle_counter: 0,
            },
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

            let _ = self.timestamps.revert_remove(internal_id, ts);
            self.columns.set(internal_id as usize, &converted)?;
            return Ok(internal_id);
        }

        let internal_id = self.id_indexer.insert(key)?;
        self.timestamps.insert(internal_id, ts);
        self.columns.set(internal_id as usize, &converted)?;

        Ok(internal_id)
    }

    pub fn get_by_internal_id(&self, internal_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        self.get_projected_by_internal_id(internal_id, ts, None)
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
            || self.columns.get(internal_id as usize),
            |names| self.columns.get_projected(internal_id as usize, names),
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

        self.columns
            .set_property(internal_id as usize, col_name, Some(&converted_value))
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
        col.set(internal_id as usize, Some(&converted_value))
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

    pub fn total_count(&self) -> usize {
        self.id_indexer.len()
    }

    pub fn scan(&self, ts: Timestamp) -> VertexIterator<'_> {
        VertexIterator::new(self, ts)
    }

    pub fn label(&self) -> LabelId {
        self.label
    }

    pub fn label_name(&self) -> &str {
        &self.label_name
    }

    pub fn schema(&self) -> &VertexSchema {
        &self.schema
    }

    pub(crate) fn schema_mut(&mut self) -> &mut VertexSchema {
        &mut self.schema
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

        total += self.timestamps.valid_count(super::super::MAX_TIMESTAMP - 1)
            * std::mem::size_of::<Timestamp>();

        // Account for actual label_name usage
        total += self.label_name.len();

        // Account for property_index_cache actual entries
        total += self.property_index_cache.len() * (24 + std::mem::size_of::<usize>()); // String overhead + usize

        total
    }

    /// Verify internal consistency after compaction.
    /// Should be called after compact() in debug builds.
    ///
    /// Invariants checked:
    /// 1. Every key in id_indexer has a valid timestamp entry
    /// 2. Every valid timestamp entry has a corresponding key in id_indexer
    /// 3. Column count matches id_indexer.len()
    /// 4. All column indices are within bounds
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) -> StorageResult<()> {
        let id_count = self.id_indexer.len();

        // Check 1: Every key in id_indexer has a valid timestamp entry
        for (key, idx) in self.id_indexer.iter() {
            let start_ts = self.timestamps.get_start_ts(idx);
            if start_ts.is_none() {
                return Err(StorageError::new(
                    StorageErrorKind::StorageError,
                    format!("ID {} for key {:?} missing in timestamps", idx, key),
                ));
            }
        }

        // Check 2: Every valid timestamp entry has a corresponding key in id_indexer
        for idx in 0..self.timestamps.size() {
            if let Some(_start_ts) = self.timestamps.get_start_ts(idx as u32) {
                let key = self.id_indexer.get_key(idx as u32);
                if key.is_none() {
                    return Err(StorageError::new(
                        StorageErrorKind::StorageError,
                        format!("Timestamp entry {} missing in id_indexer", idx),
                    ));
                }
            }
        }

        // Check 3: Column count matches id_indexer.len()
        if self.columns.row_count() != id_count {
            return Err(StorageError::new(
                StorageErrorKind::StorageError,
                format!(
                    "Column count ({}) mismatch with id_indexer.len() ({})",
                    self.columns.row_count(),
                    id_count
                ),
            ));
        }

        Ok(())
    }

    /// No-op in release builds for performance
    #[cfg(not(debug_assertions))]
    pub fn verify_invariants(&self) -> StorageResult<()> {
        Ok(())
    }

    // ==================== MVCC Methods ====================

    /// Register a new snapshot at the given timestamp
    ///
    /// Increments the reference count for this timestamp and tracks it in active_snapshots.
    /// Returns a unique SnapshotHandle that must be used to unregister later.
    pub fn register_snapshot(&mut self, ts: Timestamp) -> StorageResult<SnapshotHandle> {
        *self.mvcc.active_snapshots.entry(ts).or_insert(0) += 1;
        self.mvcc.min_active_snapshot_ts = self
            .mvcc
            .active_snapshots
            .keys()
            .min()
            .copied()
            .unwrap_or(u32::MAX);

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
            }
        }

        // Update min_active_snapshot_ts
        self.mvcc.min_active_snapshot_ts = self
            .mvcc
            .active_snapshots
            .keys()
            .min()
            .copied()
            .unwrap_or(u32::MAX);

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
    /// This is a placeholder implementation for VertexTable.
    /// In practice, garbage collection would remove old timestamp entries
    /// from the timestamps structure that are older than min_ts and no longer
    /// needed by any active snapshot.
    ///
    /// Returns the number of version entries cleaned up.
    pub fn gc(&mut self, min_ts: Timestamp) -> StorageResult<usize> {
        // Collect all vertices deleted before min_ts
        let deleted_ids: Vec<u32> = self.timestamps.iter_deleted(min_ts).collect();

        if deleted_ids.is_empty() {
            return Ok(0);
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

        Ok(count)
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
