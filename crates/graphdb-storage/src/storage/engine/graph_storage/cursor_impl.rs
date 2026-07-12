use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::core::types::{EdgeId, LabelId, Timestamp, VertexId};
use crate::core::vertex_edge_path::Tag;
use crate::core::{Edge, StorageError, StorageResult, Value, Vertex};
use crate::storage::cursor::{EdgeCursor, ScanOptions, VertexCursor};
use crate::storage::edge::edge_table::core::TimeTravelEdgeStore;
use crate::storage::edge::{EdgeRecord, EdgeStore, Nbr};
use crate::storage::engine::data_store::EdgeTableKey;

use super::context::GraphStorageContext;
use super::ops::endpoint_label_id;

// ---------------------------------------------------------------------------
// GraphVertexCursor (unchanged)
// ---------------------------------------------------------------------------

struct TagCache {
    labels: Vec<LabelId>,
    names: HashMap<LabelId, String>,
}

pub(crate) struct GraphVertexCursor {
    ctx: Arc<GraphStorageContext>,
    space: String,
    tags: TagCache,
    current_internal_id: u32,
    max_internal_id: u32,
    limit: Option<usize>,
    emitted: usize,
    id_range: Option<Range<i64>>,
    exhausted: bool,
}

impl std::fmt::Debug for GraphVertexCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphVertexCursor")
            .field("space", &self.space)
            .field("tags", &self.tags.labels.len())
            .field("current_internal_id", &self.current_internal_id)
            .field("max_internal_id", &self.max_internal_id)
            .field("limit", &self.limit)
            .field("exhausted", &self.exhausted)
            .finish()
    }
}

impl GraphVertexCursor {
    pub fn new(
        ctx: Arc<GraphStorageContext>,
        space: String,
        options: &ScanOptions,
    ) -> StorageResult<Self> {
        let tag_infos = ctx.schema_manager().list_tags(&space)?;
        let tags = TagCache {
            labels: tag_infos.iter().map(|t| t.tag_id).collect(),
            names: tag_infos.into_iter().map(|t| (t.tag_id, t.tag_name)).collect(),
        };

        let max_internal_id = {
            let tables = ctx.data_store().vertex_tables().read();
            tags.labels
                .iter()
                .filter_map(|label_id| tables.get(label_id))
                .map(|t| t.total_count() as u32)
                .max()
                .unwrap_or(0)
        };

        Ok(Self {
            ctx,
            space,
            tags,
            current_internal_id: 0,
            max_internal_id,
            limit: options.limit,
            emitted: 0,
            id_range: options.vertex_id_range.clone(),
            exhausted: max_internal_id == 0,
        })
    }
}

impl VertexCursor for GraphVertexCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Vertex>, StorageError> {
        if self.exhausted || self.tags.labels.is_empty() {
            return Ok(Vec::new());
        }

        let ts = self.ctx.get_read_timestamp();
        let tables = self.ctx.data_store().vertex_tables().read();
        let mut batch = Vec::new();

        while batch.len() < batch_size && self.current_internal_id < self.max_internal_id {
            let internal_id = self.current_internal_id;
            self.current_internal_id += 1;

            if let Some(ref range) = self.id_range {
                if !((range.start as u32)..(range.end as u32)).contains(&internal_id) {
                    continue;
                }
            }

            let mut merged_vid = None;
            let mut merged_tags: Vec<Tag> = Vec::new();
            let mut all_properties: HashMap<String, Value> = HashMap::new();

            for label_id in &self.tags.labels {
                if let Some(table) = tables.get(label_id) {
                    if let Some(record) = table.get_by_internal_id(internal_id, ts) {
                        if merged_vid.is_none() {
                            merged_vid = Some(record.vid);
                        }
                        let tag_name = self
                            .tags
                            .names
                            .get(label_id)
                            .map(|s| s.as_str())
                            .unwrap_or("unknown");
                        let props: HashMap<String, Value> =
                            record.properties.iter().cloned().collect();
                        merged_tags.push(Tag::new(tag_name.to_string(), props.clone()));
                        all_properties.extend(props);
                    }
                }
            }

            if let Some(vid) = merged_vid {
                batch.push(Vertex {
                    vid,
                    id: internal_id as i64,
                    tags: merged_tags,
                    properties: all_properties,
                });
                self.emitted += 1;
                if let Some(limit) = self.limit {
                    if self.emitted >= limit {
                        self.exhausted = true;
                        break;
                    }
                }
            }
        }

        if self.current_internal_id >= self.max_internal_id {
            self.exhausted = true;
        }

        Ok(batch)
    }
}

// ---------------------------------------------------------------------------
// GraphEdgeCursor — truly lazy CSR-scanning edge cursor
// ---------------------------------------------------------------------------

struct TableDef {
    key: EdgeTableKey,
    tbl_src: LabelId,
    tbl_dst: LabelId,
}

struct TargetDef {
    edge_type_name: String,
    tables: Vec<TableDef>,
}

#[derive(Clone, Debug)]
enum TablePhase {
    Mutable,
    Segment(usize),
    Done,
}

#[derive(Clone, Debug)]
struct TableScanState {
    phase: TablePhase,
    /// Number of valid (non-tombstoned) edges already consumed from
    /// the mutable CSR.
    mutable_consumed: usize,
    /// Number of raw edges (valid + tombstoned) already consumed from
    /// the current segment's CsrIterator.
    seg_raw_consumed: usize,
    /// Edge IDs already emitted from this table (dedup across phases).
    seen: HashSet<EdgeId>,
}

impl TableScanState {
    fn new() -> Self {
        Self {
            phase: TablePhase::Mutable,
            mutable_consumed: 0,
            seg_raw_consumed: 0,
            seen: HashSet::new(),
        }
    }
}

pub(crate) struct GraphEdgeCursor {
    ctx: Arc<GraphStorageContext>,
    limit: Option<usize>,
    emitted: usize,
    src_id_range: Option<Range<i64>>,
    exhausted: bool,
    ts: Timestamp,
    targets: Vec<TargetDef>,
    target_idx: usize,
    table_idx: usize,
    table_state: TableScanState,
}

impl std::fmt::Debug for GraphEdgeCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphEdgeCursor")
            .field("target_idx", &self.target_idx)
            .field("table_idx", &self.table_idx)
            .field("phase", &self.table_state.phase)
            .field("limit", &self.limit)
            .field("emitted", &self.emitted)
            .field("exhausted", &self.exhausted)
            .finish()
    }
}

impl GraphEdgeCursor {
    pub fn new(
        ctx: Arc<GraphStorageContext>,
        space: &str,
        options: &ScanOptions,
    ) -> StorageResult<Self> {
        let ts = ctx.get_read_timestamp();
        let targets = if let Some(ref et) = options.edge_type {
            vec![build_target(&ctx, space, et)?]
        } else {
            let edge_types = ctx.schema_manager().list_edge_types(space)?;
            edge_types
                .into_iter()
                .filter_map(|et| build_target(&ctx, space, &et.edge_type_name).ok())
                .collect()
        };

        Ok(Self {
            ctx,
            limit: options.limit,
            emitted: 0,
            src_id_range: options.edge_src_id_range.clone(),
            exhausted: targets.is_empty(),
            ts,
            targets,
            target_idx: 0,
            table_idx: 0,
            table_state: TableScanState::new(),
        })
    }
}

impl EdgeCursor for GraphEdgeCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Edge>, StorageError> {
        if self.exhausted {
            return Ok(Vec::new());
        }

        let mut batch = Vec::new();

        let ctx = &*self.ctx;
        let ts = self.ts;
        let limit = self.limit;
        let src_id_range = &self.src_id_range;
        let targets = &self.targets;
        let target_idx = &mut self.target_idx;
        let table_idx = &mut self.table_idx;
        let table_state = &mut self.table_state;
        let emitted = &mut self.emitted;
        let exhausted = &mut self.exhausted;

        let edge_tables = ctx.data_store().edge_tables().read();

        'outer: while batch.len() < batch_size {
            while *target_idx >= targets.len() {
                *exhausted = true;
                break 'outer;
            }

            let target = &targets[*target_idx];

            while *table_idx >= target.tables.len() {
                *target_idx += 1;
                *table_idx = 0;
                *table_state = TableScanState::new();
                continue 'outer;
            }

            let td = &target.tables[*table_idx];
            let store = match edge_tables.get(&td.key) {
                Some(EdgeStore::TimeTravel(s)) => s,
                _ => {
                    *table_idx += 1;
                    *table_state = TableScanState::new();
                    continue;
                }
            };

            match table_state.phase {
                TablePhase::Mutable => {
                    scan_mutable(
                        ctx, store, target, td, ts, src_id_range, limit,
                        emitted, table_state, &mut batch, batch_size,
                    );
                }
                TablePhase::Segment(seg_idx) => {
                    scan_segments(
                        ctx, store, target, td, seg_idx, ts, src_id_range, limit,
                        emitted, table_state, &mut batch, batch_size,
                    );
                }
                TablePhase::Done => {
                    *table_idx += 1;
                    *table_state = TableScanState::new();
                    continue;
                }
            }

            if limit.is_some_and(|l| *emitted >= l) {
                *exhausted = true;
            }
        }

        Ok(batch)
    }
}

// ---------------------------------------------------------------------------
// Free-function scan helpers
// ---------------------------------------------------------------------------

fn scan_mutable(
    ctx: &GraphStorageContext,
    store: &TimeTravelEdgeStore,
    target: &TargetDef,
    td: &TableDef,
    ts: Timestamp,
    src_id_range: &Option<Range<i64>>,
    limit: Option<usize>,
    emitted: &mut usize,
    state: &mut TableScanState,
    batch: &mut Vec<Edge>,
    batch_size: usize,
) {
    let mut iter = store.out_csr.iter(ts);

    let mut remaining = state.mutable_consumed;
    while remaining > 0 {
        match iter.next() {
            Some(_) => remaining -= 1,
            None => {
                state.phase = TablePhase::Segment(0);
                return;
            }
        }
    }

    for (src_vid, nbr) in iter {
        if store.mvcc.is_tombstoned(nbr.edge_id, ts) {
            continue;
        }
        if !state.seen.insert(nbr.edge_id) {
            continue;
        }
        if let Some(ref r) = *src_id_range {
            let src_int = src_vid.as_int64().unwrap_or(0);
            if src_int < r.start || src_int >= r.end {
                continue;
            }
        }

        let edge = build_edge(ctx, store, target, td, &src_vid, nbr, ts);
        batch.push(edge);
        *emitted += 1;
        state.mutable_consumed += 1;

        if batch.len() >= batch_size {
            return;
        }
        if limit.is_some_and(|l| *emitted >= l) {
            return;
        }
    }

    state.phase = TablePhase::Segment(0);
}

fn scan_segments(
    ctx: &GraphStorageContext,
    store: &TimeTravelEdgeStore,
    target: &TargetDef,
    td: &TableDef,
    seg_idx: usize,
    ts: Timestamp,
    src_id_range: &Option<Range<i64>>,
    limit: Option<usize>,
    emitted: &mut usize,
    state: &mut TableScanState,
    batch: &mut Vec<Edge>,
    batch_size: usize,
) {
    let seg_count = store.out_segments.len();
    if seg_idx >= seg_count {
        state.phase = TablePhase::Done;
        return;
    }

    let segment = &store.out_segments[seg_count - 1 - seg_idx];
    let mut iter = segment.csr.iter();

    let mut skip = state.seg_raw_consumed;
    while skip > 0 {
        if iter.next().is_none() {
            state.phase = TablePhase::Done;
            return;
        }
        skip -= 1;
    }

    for (src_vid, edge) in &mut iter {
        state.seg_raw_consumed += 1;

        if edge.timestamp > ts {
            continue;
        }
        if store.mvcc.is_tombstoned(edge.edge_id, ts) {
            continue;
        }
        if !state.seen.insert(edge.edge_id) {
            continue;
        }
        if let Some(ref r) = *src_id_range {
            let src_int = src_vid.as_int64().unwrap_or(0);
            if src_int < r.start || src_int >= r.end {
                continue;
            }
        }

        let nbr = Nbr::new(edge.neighbor, edge.edge_id, edge.prop_offset, edge.timestamp);
        let edge = build_edge(ctx, store, target, td, &src_vid, nbr, ts);
        batch.push(edge);
        *emitted += 1;

        if batch.len() >= batch_size {
            return;
        }
        if limit.is_some_and(|l| *emitted >= l) {
            return;
        }
    }

    let next = seg_idx + 1;
    if next >= seg_count {
        state.phase = TablePhase::Done;
    } else {
        state.phase = TablePhase::Segment(next);
        state.seg_raw_consumed = 0;
    }
}

// ---------------------------------------------------------------------------
// Edge construction
// ---------------------------------------------------------------------------

fn build_edge(
    ctx: &GraphStorageContext,
    store: &TimeTravelEdgeStore,
    target: &TargetDef,
    td: &TableDef,
    src_vid: &VertexId,
    nbr: Nbr,
    ts: Timestamp,
) -> Edge {
    let src_internal = src_vid.as_int64().unwrap_or(0) as u32;
    let (dst_vid, rank) = decode_endpoint(nbr.neighbor);
    let properties = properties_for(store, nbr.prop_offset);

    let record = EdgeRecord {
        src_vid: VertexId::from_int64(src_internal as i64),
        dst_vid,
        rank,
        properties,
    };

    let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
    let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;

    let src_external = resolve_vertex_id(ctx, src_internal, td.tbl_src, &record.src_vid, ts);
    let dst_external = resolve_vertex_id(ctx, dst_internal, td.tbl_dst, &record.dst_vid, ts);

    let props: HashMap<String, Value> = record.properties.into_iter().collect();
    Edge {
        src: make_vid(&src_external),
        dst: make_vid(&dst_external),
        edge_type: target.edge_type_name.clone(),
        ranking: record.rank,
        props,
    }
}

fn decode_endpoint(key: VertexId) -> (VertexId, i64) {
    let bytes = key.as_bytes();
    if bytes.len() != 16 {
        return (key, 0);
    }
    let mut endpoint_bytes = [0u8; 8];
    endpoint_bytes.copy_from_slice(&bytes[..8]);
    let mut rank_bytes = [0u8; 8];
    rank_bytes.copy_from_slice(&bytes[8..16]);
    (
        VertexId::from_int64(i64::from_be_bytes(endpoint_bytes)),
        i64::from_be_bytes(rank_bytes),
    )
}

fn properties_for(store: &TimeTravelEdgeStore, prop_offset: u32) -> Vec<(String, Value)> {
    if prop_offset == 0 {
        return Vec::new();
    }
    store
        .properties
        .get(prop_offset, None)
        .map(|props| {
            props
                .into_iter()
                .filter_map(|(k, v)| v.map(|v| (k, v)))
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_vertex_id(
    ctx: &GraphStorageContext,
    internal: u32,
    label: LabelId,
    fallback: &VertexId,
    ts: Timestamp,
) -> String {
    if label != 0 {
        ctx.get_external_id(label, internal, ts)
            .or_else(|| {
                ctx.get_external_id_by_internal_id(label, internal)
                    .map(|v| format!("{}", v))
            })
            .unwrap_or_else(|| format!("{}", fallback))
    } else {
        ctx.get_external_id_any(internal, ts)
            .unwrap_or_else(|| format!("{}", fallback))
    }
}

fn make_vid(s: &str) -> VertexId {
    s.parse::<i64>()
        .map(VertexId::from_int64)
        .unwrap_or_else(|_| VertexId::from_string(s))
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

fn build_target(
    ctx: &Arc<GraphStorageContext>,
    space: &str,
    edge_type: &str,
) -> StorageResult<TargetDef> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let edge_label_id = edge_info.edge_type_id;
    let src_label_id = endpoint_label_id(ctx, space, &edge_info.src_tag_name)?.unwrap_or(0);
    let dst_label_id = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)?.unwrap_or(0);

    let tables = if src_label_id == 0 && dst_label_id == 0 {
        let edge_tables = ctx.data_store().edge_tables().read();
        edge_tables
            .iter()
            .filter(|(_, store)| store.label() == edge_label_id)
            .map(|(key, store)| TableDef {
                key: *key,
                tbl_src: store.src_label(),
                tbl_dst: store.dst_label(),
            })
            .collect()
    } else {
        let key = EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
        vec![TableDef {
            key,
            tbl_src: src_label_id,
            tbl_dst: dst_label_id,
        }]
    };

    Ok(TargetDef {
        edge_type_name: edge_type.to_string(),
        tables,
    })
}
