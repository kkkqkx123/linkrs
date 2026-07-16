//! Vertex Index Management Module
//!
//! Provide functions for updating, deleting, and querying vertex indices.
//! This implementation uses in-memory storage with BTreeMap for efficient range queries.
//! Supports persistence through flush/load operations.
//! Supports MVCC (Multi-Version Concurrency Control) for snapshot isolation.
//!
//! Index keys now use the order-preserving `OrderedCodec`, eliminating the
//! need for post-filtering predicate matches on range scans.

use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::types::{Index, Timestamp, MAX_TIMESTAMP};
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::index_data_manager::IndexRecord;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser, VertexIndexKeyGen};
use crate::storage::index::manifest::{ManifestCatalog, ManifestHandle};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

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
            let entity_ref = vertex_id_to_entity_ref(vertex_id);
            let entry = if let Some(er) = entity_ref {
                IndexRecord::new(write_ts).with_entity_ref(er)
            } else {
                IndexRecord::new(write_ts)
            };
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

        let forward_index = self.base.forward_index().read();
        for (compressed_key, entry) in forward_index.range(prefix.0.clone()..end.0) {
            if !entry.is_visible_at(read_ts) {
                continue;
            }

            let key_bytes = compressed_key.as_slice();
            if let Ok(vertex_id) = KeyParser::parse_vertex_id_from_key(key_bytes) {
                if let Ok(stored_value) = KeyParser::parse_prop_value_from_key(key_bytes) {
                    if &stored_value == value && seen.insert(vertex_id.clone()) {
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

    pub fn base(&self) -> &GenericIndexManager<VertexIndexKeyGen> {
        &self.base
    }

    pub fn open_tag_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<VertexIndexCursor> {
        self.open_tag_index_cursor_full(space_id, index, plan, None, None)
    }

    pub fn open_tag_index_cursor_with_checker(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<Arc<dyn Fn(&EntityRef) -> bool + Send + Sync>>,
    ) -> StorageResult<VertexIndexCursor> {
        self.open_tag_index_cursor_full(space_id, index, plan, stale_checker, None)
    }

    pub fn open_tag_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<Arc<dyn Fn(&EntityRef) -> bool + Send + Sync>>,
        catalog: Option<&ManifestCatalog>,
    ) -> StorageResult<VertexIndexCursor> {
        let index_prefix = KeyBuilder::build_vertex_index_prefix(space_id, &index.name);

        // Compute range bounds based on predicate.
        // Since the encoding is now order-preserving, the BTreeMap range
        // directly gives the correct results — no post-filtering needed.
        let (start, end) = match &plan.predicate {
            IndexPredicate::Equal(value) => {
                let prefix =
                    KeyBuilder::build_vertex_index_value_prefix(space_id, &index.name, value)?;
                let end = KeyBuilder::build_range_end(&prefix);
                (prefix.0, end.0)
            }
            IndexPredicate::Range {
                lower,
                upper,
                include_lower,
                include_upper,
            } => {
                let start = match lower {
                    Some(value) => {
                        let prefix = KeyBuilder::build_vertex_index_value_prefix(
                            space_id,
                            &index.name,
                            value,
                        )?;
                        if *include_lower {
                            prefix.0
                        } else {
                            KeyBuilder::build_range_end(&prefix).0
                        }
                    }
                    None => index_prefix.0.clone(),
                };
                let end = match upper {
                    Some(value) => {
                        let prefix = KeyBuilder::build_vertex_index_value_prefix(
                            space_id,
                            &index.name,
                            value,
                        )?;
                        if *include_upper {
                            KeyBuilder::build_range_end(&prefix).0
                        } else {
                            prefix.0
                        }
                    }
                    None => KeyBuilder::build_range_end(&index_prefix).0,
                };
                (start, end)
            }
            IndexPredicate::Prefix(value) => {
                // Tight prefix bounds using the codec's terminator-based encoding.
                let (value_lower, value_upper) = OrderedCodec::new().prefix_bounds(value)?;
                let mut start = KeyBuilder::build_vertex_index_prefix(space_id, &index.name).0;
                start.extend_from_slice(&value_lower);
                let mut end = KeyBuilder::build_vertex_index_prefix(space_id, &index.name).0;
                end.extend_from_slice(&value_upper);
                (start, end)
            }
            IndexPredicate::All => (
                index_prefix.0.clone(),
                KeyBuilder::build_range_end(&index_prefix).0,
            ),
        };

        let forward_index = self.base.forward_index_handle();
        let estimated_match_count = {
            let index = forward_index.read();
            index
                .range(start.clone()..end.clone())
                .filter(|(_key, entry)| entry.is_visible_at(plan.read_timestamp))
                .count() as u64
        };

        let manifest_handle = catalog.map(|c| c.acquire());

        Ok(VertexIndexCursor {
            forward_index,
            range_start: start,
            range_end: end,
            next_key: None,
            exhausted: false,
            offset_remaining: plan.offset,
            limit: plan.limit,
            emitted: 0,
            read_timestamp: plan.read_timestamp,
            invisible_skipped: 0,
            malformed_skipped: 0,
            stale_skipped: 0,
            estimated_match_count,
            manifest_handle,
            stale_checker,
        })
    }
}

pub struct VertexIndexCursor {
    forward_index:
        Arc<parking_lot::RwLock<std::collections::BTreeMap<SecondaryIndexKey, IndexRecord>>>,
    range_start: Vec<u8>,
    range_end: Vec<u8>,
    next_key: Option<SecondaryIndexKey>,
    exhausted: bool,
    offset_remaining: usize,
    limit: Option<usize>,
    emitted: usize,
    read_timestamp: Timestamp,
    invisible_skipped: u64,
    malformed_skipped: u64,
    stale_skipped: u64,
    estimated_match_count: u64,
    manifest_handle: Option<ManifestHandle>,
    stale_checker: Option<Arc<dyn Fn(&EntityRef) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for VertexIndexCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexIndexCursor")
            .field("range_start", &self.range_start)
            .field("range_end", &self.range_end)
            .field("next_key", &self.next_key)
            .field("exhausted", &self.exhausted)
            .field("offset_remaining", &self.offset_remaining)
            .field("limit", &self.limit)
            .field("emitted", &self.emitted)
            .field("read_timestamp", &self.read_timestamp)
            .field("invisible_skipped", &self.invisible_skipped)
            .field("malformed_skipped", &self.malformed_skipped)
            .field("stale_skipped", &self.stale_skipped)
            .field("estimated_match_count", &self.estimated_match_count)
            .field("manifest_handle", &self.manifest_handle)
            .field("stale_checker", &self.stale_checker.as_ref().map(|_| "…"))
            .finish()
    }
}

impl IndexCursor for VertexIndexCursor {
    type Row = IndexRow;

    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Self::Row>, StorageError> {
        if self.exhausted {
            return Ok(Vec::new());
        }
        let mut rows = Vec::with_capacity(batch_size.max(1));
        let index = self.forward_index.read();
        let mut last_key = None;
        let mut visited = false;
        let batch_limit = batch_size.max(1);

        let scan = if let Some(next_key) = self.next_key.clone() {
            index.range((
                std::ops::Bound::Excluded(next_key),
                std::ops::Bound::Excluded(self.range_end.clone()),
            ))
        } else {
            index.range((
                std::ops::Bound::Included(self.range_start.clone()),
                std::ops::Bound::Excluded(self.range_end.clone()),
            ))
        };
        for (key, entry) in scan {
            visited = true;
            last_key = Some(key.clone());
            if !entry.is_visible_at(self.read_timestamp) {
                self.invisible_skipped += 1;
                continue;
            }

            if self.offset_remaining > 0 {
                self.offset_remaining -= 1;
                continue;
            }

            // Get entity_ref from IndexRecord (new path) or fall back to key parsing.
            let entity_ref = match &entry.entity_ref {
                Some(er) => er.clone(),
                None => {
                    let vertex_id = match KeyParser::parse_vertex_id_from_key(key) {
                        Ok(vid) => vid,
                        Err(_) => {
                            self.malformed_skipped += 1;
                            continue;
                        }
                    };
                    match vertex_id_to_entity_ref(&vertex_id) {
                        Some(er) => er,
                        None => {
                            self.stale_skipped += 1;
                            continue;
                        }
                    }
                }
            };

            // Validate entity existence via optional stale checker.
            if let Some(ref checker) = self.stale_checker {
                if !checker(&entity_ref) {
                    self.stale_skipped += 1;
                    continue;
                }
            }

            // Covering projection support: when projection matches included_columns,
            // return IndexRow::Covering instead of RowId (phased in with planner support).
            let row = if !entry.included_columns.is_empty() {
                IndexRow::Covering {
                    entity_ref,
                    columns: entry.included_columns.clone(),
                }
            } else {
                IndexRow::RowId(entity_ref)
            };
            rows.push(row);
            self.emitted += 1;
            if self.limit.is_some_and(|limit| self.emitted >= limit) {
                break;
            }
            if rows.len() >= batch_limit {
                break;
            }
        }
        drop(index);
        self.next_key = last_key;
        if !visited || self.limit.is_some_and(|limit| self.emitted >= limit) {
            self.exhausted = true;
        }
        Ok(rows)
    }

    fn estimated_match_count(&self) -> Option<u64> {
        Some(self.estimated_match_count)
    }

    fn stale_skipped(&self) -> u64 {
        self.invisible_skipped + self.malformed_skipped + self.stale_skipped
    }

    fn invisible_skipped(&self) -> u64 {
        self.invisible_skipped
    }

    fn malformed_skipped(&self) -> u64 {
        self.malformed_skipped
    }

    fn is_exhausted(&self) -> bool {
        self.exhausted
    }
}

/// Convert a Value that represents a vertex ID into an EntityRef.
fn vertex_id_to_entity_ref(v: &Value) -> Option<EntityRef> {
    match v {
        Value::BigInt(id) => Some(EntityRef::Vertex(
            crate::core::types::storage_ids::VertexId::from_int64(*id),
        )),
        Value::Int(id) => Some(EntityRef::Vertex(
            crate::core::types::storage_ids::VertexId::from_int64(*id as i64),
        )),
        Value::String(s) => {
            // Try to parse as i64 first, then treat as string ID
            if let Ok(id) = s.parse::<i64>() {
                Some(EntityRef::Vertex(
                    crate::core::types::storage_ids::VertexId::from_int64(id),
                ))
            } else {
                Some(EntityRef::Vertex(
                    crate::core::types::storage_ids::VertexId::from_string(s.clone()),
                ))
            }
        }
        Value::Vertex(v) => Some(EntityRef::Vertex(v.vid)),
        _ => None,
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
    use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};

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

    #[test]
    fn native_cursor_filters_by_predicate_and_preserves_snapshot_visibility() {
        let manager = VertexIndexManager::new();
        let index = create_test_index("idx_name", "person");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(1),
                "idx_name",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                10,
            )
            .expect("first index entry should be written");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(2),
                "idx_name",
                &[("name".to_string(), Value::String("Bob".to_string()))],
                20,
            )
            .expect("second index entry should be written");

        let plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::Prefix(Value::String("A".to_string())),
            projection: None,
            limit: None,
            offset: 0,
            read_timestamp: 10,
        };
        let mut cursor = manager
            .open_tag_index_cursor(1, &index, &plan)
            .expect("native cursor should open");
        assert_eq!(cursor.estimated_match_count(), Some(1));

        // Cursor now returns IndexRow, not Value
        let batch = cursor.next_batch(8).expect("cursor should read entries");
        assert_eq!(batch.len(), 1);
        match &batch[0] {
            IndexRow::RowId(_) => {} // RowId is the expected return type
            _ => panic!("expected RowId"),
        }

        let empty = cursor.next_batch(8).expect("cursor should reach end");
        assert!(empty.is_empty());
    }

    #[test]
    fn native_cursor_paginates_across_index_range() {
        let manager = VertexIndexManager::new();
        let index = create_test_index("idx_name", "person");
        for id in 0..32 {
            manager
                .update_vertex_indexes_mvcc(
                    1,
                    &Value::Int(id),
                    "idx_name",
                    &[("name".to_string(), Value::String(format!("name-{id}")))],
                    10,
                )
                .expect("index entry should be written");
        }

        let plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::Prefix(Value::String("name-".to_string())),
            projection: None,
            limit: None,
            offset: 0,
            read_timestamp: 10,
        };
        let mut cursor = manager
            .open_tag_index_cursor(1, &index, &plan)
            .expect("native cursor should open");
        let mut count = 0;
        loop {
            let batch = cursor.next_batch(3).expect("cursor should read entries");
            if batch.is_empty() {
                break;
            }
            count += batch.len();
        }

        assert_eq!(count, 32);
    }
}
