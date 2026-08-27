use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::core::types::{EdgeId, LabelId, Timestamp, VertexId};
use crate::core::vertex_edge_path::Tag;
use crate::core::{Edge, StorageError, StorageResult, Value, Vertex};
use crate::cold::ColdSnapshot;
use crate::cursor::{EdgeCursor, FlatVertexRecord, ScanOptions, VertexCursor};
use crate::edge::edge_table::core::TimeTravelEdgeStore;
use crate::edge::Nbr;
use crate::engine::data_store::EdgeTableKey;
use crate::vertex::ShardedVertexTable;

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
    /// Index into `tags.labels` indicating which table is being scanned.
    current_table_idx: usize,
    /// Table currently being scanned, loaded lazily per table.
    current_table: Option<Arc<ShardedVertexTable>>,
    /// Label of the currently loaded table.
    current_label: Option<LabelId>,
    /// Live internal ids of the current table, in scan order.
    pending_ids: Vec<u32>,
    /// Index into `pending_ids`.
    pending_idx: usize,
    limit: Option<usize>,
    offset_remaining: usize,
    emitted: usize,
    id_range: Option<Range<i64>>,
    projection: Option<Vec<String>>,
    /// Pushed conjunctive scan predicates evaluated on decoded rows.
    predicate: Vec<crate::cursor::ScanPredicate>,
    exhausted: bool,
    /// Read timestamp captured when the cursor is opened.
    ts: Timestamp,
}

impl std::fmt::Debug for GraphVertexCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphVertexCursor")
            .field("space", &self.space)
            .field("tags", &self.tags.labels.len())
            .field("current_table_idx", &self.current_table_idx)
            .field("pending_ids", &self.pending_ids.len())
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
        let tag_infos = ctx.schema_manager().list_tags(&space)?;
        let tags = TagCache {
            labels: tag_infos.iter().map(|t| t.tag_id).collect(),
            names: tag_infos
                .into_iter()
                .map(|t| (t.tag_id, t.tag_name))
                .collect(),
        };
        // When the scan is tag-restricted, scan only that tag's table.  A tag
        // that does not exist in the schema yields no rows (an unknown tag
        // matches no vertex), which mirrors the old residual
        // `contains(labels(v), ...)` filter evaluating to false for every row.
        let mut labels = tags.labels.clone();
        if let Some(tag_name) = options.tag.as_deref() {
            labels.retain(|label_id| {
                tags.names
                    .get(label_id)
                    .map(|name| name == tag_name)
                    .unwrap_or(false)
            });
        }
        let tags = TagCache {
            labels,
            names: tags.names,
        };

        let exhausted = ctx.data_store().with_vertex_tables(|tables| {
            tags.labels
                .iter()
                .all(|label_id| tables.get(label_id).is_none_or(|t| t.total_count() == 0))
        });

        Ok(Self {
            ctx,
            space,
            tags,
            current_table_idx: 0,
            current_table: None,
            current_label: None,
            pending_ids: Vec::new(),
            pending_idx: 0,
            limit: options.limit,
            offset_remaining: options.offset,
            emitted: 0,
            id_range: options.vertex_id_range.clone(),
            projection: options
                .projection
                .as_ref()
                .map(|p| p.iter().map(|rp| rp.name.clone()).collect()),
            predicate: options.predicate.clone().unwrap_or_default(),
            exhausted,
            ts,
        })
    }

    /// Load the next non-empty table's live ids into `pending_ids`, advancing
    /// through tables until one has ids. Sets `exhausted` when no table remains.
    fn load_next_table(&mut self, tables: &HashMap<LabelId, Arc<ShardedVertexTable>>) {
        self.current_table = None;
        self.current_label = None;
        self.pending_ids.clear();
        self.pending_idx = 0;
        while self.current_table_idx < self.tags.labels.len() {
            let label_id = self.tags.labels[self.current_table_idx];
            self.current_table_idx += 1;
            if let Some(table) = tables.get(&label_id) {
                // Lazily register the statement snapshot for this label.
                self.ctx.ensure_vertex_snapshot_registered(label_id);
                let ids = table.live_ids();
                if !ids.is_empty() {
                    self.current_label = Some(label_id);
                    self.pending_ids = ids;
                    self.current_table = Some(Arc::clone(table));
                    return;
                }
            }
        }
        self.exhausted = true;
    }
}

impl VertexCursor for GraphVertexCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Vertex>, StorageError> {
        self.scan_batch(batch_size, |vid, internal_id, tag_name, props| {
            let props_map: HashMap<String, Value> = props.into_iter().collect();
            Vertex {
                vid,
                id: internal_id,
                tags: vec![Tag::new(tag_name, props_map.clone())],
                properties: props_map,
            }
        })
    }

    fn next_flat_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<Vec<FlatVertexRecord>, StorageError> {
        self.scan_batch(batch_size, |vid, internal_id, tag_name, props| {
            FlatVertexRecord {
                vid,
                internal_id,
                tag_name,
                props,
            }
        })
    }

    fn next_column_batch(
        &mut self,
        prop_names: &[String],
        batch_size: usize,
    ) -> Result<crate::cursor::VertexColumnBatch, StorageError> {
        if self.exhausted || self.tags.labels.is_empty() {
            return Ok(crate::cursor::VertexColumnBatch::empty());
        }
        let batch_size = batch_size.max(1);
        loop {
            let batch = self.collect_column_batch(prop_names, batch_size)?;
            if !batch.is_empty() || self.exhausted {
                return Ok(batch);
            }
            // Every row collected in this window was filtered out by the
            // pushed predicates: keep going so an empty window never ends the
            // scan early (mirrors the row-based scan loop).
        }
    }
}

impl GraphVertexCursor {
    /// Collect one column-major batch: gather candidates across tables,
    /// decode the requested columns, apply pushed predicates and the row
    /// limit, and assemble the final [`VertexColumnBatch`].
    fn collect_column_batch(
        &mut self,
        prop_names: &[String],
        batch_size: usize,
    ) -> Result<crate::cursor::VertexColumnBatch, StorageError> {
        let data_store = self.ctx.data_store().clone();
        let names = self.tags.names.clone();
        let result = data_store.with_vertex_tables(|tables| {
            let mut vids: Vec<VertexId> = Vec::new();
            let mut internal_ids: Vec<u32> = Vec::new();
            let mut tag_names: Vec<String> = Vec::new();
            // Union of decoded column names, grown as tables are processed.
            let mut union_names: Vec<String> = Vec::new();
            let mut columns: Vec<crate::cursor::ColumnValues> = Vec::new();

            while internal_ids.len() < batch_size && !self.exhausted {
                if self.current_table.is_none() {
                    self.load_next_table(tables);
                    continue;
                }
                if self.pending_idx >= self.pending_ids.len() {
                    self.current_table = None;
                    self.current_label = None;
                    self.pending_ids.clear();
                    self.pending_idx = 0;
                    continue;
                }

                let end = (self.pending_idx + (batch_size - internal_ids.len()))
                    .min(self.pending_ids.len());
                let ids = &self.pending_ids[self.pending_idx..end];
                self.pending_idx = end;

                let Some(table) = self.current_table.clone() else {
                    continue;
                };
                let label_id = self.current_label;
                let tag_name = label_id
                    .and_then(|l| names.get(&l))
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");

                // Decode names for this table run.  A full-row decode (empty
                // projection) decodes every column of the table; otherwise the
                // projection plus any pushed-predicate columns.
                let run_names: Vec<String> = if prop_names.is_empty() {
                    Vec::new()
                } else {
                    let mut run = prop_names.to_vec();
                    for predicate in &self.predicate {
                        let column = predicate.column().to_string();
                        if !run.contains(&column) {
                            run.push(column);
                        }
                    }
                    run
                };

                let ids_vec: Vec<u32> = ids.to_vec();
                let resolved = table.resolve_valid_ids(&ids_vec, self.ts);
                let mut run_internal: Vec<u32> = Vec::new();
                let mut run_vids: Vec<VertexId> = Vec::new();
                for (pos, &id) in ids_vec.iter().enumerate() {
                    let Some(vid) = resolved[pos] else {
                        continue;
                    };
                    if let Some(ref range) = self.id_range {
                        match vid.as_int64() {
                            Some(vid) if (range.start..range.end).contains(&vid) => {}
                            _ => continue,
                        }
                    }
                    if self.offset_remaining > 0 {
                        self.offset_remaining -= 1;
                        continue;
                    }
                    run_internal.push(id);
                    run_vids.push(vid);
                }

                let run_rows = run_internal.len();
                if run_rows == 0 {
                    continue;
                }

                // Zone-map pruning over the offset-selected candidates: rows
                // dropped here are exactly those the pushed predicates would
                // reject after decoding, so skipping their decode is a pure
                // optimization with identical results.
                if !self.predicate.is_empty() {
                    let ranges =
                        crate::cursor::ScanPredicate::merged_ranges(&self.predicate);
                    let mask = table.zone_prune_mask(&run_internal, &ranges);
                    if mask.iter().any(|&keep| !keep) {
                        let mut kept_internal = Vec::with_capacity(run_rows);
                        let mut kept_vids = Vec::with_capacity(run_rows);
                        for (row, &keep) in mask.iter().enumerate() {
                            if keep {
                                kept_internal.push(run_internal[row]);
                                kept_vids.push(run_vids[row]);
                            }
                        }
                        run_internal = kept_internal;
                        run_vids = kept_vids;
                    }
                }

                let decoded = table.get_projected_columns(&run_internal, self.ts, &run_names);

                // Merge the run into the batch's column union.
                let before = internal_ids.len();
                for (name, _) in &decoded {
                    if !union_names.contains(name) {
                        union_names.push(name.clone());
                        let mut new_column =
                            crate::cursor::ColumnValues::General(Vec::new());
                        new_column.append_nulls(before);
                        columns.push(new_column);
                    }
                }
                for (index, uname) in union_names.iter().enumerate() {
                    match decoded.iter().position(|(n, _)| n == uname) {
                        Some(run_index) => {
                            let run_column = decoded[run_index].1.clone();
                            columns[index].append(run_column);
                        }
                        None => columns[index].append_nulls(run_rows),
                    }
                }

                internal_ids.extend(run_internal);
                vids.extend(run_vids);
                tag_names.extend(std::iter::repeat_n(tag_name.to_string(), run_rows));
            }

            (
                internal_ids,
                vids,
                tag_names,
                union_names,
                columns,
                self.exhausted,
            )
        });

        let (internal_ids, vids, tag_names, union_names, mut columns, _exhausted) = result;

        // Apply pushed predicates over the decoded columns.
        let (final_ids, final_vids, final_tags) = if self.predicate.is_empty() {
            (internal_ids, vids, tag_names)
        } else {
            let mut keep = vec![true; internal_ids.len()];
            for predicate in &self.predicate {
                match union_names.iter().position(|n| n == predicate.column()) {
                    Some(index) => {
                        let column = &columns[index];
                        for (row, ok) in keep.iter_mut().enumerate() {
                            if *ok && !predicate.matches_column(column, row) {
                                *ok = false;
                            }
                        }
                    }
                    None => keep.fill(false),
                }
            }
            if keep.iter().any(|&k| !k) {
                let mut kept_ids = Vec::with_capacity(keep.iter().filter(|&&k| k).count());
                let mut kept_vids = Vec::with_capacity(kept_ids.capacity());
                let mut kept_tags = Vec::with_capacity(kept_ids.capacity());
                for (row, &k) in keep.iter().enumerate() {
                    if k {
                        kept_ids.push(internal_ids[row]);
                        kept_vids.push(vids[row]);
                        kept_tags.push(tag_names[row].clone());
                    }
                }
                for column in columns.iter_mut() {
                    column.compact(&keep);
                }
                (kept_ids, kept_vids, kept_tags)
            } else {
                (internal_ids, vids, tag_names)
            }
        };

        let mut batch = Self::assemble_column_batch(
            prop_names,
            union_names,
            columns,
            final_ids,
            final_vids,
            final_tags,
        );

        // Apply the scan limit on the returned rows.
        if let Some(limit) = self.limit {
            let remaining = limit.saturating_sub(self.emitted);
            if batch.len() > remaining {
                batch.vids.truncate(remaining);
                batch.internal_ids.truncate(remaining);
                batch.tag_names.truncate(remaining);
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
    /// Shared scan loop over the vertex tables, building one output row per
    /// emitted vertex. The `build` closure receives the decoded fields
    /// (external vid, internal id, tag name, projected properties as a plain
    /// `Vec`) so both the `Vertex` and the flat-record paths share the
    /// filtering / batch logic while skipping per-row `HashMap` boxing in the
    /// flat path.
    fn scan_batch<T>(
        &mut self,
        batch_size: usize,
        mut build: impl FnMut(VertexId, i64, String, Vec<(String, Value)>) -> T,
    ) -> Result<Vec<T>, StorageError> {
        if self.exhausted || self.tags.labels.is_empty() {
            return Ok(Vec::new());
        }

        let batch_size = batch_size.max(1);
        let data_store = self.ctx.data_store().clone();
        let names = self.tags.names.clone();
        let batch = data_store.with_vertex_tables(|tables| {
            let mut batch = Vec::new();

            while batch.len() < batch_size && !self.exhausted {
                if self.current_table.is_none() {
                    self.load_next_table(tables);
                    continue;
                }
                if self.pending_idx >= self.pending_ids.len() {
                    self.current_table = None;
                    self.current_label = None;
                    self.pending_ids.clear();
                    self.pending_idx = 0;
                    continue;
                }

                let end =
                    (self.pending_idx + (batch_size - batch.len())).min(self.pending_ids.len());
                let ids = &self.pending_ids[self.pending_idx..end];
                self.pending_idx = end;

                let Some(table) = self.current_table.clone() else {
                    continue;
                };
                let label_id = self.current_label;
                let records = table.get_projected_batch(ids, self.ts, self.projection.as_deref());
                let tag_name = label_id
                    .and_then(|l| names.get(&l))
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");

                for record in records.into_iter().flatten() {
                    // The vertex-id range is applied to the external vertex ID
                    // (the same domain as `PartitionSpec` ranges). Internal IDs
                    // are shard-local and cannot be addressed by a global
                    // range. Non-numeric IDs never match an i64 range.
                    if let Some(ref range) = self.id_range {
                        let vid = record.vid.as_int64();
                        match vid {
                            Some(vid) if (range.start..range.end).contains(&vid) => {}
                            _ => continue,
                        }
                    }
                    if !self.predicate.is_empty()
                        && !self.predicate.iter().all(|p| p.matches(&record.properties))
                    {
                        continue;
                    }
                    if self.offset_remaining > 0 {
                        self.offset_remaining -= 1;
                        continue;
                    }
                    batch.push(build(
                        record.vid,
                        record.internal_id as i64,
                        tag_name.to_string(),
                        record.properties,
                    ));
                    self.emitted += 1;
                    if let Some(limit) = self.limit {
                        if self.emitted >= limit {
                            self.exhausted = true;
                            break;
                        }
                    }
                }
            }
            batch
        });

        Ok(batch)
    }

    /// Assemble the final [`VertexColumnBatch`] from the decoded union columns.
    ///
    /// When `prop_names` is non-empty only those columns are returned (in
    /// projection order, missing columns as all-null); an empty `prop_names`
    /// returns every decoded column.
    fn assemble_column_batch(
        prop_names: &[String],
        union_names: Vec<String>,
        columns: Vec<crate::cursor::ColumnValues>,
        internal_ids: Vec<u32>,
        vids: Vec<VertexId>,
        tag_names: Vec<String>,
    ) -> crate::cursor::VertexColumnBatch {
        let output_columns: Vec<crate::cursor::PropertyColumn> = if prop_names.is_empty() {
            union_names
                .into_iter()
                .zip(columns)
                .map(|(name, values)| crate::cursor::PropertyColumn {
                    name,
                    data_type: crate::core::types::DataType::Empty,
                    values,
                })
                .collect()
        } else {
            let row_count = vids.len();
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
                    crate::cursor::PropertyColumn {
                        name: name.clone(),
                        data_type: crate::core::types::DataType::Empty,
                        values,
                    }
                })
                .collect()
        };
        crate::cursor::VertexColumnBatch {
            vids,
            internal_ids: internal_ids.into_iter().map(|id| id as i64).collect(),
            tag_names,
            columns: output_columns,
        }
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
            nbr.prop_offset,
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

        let nbr = Nbr::new(
            edge.neighbor,
            edge.edge_id,
            edge.prop_offset,
            edge.timestamp,
        );

        // Same decode-once / pre-filter discipline as the mutable scan.
        let mut properties = decode_edge_properties(
            args.store,
            nbr.prop_offset,
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
    let (dst_vid, rank) = decode_endpoint(args.nbr.neighbor);

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

/// Decode edge properties keeping projected columns plus any extra columns
/// required by pushed scan predicates.
fn decode_edge_properties(
    store: &TimeTravelEdgeStore,
    prop_offset: u32,
    projection: &Option<Vec<String>>,
    predicate_columns: &[String],
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

// ---------------------------------------------------------------------------
// ColdEdgeCursor — CSR-scanning cursor over one cold snapshot
// ---------------------------------------------------------------------------

/// Lazy cursor over a single cold snapshot's out-CSR.
///
/// Rows are prefetched one vertex at a time; the cursor applies the source
/// ID range and property projection but leaves offset/limit to the
/// [`MultiSourceEdgeCursor`] wrapper so dedup ordering stays global.
pub(crate) struct ColdEdgeCursor {
    ctx: Arc<GraphStorageContext>,
    snapshot: Arc<ColdSnapshot>,
    edge_type_name: String,
    src_label: LabelId,
    dst_label: LabelId,
    ts: Timestamp,
    src_id_range: Option<Range<i64>>,
    projection: Option<Vec<String>>,
    predicate: Vec<crate::cursor::ScanPredicate>,
    src_cursor: usize,
    row_edges: Vec<Nbr>,
    row_idx: usize,
    exhausted: bool,
}

impl std::fmt::Debug for ColdEdgeCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColdEdgeCursor")
            .field("label", &self.snapshot.label())
            .field("snapshot_ts", &self.snapshot.snapshot_ts())
            .field("src_cursor", &self.src_cursor)
            .field("row_idx", &self.row_idx)
            .field("exhausted", &self.exhausted)
            .finish()
    }
}

impl ColdEdgeCursor {
    pub fn new(
        ctx: Arc<GraphStorageContext>,
        snapshot: Arc<ColdSnapshot>,
        edge_type_name: String,
        src_label: LabelId,
        dst_label: LabelId,
        ts: Timestamp,
        options: &ScanOptions,
    ) -> Self {
        Self {
            ctx,
            snapshot,
            edge_type_name,
            src_label,
            dst_label,
            ts,
            src_id_range: options.edge_src_id_range.clone(),
            projection: options
                .projection
                .as_ref()
                .map(|p| p.iter().map(|rp| rp.name.clone()).collect()),
            predicate: options.predicate.clone().unwrap_or_default(),
            src_cursor: 0,
            row_edges: Vec::new(),
            row_idx: 0,
            exhausted: false,
        }
    }

    fn load_next_row(&mut self) {
        let capacity = self.snapshot.vertex_capacity();
        while self.row_idx >= self.row_edges.len() {
            if self.src_cursor >= capacity {
                self.exhausted = true;
                return;
            }
            self.row_edges = self.snapshot.get_out_edges(self.src_cursor as u32);
            self.src_cursor += 1;
            self.row_idx = 0;
        }
    }
}

impl EdgeCursor for ColdEdgeCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Edge>, StorageError> {
        if self.exhausted {
            return Ok(Vec::new());
        }
        let batch_size = batch_size.max(1);
        let mut batch = Vec::with_capacity(batch_size);
        let src_label = self.src_label;
        let dst_label = self.dst_label;
        let ts = self.ts;
        let src_id_range = self.src_id_range.clone();
        let projection = self.projection.clone();

        while batch.len() < batch_size && !self.exhausted {
            self.load_next_row();
            if self.exhausted {
                break;
            }
            let src_internal = (self.src_cursor - 1) as u32;
            let nbr = self.row_edges[self.row_idx];
            self.row_idx += 1;

            let (dst_vid, rank) = TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
            let dst_internal = dst_vid.as_int64().unwrap_or(0) as u32;
            let src_vid = VertexId::from_int64(src_internal as i64);
            // Predicates are evaluated on the full property set before the
            // projection narrows it (pure pre-filter).
            let all_props: Vec<(String, Value)> = self
                .snapshot
                .nbr_to_edge_record(&nbr, src_vid, dst_vid)
                .properties;
            if !self
                .predicate
                .iter()
                .all(|p| p.matches(all_props.as_slice()))
            {
                continue;
            }
            let props: HashMap<String, Value> = all_props
                .into_iter()
                .filter(|(name, _)| {
                    projection
                        .as_ref()
                        .is_none_or(|proj| proj.iter().any(|p| p == name))
                })
                .collect();

            let src_ext = resolve_vertex_id(&self.ctx, src_internal, src_label, &src_vid, ts);
            let dst_ext = resolve_vertex_id(&self.ctx, dst_internal, dst_label, &dst_vid, ts);

            // The CSR iterates internal vertex indices; the src-id range is
            // expressed over external ids, so filter on the resolved id.
            if let Some(ref range) = src_id_range {
                let src_int = src_ext.parse::<i64>().unwrap_or(i64::MIN);
                if src_int < range.start || src_int >= range.end {
                    continue;
                }
            }

            batch.push(Edge {
                src: make_vid(&src_ext),
                dst: make_vid(&dst_ext),
                edge_type: self.edge_type_name.clone(),
                ranking: rank,
                props,
            });
        }
        Ok(batch)
    }
}

// ---------------------------------------------------------------------------
// MultiSourceEdgeCursor — merged hot + cold scan with global offset/limit
// ---------------------------------------------------------------------------

/// Merged edge cursor over one hot cursor followed by any number of cold
/// cursors. Dedup keys on (src, dst, ranking) in external-ID space so edges
/// present in both tiers (or in several snapshots) are emitted once; the
/// global offset and limit apply to the deduplicated stream.
pub(crate) struct MultiSourceEdgeCursor {
    sources: Vec<Box<dyn EdgeCursor>>,
    source_idx: usize,
    offset_remaining: usize,
    limit: Option<usize>,
    emitted: usize,
    seen: HashSet<(VertexId, VertexId, i64)>,
    exhausted: bool,
}

impl std::fmt::Debug for MultiSourceEdgeCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiSourceEdgeCursor")
            .field("sources", &self.sources.len())
            .field("source_idx", &self.source_idx)
            .field("emitted", &self.emitted)
            .field("exhausted", &self.exhausted)
            .finish()
    }
}

impl MultiSourceEdgeCursor {
    pub fn new(sources: Vec<Box<dyn EdgeCursor>>, options: &ScanOptions) -> Self {
        let exhausted = sources.is_empty();
        Self {
            sources,
            source_idx: 0,
            offset_remaining: options.offset,
            limit: options.limit,
            emitted: 0,
            seen: HashSet::new(),
            exhausted,
        }
    }
}

impl EdgeCursor for MultiSourceEdgeCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Edge>, StorageError> {
        if self.exhausted {
            return Ok(Vec::new());
        }
        let batch_size = batch_size.max(1);
        let mut batch = Vec::with_capacity(batch_size);

        while batch.len() < batch_size {
            if self.source_idx >= self.sources.len() {
                self.exhausted = true;
                break;
            }
            let source_batch = self.sources[self.source_idx].next_batch(batch_size)?;
            if source_batch.is_empty() {
                self.source_idx += 1;
                continue;
            }
            for edge in source_batch {
                if !self.seen.insert((edge.src, edge.dst, edge.ranking)) {
                    continue;
                }
                if self.offset_remaining > 0 {
                    self.offset_remaining -= 1;
                    continue;
                }
                batch.push(edge);
                self.emitted += 1;
                if self.limit.is_some_and(|l| self.emitted >= l) {
                    self.exhausted = true;
                    break;
                }
            }
        }
        Ok(batch)
    }
}

/// Open an edge scan cursor that reads hot tables first and then appends
/// every cold snapshot matching the scan's edge type.
///
/// The hot cursor is created without offset/limit; the wrapping
/// [`MultiSourceEdgeCursor`] applies them to the deduplicated stream.
pub(crate) fn create_edge_cursor(
    ctx: Arc<GraphStorageContext>,
    space: &str,
    options: &ScanOptions,
) -> StorageResult<Box<dyn EdgeCursor>> {
    let mut hot_options = options.clone();
    hot_options.limit = None;
    hot_options.offset = 0;
    let hot = GraphEdgeCursor::new(ctx.clone(), space, &hot_options)?;

    let ts = options
        .read_timestamp
        .unwrap_or_else(|| ctx.get_read_timestamp());
    let mut cold_cursors: Vec<(Timestamp, ColdEdgeCursor)> = Vec::new();
    {
        let cold = ctx.cold_snapshots().read();
        for snapshots in cold.values() {
            for snapshot in snapshots {
                if ts < snapshot.snapshot_ts() {
                    continue;
                }
                if let Some(ref edge_type) = options.edge_type {
                    let edge_info = ctx.schema_manager().get_edge_type(space, edge_type)?;
                    let Some(info) = edge_info else { continue };
                    if info.edge_type_id != snapshot.label() {
                        continue;
                    }
                }
                let schema = snapshot.schema();
                cold_cursors.push((
                    snapshot.snapshot_ts(),
                    ColdEdgeCursor::new(
                        ctx.clone(),
                        snapshot.clone(),
                        schema.label_name.clone(),
                        schema.src_label,
                        schema.dst_label,
                        ts,
                        options,
                    ),
                ));
            }
        }
    }
    // Oldest snapshots first so the merged stream is time-ordered.
    cold_cursors.sort_by_key(|(snapshot_ts, _)| *snapshot_ts);

    if cold_cursors.is_empty() {
        return Ok(Box::new(hot));
    }
    let mut sources: Vec<Box<dyn EdgeCursor>> = Vec::with_capacity(1 + cold_cursors.len());
    sources.push(Box::new(hot));
    for (_, cursor) in cold_cursors {
        sources.push(Box::new(cursor));
    }
    Ok(Box::new(MultiSourceEdgeCursor::new(sources, options)))
}
