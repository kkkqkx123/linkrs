//! Index Data Manager
//!
//! Provide update, delete and query functions for indexed data
//! The management of index metadata is handled by the IndexMetadataManager.
//! All operations identify a space by its space_id, enabling multi-space data segregation.
//! Supports persistence through flush/load operations.
//! Supports MVCC (Multi-Version Concurrency Control) for snapshot isolation.

use crate::core::types::{Index, Timestamp, MAX_TIMESTAMP};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::vertex_index_manager::VertexIndexManager;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub created_ts: Timestamp,
    pub deleted_ts: Option<Timestamp>,
}

impl IndexEntry {
    pub fn new(created_ts: Timestamp) -> Self {
        Self {
            created_ts,
            deleted_ts: None,
        }
    }

    pub fn is_visible_at(&self, read_ts: Timestamp) -> bool {
        self.created_ts <= read_ts
            && self
                .deleted_ts
                .is_none_or(|deleted_ts| deleted_ts > read_ts)
    }

    pub fn mark_deleted(&mut self, deleted_ts: Timestamp) {
        self.deleted_ts = Some(deleted_ts);
    }
}

impl Default for IndexEntry {
    fn default() -> Self {
        Self::new(MAX_TIMESTAMP)
    }
}

/// Vertex index operations trait.
/// Provides update, delete, and lookup operations for vertex indexes.
pub trait VertexIndexOps: Send + Sync {
    fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn lookup_tag_index(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        self.lookup_tag_index_mvcc(space_id, index, value, MAX_TIMESTAMP)
    }

    fn lookup_tag_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError>;

    fn clear_tag_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError>;
}

/// Index garbage collection operations trait.
pub trait IndexGcOps: Send + Sync {
    fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<GcStats, StorageError>;
    fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<GcStats, StorageError>;
    fn tombstone_count(&self) -> usize;
}

#[derive(Clone)]
pub struct IndexDataManagerImpl {
    vertex_manager: VertexIndexManager,
}

impl IndexDataManagerImpl {
    pub fn new() -> Self {
        Self {
            vertex_manager: VertexIndexManager::new(),
        }
    }

    pub fn flush<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        self.vertex_manager.flush(path.join("vertex_index"))?;
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        self.vertex_manager.load(path.join("vertex_index"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GcStats {
    pub vertex_entries_removed: usize,
}

impl GcStats {
    pub fn total_removed(&self) -> usize {
        self.vertex_entries_removed
    }

    pub fn is_empty(&self) -> bool {
        self.vertex_entries_removed == 0
    }
}

impl Default for IndexDataManagerImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl VertexIndexOps for IndexDataManagerImpl {
    fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        if write_ts == MAX_TIMESTAMP {
            self.vertex_manager
                .update_vertex_indexes(space_id, vertex_id, index_name, props)
        } else {
            self.vertex_manager
                .update_vertex_indexes_mvcc(space_id, vertex_id, index_name, props, write_ts)
        }
    }

    fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        if write_ts == MAX_TIMESTAMP {
            self.vertex_manager
                .delete_vertex_indexes(space_id, vertex_id, index_names)
        } else {
            self.vertex_manager.delete_vertex_indexes_mvcc(
                space_id,
                vertex_id,
                index_names,
                write_ts,
            )
        }
    }

    fn lookup_tag_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError> {
        if read_ts == MAX_TIMESTAMP {
            self.vertex_manager.lookup_tag_index(space_id, index, value)
        } else {
            self.vertex_manager
                .lookup_tag_index_mvcc(space_id, index, value, read_ts)
        }
    }

    fn clear_tag_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError> {
        self.vertex_manager.clear_tag_index(space_id, index_name)
    }
}

impl IndexGcOps for IndexDataManagerImpl {
    fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<GcStats, StorageError> {
        let vertex_removed = self.vertex_manager.gc_tombstones(safe_ts)?;

        Ok(GcStats {
            vertex_entries_removed: vertex_removed,
        })
    }

    fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<GcStats, StorageError> {
        let vertex_removed = self
            .vertex_manager
            .gc_tombstones_incremental(safe_ts, batch_size)?;

        Ok(GcStats {
            vertex_entries_removed: vertex_removed,
        })
    }

    fn tombstone_count(&self) -> usize {
        self.vertex_manager.tombstone_count()
    }
}

#[cfg(test)]
mod tests {
    use crate::core::types::{Index, IndexConfig, IndexField, IndexType};
    use crate::core::Value;
    use crate::storage::index::*;

    fn create_test_index(name: &str, schema_name: &str) -> Index {
        Index::new(IndexConfig {
            id: 1,
            name: name.to_string(),
            space_id: 1,
            schema_name: schema_name.to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::String("".to_string()),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        })
    }

    #[test]
    fn test_serialize_deserialize_value() {
        let value = Value::String("test".to_string());
        let bytes = crate::storage::index::key_codec::key_types::serialize_value(&value)
            .expect("serialize should succeed");
        let decoded = crate::storage::index::key_codec::key_types::deserialize_value(&bytes)
            .expect("deserialize should succeed");
        assert_eq!(value, decoded);
    }

    #[test]
    fn test_update_and_lookup_vertex_index() {
        let manager = IndexDataManagerImpl::new();

        let space_id = 1u64;
        let vertex_id = Value::Int(1);
        let index_name = "idx_person_name";
        let props = vec![("name".to_string(), Value::String("Alice".to_string()))];

        manager
            .vertex_manager
            .update_vertex_indexes(space_id, &vertex_id, index_name, &props)
            .expect("Failed to update vertex indexes");

        let index = create_test_index(index_name, "person");

        let results = manager
            .lookup_tag_index(space_id, &index, &Value::String("Alice".to_string()))
            .expect("Failed to lookup tag index");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], vertex_id);
    }
}
