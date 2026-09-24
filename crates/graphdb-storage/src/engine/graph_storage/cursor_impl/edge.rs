use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use crate::cursor::{EdgeColumnBatch, EdgeCursor, ScanOptions};
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::Nbr;
use crate::engine::data_store::EdgeTableKey;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::endpoint_label_id;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, StorageError, StorageResult, Value};

// ---------------------------------------------------------------------------
// GraphEdgeCursor — truly lazy CSR-scanning edge cursor
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TableDef {
    key: EdgeTableKey,
    tbl_src: LabelId,
    tbl_dst: LabelId,
}

#[derive(Clone)]
struct TargetDef {
    edge_type_name: String,
    tables: Vec<TableDef>,
}

#[derive(Clone, Debug)]
enum TablePhase {
    Mutable,
    Done,
}

#[derive(Clone, Debug)]
struct TableScanState {
    phase: TablePhase,
    /// Resumable position: group to resume from plus physical entries to skip
    /// within that group. Whole groups before `resume_group` are never
    /// revisited, so multi-batch scans stay linear instead of replaying from
    /// the table start on every batch.
    resume_group: usize,
    skip_in_group: usize,
}

impl TableScanState {
    fn new() -> Self {
        Self {
            phase: TablePhase::Mutable,
            resume_group: 0,
            skip_in_group: 0,
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
    /// Malformed/unparseable entries skipped so far, exposed through
    /// `EdgeCursor::malformed_skipped` for diagnostics.
    malformed_skipped: u64,
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
            .field("malformed_skipped", &self.malformed_skipped)
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
            malformed_skipped: 0,
            ts,
            targets,
            target_idx: 0,
            table_idx: 0,
            table_state: TableScanState::new(),
        })
    }

    /// Native columnar batch collection: topology plus typed property
    /// columns decoded straight from the stores.
    ///
    /// Reuses the row-path visibility gate and segment prune, skips decoded
    /// groups without live edges, and degrades per column to `General` on
    /// type mismatch instead of falling back to row transpose.
    fn collect_column_batch(
        &mut self,
        prop_names: &[String],
        batch_size: usize,
    ) -> Result<EdgeColumnBatch, StorageError> {
        use crate::cursor::ColumnValues;

        let fetch_names: Vec<String> = if prop_names.is_empty() {
            Vec::new()
        } else {
            let mut run = prop_names.to_vec();
            for extra in self.predicate_columns.iter() {
                if !run.iter().any(|c| c == extra) {
                    run.push(extra.clone());
                }
            }
            run
        };
        let fetch_opt: Option<Vec<String>> = if prop_names.is_empty() {
            None
        } else {
            Some(fetch_names.clone())
        };

        let ctx = Arc::clone(&self.ctx);
        let ts = self.ts;
        let predicate = self.predicate.clone();
        let predicate_columns = self.predicate_columns.clone();
        let src_id_range = self.src_id_range.clone();
        let targets = self.targets.clone();

        let mut out_srcs: Vec<VertexId> = Vec::new();
        let mut out_dsts: Vec<VertexId> = Vec::new();
        let mut out_types: Vec<String> = Vec::new();
        let mut out_ranks: Vec<i64> = Vec::new();
        let mut union_names: Vec<String> = Vec::new();
        let mut union_columns: Vec<ColumnValues> = Vec::new();

        let data_store = ctx.data_store().clone();
        data_store.with_edge_tables(|edge_tables| {
            while out_srcs.len() < batch_size && !self.exhausted {
                if self.target_idx >= targets.len() {
                    self.exhausted = true;
                    break;
                }
                let target = &targets[self.target_idx];
                if self.table_idx >= target.tables.len() {
                    self.target_idx += 1;
                    self.table_idx = 0;
                    self.table_state = TableScanState::new();
                    continue;
                }
                let td = &target.tables[self.table_idx];
                let arc = match edge_tables.get(&td.key) {
                    Some(a) => a.clone(),
                    None => {
                        self.table_idx += 1;
                        self.table_state = TableScanState::new();
                        continue;
                    }
                };
                let guard = arc.read();
                let store: &EdgeStore = &guard;
                let gate = ctx.pending_gate();

                if !matches!(self.table_state.phase, TablePhase::Mutable) {
                    self.table_idx += 1;
                    self.table_state = TableScanState::new();
                    continue;
                }

                let mut pruned: std::collections::HashSet<usize> = std::collections::HashSet::new();
                if !predicate.is_empty() {
                    for gid in store.out_csr.existing_group_ids() {
                        if !store.segment_may_contain(gid as u32, &predicate) {
                            pruned.insert(gid);
                        }
                    }
                }

                let existing = store.out_csr.existing_group_ids();
                let group_bits = store.out_csr.group_bits();
                let start_pos =
                    existing.partition_point(|gid| *gid < self.table_state.resume_group);
                let mut raw_src: Vec<u32> = Vec::new();
                let mut raw_dst: Vec<u32> = Vec::new();
                let mut raw_rank: Vec<i64> = Vec::new();
                let mut raw_edge: Vec<graphdb_core::types::EdgeId> = Vec::new();
                let mut raw_row: Vec<u32> = Vec::new();
                let mut table_done = true;

                for gid in existing.into_iter().skip(start_pos) {
                    if out_srcs.len() + raw_src.len() >= batch_size {
                        table_done = false;
                        break;
                    }
                    if pruned.contains(&gid) {
                        self.table_state.resume_group = gid + 1;
                        self.table_state.skip_in_group = 0;
                        continue;
                    }
                    let Some(variant) = store.out_csr.group_variant(gid) else {
                        self.malformed_skipped += 1;
                        self.table_state.resume_group = gid + 1;
                        self.table_state.skip_in_group = 0;
                        continue;
                    };
                    let base = crate::edge::node_group::group_base(gid, group_bits);
                    let mut iter = variant.iter_all();
                    if gid == self.table_state.resume_group {
                        let skip = self.table_state.skip_in_group;
                        for _ in 0..skip {
                            if iter.next().is_none() {
                                break;
                            }
                        }
                    } else {
                        self.table_state.resume_group = gid;
                        self.table_state.skip_in_group = 0;
                    }
                    for (local_vid, nbr) in iter.by_ref() {
                        self.table_state.skip_in_group += 1;
                        if out_srcs.len() + raw_src.len() >= batch_size {
                            table_done = false;
                            break;
                        }
                        if !store.is_visible_with_gate(nbr.edge_id, ts, &gate) {
                            continue;
                        }
                        let Some(local) = local_vid.as_internal_u32() else {
                            self.malformed_skipped += 1;
                            continue;
                        };
                        let Some(global) = local.checked_add(base) else {
                            self.malformed_skipped += 1;
                            continue;
                        };
                        if let Some(ref r) = src_id_range {
                            let src_internal = VertexId::from_u32(global)
                                .as_internal_u32()
                                .unwrap_or(u32::MAX);
                            let Some(src_ext) =
                                resolve_vertex_id(&ctx, src_internal, td.tbl_src, ts)
                            else {
                                self.malformed_skipped += 1;
                                continue;
                            };
                            let src_int = match src_ext.as_int64() {
                                Some(v) => v,
                                None => match src_ext.as_u64() {
                                    Some(v) => match i64::try_from(v) {
                                        Ok(v) => v,
                                        Err(_) => {
                                            self.malformed_skipped += 1;
                                            continue;
                                        }
                                    },
                                    None => {
                                        self.malformed_skipped += 1;
                                        continue;
                                    }
                                },
                            };
                            if src_int < r.start || src_int >= r.end {
                                continue;
                            }
                        }
                        raw_src.push(global);
                        raw_dst.push(nbr.endpoint);
                        raw_rank.push(nbr.rank);
                        raw_edge.push(nbr.edge_id);
                        raw_row.push(global);
                    }
                    if out_srcs.len() + raw_src.len() >= batch_size {
                        table_done = false;
                        break;
                    }
                    self.table_state.resume_group = gid + 1;
                    self.table_state.skip_in_group = 0;
                }

                if raw_src.is_empty() {
                    if table_done {
                        self.table_state.phase = TablePhase::Done;
                    }
                    if matches!(self.table_state.phase, TablePhase::Done) {
                        self.table_idx += 1;
                        self.table_state = TableScanState::new();
                        continue;
                    }
                    continue;
                }

                let decoded = store.typed_columns_for_edges_assume_visible(
                    &raw_edge,
                    &raw_row,
                    ts,
                    fetch_opt.as_deref(),
                );
                let mut col_map: std::collections::HashMap<String, ColumnValues> =
                    std::collections::HashMap::new();
                for (name, values) in decoded {
                    col_map.insert(name, values);
                }
                let run_names: Vec<String> = if prop_names.is_empty() {
                    let mut names: Vec<String> = col_map.keys().cloned().collect::<Vec<_>>();
                    names.sort();
                    names
                } else {
                    fetch_names.clone()
                };
                let mut run_columns: Vec<ColumnValues> = run_names
                    .iter()
                    .map(|name| {
                        col_map
                            .remove(name)
                            .unwrap_or_else(|| ColumnValues::General(vec![None; raw_src.len()]))
                    })
                    .collect();

                if !predicate.is_empty() {
                    let mut selection: Vec<usize> = Vec::new();
                    for row in 0..raw_src.len() {
                        let mut ok = true;
                        for pred in predicate.iter() {
                            let col_name = pred.column();
                            let idx_opt = run_names.iter().position(|n| n == col_name);
                            match idx_opt {
                                Some(idx) => {
                                    if !pred.matches_column(&run_columns[idx], row) {
                                        ok = false;
                                        break;
                                    }
                                }
                                None => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if ok {
                            selection.push(row);
                        }
                    }
                    if selection.len() != raw_src.len() {
                        let mut next_src = Vec::with_capacity(selection.len());
                        let mut next_dst = Vec::with_capacity(selection.len());
                        let mut next_rank = Vec::with_capacity(selection.len());
                        let mut next_edge = Vec::with_capacity(selection.len());
                        let mut next_row = Vec::with_capacity(selection.len());
                        for &i in &selection {
                            next_src.push(raw_src[i]);
                            next_dst.push(raw_dst[i]);
                            next_rank.push(raw_rank[i]);
                            next_edge.push(raw_edge[i]);
                            next_row.push(raw_row[i]);
                        }
                        raw_src = next_src;
                        raw_dst = next_dst;
                        raw_rank = next_rank;
                        raw_edge = next_edge;
                        raw_row = next_row;
                        for col in run_columns.iter_mut() {
                            col.select(&selection);
                        }
                    }
                    let _ = &predicate_columns;
                }

                if !raw_src.is_empty() && self.offset_remaining > 0 {
                    let skip = self.offset_remaining.min(raw_src.len());
                    let suffix: Vec<usize> = (skip..raw_src.len()).collect();
                    for col in run_columns.iter_mut() {
                        col.select(&suffix);
                    }
                    raw_src.drain(..skip);
                    raw_dst.drain(..skip);
                    raw_rank.drain(..skip);
                    raw_edge.drain(..skip);
                    raw_row.drain(..skip);
                    self.offset_remaining -= skip;
                }

                if raw_src.is_empty() {
                    if table_done {
                        self.table_state.phase = TablePhase::Done;
                    }
                    continue;
                }

                for (name, _) in run_names.iter().zip(run_columns.iter()) {
                    if !union_names.contains(name) {
                        union_names.push(name.clone());
                        let mut new_column = ColumnValues::General(Vec::new());
                        new_column.append_nulls(out_srcs.len());
                        union_columns.push(new_column);
                    }
                }
                for (idx, uname) in union_names.iter().enumerate() {
                    match run_names.iter().position(|n| n == uname) {
                        Some(run_idx) => {
                            let run_column = run_columns[run_idx].clone();
                            union_columns[idx].append(run_column);
                        }
                        None => {
                            union_columns[idx].append_nulls(raw_src.len());
                        }
                    }
                }
                for (src, dst, rank) in raw_src
                    .iter()
                    .zip(raw_dst.iter())
                    .zip(raw_rank.iter())
                    .map(|((s, d), r)| (*s, *d, *r))
                {
                    let src_internal = src;
                    let dst_internal = dst;
                    let src_ext = resolve_vertex_id(&ctx, src_internal, td.tbl_src, ts)
                        .unwrap_or(VertexId::from_u32(src_internal));
                    let dst_ext = resolve_vertex_id(&ctx, dst_internal, td.tbl_dst, ts)
                        .unwrap_or(VertexId::from_u32(dst_internal));
                    out_srcs.push(src_ext);
                    out_dsts.push(dst_ext);
                    out_types.push(target.edge_type_name.clone());
                    out_ranks.push(rank);
                }

                if table_done {
                    self.table_state.phase = TablePhase::Done;
                }
                if matches!(self.table_state.phase, TablePhase::Done) {
                    self.table_idx += 1;
                    self.table_state = TableScanState::new();
                }
                if let Some(limit) = self.limit {
                    if self.emitted + out_srcs.len() >= limit {
                        self.exhausted = true;
                        break;
                    }
                }
            }
        });

        let mut batch = Self::assemble_edge_column_batch(
            prop_names,
            union_names,
            union_columns,
            out_srcs,
            out_dsts,
            out_types,
            out_ranks,
        );
        if let Some(limit) = self.limit {
            let remaining = limit.saturating_sub(self.emitted);
            if batch.len() > remaining {
                batch.srcs.truncate(remaining);
                batch.dsts.truncate(remaining);
                batch.edge_types.truncate(remaining);
                batch.rankings.truncate(remaining);
                for column in batch.columns.iter_mut() {
                    column.values.truncate(remaining);
                }
                self.emitted += remaining;
                self.exhausted = true;
            } else {
                self.emitted += batch.len();
            }
        }
        Ok(batch)
    }

    /// Assemble the final [`EdgeColumnBatch`] from decoded union columns.
    fn assemble_edge_column_batch(
        prop_names: &[String],
        union_names: Vec<String>,
        columns: Vec<crate::cursor::ColumnValues>,
        srcs: Vec<VertexId>,
        dsts: Vec<VertexId>,
        edge_types: Vec<String>,
        rankings: Vec<i64>,
    ) -> EdgeColumnBatch {
        use crate::cursor::PropertyColumn;
        use graphdb_core::types::DataType;
        let output_columns: Vec<PropertyColumn> = if prop_names.is_empty() {
            union_names
                .into_iter()
                .zip(columns)
                .map(|(name, values)| PropertyColumn {
                    name,
                    data_type: DataType::Empty,
                    values,
                })
                .collect()
        } else {
            let row_count = srcs.len();
            prop_names
                .iter()
                .map(|name| {
                    let values = union_names
                        .iter()
                        .position(|n| n == name)
                        .and_then(|index| columns.get(index).cloned())
                        .unwrap_or_else(|| {
                            crate::cursor::ColumnValues::General(vec![None; row_count])
                        });
                    PropertyColumn {
                        name: name.clone(),
                        data_type: DataType::Empty,
                        values,
                    }
                })
                .collect()
        };
        EdgeColumnBatch {
            srcs,
            dsts,
            edge_types,
            rankings,
            columns: output_columns,
        }
    }
}

impl EdgeCursor for GraphEdgeCursor {
    fn malformed_skipped(&self) -> u64 {
        self.malformed_skipped
    }

    fn next_column_batch(
        &mut self,
        prop_names: &[String],
        batch_size: usize,
    ) -> Result<EdgeColumnBatch, StorageError> {
        if self.exhausted {
            return Ok(EdgeColumnBatch::empty());
        }
        let batch_size = batch_size.max(1);
        loop {
            let batch = self.collect_column_batch(prop_names, batch_size)?;
            if !batch.is_empty() || self.exhausted {
                return Ok(batch);
            }
        }
    }

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
        let malformed = &mut self.malformed_skipped;

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
                let arc = match edge_tables.get(&td.key) {
                    Some(a) => a.clone(),
                    None => {
                        *table_idx += 1;
                        *table_state = TableScanState::new();
                        continue;
                    }
                };
                let guard = arc.read();
                let store: &EdgeStore = &guard;

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
                            malformed,
                        });
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
            .filter_map(|candidate| materialize_edge(ctx, candidate, ts))
            .collect())
    }
}

struct ScanArgs<'a> {
    ctx: &'a GraphStorageContext,
    store: &'a EdgeStore,
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
    malformed: &'a mut u64,
}

// ---------------------------------------------------------------------------
// Free-function scan helpers
// ---------------------------------------------------------------------------

fn scan_mutable(args: ScanArgs) {
    let gate = args.ctx.pending_gate();

    // Column pruning: fetch the projection plus any predicate-only columns
    // in one storage read instead of decoding every column per edge.
    // `None` still means all columns.
    let fetch_columns: Option<Vec<String>> = match *args.projection {
        None => None,
        Some(ref names) => {
            let mut cols = names.clone();
            for extra in args.predicate_columns.iter() {
                if !cols.iter().any(|c| c == extra) {
                    cols.push(extra.clone());
                }
            }
            Some(cols)
        }
    };

    // Segment pruning before decoding: groups whose flushed statistics
    // provably exclude the predicates are skipped without touching property
    // columns. Safe mid-scan because visibility is snapshot-fixed, bounds
    // only widen, and dirty groups never prune; the resume accounting below
    // advances past skipped groups exactly like a full walk.
    let mut pruned: std::collections::HashSet<usize> = std::collections::HashSet::new();
    if !args.predicate.is_empty() {
        for gid in args.store.out_csr.existing_group_ids() {
            if !args.store.segment_may_contain(gid as u32, args.predicate) {
                pruned.insert(gid);
            }
        }
        if !pruned.is_empty() {
            log::debug!(
                "edge scan pruned {} of {} groups",
                pruned.len(),
                args.store.out_csr.existing_group_ids().len()
            );
        }
    }

    let existing = args.store.out_csr.existing_group_ids();
    let group_bits = args.store.out_csr.group_bits();
    let start_pos = existing.partition_point(|gid| *gid < args.state.resume_group);
    for gid in existing.into_iter().skip(start_pos) {
        if pruned.contains(&gid) {
            // Fully consumed without decoding: pruned groups yield no
            // candidates, so batches never end inside them and resume
            // advances past the whole group.
            args.state.resume_group = gid + 1;
            args.state.skip_in_group = 0;
            continue;
        }
        let Some(variant) = args.store.out_csr.group_variant(gid) else {
            // Listed as existing but unreadable: metadata inconsistency,
            // counted instead of silently dropped.
            *args.malformed += 1;
            args.state.resume_group = gid + 1;
            args.state.skip_in_group = 0;
            continue;
        };
        let base = crate::edge::node_group::group_base(gid, group_bits);
        let mut iter = variant.iter_all();
        if gid == args.state.resume_group {
            let skip = args.state.skip_in_group;
            for _ in 0..skip {
                if iter.next().is_none() {
                    break;
                }
            }
            // Keep the absolute physical offset: entries below keep incrementing
            // it so the next batch resumes after all consumed entries.
        } else {
            args.state.resume_group = gid;
            args.state.skip_in_group = 0;
        }
        for (local_vid, nbr) in iter.by_ref() {
            args.state.skip_in_group += 1;
            if !args.store.is_visible_with_gate(nbr.edge_id, args.ts, &gate) {
                continue;
            }
            let Some(local) = local_vid.as_internal_u32() else {
                // A stored group row that is not an internal id indicates
                // corruption; count it instead of silently dropping it.
                *args.malformed += 1;
                continue;
            };
            let Some(global) = local.checked_add(base) else {
                *args.malformed += 1;
                continue;
            };
            let src_vid = VertexId::from_u32(global);
            if let Some(ref r) = *args.src_id_range {
                let src_internal = src_vid.as_internal_u32().unwrap_or(u32::MAX);
                let Some(src_ext) =
                    resolve_vertex_id(args.ctx, src_internal, args.td.tbl_src, args.ts)
                else {
                    *args.malformed += 1;
                    continue;
                };
                // An external id outside the integer domain cannot satisfy a
                // numeric range; count and skip instead of mapping it to a
                // sentinel that silently mis-filters the row.
                let src_int = match src_ext.as_int64() {
                    Some(v) => v,
                    None => match src_ext.as_u64() {
                        Some(v) => match i64::try_from(v) {
                            Ok(v) => v,
                            Err(_) => {
                                *args.malformed += 1;
                                continue;
                            }
                        },
                        None => {
                            *args.malformed += 1;
                            continue;
                        }
                    },
                };
                if src_int < r.start || src_int >= r.end {
                    continue;
                }
            }

            // Decode once with predicate columns included so pushed predicates
            // can be evaluated; matching rows are then trimmed back to the
            // projection. Filtering happens before offset/limit accounting.
            // With predicates, the column-scan layer filters row numbers
            // first on predicate columns only, and hits decode the fetch
            // set afterwards; misses never materialize a record and nulls
            // use bitmap semantics (missing never matches).
            if !args.predicate.is_empty() {
                let probe = decode_edge_properties(
                    args.store,
                    nbr.edge_id,
                    args.ts,
                    Some(args.predicate_columns),
                );
                if !args.predicate.iter().all(|p| p.matches(probe.as_slice())) {
                    continue;
                }
            }
            let mut properties =
                decode_edge_properties(args.store, nbr.edge_id, args.ts, fetch_columns.as_deref());
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
                args.state.resume_group = gid;
                return;
            }
            if args.limit.is_some_and(|l| *args.emitted >= l) {
                args.state.resume_group = gid + 1;
                args.state.skip_in_group = 0;
                return;
            }
        }
        args.state.resume_group = gid + 1;
        args.state.skip_in_group = 0;
    }

    args.state.phase = TablePhase::Done;
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
    let src_internal = args.src_vid.as_internal_u32().unwrap_or(u32::MAX);
    let rank = args.nbr.rank;
    let dst_vid = VertexId::from_u32(args.nbr.endpoint);

    let src_vid = VertexId::from_u32(src_internal);
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

fn materialize_edge(
    ctx: &GraphStorageContext,
    candidate: EdgeCandidate,
    ts: Timestamp,
) -> Option<Edge> {
    let src_internal = candidate.src_vid.as_internal_u32().unwrap_or(u32::MAX);
    let dst_internal = candidate.dst_vid.as_internal_u32().unwrap_or(u32::MAX);
    let src_external =
        resolve_vertex_id(ctx, src_internal, candidate.src_label, ts).unwrap_or(candidate.src_vid);
    let dst_external =
        resolve_vertex_id(ctx, dst_internal, candidate.dst_label, ts).unwrap_or(candidate.dst_vid);
    Some(Edge {
        src: src_external,
        dst: dst_external,
        edge_type: candidate.edge_type_name,
        ranking: candidate.rank,
        props: candidate.props,
    })
}

/// Decode edge properties for the precomputed fetch set (projection plus
/// any extra columns required by pushed scan predicates).
///
/// MVCCManager is the single visibility authority; the property row
/// timestamps are physical replicas and must not decide visibility here.
fn decode_edge_properties(
    store: &EdgeStore,
    edge_id: graphdb_core::types::EdgeId,
    ts: Timestamp,
    fetch: Option<&[String]>,
) -> Vec<(String, Value)> {
    if !store.is_visible(edge_id, ts) {
        return Vec::new();
    }
    // Snapshot read through the property version chain so old readers see
    // the before-image instead of the latest write. Row stamps never filter;
    // authority above already decided visibility.
    let props_opt = store
        .properties
        .get_projected_physical_by_edge_id(edge_id, ts, fetch);
    props_opt
        .map(|props| {
            props
                .into_iter()
                .filter_map(|(k, v)| v.map(|value| (k, value)))
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
    ts: Timestamp,
) -> Option<VertexId> {
    if let Some(vid) = ctx.get_external_id_by_internal_id(label, internal) {
        return Some(vid);
    }
    ctx.get_external_vertex_id(label, internal, ts)
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

/// Open a hot single-segment edge scan cursor.
pub(crate) fn create_edge_cursor(
    ctx: Arc<GraphStorageContext>,
    space: &str,
    options: &ScanOptions,
) -> StorageResult<Box<dyn EdgeCursor>> {
    let cursor = GraphEdgeCursor::new(ctx, space, options)?;
    Ok(Box::new(cursor))
}
