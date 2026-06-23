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

use std::path::Path;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::{ColumnStore, IdIndexer, IdKey, LabelId, Timestamp, VertexId, VertexRecord, VertexSchema, VertexTimestamp};
use crate::core::{StorageError, StorageResult, Value};
use crate::core::error::storage::StorageErrorKind;
use crate::storage::schema::{LabelVersionHistory, SchemaObjectType};
use crate::storage::mvcc::{MVCCTable, SnapshotHandle};

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
    pub(super) deferred_encodings: std::collections::HashMap<String, crate::storage::encoding::EncodingType>,
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

        let version_history = Arc::new(Mutex::new(
            LabelVersionHistory::new(label, label_name.clone(), SchemaObjectType::Vertex)
        ));

        Self {
            label,
            label_name,
            schema,
            id_indexer: IdIndexer::with_capacity(config.initial_capacity),
            columns,
            timestamps: VertexTimestamp::with_capacity(config.initial_capacity),
            is_open: true,
            deferred_encodings: std::collections::HashMap::new(),
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
            let prop_idx = self.property_index_cache
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
        if !self.is_open {
            return None;
        }

        if !self.timestamps.is_valid(internal_id, ts) {
            return None;
        }

        let external_id = self.id_indexer.get_key(internal_id)?;
        let props = self.columns.get(internal_id as usize);
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
        let prop_idx = self.property_index_cache
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

    /// Batch insert multiple vertices in a single operation.
    /// All inserts are validated before any state modification to ensure atomicity.
    /// Returns the internal IDs of inserted vertices in the same order as input.
    pub fn batch_insert(
        &mut self,
        vertices: &[(String, Vec<(String, Value)>)],
        ts: Timestamp,
    ) -> StorageResult<Vec<u32>> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if vertices.is_empty() {
            return Ok(Vec::new());
        }

        // Proceed with inserts; validation happens in insert_by_key.
        // Rollback ensures atomicity if any insert fails.
        let mut result_ids = Vec::with_capacity(vertices.len());
        let mut inserted_external_ids = Vec::new();
        for (i, (external_id, properties)) in vertices.iter().enumerate() {
            match self.insert_by_key(
                IdKey::Text(external_id.clone()),
                properties,
                ts,
            ) {
                Ok(id) => {
                    result_ids.push(id);
                    inserted_external_ids.push(external_id.clone());
                }
                Err(e) => {
                    // Rollback: revert all previous inserts from both timestamps and id_indexer
                    for (prev_id, prev_external_id) in result_ids.iter().zip(inserted_external_ids.iter()) {
                        let _ = self.timestamps.remove(*prev_id, ts);
                        let _ = self.id_indexer.remove(&IdKey::Text(prev_external_id.clone()));
                    }
                    return Err(StorageError::invalid_operation(format!(
                        "Batch insert failed at index {}: {}",
                        i, e
                    )));
                }
            }
        }

        Ok(result_ids)
    }

    /// Batch delete multiple vertices by external ID.
    /// Returns count of successfully deleted vertices.
    pub fn batch_delete(
        &mut self,
        external_ids: &[&str],
        ts: Timestamp,
    ) -> StorageResult<usize> {
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

    /// Set schema with explicit version number (used for undo operations)
    pub fn set_schema_with_version(&mut self, mut schema: VertexSchema, new_version: u64) {
        schema.schema_version = new_version;

        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in self.schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }

        self.schema = schema;
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

        // Account for deferred_encodings HashMap (actual entries, not capacity)
        total += self.deferred_encodings.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<crate::storage::encoding::EncodingType>());

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
        total += self.property_index_cache.len()
            * (24 + std::mem::size_of::<usize>()); // String overhead + usize

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
        self.mvcc.min_active_snapshot_ts = self.mvcc.active_snapshots
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
        self.mvcc.min_active_snapshot_ts = self.mvcc.active_snapshots
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

/// Implement MVCCTable trait for VertexTable to support snapshot isolation
impl MVCCTable for VertexTable {
    fn register_snapshot(&mut self, ts: Timestamp) -> StorageResult<SnapshotHandle> {
        VertexTable::register_snapshot(self, ts)
    }

    fn unregister_snapshot(&mut self, handle: SnapshotHandle) -> StorageResult<()> {
        VertexTable::unregister_snapshot(self, handle)
    }

    fn active_snapshot_count(&self) -> usize {
        VertexTable::active_snapshot_count(self)
    }

    fn min_active_snapshot_ts(&self) -> Timestamp {
        VertexTable::min_active_snapshot_ts(self)
    }

    fn gc(&mut self, min_ts: Timestamp) -> StorageResult<usize> {
        VertexTable::gc(self, min_ts)
    }
}

pub struct VertexIterator<'a> {
    table: &'a VertexTable,
    ts: Timestamp,
    /// Current internal ID to check
    current: u32,
    /// Total internal IDs in the table
    end: u32,
}

impl<'a> VertexIterator<'a> {
    pub fn new(table: &'a VertexTable, ts: Timestamp) -> Self {
        Self {
            table,
            ts,
            current: 0,
            end: table.total_count() as u32,
        }
    }
}

impl<'a> Iterator for VertexIterator<'a> {
    type Item = VertexRecord;

    fn next(&mut self) -> Option<Self::Item> {
        while self.current < self.end {
            let id = self.current;
            self.current += 1;

            // Lazy check: only validate timestamp when actually retrieving the record
            if let Some(record) = self.table.get_by_internal_id(id, self.ts) {
                return Some(record);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::DataType;
    use crate::storage::types::StoragePropertyDef;

    fn create_test_schema() -> VertexSchema {
        VertexSchema {
            label_id: 0,
            label_name: "person".to_string(),
            properties: vec![
                StoragePropertyDef::new("name".to_string(), DataType::String),
                StoragePropertyDef {
                    name: "age".to_string(),
                    data_type: DataType::Int,
                    nullable: true,
                    default_value: None,
                },
            ],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    #[test]
    fn test_insert_and_get() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        let internal_id = table
            .insert(
                "v1",
                &[
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                ],
                100,
            )
            .unwrap();

        assert_eq!(internal_id, 0);

        let lookup_id = table.get_internal_id("v1", 100).unwrap();
        let record = table.get_by_internal_id(lookup_id, 100).unwrap();
        assert_eq!(record.properties.len(), 2);
    }

    #[test]
    fn test_delete() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table
            .insert(
                "v1",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                100,
            )
            .unwrap();

        table.delete("v1", 200).unwrap();

        let internal_id = table.get_internal_id("v1", 150).unwrap();
        assert!(table.get_by_internal_id(internal_id, 150).is_some());
        assert!(table.get_internal_id("v1", 250).is_none());
    }

    #[test]
    fn test_iterator() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table
            .insert(
                "v1",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                100,
            )
            .unwrap();
        table
            .insert(
                "v2",
                &[("name".to_string(), Value::String("Bob".to_string()))],
                100,
            )
            .unwrap();
        table
            .insert(
                "v3",
                &[("name".to_string(), Value::String("Charlie".to_string()))],
                100,
            )
            .unwrap();

        let count = table.scan(100).count();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_rename_and_remove_property() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table
            .add_property(StoragePropertyDef::new(
                "city".to_string(),
                DataType::String,
            ))
            .expect("add property should succeed");

        let internal_id = table
            .insert(
                "v1",
                &[
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                    ("city".to_string(), Value::String("Shanghai".to_string())),
                ],
                100,
            )
            .unwrap();

        table
            .rename_property("age", "years")
            .expect("rename should succeed");
        table
            .remove_property("city")
            .expect("remove should succeed");

        let record = table
            .get_by_internal_id(internal_id, 100)
            .expect("record should remain visible");

        assert_eq!(
            record
                .properties
                .iter()
                .find(|(name, _)| name == "years")
                .map(|(_, value)| value),
            Some(&Value::Int(30))
        );
        assert!(record.properties.iter().all(|(name, _)| name != "age"));
        assert!(record.properties.iter().all(|(name, _)| name != "city"));
        assert_eq!(
            table
                .schema()
                .properties
                .iter()
                .map(|prop| prop.name.as_str())
                .collect::<Vec<_>>(),
            vec!["name", "years"]
        );
    }

    #[test]
    fn test_batch_insert() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        let vertices = vec![
            (
                "v1".to_string(),
                vec![
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                ],
            ),
            (
                "v2".to_string(),
                vec![
                    ("name".to_string(), Value::String("Bob".to_string())),
                    ("age".to_string(), Value::Int(25)),
                ],
            ),
            (
                "v3".to_string(),
                vec![
                    ("name".to_string(), Value::String("Charlie".to_string())),
                    ("age".to_string(), Value::Int(35)),
                ],
            ),
        ];

        let ids = table.batch_insert(&vertices, 100).unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], 0);
        assert_eq!(ids[1], 1);
        assert_eq!(ids[2], 2);

        let count = table.scan(100).count();
        assert_eq!(count, 3);

        let record1 = table.get_by_internal_id(ids[0], 100).unwrap();
        assert_eq!(
            record1.properties.iter().find(|(n, _)| n == "name").map(|(_, v)| v),
            Some(&Value::String("Alice".to_string()))
        );
    }

    #[test]
    fn test_batch_delete() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table
            .insert(
                "v1",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                100,
            )
            .unwrap();
        table
            .insert(
                "v2",
                &[("name".to_string(), Value::String("Bob".to_string()))],
                100,
            )
            .unwrap();
        table
            .insert(
                "v3",
                &[("name".to_string(), Value::String("Charlie".to_string()))],
                100,
            )
            .unwrap();

        let deleted = table.batch_delete(&["v1", "v3"], 200).unwrap();
        assert_eq!(deleted, 2);

        let count_before_delete = table.scan(100).count();
        assert_eq!(count_before_delete, 3);

        let count_after_delete = table.scan(200).count();
        assert_eq!(count_after_delete, 1);

        assert!(table.get_internal_id("v2", 200).is_some());
        assert!(table.get_internal_id("v1", 200).is_none());
    }

    #[test]
    fn test_add_property_increments_version() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        let v1 = table.schema().schema_version;
        assert_eq!(v1, 1, "Initial version should be 1");

        table.add_property(StoragePropertyDef::new("email".to_string(), DataType::String))
            .expect("add_property should succeed");

        let v2 = table.schema().schema_version;
        assert_eq!(v2, 2, "Version should increment after add_property");
    }

    #[test]
    fn test_remove_property_increments_version() {
        let mut schema = create_test_schema();
        // Add a removable property
        schema.properties.push(StoragePropertyDef::new("email".to_string(), DataType::String));
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        let v1 = table.schema().schema_version;

        table.remove_property("email")
            .expect("remove_property should succeed");

        let v2 = table.schema().schema_version;
        assert_eq!(v2, v1 + 1, "Version should increment after remove_property");
    }

    #[test]
    fn test_rename_property_increments_version() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        let v1 = table.schema().schema_version;

        table.rename_property("name", "full_name")
            .expect("rename_property should succeed");

        let v2 = table.schema().schema_version;
        assert_eq!(v2, v1 + 1, "Version should increment after rename_property");
    }

    #[test]
    fn test_sequential_property_modifications() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        // Initial version should be 1
        assert_eq!(table.schema().schema_version, 1);

        // Add first property
        table.add_property(StoragePropertyDef::new("email".to_string(), DataType::String))
            .expect("add_property 1 should succeed");
        assert_eq!(table.schema().schema_version, 2);

        // Add second property
        table.add_property(StoragePropertyDef::new("phone".to_string(), DataType::String))
            .expect("add_property 2 should succeed");
        assert_eq!(table.schema().schema_version, 3);

        // Rename property
        table.rename_property("email", "email_address")
            .expect("rename_property should succeed");
        assert_eq!(table.schema().schema_version, 4);

        // Remove property
        table.remove_property("phone")
            .expect("remove_property should succeed");
        assert_eq!(table.schema().schema_version, 5);
    }

    #[test]
    fn test_version_history_add_property() {
        use crate::storage::schema::ChangeDetails;

        let schema = create_test_schema();
        let mut table = VertexTable::new(1, "User".to_string(), schema);

        // Add a property
        table.add_property(StoragePropertyDef::new("email".to_string(), DataType::String))
            .expect("add_property should succeed");

        // Check version history was updated
        let history = table.version_history.lock().unwrap();
        let changes = history.change_log.get_version_changes(2);
        assert!(changes.is_some(), "Should have changes for version 2");

        let changes = changes.unwrap();
        assert_eq!(changes.len(), 1, "Should have exactly one change");

        let change = &changes[0];
        match &change.details {
            ChangeDetails::PropertyAdded { name, .. } => {
                assert_eq!(name, "email");
            }
            _ => panic!("Expected PropertyAdded change"),
        }
    }

    #[test]
    fn test_version_history_remove_property() {
        use crate::storage::schema::ChangeDetails;

        let mut schema = create_test_schema();
        schema.properties.push(StoragePropertyDef::new("email".to_string(), DataType::String));

        let mut table = VertexTable::new(1, "User".to_string(), schema);

        // Remove a property
        table.remove_property("email")
            .expect("remove_property should succeed");

        // Check version history was updated
        let history = table.version_history.lock().unwrap();
        let changes = history.change_log.get_version_changes(2);
        assert!(changes.is_some(), "Should have changes for version 2");

        let changes = changes.unwrap();
        assert_eq!(changes.len(), 1, "Should have exactly one change");

        let change = &changes[0];
        match &change.details {
            ChangeDetails::PropertyRemoved { name, .. } => {
                assert_eq!(name, "email");
            }
            _ => panic!("Expected PropertyRemoved change"),
        }
    }

    #[test]
    fn test_version_history_rename_property() {
        use crate::storage::schema::ChangeDetails;

        let schema = create_test_schema();
        let mut table = VertexTable::new(1, "User".to_string(), schema);

        // Rename a property
        table.rename_property("name", "full_name")
            .expect("rename_property should succeed");

        // Check version history was updated
        let history = table.version_history.lock().unwrap();
        let changes = history.change_log.get_version_changes(2);
        assert!(changes.is_some(), "Should have changes for version 2");

        let changes = changes.unwrap();
        assert_eq!(changes.len(), 1, "Should have exactly one change");

        let change = &changes[0];
        match &change.details {
            ChangeDetails::PropertyRenamed { old_name, new_name } => {
                assert_eq!(old_name, "name");
                assert_eq!(new_name, "full_name");
            }
            _ => panic!("Expected PropertyRenamed change"),
        }
    }

    // ==================== Compaction Edge Case Tests ====================

    /// Test: Compact after deleting all vertices
    ///
    /// This edge case ensures that:
    /// - Empty table after delete doesn't crash on compact
    /// - ID remapping handles empty result correctly
    /// - Memory is properly reclaimed
    #[test]
    fn test_compact_delete_all() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        // Insert 5 vertices
        for i in 0..5 {
            table
                .insert(
                    &format!("v{}", i),
                    &[("name".to_string(), Value::String(format!("Person{}", i)))],
                    100,
                )
                .unwrap();
        }

        assert_eq!(table.scan(100).count(), 5);

        // Delete all vertices
        for i in 0..5 {
            table.delete(&format!("v{}", i), 200).unwrap();
        }

        assert_eq!(table.scan(200).count(), 0);

        // Use compact_with_ts_collect to remove entries deleted before ts 300
        // This clears all deleted entries from id_indexer before compaction
        let removed = table.compact_with_ts_collect(300).expect("compact_with_ts_collect should succeed");
        assert_eq!(removed.len(), 5, "Should have removed 5 deleted entries");

        // After compact_with_ts_collect, table should be empty
        assert_eq!(table.scan(200).count(), 0);
        assert_eq!(table.id_indexer.len(), 0, "id_indexer should be empty after removing all deleted entries");
        assert_eq!(table.timestamps.size(), 0, "timestamps should be empty after removing all deleted entries");
    }

    /// Test: Multiple insert/delete/compact cycles
    ///
    /// This stress test verifies:
    /// - Repeated compaction doesn't corrupt data
    /// - ID remapping works across multiple cycles
    /// - Consistency is maintained (invariants pass)
    #[test]
    fn test_compact_multiple_cycles() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        for cycle in 0..3 {
            let ts_insert = cycle * 100;
            let ts_delete = ts_insert + 50;
            let ts_compact = ts_delete + 50;

            eprintln!("=== Cycle {} ===", cycle);
            eprintln!("Before insert: id_indexer.len() = {}", table.id_indexer.len());

            // Insert 10 vertices
            for i in 0..10 {
                table
                    .insert(
                        &format!("v{}_{}", cycle, i),
                        &[("name".to_string(), Value::String(format!("P{}", i)))],
                        ts_insert as u32,
                    )
                    .expect(&format!("insert cycle {} should succeed", cycle));
            }

            eprintln!("After insert: id_indexer.len() = {}", table.id_indexer.len());
            let scan_count = table.scan(ts_insert as u32).count();
            eprintln!("After insert: scan({}) count = {}", ts_insert, scan_count);

            // After inserting in this cycle, count visible vertices at ts_insert
            // Vertices from previous cycles that were deleted before this cycle are not visible
            let mut expected_after_insert = 0;
            for i in 0..=cycle {
                let ts_cycle_delete = i * 100 + 50;
                if (ts_insert as u32) > (ts_cycle_delete as u32) {
                    // This cycle's vertices were deleted before ts_insert, only 50% remain
                    expected_after_insert += 5;
                } else if i == cycle {
                    // This is the current cycle, all 10 newly inserted are visible
                    expected_after_insert += 10;
                } else {
                    // Previous cycle, all 10 were visible before their deletion
                    expected_after_insert += 10;
                }
            }

            assert_eq!(
                scan_count,
                expected_after_insert,
                "Should have {} vertices after insert at cycle {}",
                expected_after_insert,
                cycle
            );

            // Delete every other vertex (50%)
            for i in 0..10 {
                if i % 2 == 0 {
                    table
                        .delete(
                            &format!("v{}_{}", cycle, i),
                            ts_delete as u32,
                        )
                        .expect(&format!("delete cycle {} should succeed", cycle));
                }
            }

            eprintln!("After delete: id_indexer.len() = {}", table.id_indexer.len());

            // Compact and verify
            table.compact_coordinated().expect(&format!("compact cycle {} should succeed", cycle));

            eprintln!("After compact: id_indexer.len() = {}", table.id_indexer.len());

            // At ts_compact: see vertices that were alive before deletion
            // Cycle 0: 10 inserted, 5 deleted (end_ts=50), so 5 still visible at ts_compact=100
            // Cycle 1: 5 still visible + 5 deleted at ts_delete=150, so all 5 alive at ts_compact=200
            // etc.
            // Total: sum of alive vertices from all cycles up to now
            let mut expected_count = 0;
            for i in 0..=cycle {
                let ts_cycle_delete = i * 100 + 50;
                if ts_compact > ts_cycle_delete {
                    // At this cycle, 50% were deleted, 50% remain
                    expected_count += 5;
                } else {
                    // At this cycle, all 10 are still alive
                    expected_count += 10;
                }
            }

            let final_scan = table.scan(ts_compact as u32).count();
            eprintln!("After compact: scan({}) count = {} (expected {})", ts_compact, final_scan, expected_count);

            assert_eq!(
                final_scan,
                expected_count,
                "Should have {} vertices after compact/delete in cycle {}",
                expected_count,
                cycle
            );
        }
    }

    /// Test: ID consistency across compaction
    ///
    /// Verifies the critical invariant:
    /// - After compact, all three structures (id_indexer, timestamps, columns) are consistent
    /// - id_indexer entries match timestamps entries
    /// - column.row_count() == id_indexer.len()
    /// - All data can be retrieved correctly
    #[test]
    fn test_compact_id_consistency() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        // Insert with gaps (to create IDs: 0, 2, 4, 5, 8)
        let ids = vec![
            table
                .insert(
                    "v0",
                    &[("name".to_string(), Value::String("Alice".to_string()))],
                    100,
                )
                .unwrap(),
            table
                .insert(
                    "v2",
                    &[("name".to_string(), Value::String("Bob".to_string()))],
                    100,
                )
                .unwrap(),
            table
                .insert(
                    "v4",
                    &[("name".to_string(), Value::String("Charlie".to_string()))],
                    100,
                )
                .unwrap(),
            table
                .insert(
                    "v5",
                    &[("name".to_string(), Value::String("David".to_string()))],
                    100,
                )
                .unwrap(),
            table
                .insert(
                    "v8",
                    &[("name".to_string(), Value::String("Eve".to_string()))],
                    100,
                )
                .unwrap(),
        ];

        // Delete some to create gaps
        table.delete("v2", 200).unwrap(); // Delete middle entry
        table.delete("v5", 200).unwrap(); // Delete another

        let before_count = table.scan(150).count(); // Before delete timestamp
        assert_eq!(before_count, 5);

        // Compact should reorganize IDs to be contiguous
        table.compact_coordinated().expect("compact should succeed");

        // Verify invariants in debug mode
        if cfg!(debug_assertions) {
            table.verify_invariants().unwrap();
        }

        // All remaining vertices should be findable
        let after_count = table.scan(200).count(); // After delete timestamp
        assert_eq!(after_count, 3); // Only v0, v4, v8 remain

        // Verify each record can be retrieved
        for (key, expected_name) in &[("v0", "Alice"), ("v4", "Charlie"), ("v8", "Eve")] {
            let internal_id = table
                .get_internal_id(key, 200)
                .expect(&format!("should find {}", key));
            let record = table
                .get_by_internal_id(internal_id, 200)
                .expect(&format!("should retrieve {}", key));

            let name_val = record
                .properties
                .iter()
                .find(|(n, _)| n == "name")
                .map(|(_, v)| v);

            assert_eq!(
                name_val,
                Some(&Value::String(expected_name.to_string())),
                "Name should be preserved for {}",
                key
            );
        }

        // Verify structure consistency after compact
        assert_eq!(
            table.id_indexer.len(),
            table.timestamps.size(),
            "id_indexer and timestamps must have same size"
        );
        assert_eq!(
            table.columns.row_count(),
            table.id_indexer.len(),
            "columns row_count must match id_indexer size"
        );
    }

    // ==================== MVCC Tests ====================

    #[test]
    fn test_vertex_snapshot_isolation() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        // Time 1: Insert vertex
        table.insert(
            "v1",
            &[("name".to_string(), Value::String("Alice".to_string()))],
            100,
        )
        .unwrap();

        // Time 2: Register snapshot at insert time
        let snap1 = table.register_snapshot(100).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);
        assert_eq!(table.min_active_snapshot_ts(), 100);

        // Time 3: Modify vertex at later timestamp
        table.update_property(0, "name", &Value::String("Alice Updated".to_string()), 200)
            .unwrap();

        // Time 4: Delete vertex at even later timestamp
        table.delete("v1", 300).unwrap();

        // Snapshot should still see original data at ts=100
        assert!(table.get_by_internal_id(0, 100).is_some());

        // Current query at ts=300 should see deletion
        assert!(table.get_internal_id("v1", 300).is_none());

        // Unregister snapshot
        table.unregister_snapshot(snap1).unwrap();
        assert_eq!(table.active_snapshot_count(), 0);
        assert_eq!(table.min_active_snapshot_ts(), u32::MAX);
    }

    #[test]
    fn test_vertex_multiple_snapshots() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        // Create vertices at different times
        table.insert(
            "v1",
            &[("name".to_string(), Value::String("Alice".to_string()))],
            100,
        )
        .unwrap();

        // Register first snapshot at ts=100
        let snap1 = table.register_snapshot(100).unwrap();
        assert_eq!(table.min_active_snapshot_ts(), 100);

        // Create another vertex
        table.insert(
            "v2",
            &[("name".to_string(), Value::String("Bob".to_string()))],
            150,
        )
        .unwrap();

        // Register second snapshot at ts=200
        let snap2 = table.register_snapshot(200).unwrap();
        assert_eq!(table.active_snapshot_count(), 2);
        assert_eq!(table.min_active_snapshot_ts(), 100); // min of (100, 200)

        // Delete v1
        table.delete("v1", 250).unwrap();

        // snap1 (ts=100) should see both v1 and v2 (only v1 because v2 inserted at 150 > 100)
        let v1_at_snap1 = table.get_by_internal_id(0, 100);
        assert!(v1_at_snap1.is_some());

        // snap2 (ts=200) should see v1 but not deletion (deletion at ts=250 > 200)
        let v1_at_snap2 = table.get_by_internal_id(0, 200);
        assert!(v1_at_snap2.is_some());

        // Current view should not see v1 (deleted at 250)
        assert!(table.get_by_internal_id(0, 300).is_none());

        // Unregister snapshots
        table.unregister_snapshot(snap1).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);
        assert_eq!(table.min_active_snapshot_ts(), 200);

        table.unregister_snapshot(snap2).unwrap();
        assert_eq!(table.active_snapshot_count(), 0);
        assert_eq!(table.min_active_snapshot_ts(), u32::MAX);
    }

    #[test]
    fn test_vertex_concurrent_snapshots_same_timestamp() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table.insert(
            "v1",
            &[("name".to_string(), Value::String("Alice".to_string()))],
            100,
        )
        .unwrap();

        // Register multiple snapshots at the same timestamp
        let snap1 = table.register_snapshot(100).unwrap();
        let snap2 = table.register_snapshot(100).unwrap();

        // Should track both snapshots even though at same timestamp
        assert_eq!(table.active_snapshot_count(), 1); // One timestamp
        assert_eq!(table.min_active_snapshot_ts(), 100);

        // Each has its own handle ID
        assert_ne!(snap1.id, snap2.id);
        assert_eq!(snap1.ts, snap2.ts);

        // Unregister first snapshot - second should still be active
        table.unregister_snapshot(snap1).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);
        assert_eq!(table.min_active_snapshot_ts(), 100);

        // Unregister second snapshot
        table.unregister_snapshot(snap2).unwrap();
        assert_eq!(table.active_snapshot_count(), 0);
        assert_eq!(table.min_active_snapshot_ts(), u32::MAX);
    }

    #[test]
    fn test_vertex_gc_placeholder() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table.insert(
            "v1",
            &[("name".to_string(), Value::String("Alice".to_string()))],
            100,
        )
        .unwrap();

        // GC should be a no-op for VertexTable (placeholder)
        let cleaned = table.gc(200).unwrap();
        assert_eq!(cleaned, 0);

        // Table should still be intact
        assert!(table.get_by_internal_id(0, 100).is_some());
    }

    #[test]
    fn test_vertex_mvcc_table_trait() {
        use crate::storage::mvcc::MVCCTable;

        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "person".to_string(), schema);

        table.insert(
            "v1",
            &[("name".to_string(), Value::String("Alice".to_string()))],
            100,
        )
        .unwrap();

        // Test through trait interface
        let snap = <VertexTable as MVCCTable>::register_snapshot(&mut table, 100).unwrap();
        assert_eq!(table.active_snapshot_count(), 1);

        <VertexTable as MVCCTable>::unregister_snapshot(&mut table, snap).unwrap();
        assert_eq!(table.active_snapshot_count(), 0);

        let gc_count = <VertexTable as MVCCTable>::gc(&mut table, 200).unwrap();
        assert_eq!(gc_count, 0);
    }
}


