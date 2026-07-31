use std::collections::HashMap;

use crate::core::types::{Index, Timestamp};
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};
use crate::storage::edge::bloom_filter::EdgeDeletionBloomFilter;
use crate::storage::index::chunk::chunked_index::ChunkedIndex;
use crate::storage::index::cursor::ChainForwardIterator;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser};
use crate::storage::index::manifest::ManifestHandle;
use crate::storage::index::types::{IndexRecord, StaleChecker};

pub(crate) fn compute_edge_index_scan_range(
    space_id: u64,
    index: &Index,
    plan: &IndexScanPlan,
) -> StorageResult<(Vec<u8>, Vec<u8>)> {
    let index_prefix = KeyBuilder::build_edge_index_prefix(space_id, &index.name);
    match &plan.predicate {
        IndexPredicate::Equal(value) => {
            let prefix =
                KeyBuilder::build_edge_index_value_prefix(space_id, &index.name, value)?;
            let end = KeyBuilder::build_range_end(&prefix);
            Ok((prefix.0, end.0))
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
            Ok((start, end))
        }
        IndexPredicate::Prefix(value) => {
            let (value_lower, value_upper) = OrderedCodec::new().prefix_bounds(value)?;
            let mut start = KeyBuilder::build_edge_index_prefix(space_id, &index.name).0;
            start.extend_from_slice(&value_lower);
            let mut end = KeyBuilder::build_edge_index_prefix(space_id, &index.name).0;
            end.extend_from_slice(&value_upper);
            Ok((start, end))
        }
        IndexPredicate::All => {
            let end = KeyBuilder::build_range_end(&index_prefix).0;
            Ok((index_prefix.0, end))
        }
    }
}

pub struct EdgeIndexCursor {
    shard_iterators: Vec<ChainForwardIterator>,
    current_range: usize,
    exhausted: bool,
    offset_remaining: usize,
    limit: Option<usize>,
    emitted: usize,
    projection: Option<Vec<String>>,
    read_timestamp: Timestamp,
    invisible_skipped: u64,
    malformed_skipped: u64,
    stale_skipped: u64,
    manifest_handle: Option<ManifestHandle>,
    stale_checker: Option<StaleChecker>,
    partition_id_range: Option<std::ops::Range<i64>>,
}

impl std::fmt::Debug for EdgeIndexCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeIndexCursor")
            .field("shard_iterators_count", &self.shard_iterators.len())
            .field("current_range", &self.current_range)
            .field("exhausted", &self.exhausted)
            .field("offset_remaining", &self.offset_remaining)
            .field("limit", &self.limit)
            .field("emitted", &self.emitted)
            .field("read_timestamp", &self.read_timestamp)
            .field("invisible_skipped", &self.invisible_skipped)
            .field("malformed_skipped", &self.malformed_skipped)
            .field("stale_skipped", &self.stale_skipped)
            .field("manifest_handle", &self.manifest_handle)
            .field("stale_checker", &self.stale_checker.as_ref().map(|_| "…"))
            .finish()
    }
}

impl EdgeIndexCursor {
    pub(crate) fn new(
        shard_iterators: Vec<ChainForwardIterator>,
        plan: &IndexScanPlan,
        stale_checker: Option<StaleChecker>,
        manifest_handle: Option<ManifestHandle>,
    ) -> Self {
        Self {
            shard_iterators,
            current_range: 0,
            exhausted: false,
            offset_remaining: plan.offset,
            limit: plan.limit,
            emitted: 0,
            projection: plan.projection.clone(),
            read_timestamp: plan.read_timestamp,
            invisible_skipped: 0,
            malformed_skipped: 0,
            stale_skipped: 0,
            manifest_handle,
            stale_checker,
            partition_id_range: plan.partition_id_range.clone(),
        }
    }
}

impl IndexCursor for EdgeIndexCursor {
    type Row = IndexRow;

    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Self::Row>, StorageError> {
        if self.exhausted || self.shard_iterators.is_empty() {
            self.exhausted = true;
            return Ok(Vec::new());
        }
        let mut rows = Vec::with_capacity(batch_size.max(1));
        let batch_limit = batch_size.max(1);
        while self.current_range < self.shard_iterators.len() && rows.len() < batch_limit {
            let key_entry = self.shard_iterators[self.current_range].next();
            let Some((key, entry)) = key_entry else {
                self.current_range += 1;
                continue;
            };

            let entity_ref = match &entry.entity_ref {
                Some(entity_ref) => entity_ref.clone(),
                None => match parse_edge_entity_ref(&key) {
                    Some(entity_ref) => entity_ref,
                    None => {
                        self.malformed_skipped += 1;
                        continue;
                    }
                },
            };
            if self
                .stale_checker
                .as_ref()
                .is_some_and(|checker| !checker(&entity_ref, entry.entity_version))
            {
                self.stale_skipped += 1;
                continue;
            }
            if let Some(ref prange) = self.partition_id_range {
                let src = match &entity_ref {
                    crate::core::wal::EntityRef::Edge { src, .. } => src,
                    _ => continue,
                };
                let bytes = src.as_bytes();
                if bytes.len() == 8 {
                    let mut buf = [0u8; 8];
                    buf.copy_from_slice(bytes);
                    let vid_i64 = i64::from_be_bytes(buf);
                    if vid_i64 < prange.start || vid_i64 >= prange.end {
                        continue;
                    }
                }
            }
            if self.offset_remaining > 0 {
                self.offset_remaining -= 1;
                continue;
            }
            rows.push(project_edge_row(
                entity_ref,
                entry.included_columns.as_deref(),
                self.projection.as_deref(),
            ));
            self.emitted += 1;
            if self.limit.is_some_and(|limit| self.emitted >= limit) {
                if self.current_range + 1 >= self.shard_iterators.len() {
                    self.exhausted = true;
                }
                break;
            }
        }
        self.exhausted |= self.current_range >= self.shard_iterators.len();
        Ok(rows)
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

fn project_edge_row(
    entity_ref: EntityRef,
    included_columns: Option<&[(String, Value)]>,
    projection: Option<&[String]>,
) -> IndexRow {
    let Some(projection) = projection else {
        return IndexRow::RowId(entity_ref);
    };
    let columns = included_columns.unwrap_or(&[]);
    if !projection.is_empty()
        && !projection.iter().all(|name| {
            columns
                .iter()
                .any(|(candidate, _)| candidate == name)
        })
    {
        return IndexRow::RowId(entity_ref);
    }
    let columns = projection
        .iter()
        .filter_map(|name| {
            columns
                .iter()
                .find(|(candidate, _)| candidate == name)
                .cloned()
        })
        .collect();
    IndexRow::Covering {
        entity_ref,
        columns,
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

/// Per-property edge index using ChunkedIndex for efficient range filtering.
///
/// Each property name maps to a ChunkedIndex keyed by:
/// `[OrderedCodec(prop_value)][OrderedCodec(BigInt(src))][OrderedCodec(BigInt(dst))][OrderedCodec(BigInt(rank))]`
/// - The property value prefix enables range queries (e.g., weight > 100)
/// - The edge identity suffix ensures unique keys within the same value
pub struct EdgePropertyIndex {
    indexes: HashMap<String, ChunkedIndex>,
    deleted_filter: EdgeDeletionBloomFilter,
    pool_capacity: u64,
}

impl EdgePropertyIndex {
    pub fn new(pool_capacity: u64) -> Self {
        Self {
            indexes: HashMap::new(),
            deleted_filter: EdgeDeletionBloomFilter::with_capacity(1000),
            pool_capacity,
        }
    }

    fn encode_edge_property_key(
        prop_value: &Value,
        src: u32,
        dst: u32,
        rank: i64,
    ) -> StorageResult<Vec<u8>> {
        let codec = OrderedCodec::new();
        let mut key = codec.encode(prop_value)?;
        key.extend_from_slice(&src.to_le_bytes());
        key.extend_from_slice(&dst.to_le_bytes());
        key.extend_from_slice(&rank.to_le_bytes());
        Ok(key)
    }

    fn get_or_create_index(&mut self, prop_name: &str) -> &mut ChunkedIndex {
        let capacity = self.pool_capacity;
        self.indexes
            .entry(prop_name.to_string())
            .or_insert_with(|| ChunkedIndex::empty(prop_name.as_bytes().to_vec(), capacity))
    }

    pub fn lookup(
        &self,
        prop_name: &str,
        value_lower: &[u8],
        value_upper: &[u8],
    ) -> Vec<((u32, u32, i64), IndexRecord)> {
        let Some(index) = self.indexes.get(prop_name) else {
            return Vec::new();
        };
        let results = index.range(value_lower, value_upper);
        results
            .into_iter()
            .filter_map(|(_key, record)| {
                let entity_ref = record.entity_ref.as_ref()?;
                match entity_ref {
                    EntityRef::Edge {
                        src, dst, ranking, ..
                    } => {
                        let src_u32 = u32_from_vertex_id(src)?;
                        let dst_u32 = u32_from_vertex_id(dst)?;
                        Some(((src_u32, dst_u32, *ranking), record))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    pub fn insert(
        &mut self,
        prop_name: &str,
        prop_value: &Value,
        src: u32,
        dst: u32,
        rank: i64,
        edge_type: u32,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let key = Self::encode_edge_property_key(prop_value, src, dst, rank)?;
        let entity_ref = EntityRef::Edge {
            src: crate::core::types::VertexId::from_int64(src as i64),
            dst: crate::core::types::VertexId::from_int64(dst as i64),
            edge_type,
            ranking: rank,
        };
        let record = IndexRecord::new(ts).with_entity_ref(entity_ref);
        let pool_capacity = self.pool_capacity;
        let index = self.get_or_create_index(prop_name);
        let mut map = index.snapshot();
        map.insert(key, record);
        let prefix = prop_name.as_bytes().to_vec();
        *index = ChunkedIndex::from_btree(prefix, &map, pool_capacity);
        Ok(())
    }

    pub fn delete(
        &mut self,
        prop_name: &str,
        prop_value: &Value,
        src: u32,
        dst: u32,
        rank: i64,
        deleted_ts: Timestamp,
    ) -> StorageResult<()> {
        let key = Self::encode_edge_property_key(prop_value, src, dst, rank)?;
        let pool_capacity = self.pool_capacity;
        let has_index = self.indexes.contains_key(prop_name);
        if !has_index {
            return Ok(());
        }
        let prefix = prop_name.as_bytes().to_vec();
        let mut map = {
            let index = self.indexes.get(prop_name).unwrap();
            index.snapshot()
        };
        if let Some(record) = map.get_mut(&key) {
            record.mark_deleted(deleted_ts);
        }
        let new_index = ChunkedIndex::from_btree(prefix, &map, pool_capacity);
        self.indexes.insert(prop_name.to_string(), new_index);
        let edge_id = ((src as u64) << 32) | (dst as u64);
        self.deleted_filter.insert(edge_id);
        Ok(())
    }

    pub fn might_be_deleted(&self, src: u32, dst: u32) -> bool {
        let edge_id = ((src as u64) << 32) | (dst as u64);
        self.deleted_filter.might_contain(edge_id)
    }

    pub fn has_index(&self, prop_name: &str) -> bool {
        self.indexes.contains_key(prop_name)
    }

    pub fn index_names(&self) -> Vec<String> {
        self.indexes.keys().cloned().collect()
    }

    pub fn memory_usage(&self) -> u64 {
        let index_mem: u64 = self
            .indexes
            .values()
            .map(|idx| idx.memory_usage())
            .sum();
        index_mem + self.deleted_filter.memory_bytes() as u64
    }
}

fn u32_from_vertex_id(v: &crate::core::types::VertexId) -> Option<u32> {
    let bytes = v.as_bytes();
    if bytes.len() == 8 {
        let val = i64::from_be_bytes(bytes.try_into().ok()?);
        Some(val as u32)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::MAX_TIMESTAMP;

    #[test]
    fn test_edge_property_index_insert_lookup() {
        let mut index = EdgePropertyIndex::new(u64::MAX);
        let codec = OrderedCodec::new();

        // Insert two edges with different weights
        index
            .insert("weight", &Value::BigInt(100), 0, 1, 0, 0, 100)
            .unwrap();
        index
            .insert("weight", &Value::BigInt(50), 0, 2, 0, 0, 200)
            .unwrap();

        // Lookup all entries with weight >= 0
        let lower = codec.encode(&Value::BigInt(0)).unwrap();
        let upper = Vec::new(); // unbounded
        let results = index.lookup("weight", &lower, &upper);
        assert_eq!(results.len(), 2);

        // Lookup weight > 80
        let lower = codec.encode(&Value::BigInt(81)).unwrap();
        let results = index.lookup("weight", &lower, &upper);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, (0, 1, 0));
    }

    #[test]
    fn test_edge_property_index_delete() {
        let mut index = EdgePropertyIndex::new(u64::MAX);
        let codec = OrderedCodec::new();

        index
            .insert("weight", &Value::BigInt(100), 0, 1, 0, 0, 100)
            .unwrap();
        index
            .delete("weight", &Value::BigInt(100), 0, 1, 0, 150)
            .unwrap();

        let lower = codec.encode(&Value::BigInt(0)).unwrap();
        let upper = Vec::new();
        let results = index.lookup("weight", &lower, &upper);
        // Entry should still exist in index (marked deleted, not removed)
        assert_eq!(results.len(), 1);
        assert!(results[0].1.deleted_ts.is_some());

        // Bloom filter should indicate possible deletion
        assert!(index.might_be_deleted(0, 1));
    }

    #[test]
    fn test_edge_property_index_nonexistent_property() {
        let index = EdgePropertyIndex::new(u64::MAX);
        let results = index.lookup("nonexistent", &[], &[]);
        assert!(results.is_empty());
    }
}


