use crate::core::types::{Index, Timestamp};
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::cursor::{IndexCursor, IndexPredicate, IndexRow, IndexScanPlan};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::{EdgeIndexKeyGen, KeyBuilder, KeyParser};
use crate::storage::index::manifest::{ManifestCatalog, ManifestHandle};
use crate::storage::index::types::{IndexRecord, StaleChecker};
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

    pub fn base(&self) -> &GenericIndexManager<EdgeIndexKeyGen> {
        &self.base
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
                Value::string(""),
                false,
            )],
            properties: vec![],
            index_type: IndexType::EdgeIndex,
            is_unique: false,
            partial_condition: None,
        })
    }

}
