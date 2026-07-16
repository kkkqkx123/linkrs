use crate::core::types::{Index, Timestamp, MAX_TIMESTAMP};
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::index_data_manager::IndexEntry;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::{EdgeIndexKeyGen, KeyBuilder, KeyParser};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct EdgeIndexManager {
    base: GenericIndexManager<EdgeIndexKeyGen>,
}

impl EdgeIndexManager {
    pub fn new() -> Self {
        Self {
            base: GenericIndexManager::new(),
        }
    }

    pub fn update_edge_indexes(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_name: &str,
        props: &[(String, Value)],
    ) -> Result<(), StorageError> {
        self.update_edge_indexes_mvcc(
            space_id,
            edge_src,
            edge_dst,
            edge_type,
            ranking,
            index_name,
            props,
            MAX_TIMESTAMP,
        )
    }

    pub fn update_edge_indexes_mvcc(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        for (_prop_name, prop_value) in props {
            let logical_forward_key = KeyBuilder::build_edge_index_key(
                space_id, index_name, prop_value, edge_src, edge_dst, edge_type, ranking,
            )?;
            let logical_reverse_key = KeyBuilder::build_edge_reverse_key(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name,
            )?;

            let mut forward_keys_to_delete: Vec<SecondaryIndexKey> = Vec::new();

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
                let mut forward_index = self.base.forward_index().write();
                for key in &forward_keys_to_delete {
                    if let Some(entry) = forward_index.get_mut(key) {
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

    pub fn delete_edge_indexes(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_names: &[String],
    ) -> Result<(), StorageError> {
        self.delete_edge_indexes_mvcc(
            space_id,
            edge_src,
            edge_dst,
            edge_type,
            ranking,
            index_names,
            MAX_TIMESTAMP,
        )
    }

    pub fn delete_edge_indexes_mvcc(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        if index_names.is_empty() {
            return Ok(());
        }

        let reverse_prefix = KeyBuilder::build_edge_reverse_prefix(
            space_id, edge_src, edge_dst, edge_type, ranking,
        )?;
        let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);

        let mut forward_keys_to_delete: Vec<SecondaryIndexKey> = Vec::new();
        let mut reverse_keys_to_delete: Vec<SecondaryIndexKey> = Vec::new();

        {
            let reverse_index = self.base.reverse_index().read();
            for (compressed_key, entry) in
                reverse_index.range(reverse_prefix.0.clone()..reverse_end.0)
            {
                if entry.is_visible_at(write_ts) {
                    reverse_keys_to_delete.push(compressed_key.clone());

                    if let Ok((
                        _src_bytes,
                        _dst_bytes,
                        _type_bytes,
                        _rank_bytes,
                        parsed_index_name,
                    )) = KeyParser::parse_edge_reverse_key(compressed_key)
                    {
                        if index_names.contains(&parsed_index_name) {
                            let forward_key_start =
                                KeyBuilder::build_edge_index_prefix(space_id, &parsed_index_name);
                            let forward_key_end = KeyBuilder::build_range_end(&forward_key_start);

                            let forward_index = self.base.forward_index().read();
                            for (fwd_compressed_key, fwd_entry) in
                                forward_index.range(forward_key_start.0.clone()..forward_key_end.0)
                            {
                                if fwd_entry.is_visible_at(write_ts) {
                                    if let Ok((fwd_src, fwd_dst, fwd_type, fwd_rank)) =
                                        KeyParser::parse_edge_identity_from_key(fwd_compressed_key)
                                    {
                                        if fwd_src == *edge_src
                                            && fwd_dst == *edge_dst
                                            && fwd_type == edge_type
                                            && fwd_rank == ranking
                                        {
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

    pub fn clear_edge_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError> {
        let prefix = KeyBuilder::build_edge_index_prefix(space_id, index_name);
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

                if let Ok((_src_bytes, _dst_bytes, _type_bytes, _rank_bytes, parsed_index_name)) =
                    KeyParser::parse_edge_reverse_key(key_bytes)
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

    pub fn lookup_edge_index(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
    ) -> Result<Vec<(Value, Value, String, i64)>, StorageError> {
        self.lookup_edge_index_mvcc(space_id, index, value, MAX_TIMESTAMP)
    }

    pub fn lookup_edge_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<(Value, Value, String, i64)>, StorageError> {
        let prefix = KeyBuilder::build_edge_index_prefix(space_id, &index.name);
        let end = KeyBuilder::build_range_end(&prefix);

        let mut results = Vec::new();
        let mut seen = HashSet::new();

        let forward_index = self.base.forward_index().read();
        for (compressed_key, entry) in forward_index.range(prefix.0.clone()..end.0) {
            if !entry.is_visible_at(read_ts) {
                continue;
            }

            let key_bytes = compressed_key.as_slice();
            if let Ok(stored_value) = KeyParser::parse_prop_value_from_edge_key(key_bytes) {
                if &stored_value == value {
                    if let Ok((src, dst, edge_type, ranking)) =
                        KeyParser::parse_edge_identity_from_key(key_bytes)
                    {
                        let key = (src.clone(), dst.clone(), edge_type.clone(), ranking);
                        if seen.insert(key.clone()) {
                            results.push((src, dst, edge_type, ranking));
                        }
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

    pub fn open_edge_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<EdgeIndexCursor> {
        let index_prefix = KeyBuilder::build_edge_index_prefix(space_id, &index.name);

        let (start, end) = match &plan.predicate {
            IndexPredicate::Equal(value) => {
                let prefix =
                    KeyBuilder::build_edge_index_value_prefix(space_id, &index.name, value)?;
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
                        let prefix = KeyBuilder::build_edge_index_value_prefix(
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
                        let prefix = KeyBuilder::build_edge_index_value_prefix(
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
                let value_prefix =
                    KeyBuilder::build_edge_index_value_prefix(space_id, &index.name, value)?;
                let end = KeyBuilder::build_range_end(&index_prefix);
                (value_prefix.0, end.0)
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

        let prefix_filter = match &plan.predicate {
            IndexPredicate::Prefix(value) => Some(value.clone()),
            _ => None,
        };

        Ok(EdgeIndexCursor {
            forward_index,
            range_start: start,
            range_end: end,
            next_key: None,
            exhausted: false,
            offset_remaining: plan.offset,
            limit: plan.limit,
            emitted: 0,
            read_timestamp: plan.read_timestamp,
            estimated_match_count,
            prefix_filter,
        })
    }
}

impl Default for EdgeIndexManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct EdgeIndexCursor {
    forward_index:
        Arc<parking_lot::RwLock<std::collections::BTreeMap<SecondaryIndexKey, IndexEntry>>>,
    range_start: Vec<u8>,
    range_end: Vec<u8>,
    next_key: Option<SecondaryIndexKey>,
    exhausted: bool,
    offset_remaining: usize,
    limit: Option<usize>,
    emitted: usize,
    read_timestamp: Timestamp,
    estimated_match_count: u64,
    prefix_filter: Option<Value>,
}

impl IndexCursor for EdgeIndexCursor {
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
                continue;
            }

            if let Some(ref prefix) = self.prefix_filter {
                let prop_value = match KeyParser::parse_prop_value_from_edge_key(key) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match (&prop_value, prefix) {
                    (Value::String(s), Value::String(prefix_str)) => {
                        if !s.starts_with(prefix_str) {
                            continue;
                        }
                    }
                    (Value::Blob(b), Value::Blob(prefix_bytes)) => {
                        if !b.starts_with(prefix_bytes) {
                            continue;
                        }
                    }
                    _ => continue,
                }
            }

            if self.offset_remaining > 0 {
                self.offset_remaining -= 1;
                continue;
            }

            let entity_ref = match parse_edge_entity_ref(key) {
                Some(er) => er,
                None => continue,
            };
            rows.push(IndexRow::RowId(entity_ref));
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
        0
    }

    fn is_exhausted(&self) -> bool {
        self.exhausted
    }
}

fn parse_edge_entity_ref(key: &[u8]) -> Option<EntityRef> {
    let (src, dst, edge_type, ranking) = KeyParser::parse_edge_identity_from_key(key).ok()?;
    let src_id = value_to_vertex_id(&src)?;
    let dst_id = value_to_vertex_id(&dst)?;
    let edge_type_id: u32 = edge_type.parse::<u32>().unwrap_or_default();
    Some(EntityRef::Edge {
        src: src_id,
        dst: dst_id,
        edge_type: edge_type_id,
        ranking,
    })
}

fn value_to_vertex_id(v: &Value) -> Option<crate::core::types::storage_ids::VertexId> {
    match v {
        Value::BigInt(id) => Some(crate::core::types::storage_ids::VertexId::from_int64(*id)),
        Value::Int(id) => Some(crate::core::types::storage_ids::VertexId::from_int64(
            *id as i64,
        )),
        Value::String(s) => {
            if let Ok(id) = s.parse::<i64>() {
                Some(crate::core::types::storage_ids::VertexId::from_int64(id))
            } else {
                Some(crate::core::types::storage_ids::VertexId::from_string(
                    s.clone(),
                ))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::core::types::storage_ids::VertexId;
    use crate::core::types::{Index, IndexConfig, IndexField, IndexType};
    use crate::core::Value;
    use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};

    use super::EdgeIndexManager;

    fn create_test_index(name: &str, schema_name: &str) -> Index {
        Index::new(IndexConfig {
            id: 1,
            name: name.to_string(),
            space_id: 1,
            schema_name: schema_name.to_string(),
            fields: vec![IndexField::new(
                "weight".to_string(),
                Value::String("".to_string()),
                false,
            )],
            properties: vec![],
            index_type: IndexType::EdgeIndex,
            is_unique: false,
            partial_condition: None,
        })
    }

    fn make_edge_values(src_id: i64, dst_id: i64, edge_type: &str, ranking: i64) -> (Value, Value) {
        (Value::BigInt(src_id), Value::BigInt(dst_id))
    }

    #[test]
    fn test_update_and_lookup_edge_index() {
        let manager = EdgeIndexManager::new();

        let space_id = 1u64;
        let (src, dst) = make_edge_values(101, 202, "knows", 1);
        let index_name = "idx_weight";
        let props = vec![("weight".to_string(), Value::Int(42))];

        manager
            .update_edge_indexes(space_id, &src, &dst, "knows", 1, index_name, &props)
            .expect("Failed to update edge indexes");

        let index = create_test_index(index_name, "knows");

        let results = manager
            .lookup_edge_index(space_id, &index, &Value::Int(42))
            .expect("Failed to lookup edge index");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, src);
        assert_eq!(results[0].1, dst);
        assert_eq!(results[0].2, "knows");
        assert_eq!(results[0].3, 1);
    }

    #[test]
    fn test_delete_edge_indexes() {
        let manager = EdgeIndexManager::new();

        let space_id = 1u64;
        let (src, dst) = make_edge_values(101, 202, "knows", 1);
        let index_name = "idx_weight";
        let props = vec![("weight".to_string(), Value::Int(42))];

        manager
            .update_edge_indexes(space_id, &src, &dst, "knows", 1, index_name, &props)
            .expect("Failed to update edge indexes");

        let index = create_test_index(index_name, "knows");
        let results = manager
            .lookup_edge_index(space_id, &index, &Value::Int(42))
            .expect("Failed to lookup");
        assert_eq!(results.len(), 1);

        manager
            .delete_edge_indexes(space_id, &src, &dst, "knows", 1, &[index_name.to_string()])
            .expect("Failed to delete edge indexes");

        let results_after = manager
            .lookup_edge_index(space_id, &index, &Value::Int(42))
            .expect("Failed to lookup after delete");
        assert!(results_after.is_empty());
    }

    #[test]
    fn test_clear_edge_index() {
        let manager = EdgeIndexManager::new();

        let space_id = 1u64;
        let (src1, dst1) = make_edge_values(101, 202, "knows", 1);
        let (src2, dst2) = make_edge_values(102, 203, "knows", 2);
        let index_name = "idx_weight";

        manager
            .update_edge_indexes(
                space_id,
                &src1,
                &dst1,
                "knows",
                1,
                index_name,
                &[("weight".to_string(), Value::Int(42))],
            )
            .expect("insert edge 1");
        manager
            .update_edge_indexes(
                space_id,
                &src2,
                &dst2,
                "knows",
                2,
                index_name,
                &[("weight".to_string(), Value::Int(99))],
            )
            .expect("insert edge 2");

        manager
            .clear_edge_index(space_id, index_name)
            .expect("clear edge index");

        let index = create_test_index(index_name, "knows");
        let results = manager
            .lookup_edge_index(space_id, &index, &Value::Int(42))
            .expect("lookup");
        assert!(results.is_empty());

        let (fwd, rev) = manager.base.entry_count();
        assert!(fwd >= 1);
        assert!(rev >= 1);
    }

    #[test]
    fn test_edge_index_cursor() {
        let manager = EdgeIndexManager::new();
        let index = create_test_index("idx_weight", "knows");

        manager
            .update_edge_indexes_mvcc(
                1,
                &Value::BigInt(1),
                &Value::BigInt(2),
                "knows",
                0,
                "idx_weight",
                &[("weight".to_string(), Value::Int(10))],
                10,
            )
            .expect("edge entry");
        manager
            .update_edge_indexes_mvcc(
                1,
                &Value::BigInt(3),
                &Value::BigInt(4),
                "knows",
                1,
                "idx_weight",
                &[("weight".to_string(), Value::Int(20))],
                20,
            )
            .expect("edge entry");

        let plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            projection: None,
            limit: None,
            offset: 0,
            read_timestamp: 20,
        };
        let mut cursor = manager
            .open_edge_index_cursor(1, &index, &plan)
            .expect("cursor");
        assert_eq!(cursor.estimated_match_count(), Some(2));

        let batch = cursor.next_batch(8).expect("read");
        assert_eq!(batch.len(), 2);
    }
}
