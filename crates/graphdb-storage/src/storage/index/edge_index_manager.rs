use crate::core::types::{Index, Timestamp, MAX_TIMESTAMP};
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::key_builder::normalize_int_value;
use crate::storage::index::key_codec::{EdgeIndexKeyGen, KeyBuilder, KeyParser};
use crate::storage::index::manifest::{ManifestCatalog, ManifestHandle};
use crate::storage::index::types::{EdgeIdentity, IndexRecord, StaleChecker};
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
        edge: &EdgeIdentity<'_>,
        index_name: &str,
        props: &[(String, Value)],
    ) -> Result<(), StorageError> {
        self.update_edge_indexes_mvcc(edge, index_name, props, MAX_TIMESTAMP)
    }

    pub fn update_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        for (_prop_name, prop_value) in props {
            let logical_forward_key = KeyBuilder::build_edge_index_key(
                edge.space_id,
                index_name,
                prop_value,
                edge.src,
                edge.dst,
                edge.edge_type,
                edge.ranking,
            )?;
            let logical_reverse_key = KeyBuilder::build_edge_reverse_key(
                edge.space_id,
                edge.src,
                edge.dst,
                edge.edge_type,
                edge.ranking,
                index_name,
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
            let entity_ref = make_edge_entity_ref(edge.src, edge.dst, edge.edge_type, edge.ranking);
            let entry = if let Some(er) = entity_ref {
                IndexRecord::new(write_ts)
                    .with_entity_version(write_ts)
                    .with_entity_ref(er)
            } else {
                IndexRecord::new(write_ts).with_entity_version(write_ts)
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

    pub fn delete_edge_indexes(
        &self,
        edge: &EdgeIdentity<'_>,
        index_names: &[String],
    ) -> Result<(), StorageError> {
        self.delete_edge_indexes_mvcc(edge, index_names, MAX_TIMESTAMP)
    }

    pub fn delete_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        if index_names.is_empty() {
            return Ok(());
        }

        let reverse_prefix = KeyBuilder::build_edge_reverse_prefix(
            edge.space_id,
            edge.src,
            edge.dst,
            edge.edge_type,
            edge.ranking,
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
                            let forward_key_start = KeyBuilder::build_edge_index_prefix(
                                edge.space_id,
                                &parsed_index_name,
                            );
                            let forward_key_end = KeyBuilder::build_range_end(&forward_key_start);

                            let forward_index = self.base.forward_index().read();
                            for (fwd_compressed_key, fwd_entry) in
                                forward_index.range(forward_key_start.0.clone()..forward_key_end.0)
                            {
                                if fwd_entry.is_visible_at(write_ts) {
                                    if let Ok((fwd_src, fwd_dst, fwd_type, fwd_rank)) =
                                        KeyParser::parse_edge_identity_from_key(fwd_compressed_key)
                                    {
                                        if fwd_src == *edge.src
                                            && fwd_dst == *edge.dst
                                            && fwd_type == edge.edge_type
                                            && fwd_rank == edge.ranking
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
                if normalize_int_value(&stored_value) == normalize_int_value(value) {
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

    pub fn base(&self) -> &GenericIndexManager<EdgeIndexKeyGen> {
        &self.base
    }

    pub fn open_edge_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<EdgeIndexCursor> {
        self.open_edge_index_cursor_full(space_id, index, plan, None, None)
    }

    pub fn open_edge_index_cursor_with_checker(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<StaleChecker>,
    ) -> StorageResult<EdgeIndexCursor> {
        self.open_edge_index_cursor_full(space_id, index, plan, stale_checker, None)
    }

    pub fn open_edge_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<StaleChecker>,
        catalog: Option<&ManifestCatalog>,
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
                let (value_lower, value_upper) = OrderedCodec::new().prefix_bounds(value)?;
                let mut start = KeyBuilder::build_edge_index_prefix(space_id, &index.name).0;
                start.extend_from_slice(&value_lower);
                let mut end = KeyBuilder::build_edge_index_prefix(space_id, &index.name).0;
                end.extend_from_slice(&value_upper);
                (start, end)
            }
            IndexPredicate::All => (
                index_prefix.0.clone(),
                KeyBuilder::build_range_end(&index_prefix).0,
            ),
        };

        let manifest_handle = catalog.map(|catalog| catalog.acquire());
        let ranges = manifest_handle.as_ref().map_or_else(
            || vec![(start.clone(), end.clone())],
            |handle| handle.manifest().scan_ranges(&plan.partition, &start, &end),
        );
        let forward_index = self.base.forward_index_handle();
        let estimated_match_count = {
            let index = forward_index.read();
            ranges
                .iter()
                .map(|(lower, upper)| {
                    index
                        .range(lower.clone()..upper.clone())
                        .filter(|(_key, entry)| entry.is_visible_at(plan.read_timestamp))
                        .count() as u64
                })
                .sum()
        };

        Ok(EdgeIndexCursor {
            forward_index,
            ranges,
            range_index: 0,
            next_key: None,
            exhausted: false,
            offset_remaining: plan.offset,
            limit: plan.limit,
            emitted: 0,
            projection: plan.projection.clone(),
            read_timestamp: plan.read_timestamp,
            invisible_skipped: 0,
            malformed_skipped: 0,
            stale_skipped: 0,
            estimated_match_count,
            manifest_handle,
            stale_checker,
            partition_id_range: plan.partition_id_range.clone(),
        })
    }
}

impl Default for EdgeIndexManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct EdgeIndexCursor {
    forward_index:
        Arc<parking_lot::RwLock<std::collections::BTreeMap<SecondaryIndexKey, IndexRecord>>>,
    ranges: Vec<(Vec<u8>, Vec<u8>)>,
    range_index: usize,
    next_key: Option<SecondaryIndexKey>,
    exhausted: bool,
    offset_remaining: usize,
    limit: Option<usize>,
    emitted: usize,
    projection: Option<Vec<String>>,
    read_timestamp: Timestamp,
    invisible_skipped: u64,
    malformed_skipped: u64,
    stale_skipped: u64,
    estimated_match_count: u64,
    manifest_handle: Option<ManifestHandle>,
    stale_checker: Option<StaleChecker>,
    partition_id_range: Option<std::ops::Range<i64>>,
}

impl std::fmt::Debug for EdgeIndexCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeIndexCursor")
            .field("ranges", &self.ranges)
            .field("range_index", &self.range_index)
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

impl EdgeIndexCursor {
    pub(crate) fn set_manifest_handle(&mut self, manifest_handle: ManifestHandle) {
        self.manifest_handle = Some(manifest_handle);
    }
}

impl IndexCursor for EdgeIndexCursor {
    type Row = IndexRow;

    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Self::Row>, StorageError> {
        if self.exhausted || self.ranges.is_empty() {
            self.exhausted = true;
            return Ok(Vec::new());
        }
        let mut rows = Vec::with_capacity(batch_size.max(1));
        let index = self.forward_index.read();
        let batch_limit = batch_size.max(1);
        while self.range_index < self.ranges.len() && rows.len() < batch_limit {
            let (range_start, range_end) = &self.ranges[self.range_index];
            let scan = if let Some(next_key) = self.next_key.clone() {
                index.range((
                    std::ops::Bound::Excluded(next_key),
                    std::ops::Bound::Excluded(range_end.clone()),
                ))
            } else {
                index.range((
                    std::ops::Bound::Included(range_start.clone()),
                    std::ops::Bound::Excluded(range_end.clone()),
                ))
            };
            let mut paused = false;
            for (key, entry) in scan {
                self.next_key = Some(key.clone());
                if !entry.is_visible_at(self.read_timestamp) {
                    self.invisible_skipped += 1;
                    continue;
                }
                let entity_ref = match &entry.entity_ref {
                    Some(entity_ref) => entity_ref.clone(),
                    None => match parse_edge_entity_ref(key) {
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
                    &entry.included_columns,
                    self.projection.as_deref(),
                ));
                self.emitted += 1;
                if self.limit.is_some_and(|limit| self.emitted >= limit)
                    || rows.len() >= batch_limit
                {
                    paused = true;
                    break;
                }
            }
            if self.limit.is_some_and(|limit| self.emitted >= limit) {
                self.exhausted = true;
                break;
            }
            if paused {
                break;
            }
            self.range_index += 1;
            self.next_key = None;
        }
        self.exhausted |= self.range_index >= self.ranges.len();
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

fn project_edge_row(
    entity_ref: EntityRef,
    included_columns: &[(String, Value)],
    projection: Option<&[String]>,
) -> IndexRow {
    let Some(projection) = projection else {
        return IndexRow::RowId(entity_ref);
    };
    if !projection.is_empty()
        && !projection.iter().all(|name| {
            included_columns
                .iter()
                .any(|(candidate, _)| candidate == name)
        })
    {
        return IndexRow::RowId(entity_ref);
    }
    let columns = projection
        .iter()
        .filter_map(|name| {
            included_columns
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

fn make_edge_entity_ref(
    edge_src: &Value,
    edge_dst: &Value,
    edge_type: &str,
    ranking: i64,
) -> Option<EntityRef> {
    let src_id = value_to_vertex_id(edge_src)?;
    let dst_id = value_to_vertex_id(edge_dst)?;
    let edge_type_id: u32 = edge_type.parse::<u32>().unwrap_or_default();
    Some(EntityRef::Edge {
        src: src_id,
        dst: dst_id,
        edge_type: edge_type_id,
        ranking,
    })
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
    use crate::core::types::{Index, IndexConfig, IndexField, IndexType};
    use crate::core::Value;
    use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};

    use super::{EdgeIdentity, EdgeIndexManager};

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

    fn make_edge_values(
        src_id: i64,
        dst_id: i64,
        _edge_type: &str,
        _ranking: i64,
    ) -> (Value, Value) {
        (Value::BigInt(src_id), Value::BigInt(dst_id))
    }

    #[test]
    fn test_update_and_lookup_edge_index() {
        let manager = EdgeIndexManager::new();

        let space_id = 1u64;
        let (src, dst) = make_edge_values(101, 202, "knows", 1);
        let index_name = "idx_weight";
        let props = vec![("weight".to_string(), Value::Int(42))];

        let edge = EdgeIdentity::new(space_id, &src, &dst, "knows", 1);
        manager
            .update_edge_indexes(&edge, index_name, &props)
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

        let edge = EdgeIdentity::new(space_id, &src, &dst, "knows", 1);
        manager
            .update_edge_indexes(&edge, index_name, &props)
            .expect("Failed to update edge indexes");

        let index = create_test_index(index_name, "knows");
        let results = manager
            .lookup_edge_index(space_id, &index, &Value::Int(42))
            .expect("Failed to lookup");
        assert_eq!(results.len(), 1);

        manager
            .delete_edge_indexes(&edge, &[index_name.to_string()])
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

        let edge1 = EdgeIdentity::new(space_id, &src1, &dst1, "knows", 1);
        manager
            .update_edge_indexes(
                &edge1,
                index_name,
                &[("weight".to_string(), Value::Int(42))],
            )
            .expect("insert edge 1");
        let edge2 = EdgeIdentity::new(space_id, &src2, &dst2, "knows", 2);
        manager
            .update_edge_indexes(
                &edge2,
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
        let src1 = Value::BigInt(1);
        let dst1 = Value::BigInt(2);
        let edge1 = EdgeIdentity::new(1, &src1, &dst1, "knows", 0);

        manager
            .update_edge_indexes_mvcc(
                &edge1,
                "idx_weight",
                &[("weight".to_string(), Value::Int(10))],
                10,
            )
            .expect("edge entry");
        let src2 = Value::BigInt(3);
        let dst2 = Value::BigInt(4);
        let edge2 = EdgeIdentity::new(1, &src2, &dst2, "knows", 1);
        manager
            .update_edge_indexes_mvcc(
                &edge2,
                "idx_weight",
                &[("weight".to_string(), Value::Int(20))],
                20,
            )
            .expect("edge entry");

        let plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: crate::storage::cursor::PartitionSelector::All,
            partition_id_range: None,
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

    #[test]
    fn edge_cursor_covering_and_rowid_paths_produce_consistent_results() {
        let manager = EdgeIndexManager::new();
        let index = create_test_index("idx_weight", "knows");

        let entries = vec![
            (Value::BigInt(1), Value::BigInt(2), 10, 10),
            (Value::BigInt(3), Value::BigInt(4), 20, 20),
            (Value::BigInt(5), Value::BigInt(6), 30, 10),
        ];
        for (src, dst, weight, ts) in &entries {
            let edge = EdgeIdentity::new(1, src, dst, "knows", 0);
            manager
                .update_edge_indexes_mvcc(
                    &edge,
                    "idx_weight",
                    &[("weight".to_string(), Value::Int(*weight))],
                    *ts,
                )
                .expect("edge entry");
        }

        // ---- RowId path ----
        let rowid_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: crate::storage::cursor::PartitionSelector::All,
            partition_id_range: None,
            projection: None,
            limit: None,
            offset: 0,
            read_timestamp: 20,
        };
        let mut rowid_cursor = manager
            .open_edge_index_cursor(1, &index, &rowid_plan)
            .expect("rowid cursor");
        let rowid_rows: Vec<IndexRow> = {
            let mut rows = Vec::new();
            loop {
                let batch = rowid_cursor.next_batch(10).expect("cursor read");
                if batch.is_empty() {
                    break;
                }
                rows.extend(batch);
            }
            rows
        };
        // All 3 entries have created_ts <= 20, all visible
        assert_eq!(rowid_rows.len(), 3);

        // ---- Covering path ----
        let covering_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: crate::storage::cursor::PartitionSelector::All,
            partition_id_range: None,
            projection: Some(vec![]),
            limit: None,
            offset: 0,
            read_timestamp: 20,
        };
        let mut covering_cursor = manager
            .open_edge_index_cursor(1, &index, &covering_plan)
            .expect("covering cursor");
        let covering_rows: Vec<IndexRow> = {
            let mut rows = Vec::new();
            loop {
                let batch = covering_cursor.next_batch(10).expect("cursor read");
                if batch.is_empty() {
                    break;
                }
                rows.extend(batch);
            }
            rows
        };
        assert_eq!(covering_rows.len(), 3);

        // Verify entity_ref consistency
        for (rowid_row, covering_row) in rowid_rows.iter().zip(covering_rows.iter()) {
            let rowid_ref = match rowid_row {
                IndexRow::RowId(ref entity_ref) => entity_ref,
                _ => panic!("expected RowId"),
            };
            let covering_ref = match covering_row {
                IndexRow::Covering {
                    entity_ref,
                    columns: _,
                } => entity_ref,
                _ => panic!("expected Covering"),
            };
            assert_eq!(rowid_ref, covering_ref);
        }

        // ---- offset/limit consistency ----
        let offset_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: crate::storage::cursor::PartitionSelector::All,
            partition_id_range: None,
            projection: None,
            limit: Some(1),
            offset: 1,
            read_timestamp: 20,
        };
        let mut offset_cursor = manager
            .open_edge_index_cursor(1, &index, &offset_plan)
            .expect("offset cursor");
        let offset_rows = offset_cursor.next_batch(10).expect("cursor read");
        assert_eq!(
            offset_rows.len(),
            1,
            "offset=1, limit=1 should return 1 entry"
        );
    }
}
