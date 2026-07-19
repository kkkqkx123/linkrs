use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::core::types::{EdgeId, LabelId, Timestamp, VertexId};
use crate::core::vertex_edge_path::Tag;
use crate::core::{Edge, StorageError, StorageResult, Value, Vertex};
use crate::storage::cursor::{EdgeCursor, ScanOptions, VertexCursor};
use crate::storage::edge::edge_table::core::TimeTravelEdgeStore;
use crate::storage::edge::{EdgeStore, Nbr};
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
    /// Index into `tags.labels` indicating which table is currently being scanned.
    current_table_idx: usize,
    /// Current internal ID within the current table.
    current_internal_id: u32,
    limit: Option<usize>,
    offset_remaining: usize,
    emitted: usize,
    id_range: Option<Range<i64>>,
    projection: Option<Vec<String>>,
    exhausted: bool,
    /// Read timestamp captured when the cursor is opened.
    ts: Timestamp,
    /// Per-table max internal IDs, parallel to `tags.labels`.
    table_max_ids: Vec<u32>,
}

impl std::fmt::Debug for GraphVertexCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphVertexCursor")
            .field("space", &self.space)
            .field("tags", &self.tags.labels.len())
            .field("current_internal_id", &self.current_internal_id)
            .field("current_table_idx", &self.current_table_idx)
            .field("limit", &self.limit)
            .field("offset_remaining", &self.offset_remaining)
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
        let ts = options
            .read_timestamp
            .unwrap_or_else(|| ctx.get_read_timestamp());
        if options
            .vertex_id_range
            .as_ref()
            .is_some_and(|range| range.start < 0 || range.end < 0 || range.end > u32::MAX as i64)
        {
            return Err(StorageError::invalid_operation(
                "vertex_id_range must fit non-negative u32 internal IDs",
            ));
        }
        let tag_infos = ctx.schema_manager().list_tags(&space)?;
        let tags = TagCache {
            labels: tag_infos.iter().map(|t| t.tag_id).collect(),
            names: tag_infos
                .into_iter()
                .map(|t| (t.tag_id, t.tag_name))
                .collect(),
        };

        let (exhausted, table_max_ids) = ctx.data_store().with_vertex_tables(|tables| {
            let max_ids: Vec<u32> = tags
                .labels
                .iter()
                .map(|label_id| {
                    tables
                        .get(label_id)
                        .map(|t| t.total_count() as u32)
                        .unwrap_or(0)
                })
                .collect();
            let done = max_ids.iter().all(|&count| count == 0);
            (done, max_ids)
        });

        Ok(Self {
            ctx,
            space,
            tags,
            current_table_idx: 0,
            current_internal_id: 0,
            limit: options.limit,
            offset_remaining: options.offset,
            emitted: 0,
            id_range: options.vertex_id_range.clone(),
            projection: options.projection.clone(),
            exhausted,
            ts,
            table_max_ids,
        })
    }
}

impl VertexCursor for GraphVertexCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Vertex>, StorageError> {
        if self.exhausted || self.tags.labels.is_empty() {
            return Ok(Vec::new());
        }

        let batch_size = batch_size.max(1);
        let ts = self.ts;
        let data_store = self.ctx.data_store().clone();
        let tags = &self.tags;
        let id_range = &self.id_range;
        let projection = &self.projection;
        let batch = data_store.with_vertex_tables(|tables| {
            let mut batch = Vec::new();

            while batch.len() < batch_size && !self.exhausted {
                // Advance past exhausted tables.
                while self.current_table_idx < tags.labels.len()
                    && self.current_internal_id >= self.table_max_ids[self.current_table_idx]
                {
                    self.current_table_idx += 1;
                    self.current_internal_id = 0;
                }

                if self.current_table_idx >= tags.labels.len() {
                    self.exhausted = true;
                    break;
                }

                let internal_id = self.current_internal_id;
                self.current_internal_id += 1;

                if let Some(range) = id_range {
                    if !((range.start as u32)..(range.end as u32)).contains(&internal_id) {
                        continue;
                    }
                }

                let label_id = tags.labels[self.current_table_idx];
                if let Some(table) = tables.get(&label_id) {
                    if let Some(record) = table.get_by_internal_id(internal_id, ts) {
                        let vid = record.vid;
                        let tag_name = tags
                            .names
                            .get(&label_id)
                            .map(|s| s.as_str())
                            .unwrap_or("unknown");
                        let props: HashMap<String, Value> = record
                            .properties
                            .iter()
                            .filter(|(name, _)| {
                                projection
                                    .as_ref()
                                    .is_none_or(|projection| projection.iter().any(|p| name == p))
                            })
                            .cloned()
                            .collect();
                        let vertex_tag = Tag::new(tag_name.to_string(), props.clone());

                        if self.offset_remaining > 0 {
                            self.offset_remaining -= 1;
                            continue;
                        }

                        batch.push(Vertex {
                            vid,
                            id: internal_id as i64,
                            tags: vec![vertex_tag],
                            properties: props,
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
            }
            batch
        });

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
    offset_remaining: usize,
    emitted: usize,
    src_id_range: Option<Range<i64>>,
    projection: Option<Vec<String>>,
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
            .field("offset_remaining", &self.offset_remaining)
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
        let ts = options
            .read_timestamp
            .unwrap_or_else(|| ctx.get_read_timestamp());
        let targets = if let Some(ref et) = options.edge_type {
            vec![build_target(&ctx, space, et)?]
        } else {
            let edge_types = ctx.schema_manager().list_edge_types(space)?;
            edge_types
                .into_iter()
                .map(|et| build_target(&ctx, space, &et.edge_type_name))
                .collect::<StorageResult<Vec<_>>>()?
        };

        Ok(Self {
            ctx,
            limit: options.limit,
            offset_remaining: options.offset,
            emitted: 0,
            src_id_range: options.edge_src_id_range.clone(),
            projection: options.projection.clone(),
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

        let batch_size = batch_size.max(1);
        let mut candidates = Vec::new();

        let ctx = &*self.ctx;
        let ts = self.ts;
        let limit = self.limit;
        let src_id_range = &self.src_id_range;
        let projection = &self.projection;
        let targets = &self.targets;
        let target_idx = &mut self.target_idx;
        let table_idx = &mut self.table_idx;
        let table_state = &mut self.table_state;
        let emitted = &mut self.emitted;
        let offset_remaining = &mut self.offset_remaining;
        let exhausted = &mut self.exhausted;

        let data_store = ctx.data_store().clone();
        data_store.with_edge_tables(|edge_tables| {
            'outer: while candidates.len() < batch_size {
                if *target_idx >= targets.len() {
                    *exhausted = true;
                    break 'outer;
                }

                let target = &targets[*target_idx];

                if *table_idx >= target.tables.len() {
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
                        scan_mutable(ScanArgs {
                            store,
                            target,
                            td,
                            ts,
                            src_id_range,
                            projection,
                            limit,
                            emitted,
                            offset_remaining,
                            state: table_state,
                            batch: &mut candidates,
                            batch_size,
                        });
                    }
                    TablePhase::Segment(seg_idx) => {
                        scan_segments(
                            ScanArgs {
                                store,
                                target,
                                td,
                                ts,
                                src_id_range,
                                projection,
                                limit,
                                emitted,
                                offset_remaining,
                                state: table_state,
                                batch: &mut candidates,
                                batch_size,
                            },
                            seg_idx,
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
        });

        Ok(candidates
            .into_iter()
            .map(|candidate| materialize_edge(ctx, candidate, ts))
            .collect())
    }
}

struct ScanArgs<'a> {
    store: &'a TimeTravelEdgeStore,
    target: &'a TargetDef,
    td: &'a TableDef,
    ts: Timestamp,
    src_id_range: &'a Option<Range<i64>>,
    projection: &'a Option<Vec<String>>,
    limit: Option<usize>,
    emitted: &'a mut usize,
    offset_remaining: &'a mut usize,
    state: &'a mut TableScanState,
    batch: &'a mut Vec<EdgeCandidate>,
    batch_size: usize,
}

// ---------------------------------------------------------------------------
// Free-function scan helpers
// ---------------------------------------------------------------------------

fn scan_mutable(args: ScanArgs) {
    let mut iter = args.store.out_csr.iter(args.ts);

    let mut remaining = args.state.mutable_consumed;
    while remaining > 0 {
        match iter.next() {
            Some(_) => remaining -= 1,
            None => {
                args.state.phase = TablePhase::Segment(0);
                return;
            }
        }
    }

    for (src_vid, nbr) in iter {
        args.state.mutable_consumed += 1;
        if args.store.mvcc.is_tombstoned(nbr.edge_id, args.ts) {
            continue;
        }
        if !args.state.seen.insert(nbr.edge_id) {
            continue;
        }
        if let Some(ref r) = *args.src_id_range {
            let src_int = src_vid.as_int64().unwrap_or(0);
            if src_int < r.start || src_int >= r.end {
                continue;
            }
        }

        if *args.offset_remaining > 0 {
            *args.offset_remaining -= 1;
            continue;
        }

        let edge = build_edge_candidate(EdgeBuildArgs {
            store: args.store,
            target: args.target,
            td: args.td,
            src_vid: &src_vid,
            nbr,
            projection: args.projection,
        });
        args.batch.push(edge);
        *args.emitted += 1;

        if args.batch.len() >= args.batch_size {
            return;
        }
        if args.limit.is_some_and(|l| *args.emitted >= l) {
            return;
        }
    }

    args.state.phase = TablePhase::Segment(0);
}

fn scan_segments(args: ScanArgs, seg_idx: usize) {
    let seg_count = args.store.out_segments.len();
    if seg_idx >= seg_count {
        args.state.phase = TablePhase::Done;
        return;
    }

    let segment = &args.store.out_segments[seg_count - 1 - seg_idx];
    let csr = segment.csr.read();
    let edges: Vec<_> = csr.iter().collect();
    let mut iter = edges.iter();

    let mut skip = args.state.seg_raw_consumed;
    while skip > 0 {
        if iter.next().is_none() {
            args.state.phase = TablePhase::Done;
            return;
        }
        skip -= 1;
    }

    for (src_vid, edge) in &mut iter {
        args.state.seg_raw_consumed += 1;

        if edge.timestamp > args.ts {
            continue;
        }
        if args.store.mvcc.is_tombstoned(edge.edge_id, args.ts) {
            continue;
        }
        if !args.state.seen.insert(edge.edge_id) {
            continue;
        }
        if let Some(ref r) = *args.src_id_range {
            let src_int = src_vid.as_int64().unwrap_or(0);
            if src_int < r.start || src_int >= r.end {
                continue;
            }
        }

        let nbr = Nbr::new(
            edge.neighbor,
            edge.edge_id,
            edge.prop_offset,
            edge.timestamp,
        );
        let edge = build_edge_candidate(EdgeBuildArgs {
            store: args.store,
            target: args.target,
            td: args.td,
            src_vid: &src_vid,
            nbr,
            projection: args.projection,
        });

        if *args.offset_remaining > 0 {
            *args.offset_remaining -= 1;
            continue;
        }

        args.batch.push(edge);
        *args.emitted += 1;

        if args.batch.len() >= args.batch_size {
            return;
        }
        if args.limit.is_some_and(|l| *args.emitted >= l) {
            return;
        }
    }

    let next = seg_idx + 1;
    if next >= seg_count {
        args.state.phase = TablePhase::Done;
    } else {
        args.state.phase = TablePhase::Segment(next);
        args.state.seg_raw_consumed = 0;
    }
}

// ---------------------------------------------------------------------------
// Edge construction
// ---------------------------------------------------------------------------

struct EdgeBuildArgs<'a> {
    store: &'a TimeTravelEdgeStore,
    target: &'a TargetDef,
    td: &'a TableDef,
    src_vid: &'a VertexId,
    nbr: Nbr,
    projection: &'a Option<Vec<String>>,
}

struct EdgeCandidate {
    edge_type_name: String,
    src_label: LabelId,
    dst_label: LabelId,
    src_vid: VertexId,
    dst_vid: VertexId,
    rank: i64,
    props: HashMap<String, Value>,
}

fn build_edge_candidate(args: EdgeBuildArgs<'_>) -> EdgeCandidate {
    let src_internal = args.src_vid.as_int64().unwrap_or(0) as u32;
    let (dst_vid, rank) = decode_endpoint(args.nbr.neighbor);
    let properties = properties_for(args.store, args.nbr.prop_offset, args.projection);

    let src_vid = VertexId::from_int64(src_internal as i64);
    let props: HashMap<String, Value> = properties.into_iter().collect();
    EdgeCandidate {
        edge_type_name: args.target.edge_type_name.clone(),
        src_label: args.td.tbl_src,
        dst_label: args.td.tbl_dst,
        src_vid,
        dst_vid,
        rank,
        props,
    }
}

fn materialize_edge(ctx: &GraphStorageContext, candidate: EdgeCandidate, ts: Timestamp) -> Edge {
    let src_internal = candidate.src_vid.as_int64().unwrap_or(0) as u32;
    let dst_internal = candidate.dst_vid.as_int64().unwrap_or(0) as u32;
    let src_external = resolve_vertex_id(
        ctx,
        src_internal,
        candidate.src_label,
        &candidate.src_vid,
        ts,
    );
    let dst_external = resolve_vertex_id(
        ctx,
        dst_internal,
        candidate.dst_label,
        &candidate.dst_vid,
        ts,
    );
    Edge {
        src: make_vid(&src_external),
        dst: make_vid(&dst_external),
        edge_type: candidate.edge_type_name,
        ranking: candidate.rank,
        props: candidate.props,
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

fn properties_for(
    store: &TimeTravelEdgeStore,
    prop_offset: u32,
    projection: &Option<Vec<String>>,
) -> Vec<(String, Value)> {
    if prop_offset == 0 {
        return Vec::new();
    }
    store
        .properties
        .get(prop_offset, None)
        .map(|props| {
            props
                .into_iter()
                .filter_map(|(k, v)| {
                    v.filter(|_| {
                        projection
                            .as_ref()
                            .is_none_or(|names| names.iter().any(|name| name == &k))
                    })
                    .map(|v| (k, v))
                })
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
        ctx.data_store().with_edge_tables(|edge_tables| {
            edge_tables
                .iter()
                .filter(|(_, store)| store.label() == edge_label_id)
                .map(|(key, store)| TableDef {
                    key: *key,
                    tbl_src: store.src_label(),
                    tbl_dst: store.dst_label(),
                })
                .collect()
        })
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
