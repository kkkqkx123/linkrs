//! Vertex Index Management Module
//!
//! Provide functions for updating, deleting, and querying vertex indices.
//! This implementation uses in-memory storage with BTreeMap for efficient range queries.
//! Supports persistence through flush/load operations.
//! Supports MVCC (Multi-Version Concurrency Control) for snapshot isolation.

use crate::core::types::{Index, Timestamp, MAX_TIMESTAMP};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::index_data_manager::IndexEntry;
use crate::storage::index::key_codec::key_types::{serialize_value, SecondaryIndexKey};
use crate::storage::index::key_codec::{KeyBuilder, KeyParser, VertexIndexKeyGen};
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone)]
pub struct VertexIndexManager {
    base: GenericIndexManager<VertexIndexKeyGen>,
}

impl VertexIndexManager {
    pub fn new() -> Self {
        Self {
            base: GenericIndexManager::new(),
        }
    }

    pub fn update_vertex_indexes(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
    ) -> Result<(), StorageError> {
        self.update_vertex_indexes_mvcc(space_id, vertex_id, index_name, props, MAX_TIMESTAMP)
    }

    pub fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        for (_prop_name, prop_value) in props {
            let logical_forward_key =
                KeyBuilder::build_vertex_index_key(space_id, index_name, prop_value, vertex_id)?;
            let logical_reverse_key =
                KeyBuilder::build_vertex_reverse_key_v2(space_id, vertex_id, index_name)?;

            let mut forward_keys_to_delete: Vec<SecondaryIndexKey> = Vec::new();
            let mut reverse_keys_to_delete: Vec<SecondaryIndexKey> = Vec::new();

            {
                let forward_index = self.base.forward_index().read();
                let forward_end = KeyBuilder::build_range_end(&logical_forward_key);
                for (key, entry) in
                    forward_index.range(logical_forward_key.0.clone()..forward_end.0)
                {
                    if entry.is_visible_at(write_ts) {
                        forward_keys_to_delete.push(key.clone());
                    }
                }
            }

            {
                let reverse_index = self.base.reverse_index().read();
                let reverse_end = KeyBuilder::build_range_end(&logical_reverse_key);
                for (key, entry) in
                    reverse_index.range(logical_reverse_key.0.clone()..reverse_end.0)
                {
                    if entry.is_visible_at(write_ts) {
                        reverse_keys_to_delete.push(key.clone());
                    }
                }
            }

            {
                let mut forward_index = self.base.forward_index().write();
                for key in &forward_keys_to_delete {
                    if let Some(entry) = forward_index.get_mut(key) {
                        entry.mark_deleted(write_ts);
                    }
                }
            }

            {
                let mut reverse_index = self.base.reverse_index().write();
                for key in &reverse_keys_to_delete {
                    if let Some(entry) = reverse_index.get_mut(key) {
                        entry.mark_deleted(write_ts);
                    }
                }
            }

            let index_key = logical_forward_key;
            let reverse_key = logical_reverse_key;
            let entry = IndexEntry::new(write_ts);
            let compressed_forward = self.base.physical_key(&index_key.0);
            let compressed_reverse = self.base.physical_key(&reverse_key.0);
            {
                let mut forward_index = self.base.forward_index().write();
                forward_index.insert(compressed_forward, entry.clone());
            }
            {
                let mut reverse_index = self.base.reverse_index().write();
                reverse_index.insert(compressed_reverse, entry);
            }
        }

        Ok(())
    }

    pub fn delete_vertex_indexes(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
    ) -> Result<(), StorageError> {
        self.delete_vertex_indexes_mvcc(space_id, vertex_id, index_names, MAX_TIMESTAMP)
    }

    pub fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        if index_names.is_empty() {
            return Ok(());
        }

        let reverse_prefix = KeyBuilder::build_vertex_reverse_prefix_v2(space_id, vertex_id)?;
        let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);

        let mut forward_keys_to_delete: Vec<SecondaryIndexKey> = Vec::new();
        let mut reverse_keys_to_delete: Vec<SecondaryIndexKey> = Vec::new();

        {
            let reverse_index = self.base.reverse_index().read();
            for (compressed_key, entry) in
                reverse_index.range(reverse_prefix.0.clone()..reverse_end.0)
            {
                if entry.is_visible_at(write_ts) {
                    let key_bytes = compressed_key.as_slice();
                    reverse_keys_to_delete.push(compressed_key.clone());

                    if let Ok((_vertex_id_bytes, index_name)) =
                        KeyParser::parse_vertex_reverse_key_v2(key_bytes)
                    {
                        if index_names.contains(&index_name) {
                            let forward_key_start =
                                KeyBuilder::build_vertex_index_prefix(space_id, &index_name);
                            let forward_key_end = KeyBuilder::build_range_end(&forward_key_start);

                            let forward_index = self.base.forward_index().read();
                            for (fwd_compressed_key, fwd_entry) in
                                forward_index.range(forward_key_start.0.clone()..forward_key_end.0)
                            {
                                if fwd_entry.is_visible_at(write_ts) {
                                    if let Ok(vid) = KeyParser::parse_vertex_id_from_key(
                                        fwd_compressed_key.as_slice(),
                                    ) {
                                        if vid == *vertex_id {
                                            forward_keys_to_delete.push(fwd_compressed_key.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        {
            let mut reverse_index = self.base.reverse_index().write();
            for key in &reverse_keys_to_delete {
                if let Some(entry) = reverse_index.get_mut(key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }

        {
            let mut forward_index = self.base.forward_index().write();
            for key in &forward_keys_to_delete {
                if let Some(entry) = forward_index.get_mut(key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }

        Ok(())
    }

    pub fn clear_tag_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError> {
        let prefix = KeyBuilder::build_vertex_index_prefix(space_id, index_name);
        let end = KeyBuilder::build_range_end(&prefix);

        let mut forward_keys_to_mark: Vec<SecondaryIndexKey> = Vec::new();
        let mut reverse_keys_to_mark: Vec<SecondaryIndexKey> = Vec::new();

        {
            let forward_index = self.base.forward_index().read();
            for (key_bytes, entry) in forward_index.range(prefix.0.clone()..end.0) {
                if entry.is_visible_at(MAX_TIMESTAMP) {
                    forward_keys_to_mark.push(key_bytes.clone());
                }
            }
        }

        {
            let reverse_index = self.base.reverse_index().read();
            for (key_bytes, entry) in reverse_index.iter() {
                if !entry.is_visible_at(MAX_TIMESTAMP) {
                    continue;
                }
                if key_bytes.len() < 9 || key_bytes[0..8] != space_id.to_le_bytes() {
                    continue;
                }

                if let Ok((_vertex_id_bytes, parsed_index_name)) =
                    KeyParser::parse_vertex_reverse_key_v2(key_bytes)
                {
                    if parsed_index_name == index_name {
                        reverse_keys_to_mark.push(key_bytes.clone());
                    }
                }
            }
        }

        {
            let mut forward_index = self.base.forward_index().write();
            for key in &forward_keys_to_mark {
                if let Some(entry) = forward_index.get_mut(key) {
                    entry.mark_deleted(MAX_TIMESTAMP);
                }
            }
        }

        {
            let mut reverse_index = self.base.reverse_index().write();
            for key in &reverse_keys_to_mark {
                if let Some(entry) = reverse_index.get_mut(key) {
                    entry.mark_deleted(MAX_TIMESTAMP);
                }
            }
        }

        Ok(())
    }

    pub fn lookup_tag_index(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        self.lookup_tag_index_mvcc(space_id, index, value, MAX_TIMESTAMP)
    }

    pub fn lookup_tag_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError> {
        let prefix = KeyBuilder::build_vertex_index_prefix(space_id, &index.name);
        let end = KeyBuilder::build_range_end(&prefix);

        let mut results = Vec::new();
        let mut seen = HashSet::new();
        let value_bytes = serialize_value(value)?;

        let forward_index = self.base.forward_index().read();
        for (compressed_key, entry) in forward_index.range(prefix.0.clone()..end.0) {
            if !entry.is_visible_at(read_ts) {
                continue;
            }

            let key_bytes = compressed_key.as_slice();
            if let Ok(vertex_id) = KeyParser::parse_vertex_id_from_key(key_bytes) {
                if let Ok(stored_value) = KeyParser::parse_prop_value_from_key(key_bytes) {
                    let sv_bytes = serialize_value(&stored_value)?;
                    if sv_bytes == value_bytes && seen.insert(vertex_id.clone()) {
                        results.push(vertex_id);
                    }
                }
            }
        }

        Ok(results)
    }

    pub fn flush<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        self.base.flush(path)
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        self.base.load(path)
    }

    pub fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<usize, StorageError> {
        self.base.gc_tombstones(safe_ts)
    }

    pub fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<usize, StorageError> {
        self.base.gc_tombstones_incremental(safe_ts, batch_size)
    }

    pub fn tombstone_count(&self) -> usize {
        self.base.tombstone_count()
    }
}

impl Default for VertexIndexManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::core::types::{Index, IndexConfig, IndexField, IndexType};
    use crate::core::Value;

    use super::VertexIndexManager;

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
    fn test_update_and_lookup_vertex_index() {
        let manager = VertexIndexManager::new();

        let space_id = 1u64;
        let vertex_id = Value::Int(123);
        let index_name = "idx_name";
        let props = vec![("name".to_string(), Value::String("Alice".to_string()))];

        manager
            .update_vertex_indexes(space_id, &vertex_id, index_name, &props)
            .expect("Failed to update vertex indexes");

        let index = create_test_index(index_name, "person");

        let results = manager
            .lookup_tag_index(space_id, &index, &Value::String("Alice".to_string()))
            .expect("Failed to lookup tag index");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], vertex_id);

        let empty_results = manager
            .lookup_tag_index(space_id, &index, &Value::String("Bob".to_string()))
            .expect("Failed to lookup tag index");
        assert!(empty_results.is_empty());
    }

    #[test]
    fn test_delete_vertex_indexes() {
        let manager = VertexIndexManager::new();

        let space_id = 1u64;
        let vertex_id = Value::Int(123);
        let index_name = "idx_name";
        let props = vec![("name".to_string(), Value::String("Alice".to_string()))];

        manager
            .update_vertex_indexes(space_id, &vertex_id, index_name, &props)
            .expect("Failed to update vertex indexes");

        let index = create_test_index(index_name, "person");

        let results = manager
            .lookup_tag_index(space_id, &index, &Value::String("Alice".to_string()))
            .expect("Failed to lookup tag index");
        assert_eq!(results.len(), 1);

        manager
            .delete_vertex_indexes(space_id, &vertex_id, &[index_name.to_string()])
            .expect("Failed to delete vertex indexes");

        let results_after = manager
            .lookup_tag_index(space_id, &index, &Value::String("Alice".to_string()))
            .expect("Failed to lookup tag index");
        assert!(results_after.is_empty());
    }

    #[test]
    fn test_clear_tag_index() {
        let manager = VertexIndexManager::new();

        let space_id = 1u64;
        let vertex_id = Value::Int(123);
        let index_name = "idx_name";
        let props = vec![("name".to_string(), Value::String("Alice".to_string()))];

        manager
            .update_vertex_indexes(space_id, &vertex_id, index_name, &props)
            .expect("Failed to update vertex indexes");

        let index = create_test_index(index_name, "person");
        let results = manager
            .lookup_tag_index(space_id, &index, &Value::String("Alice".to_string()))
            .expect("Failed to lookup tag index");
        assert_eq!(results.len(), 1);

        manager
            .clear_tag_index(space_id, index_name)
            .expect("Failed to clear tag index");

        let results_after = manager
            .lookup_tag_index(space_id, &index, &Value::String("Alice".to_string()))
            .expect("Failed to lookup tag index");
        assert!(results_after.is_empty());

        // Entries remain in the BTreeMap (marked as tombstone by mark_deleted)
        // for MVCC snapshot isolation, unlike the old hard remove.
        // GC will clean them up when safe_ts advances past MAX_TIMESTAMP.
        let (fwd, rev) = manager.base.entry_count();
        assert!(fwd >= 1, "forward entries should exist as tombstones");
        assert!(rev >= 1, "reverse entries should exist as tombstones");
    }

    #[test]
    fn test_lookup_deduplicates_versions() {
        let manager = VertexIndexManager::new();

        let space_id = 1u64;
        let vertex_id = Value::Int(123);
        let index_name = "idx_name";
        let props = vec![("name".to_string(), Value::String("Alice".to_string()))];

        manager
            .update_vertex_indexes_mvcc(space_id, &vertex_id, index_name, &props, 10)
            .expect("Failed to update vertex indexes");
        manager
            .update_vertex_indexes_mvcc(space_id, &vertex_id, index_name, &props, 20)
            .expect("Failed to update vertex indexes");

        assert_eq!(manager.base.entry_count(), (2, 2));

        let index = create_test_index(index_name, "person");
        let results = manager
            .lookup_tag_index_mvcc(space_id, &index, &Value::String("Alice".to_string()), 20)
            .expect("Failed to lookup tag index");
        assert_eq!(results, vec![vertex_id.clone()]);

        manager
            .delete_vertex_indexes_mvcc(space_id, &vertex_id, &[index_name.to_string()], 30)
            .expect("Failed to delete vertex indexes");

        let results_after = manager
            .lookup_tag_index_mvcc(space_id, &index, &Value::String("Alice".to_string()), 30)
            .expect("Failed to lookup tag index");
        assert!(results_after.is_empty());
    }

    #[test]
    fn test_clear_tag_index_is_space_scoped() {
        let manager = VertexIndexManager::new();

        let vertex_id_one = Value::Int(1);
        let vertex_id_two = Value::Int(2);
        let index_name = "idx_shared";
        let index_one = create_test_index(index_name, "person");
        let index_two = create_test_index(index_name, "person");

        manager
            .update_vertex_indexes(
                1,
                &vertex_id_one,
                index_name,
                &[("name".to_string(), Value::String("Alice".to_string()))],
            )
            .expect("Failed to insert space one index");
        manager
            .update_vertex_indexes(
                2,
                &vertex_id_two,
                index_name,
                &[("name".to_string(), Value::String("Alice".to_string()))],
            )
            .expect("Failed to insert space two index");

        manager
            .clear_tag_index(1, index_name)
            .expect("Failed to clear space one index");

        let space_one_results = manager
            .lookup_tag_index(1, &index_one, &Value::String("Alice".to_string()))
            .expect("Failed to lookup cleared space");
        assert!(space_one_results.is_empty());

        let space_two_results = manager
            .lookup_tag_index(2, &index_two, &Value::String("Alice".to_string()))
            .expect("Failed to lookup retained space");
        assert_eq!(space_two_results, vec![vertex_id_two]);
    }
}
