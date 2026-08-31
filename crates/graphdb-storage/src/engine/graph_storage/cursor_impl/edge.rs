use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::cursor::{EdgeCursor, ScanOptions};
use crate::edge::edge_table::core::TimeTravelEdgeStore;
use crate::edge::Nbr;
use crate::engine::data_store::EdgeTableKey;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::endpoint_label_id;
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, StorageError, StorageResult, Value};

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
    /// Conjunctive predicates evaluated on decoded properties before
    /// offset/limit accounting; a pure pre-filter.
    predicate: Vec<crate::cursor::ScanPredicate>,
    /// Property names referenced by `predicate`; they are decoded even when
    /// absent from the projection so predicates can be evaluated.
    predicate_columns: Vec<String>,
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

        let predicate = options.predicate.clone().unwrap_or_default();
        let mut predicate_columns: Vec<String> = Vec::new();
        for pred in &predicate {
            collect_predicate_columns(pred, &mut predicate_columns);
        }

        Ok(Self {
            ctx,
            limit: options.limit,
            offset_remaining: options.offset,
            emitted: 0,
            src_id_range: options.edge_src_id_range.clone(),
            projection: options
                .projection
                .as_ref()
                .map(|p| p.iter().map(|rp| rp.name.clone()).collect()),
            predicate,
            predicate_columns,
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
        let predicate = &self.predicate;
        let predicate_columns = &self.predicate_columns;
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
                // Lazily register the statement snapshot for this partition.
                ctx.ensure_edge_snapshot_registered(td.key);
                let arc = match edge_tables.get(&td.key) {
                    Some(a) => a.clone(),
                    None => {
                        *table_idx += 1;
                        *table_state = TableScanState::new();
                        continue;
                    }
                };
                let guard = arc.read();
                let store = &guard.0;

                match table_state.phase {
                    TablePhase::Mutable => {
                        scan_mutable(ScanArgs {
                            ctx,
                            store,
                            target,
                            td,
                            ts,
                            src_id_range,
                            projection,
                            predicate,
                            predicate_columns,
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
                                ctx,
                                store,
                                target,
                                td,
                                ts,
                                src_id_range,
                                projection,
                                predicate,
                                predicate_columns,
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
    ctx: &'a GraphStorageContext,
    store: &'a TimeTravelEdgeStore,
    target: &'a TargetDef,
    td: &'a TableDef,
    ts: Timestamp,
    src_id_range: &'a Option<Range<i64>>,
    projection: &'a Option<Vec<String>>,
    predicate: &'a [crate::cursor::ScanPredicate],
    predicate_columns: &'a [String],
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
            let src_internal = src_vid.as_int64().unwrap_or(0) as u32;
            let src_ext =
                resolve_vertex_id(args.ctx, src_internal, args.td.tbl_src, &src_vid, args.ts);
            let src_int = src_ext.parse::<i64>().unwrap_or(i64::MIN);
            if src_int < r.start || src_int >= r.end {
                continue;
            }
        }

        // Decode once with predicate columns included so pushed predicates
        // can be evaluated; matching rows are then trimmed back to the
        // projection. Filtering happens before offset/limit accounting.
        let mut properties = decode_edge_properties(
            args.store,
            nbr.edge_id,
            args.projection,
            args.predicate_columns,
        );
        if !args
            .predicate
            .iter()
            .all(|p| p.matches(properties.as_slice()))
        {
            continue;
        }
        trim_to_projection(&mut properties, args.projection);

        if *args.offset_remaining > 0 {
            *args.offset_remaining -= 1;
            continue;
        }

        let edge = build_edge_candidate(EdgeBuildArgs {
            target: args.target,
            td: args.td,
            src_vid: &src_vid,
            nbr,
            props: properties,
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
            let src_internal = src_vid.as_int64().unwrap_or(0) as u32;
            let src_ext =
                resolve_vertex_id(args.ctx, src_internal, args.td.tbl_src, src_vid, args.ts);
            let src_int = src_ext.parse::<i64>().unwrap_or(i64::MIN);
            if src_int < r.start || src_int >= r.end {
                continue;
            }
        }

        let nbr = Nbr::new(edge.endpoint, edge.rank, edge.edge_id);

        // Same decode-once / pre-filter discipline as the mutable scan.
        let mut properties = decode_edge_properties(
            args.store,
            edge.edge_id,
            args.projection,
            args.predicate_columns,
        );
        if !args
            .predicate
            .iter()
            .all(|p| p.matches(properties.as_slice()))
        {
            continue;
        }
        trim_to_projection(&mut properties, args.projection);

        if *args.offset_remaining > 0 {
            *args.offset_remaining -= 1;
            continue;
        }

        let edge = build_edge_candidate(EdgeBuildArgs {
            target: args.target,
            td: args.td,
            src_vid,
            nbr,
            props: properties,
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
    target: &'a TargetDef,
    td: &'a TableDef,
    src_vid: &'a VertexId,
    nbr: Nbr,
    props: Vec<(String, Value)>,
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
    let rank = args.nbr.rank;
    let dst_vid = VertexId::from_int64(args.nbr.endpoint as i64);

    let src_vid = VertexId::from_int64(src_internal as i64);
    let props: HashMap<String, Value> = args.props.into_iter().collect();
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

/// Decode edge properties keeping projected columns plus any extra columns
/// required by pushed scan predicates.
fn decode_edge_properties(
    store: &TimeTravelEdgeStore,
    edge_id: graphdb_core::types::EdgeId,
    projection: &Option<Vec<String>>,
    predicate_columns: &[String],
) -> Vec<(String, Value)> {
    let props_opt = store.properties.get_by_edge_id(edge_id, u64::MAX);
    props_opt
        .map(|props| {
            props
                .into_iter()
                .filter_map(|(k, v)| {
                    v.filter(|_| {
                        projection.as_ref().is_none_or(|names| {
                            names.iter().any(|name| name == &k)
                                || predicate_columns.iter().any(|name| name == &k)
                        })
                    })
                    .map(|v| (k, v))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Drop predicate-only columns so emitted rows carry projected properties.
fn trim_to_projection(props: &mut Vec<(String, Value)>, projection: &Option<Vec<String>>) {
    if let Some(names) = projection {
        props.retain(|(k, _)| names.iter().any(|name| name == k));
    }
}

/// Collect the property names a scan predicate references.
fn collect_predicate_columns(pred: &crate::cursor::ScanPredicate, out: &mut Vec<String>) {
    use crate::cursor::ScanPredicate as P;
    match pred {
        P::ColumnEqual { column, .. } => {
            if !out.iter().any(|name| name == column) {
                out.push(column.clone());
            }
        }
        P::ColumnRange { column, .. } => {
            if !out.iter().any(|name| name == column) {
                out.push(column.clone());
            }
        }
    }
}

pub(crate) fn resolve_vertex_id(
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

pub(crate) fn make_vid(s: &str) -> VertexId {
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
                .filter(|(_, arc)| arc.read().label() == edge_label_id)
                .map(|(key, arc)| {
                    let store = arc.read();
                    TableDef {
                        key: *key,
                        tbl_src: store.src_label(),
                        tbl_dst: store.dst_label(),
                    }
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
