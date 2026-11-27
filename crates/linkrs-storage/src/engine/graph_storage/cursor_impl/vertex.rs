use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use crate::cursor::{
    ColumnValues, FlatVertexRecord, ScanIdProvenance, ScanOptions, ScanScratch, ScanTimings,
    ShardSlice, VertexCursor,
};
use crate::engine::graph_storage::context::vertex_ops::StagedRow;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::vertex::{ExternalKeyKind, ShardedVertexTable};
use linkrs_core::types::{LabelId, Timestamp, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{StorageError, StorageResult, Value, Vertex};

#[inline]
fn record_elapsed(start: std::time::Instant, counter: &mut u64) {
    *counter += start.elapsed().as_micros() as u64;
}

/// Gather one decoded run down to `selection`, keeping ids and external ids
/// aligned with the surviving column values.
fn select_run(
    decoded: &mut [(Arc<str>, ColumnValues)],
    ids: &mut Vec<u32>,
    vids: &mut Vec<VertexId>,
    selection: &[usize],
) {
    if selection.len() == ids.len() {
        return;
    }
    for (_, column) in decoded.iter_mut() {
        column.select(selection);
    }
    let mut write = 0;
    for &row in selection {
        ids[write] = ids[row];
        vids[write] = vids[row];
        write += 1;
    }
    ids.truncate(write);
    vids.truncate(write);
}

// ---------------------------------------------------------------------------
// GraphVertexCursor (unchanged)
// ---------------------------------------------------------------------------

struct TagCache {
    labels: Vec<LabelId>,
    names: HashMap<LabelId, String>,
    /// Per-table TTL config for tags with a valid TTL: (duration, column).
    /// Snapshotted at open from the schema; tables without TTL pay nothing.
    ttl: HashMap<LabelId, (i64, Arc<str>)>,
}

struct StagedColumnOut<'a> {
    internal_ids: &'a mut Vec<u32>,
    vids: &'a mut Vec<VertexId>,
    tag_names: &'a mut Vec<String>,
    union_names: &'a mut Vec<std::sync::Arc<str>>,
    columns: &'a mut Vec<ColumnValues>,
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
    /// External ids aligned with `pending_ids` for fused topology-only scans.
    pending_vids: Vec<VertexId>,
    /// Index into `pending_ids`.
    pending_idx: usize,
    limit: Option<usize>,
    offset_remaining: usize,
    emitted: usize,
    id_range: Option<Range<i64>>,
    /// Owned slice of every table's shard-local id space. Set for
    /// partitioned scans; enumeration then covers roughly `1/N` of each
    /// table instead of a full walk per partition.
    slice: Option<ShardSlice>,
    projection: Option<Vec<Arc<str>>>,
    /// Topology-only scan: resolve identities with zero property decode.
    /// Set from `ScanOptions`; only valid with an empty projection and no
    /// pushed predicates (rejected in `new`).
    topology_only: bool,
    /// External key shape of the scanned space, resolved once at open.
    /// Every table in the space shares it because writes normalize ids.
    key_kind: ExternalKeyKind,
    /// Reusable per-batch and per-table-load work buffers.
    scratch: ScanScratch,
    /// Pushed conjunctive scan predicates evaluated on decoded rows.
    predicate: Vec<crate::cursor::ScanPredicate>,
    exhausted: bool,
    /// Read timestamp captured when the cursor is opened.
    ts: Timestamp,
    /// Semi-mask allowlist: decode exactly these global internal IDs
    /// instead of enumerating live IDs. Only valid with a tag filter.
    allowlist: Option<Vec<u32>>,
    /// Whether the allowlist has been loaded into `pending_ids` once.
    allowlist_loaded: bool,
    /// Staged rows composed for the current label (inserts and composed
    /// updates), drained after the label's global ids are exhausted.
    staged_rows: Vec<StagedRow>,
    /// Index into `staged_rows`.
    staged_idx: usize,
    /// Whether pending ids came from this cursor's guarded enumeration and
    /// therefore need no second row-liveness check.
    ids_verified: bool,
    /// Phase timings accumulated by the column-batch read in progress.
    timings: ScanTimings,
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
            .field("allowlist", &self.allowlist.as_ref().map(Vec::len))
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
        let key_kind = ctx
            .schema_manager()
            .get_space(&space)?
            .map(|info| ExternalKeyKind::from_vid_type(&info.vid_type))
            .unwrap_or(ExternalKeyKind::Text);
        let tag_infos = ctx.schema_manager().list_tags(&space)?;
        let mut labels = Vec::with_capacity(tag_infos.len());
        let mut names = HashMap::with_capacity(tag_infos.len());
        let mut ttl = HashMap::new();
        for info in &tag_infos {
            labels.push(info.tag_id);
            names.insert(info.tag_id, info.tag_name.clone());
            if linkrs_core::types::ttl::has_valid_ttl(info.ttl_duration, info.ttl_col.as_deref()) {
                if let (Some(duration), Some(col)) = (info.ttl_duration, info.ttl_col.as_deref()) {
                    ttl.insert(info.tag_id, (duration, Arc::from(col)));
                }
            }
        }
        let tags = TagCache { labels, names, ttl };
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
            ttl: tags.ttl,
        };

        // Internal ID spaces are per tag table: an allowlist without a tag
        // restriction has no well-defined decoding domain.
        if options.internal_id_allowlist.is_some() && options.tag.is_none() {
            return Err(StorageError::invalid_operation(
                "internal_id_allowlist requires a tag filter: internal IDs are per-table",
            ));
        }

        // A slice selects one of a non-empty division; anything else
        // addresses no well-defined owned set.
        if options.shard_slice.is_some_and(|slice| {
            slice.partition_count == 0 || slice.partition_id >= slice.partition_count
        }) {
            return Err(StorageError::invalid_operation(
                "shard_slice requires partition_id < partition_count with a non-zero count",
            ));
        }

        // Topology-only scans carry no properties and no predicates by
        // construction; anything else is a planner bug and must surface at
        // open instead of silently producing wrong rows.
        if options.topology_only {
            let projection_non_empty = options.projection.as_ref().is_some_and(|p| !p.is_empty());
            if projection_non_empty || options.predicate.as_ref().is_some_and(|p| !p.is_empty()) {
                return Err(StorageError::invalid_operation(
                    "topology_only requires an empty projection and no predicates",
                ));
            }
        }

        // A transaction with staged rows must scan even when every global
        // table is empty: its own rows only exist in staging until commit.
        // Exhaustiveness must never rely on cross-shard approximate counts:
        // a concurrent insert could be missed between shard reads and an
        // approximate zero would incorrectly skip the scan. Only a missing
        // table short-circuits; empty tables simply scan to zero rows.
        let staged_active = ctx
            .active_txn_staging()
            .is_some_and(|buffer| !buffer.lock().is_empty());
        let exhausted = match &options.internal_id_allowlist {
            Some(ids) => ids.is_empty(),
            None => {
                !staged_active
                    && ctx.data_store().with_vertex_tables(|tables| {
                        tags.labels
                            .iter()
                            .all(|label_id| tables.get(label_id).is_none())
                    })
            }
        };

        Ok(Self {
            ctx,
            space,
            tags,
            current_table_idx: 0,
            current_table: None,
            current_label: None,
            pending_ids: Vec::new(),
            pending_vids: Vec::new(),
            pending_idx: 0,
            limit: options.limit,
            offset_remaining: options.offset,
            emitted: 0,
            id_range: options.vertex_id_range.clone(),
            slice: options.shard_slice,
            projection: options
                .projection
                .as_ref()
                .map(|p| p.iter().map(|rp| rp.name.clone()).collect()),
            topology_only: options.topology_only,
            key_kind,
            scratch: ScanScratch::default(),
            predicate: options.predicate.clone().unwrap_or_default(),
            exhausted,
            ts,
            allowlist: options.internal_id_allowlist.clone(),
            allowlist_loaded: false,
            staged_rows: Vec::new(),
            staged_idx: 0,
            ids_verified: true,
            timings: ScanTimings::default(),
        })
    }

    /// Source contract for the pending ids consumed by decode.
    fn id_provenance(&self) -> ScanIdProvenance {
        if self.ids_verified {
            ScanIdProvenance::CursorVerified
        } else {
            ScanIdProvenance::Unverified
        }
    }

    /// TTL config of one scanned table, if the schema gives it a valid TTL.
    /// Tables without TTL skip every expiry check below at zero decode cost.
    fn table_ttl(&self, label: LabelId) -> Option<(i64, Arc<str>)> {
        self.tags.ttl.get(&label).cloned()
    }

    /// Whether one property value is past its TTL at the scan instant.
    /// A missing column never counts as expired, matching the
    /// materializing read paths row for row.
    fn row_expired(
        ttl: &Option<(i64, Arc<str>)>,
        properties: &[(Arc<str>, Value)],
        now: i64,
    ) -> bool {
        let Some((duration, col)) = ttl.as_ref() else {
            return false;
        };
        linkrs_core::types::ttl::is_expired(
            *duration,
            properties
                .iter()
                .find(|(name, _)| name.as_ref() == col.as_ref())
                .map(|(_, value)| value),
            now,
        )
    }

    /// Load the next non-empty table's snapshot-visible live ids into
    /// `pending_ids`, advancing through tables until one has ids. Sets
    /// `exhausted` when no table remains.
    ///
    /// In allowlist mode the first existing tagged table is loaded once with
    /// the allowlist as its pending IDs; later calls exhaust the cursor.
    /// Point-lookup batch decoding skips invalid IDs, so no pre-filtering
    /// happens here.
    fn load_next_table(
        &mut self,
        tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        guard: &crate::mvcc_visibility::VisibilityGuard<'_>,
        scratch: &mut ScanScratch,
    ) {
        self.current_table = None;
        self.current_label = None;
        self.pending_ids.clear();
        self.pending_vids.clear();
        self.pending_idx = 0;
        self.staged_rows.clear();
        self.staged_idx = 0;
        if self.allowlist_loaded || self.allowlist.is_some() {
            if self.allowlist_loaded {
                self.exhausted = true;
                return;
            }
            self.allowlist_loaded = true;
            let ids = self.allowlist.take().unwrap_or_default();
            for label_id in &self.tags.labels {
                let Some(table) = tables.get(label_id) else {
                    continue;
                };
                // Allowlist decoding addresses explicit internal ids, so
                // staged inserts (which have none) cannot join it; staged
                // deletes still veto their resolved ids.
                let merge = self.ctx.staged_scan_merge(*label_id, table, guard);
                let owned: Option<(&ShardSlice, Vec<u32>)> = self
                    .slice
                    .as_ref()
                    .map(|slice| (slice, table.shard_universes()));
                self.pending_ids.clear();
                self.pending_ids.extend(
                    ids.iter()
                        .filter(|id| !merge.dropped_ids.contains(id))
                        .filter(|id| match owned.as_ref() {
                            None => true,
                            Some((slice, universes)) => table.slice_owns_id(**id, slice, universes),
                        })
                        .copied(),
                );
                self.pending_vids.clear();
                self.ids_verified = false;
                self.current_label = Some(*label_id);
                self.current_table = Some(Arc::clone(table));
                return;
            }
            self.exhausted = true;
            return;
        }
        while self.current_table_idx < self.tags.labels.len() {
            let label_id = self.tags.labels[self.current_table_idx];
            self.current_table_idx += 1;
            let Some(table) = tables.get(&label_id) else {
                continue;
            };
            let merge = self.ctx.staged_scan_merge(label_id, table, guard);
            // Sliced enumeration covers roughly `1/N` of the table: the
            // walk skips foreign chunks before any visibility or identity
            // work, so sibling slices share nothing but the table. Staged
            // ids decode to their routing shard exactly like base ids, so
            // the same ownership rule keeps every staged row on one slice.
            let owned: Option<(&ShardSlice, Vec<u32>)> = self
                .slice
                .as_ref()
                .map(|slice| (slice, table.shard_universes()));
            let fuse_identity =
                self.topology_only && self.table_ttl(label_id).is_none() && owned.is_some();
            match owned.as_ref() {
                Some((slice, universes)) if fuse_identity => table.live_ids_in_slice_with_vids(
                    guard,
                    slice,
                    universes,
                    self.key_kind,
                    scratch,
                    &mut self.pending_ids,
                    &mut self.pending_vids,
                    &mut self.timings,
                ),
                Some((slice, universes)) => table.live_ids_in_slice(
                    guard,
                    slice,
                    universes,
                    scratch,
                    &mut self.pending_ids,
                    &mut self.timings,
                ),
                None => table.live_ids(guard, &mut self.pending_ids),
            }
            if !merge.dropped_ids.is_empty() {
                let fused = self.pending_vids.len() == self.pending_ids.len();
                let mut write = 0;
                for read in 0..self.pending_ids.len() {
                    let id = self.pending_ids[read];
                    if merge.dropped_ids.contains(&id) {
                        continue;
                    }
                    self.pending_ids[write] = id;
                    if fused {
                        self.pending_vids[write] = self.pending_vids[read];
                    }
                    write += 1;
                }
                self.pending_ids.truncate(write);
                if fused {
                    self.pending_vids.truncate(write);
                } else {
                    self.pending_vids.clear();
                }
            }
            self.staged_rows = merge.rows;
            if let Some((slice, universes)) = owned.as_ref() {
                self.staged_rows
                    .retain(|row| table.slice_owns_id(row.id, slice, universes));
            }
            if !self.pending_ids.is_empty() || !self.staged_rows.is_empty() {
                self.current_label = Some(label_id);
                self.ids_verified = true;
                self.current_table = Some(Arc::clone(table));
                return;
            }
        }
        self.exhausted = true;
    }

    /// Drain staged rows of the current label into a column batch. Each
    /// surviving row contributes its composed properties as general column
    /// values; the external-id range and offset skipping mirror the
    /// global-run filters, and pushed predicates are applied later over the
    /// union columns like every other run.
    fn drain_staged_columns(
        &mut self,
        batch_size: usize,
        tag_name: &str,
        out: StagedColumnOut<'_>,
    ) {
        let StagedColumnOut {
            internal_ids,
            vids,
            tag_names,
            union_names,
            columns,
        } = out;
        let end = (self.staged_idx + (batch_size - internal_ids.len())).min(self.staged_rows.len());
        let staged_ttl = self.current_label.and_then(|label| self.table_ttl(label));
        let now = linkrs_core::types::ttl::now_secs();
        let mut rows_sel: Vec<usize> = Vec::new();
        for index in self.staged_idx..end {
            let vid = self.staged_rows[index].vid;
            if let Some(ref range) = self.id_range {
                match vid.as_int64() {
                    Some(v) if (range.start..range.end).contains(&v) => {}
                    _ => continue,
                }
            }
            // Expired staged rows are absent: skip before offset
            // accounting, exactly like the base-table paths.
            if Self::row_expired(&staged_ttl, &self.staged_rows[index].properties, now) {
                continue;
            }
            if self.offset_remaining > 0 {
                self.offset_remaining -= 1;
                continue;
            }
            rows_sel.push(index);
        }
        self.staged_idx = end;
        if rows_sel.is_empty() {
            return;
        }
        // Topology-only drain: staged properties live in memory already, so
        // skipping their collection is pure savings with identical rows.
        if self.topology_only {
            let run_rows = rows_sel.len();
            internal_ids.extend(rows_sel.iter().map(|&index| self.staged_rows[index].id));
            vids.extend(rows_sel.iter().map(|&index| self.staged_rows[index].vid));
            tag_names.extend(std::iter::repeat_n(tag_name.to_string(), run_rows));
            return;
        }
        let mut decoded_names: Vec<std::sync::Arc<str>> = Vec::new();
        for &index in &rows_sel {
            for (name, _) in &self.staged_rows[index].properties {
                if !decoded_names.contains(name) {
                    decoded_names.push(name.clone());
                }
            }
        }
        let decoded: Vec<(std::sync::Arc<str>, ColumnValues)> = decoded_names
            .into_iter()
            .map(|name| {
                let values: Vec<Option<Value>> = rows_sel
                    .iter()
                    .map(|&index| {
                        self.staged_rows[index]
                            .properties
                            .iter()
                            .find(|(existing, _)| *existing == name)
                            .map(|(_, value)| value.clone())
                    })
                    .collect();
                (name, ColumnValues::General(values))
            })
            .collect();

        // Merge the staged run into the batch's column union, exactly like a
        // decoded global run.
        let run_rows = rows_sel.len();
        let before = internal_ids.len();
        for (name, _) in &decoded {
            if !union_names.contains(name) {
                union_names.push(name.clone());
                let mut new_column = ColumnValues::General(Vec::new());
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
        internal_ids.extend(rows_sel.iter().map(|&index| self.staged_rows[index].id));
        vids.extend(rows_sel.iter().map(|&index| self.staged_rows[index].vid));
        tag_names.extend(std::iter::repeat_n(tag_name.to_string(), run_rows));
    }
}

impl VertexCursor for GraphVertexCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Vertex>, StorageError> {
        self.scan_batch(batch_size, |vid, internal_id, tag_name, props| {
            let _ = internal_id;
            let props_map: HashMap<std::sync::Arc<str>, Value> = props.into_iter().collect();
            Vertex::new(vid, Tag::new(tag_name, props_map))
        })
    }

    fn next_flat_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<Vec<FlatVertexRecord>, StorageError> {
        let label_id = self.current_label;
        self.scan_batch(batch_size, |vid, internal_id, tag_name, props| {
            FlatVertexRecord {
                vid,
                internal_id,
                label_id,
                tag_name,
                props,
            }
        })
    }

    fn next_column_batch(
        &mut self,
        prop_names: &[std::sync::Arc<str>],
        batch_size: usize,
    ) -> Result<crate::cursor::VertexColumnBatch, StorageError> {
        if self.exhausted || self.tags.labels.is_empty() {
            return Ok(crate::cursor::VertexColumnBatch::empty());
        }
        let batch_size = batch_size.max(1);
        self.timings = ScanTimings::default();
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
        prop_names: &[std::sync::Arc<str>],
        batch_size: usize,
    ) -> Result<crate::cursor::VertexColumnBatch, StorageError> {
        let data_store = self.ctx.data_store().clone();
        let names = self.tags.names.clone();
        let (gate_vm, gate_own) = self.ctx.gate_inputs();
        let ts = self.ts;
        let now = linkrs_core::types::ttl::now_secs();
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.reset();
        let result = data_store.with_vertex_tables(|tables| {
            let guard = crate::mvcc_visibility::VisibilityGuard::new(
                ts,
                crate::mvcc_visibility::PendingGate::new(&gate_vm, gate_own),
            );
            let mut vids: Vec<VertexId> = Vec::new();
            let mut internal_ids: Vec<u32> = Vec::new();
            let mut label_ids: Vec<Option<linkrs_core::types::LabelId>> = Vec::new();
            let mut tag_names: Vec<String> = Vec::new();
            // Union of decoded column names, grown as tables are processed.
            let mut union_names: Vec<std::sync::Arc<str>> = Vec::new();
            let mut columns: Vec<crate::cursor::ColumnValues> = Vec::new();

            while internal_ids.len() < batch_size && !self.exhausted {
                if self.current_table.is_none() {
                    let load_start = std::time::Instant::now();
                    let merge_before = self.timings.scan_merge_us;
                    self.load_next_table(tables, &guard, &mut scratch);
                    let load_us = load_start.elapsed().as_micros() as u64;
                    let merge_us = self.timings.scan_merge_us - merge_before;
                    self.timings
                        .record_table_load(load_us.saturating_sub(merge_us));
                    continue;
                }
                if self.pending_idx >= self.pending_ids.len() {
                    if self.staged_idx < self.staged_rows.len() {
                        let tag_name = self
                            .current_label
                            .and_then(|l| names.get(&l))
                            .map(|s| s.as_str())
                            .unwrap_or("unknown");
                        self.drain_staged_columns(
                            batch_size,
                            tag_name,
                            StagedColumnOut {
                                internal_ids: &mut internal_ids,
                                vids: &mut vids,
                                tag_names: &mut tag_names,
                                union_names: &mut union_names,
                                columns: &mut columns,
                            },
                        );
                        continue;
                    }
                    self.current_table = None;
                    self.current_label = None;
                    self.pending_ids.clear();
                    self.pending_vids.clear();
                    self.pending_idx = 0;
                    self.staged_rows.clear();
                    self.staged_idx = 0;
                    continue;
                }

                let provenance = self.id_provenance();
                let start = self.pending_idx;
                let end = (start + (batch_size - internal_ids.len())).min(self.pending_ids.len());
                let window = &self.pending_ids[start..end];
                let resolved_window: Option<&[VertexId]> =
                    if self.pending_vids.len() == self.pending_ids.len() {
                        Some(&self.pending_vids[start..end])
                    } else {
                        None
                    };
                self.pending_idx = end;

                let Some(table) = self.current_table.clone() else {
                    continue;
                };
                // Partition-range pre-filter before zone pruning and decode
                // (same rationale as the row path): a fragment keeps roughly
                // 1/P of the window, and decoding the rest only to drop it
                // repeats every sibling fragment's work.
                let window_start = std::time::Instant::now();
                let (ids, resolved_vids): (&[u32], Option<&[VertexId]>) =
                    match self.id_range.as_ref() {
                        Some(range) => {
                            scratch.range_ids.clear();
                            scratch.range_vids.clear();
                            if let Some(vids) = resolved_window {
                                for (&id, &vid) in window.iter().zip(vids.iter()) {
                                    if vid
                                        .as_int64()
                                        .is_some_and(|v| (range.start..range.end).contains(&v))
                                    {
                                        scratch.range_ids.push(id);
                                        scratch.range_vids.push(vid);
                                    }
                                }
                            } else {
                                table.scan_topology(
                                    window,
                                    &guard,
                                    provenance,
                                    &mut self.timings,
                                    &mut scratch.topology_ids,
                                    &mut scratch.topology_vids,
                                );
                                for (&id, &vid) in scratch
                                    .topology_ids
                                    .iter()
                                    .zip(scratch.topology_vids.iter())
                                {
                                    if vid
                                        .as_int64()
                                        .is_some_and(|v| (range.start..range.end).contains(&v))
                                    {
                                        scratch.range_ids.push(id);
                                        scratch.range_vids.push(vid);
                                    }
                                }
                            }
                            (&scratch.range_ids[..], Some(&scratch.range_vids[..]))
                        }
                        None => (window, resolved_window),
                    };
                self.timings
                    .record_window_filter(window_start.elapsed().as_micros() as u64);
                let label_id = self.current_label;
                let tag_name = label_id
                    .and_then(|l| names.get(&l))
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");

                let run_ttl = label_id.and_then(|label| self.table_ttl(label));
                // Decode names for this table run.  A full-row decode (empty
                // projection) decodes every column of the table; otherwise the
                // projection plus any pushed-predicate columns.  A TTL table
                // always decodes its TTL column on top so expiry is
                // decidable, even on the topology path that otherwise
                // skips decode entirely.
                scratch.run_names.clear();
                if !(prop_names.is_empty() && !self.topology_only) {
                    scratch.run_names.extend_from_slice(prop_names);
                    for predicate in &self.predicate {
                        let column: std::sync::Arc<str> = predicate.column().into();
                        if !scratch.run_names.contains(&column) {
                            scratch.run_names.push(column);
                        }
                    }
                }
                let mut ttl_added = false;
                if let Some((_, col)) = run_ttl.as_ref() {
                    if !scratch.run_names.iter().any(|name| name == col) {
                        scratch.run_names.push(col.clone());
                        ttl_added = true;
                    }
                }

                // Zone-map pruning over the candidate window: rows dropped
                // here are exactly those the pushed predicates would reject
                // after decoding, so skipping their decode is a pure
                // optimization with identical results.
                scratch.candidate_ids.clear();
                if self.predicate.is_empty() {
                    scratch.candidate_ids.extend_from_slice(ids);
                } else {
                    let predicate_start = std::time::Instant::now();
                    let ranges = crate::cursor::ScanPredicate::merged_ranges(&self.predicate);
                    let mask = table.zone_prune_mask(ids, &ranges);
                    scratch.candidate_ids.extend(
                        ids.iter()
                            .zip(mask.iter())
                            .filter_map(|(&id, &keep)| keep.then_some(id)),
                    );
                    record_elapsed(predicate_start, &mut self.timings.scan_predicate_us);
                }
                let ids = &scratch.candidate_ids[..];

                // Topology-only run: resolve identities with zero property
                // decode. Predicates are empty by construction (rejected in
                // `new`), so no predicate filtering applies here. A TTL
                // table cannot take this shortcut: it falls through to the
                // decode path, filters on the TTL column, and strips the
                // helper column back out below.
                if self.topology_only && run_ttl.is_none() {
                    let (run_internal, run_vids): (&[u32], &[VertexId]) =
                        if let Some(window) = resolved_vids {
                            (ids, window)
                        } else {
                            table.scan_topology(
                                ids,
                                &guard,
                                provenance,
                                &mut self.timings,
                                &mut scratch.run_internal,
                                &mut scratch.run_vids,
                            );
                            (&scratch.run_internal[..], &scratch.run_vids[..])
                        };
                    if run_internal.is_empty() {
                        continue;
                    }
                    let mut kept = 0usize;
                    for (row, vid) in run_vids.iter().enumerate() {
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
                        internal_ids.push(run_internal[row]);
                        vids.push(*vid);
                        kept += 1;
                    }
                    label_ids.extend(std::iter::repeat_n(self.current_label, kept));
                    tag_names.extend(std::iter::repeat_n(tag_name.to_string(), kept));
                    continue;
                }

                // Guarded decode: a property covered by a foreign uncommitted
                // write is read at the version below it, so the scan never
                // yields an uncommitted value. Rows with no visible version are
                // dropped and the columns stay aligned with the surviving ids.
                let mut decoded = table.scan_columns(
                    ids,
                    &guard,
                    &scratch.run_names,
                    provenance,
                    &mut self.timings,
                    &mut scratch.run_internal,
                    &mut scratch.run_vids,
                );
                if scratch.run_internal.is_empty() {
                    continue;
                }

                // The external-id range and offset skipping are applied to the
                // surviving rows, in scan order.
                scratch.selection.clear();
                for (row, vid) in scratch.run_vids.iter().enumerate() {
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
                    scratch.selection.push(row);
                }
                select_run(
                    &mut decoded,
                    &mut scratch.run_internal,
                    &mut scratch.run_vids,
                    &scratch.selection,
                );
                let mut run_rows = scratch.run_internal.len();
                if run_rows == 0 {
                    continue;
                }

                // Expired rows are absent: drop them before the merge so
                // they never consume offset/limit downstream. A TTL column
                // that was never written keeps every row, matching the
                // materializing reads.
                if let Some((duration, _)) = run_ttl.as_ref() {
                    if let Some(ttl_idx) = decoded
                        .iter()
                        .position(|(name, _)| Some(name) == run_ttl.as_ref().map(|(_, col)| col))
                    {
                        scratch.selection.clear();
                        for row in 0..run_rows {
                            let expired = decoded[ttl_idx].1.value_at(row).is_some_and(|value| {
                                linkrs_core::types::ttl::is_expired(*duration, Some(&value), now)
                            });
                            if !expired {
                                scratch.selection.push(row);
                            }
                        }
                        select_run(
                            &mut decoded,
                            &mut scratch.run_internal,
                            &mut scratch.run_vids,
                            &scratch.selection,
                        );
                    }
                }
                // The helper TTL column leaves the run again unless this
                // is a genuine full decode: topology output stays
                // column-free and projections stay exact.
                if ttl_added && !(prop_names.is_empty() && !self.topology_only) {
                    if let Some((_, col)) = run_ttl.as_ref() {
                        if let Some(idx) = decoded.iter().position(|(name, _)| name == col) {
                            decoded.remove(idx);
                        }
                    }
                }
                run_rows = scratch.run_internal.len();
                if run_rows == 0 {
                    continue;
                }

                // Merge the run into the batch's column union.
                let assemble_start = std::time::Instant::now();
                let before = internal_ids.len();
                for (name, _) in &decoded {
                    if !union_names.contains(name) {
                        union_names.push(name.clone());
                        let mut new_column = crate::cursor::ColumnValues::General(Vec::new());
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

                internal_ids.extend(scratch.run_internal.iter().copied());
                vids.extend(scratch.run_vids.iter().copied());
                label_ids.extend(std::iter::repeat_n(self.current_label, run_rows));
                tag_names.extend(std::iter::repeat_n(tag_name.to_string(), run_rows));
                record_elapsed(assemble_start, &mut self.timings.scan_assemble_us);
            }

            (
                internal_ids,
                label_ids,
                vids,
                tag_names,
                union_names,
                columns,
                self.exhausted,
            )
        });

        let (internal_ids, label_ids, vids, tag_names, union_names, mut columns, _exhausted) =
            result;

        // Apply pushed predicates over the decoded columns. Filtering
        // produces a selection vector (index sequence, not a boolean mask)
        // and surviving rows are gathered via that selection.
        let predicate_start = std::time::Instant::now();
        let (final_ids, final_labels, final_vids, final_tags) =
            if self.predicate.is_empty() {
                (internal_ids, label_ids, vids, tag_names)
            } else {
                scratch.keep.clear();
                scratch.keep.resize(internal_ids.len(), true);
                for predicate in &self.predicate {
                    match union_names
                        .iter()
                        .position(|n| *n.as_ref() == *predicate.column())
                    {
                        Some(index) => {
                            let column = &columns[index];
                            for (row, ok) in scratch.keep.iter_mut().enumerate() {
                                if *ok && !predicate.matches_column(column, row) {
                                    *ok = false;
                                }
                            }
                        }
                        None => scratch.keep.fill(false),
                    }
                }
                if scratch.keep.iter().any(|&k| !k) {
                    scratch.selection.clear();
                    scratch.selection.extend(
                        scratch
                            .keep
                            .iter()
                            .enumerate()
                            .filter_map(|(i, &k)| if k { Some(i) } else { None }),
                    );
                    let mut kept_ids = Vec::with_capacity(scratch.selection.len());
                    let mut kept_labels = Vec::with_capacity(scratch.selection.len());
                    let mut kept_vids = Vec::with_capacity(scratch.selection.len());
                    let mut kept_tags = Vec::with_capacity(scratch.selection.len());
                    for &row in &scratch.selection {
                        kept_ids.push(internal_ids[row]);
                        kept_labels.push(label_ids[row]);
                        kept_vids.push(vids[row]);
                        kept_tags.push(tag_names[row].clone());
                    }
                    for column in columns.iter_mut() {
                        column.select(&scratch.selection);
                    }
                    (kept_ids, kept_labels, kept_vids, kept_tags)
                } else {
                    (internal_ids, label_ids, vids, tag_names)
                }
            };
        record_elapsed(predicate_start, &mut self.timings.scan_predicate_us);

        let assemble_start = std::time::Instant::now();
        let mut batch = Self::assemble_column_batch(
            prop_names,
            union_names,
            columns,
            final_ids,
            final_labels,
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

        record_elapsed(assemble_start, &mut self.timings.scan_assemble_us);
        batch.timings = self.timings;
        self.scratch = scratch;

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
        mut build: impl FnMut(VertexId, i64, String, Vec<(std::sync::Arc<str>, Value)>) -> T,
    ) -> Result<Vec<T>, StorageError> {
        if self.exhausted || self.tags.labels.is_empty() {
            return Ok(Vec::new());
        }

        let batch_size = batch_size.max(1);
        let data_store = self.ctx.data_store().clone();
        let names = self.tags.names.clone();
        let (gate_vm, gate_own) = self.ctx.gate_inputs();
        let ts = self.ts;
        let now = linkrs_core::types::ttl::now_secs();
        let mut scratch = std::mem::take(&mut self.scratch);
        let batch = data_store.with_vertex_tables(|tables| -> Result<Vec<T>, StorageError> {
            let guard = crate::mvcc_visibility::VisibilityGuard::new(
                ts,
                crate::mvcc_visibility::PendingGate::new(&gate_vm, gate_own),
            );
            let mut batch = Vec::new();

            while batch.len() < batch_size && !self.exhausted {
                if self.current_table.is_none() {
                    self.load_next_table(tables, &guard, &mut scratch);
                    continue;
                }
                if self.pending_idx >= self.pending_ids.len() {
                    if self.staged_idx < self.staged_rows.len() {
                        let end = (self.staged_idx + (batch_size - batch.len()))
                            .min(self.staged_rows.len());
                        let tag_name = self
                            .current_label
                            .and_then(|l| names.get(&l))
                            .map(|s| s.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let staged_ttl = self.current_label.and_then(|label| self.table_ttl(label));
                        while self.staged_idx < end && !self.exhausted {
                            let index = self.staged_idx;
                            self.staged_idx += 1;
                            let vid = self.staged_rows[index].vid;
                            let internal_id = self.staged_rows[index].id;
                            let mut properties =
                                std::mem::take(&mut self.staged_rows[index].properties);
                            // Expired staged rows are absent: skip before
                            // the projection trims the TTL column away and
                            // before offset/limit accounting.
                            if Self::row_expired(&staged_ttl, &properties, now) {
                                continue;
                            }
                            if let Some(names) = self.projection.as_deref() {
                                properties.retain(|(name, _)| names.iter().any(|n| n == name));
                            }
                            if let Some(ref range) = self.id_range {
                                match vid.as_int64() {
                                    Some(vid) if (range.start..range.end).contains(&vid) => {}
                                    _ => continue,
                                }
                            }
                            if !self.predicate.is_empty()
                                && !self.predicate.iter().all(|p| p.matches(&properties))
                            {
                                continue;
                            }
                            if self.offset_remaining > 0 {
                                self.offset_remaining -= 1;
                                continue;
                            }
                            batch.push(build(
                                vid,
                                internal_id as i64,
                                tag_name.clone(),
                                properties,
                            ));
                            self.emitted += 1;
                            if let Some(limit) = self.limit {
                                if self.emitted >= limit {
                                    self.exhausted = true;
                                    break;
                                }
                            }
                        }
                        continue;
                    }
                    self.current_table = None;
                    self.current_label = None;
                    self.pending_ids.clear();
                    self.pending_vids.clear();
                    self.pending_idx = 0;
                    self.staged_rows.clear();
                    self.staged_idx = 0;
                    continue;
                }

                let end =
                    (self.pending_idx + (batch_size - batch.len())).min(self.pending_ids.len());
                let window = &self.pending_ids[self.pending_idx..end];
                self.pending_idx = end;

                let Some(table) = self.current_table.clone() else {
                    continue;
                };
                // Partition-range pre-filter before property decode: identity
                // resolution is far cheaper than versioned column reads, and
                // a partitioned fragment keeps roughly 1/P of the window, so
                // decoding the full window would repeat every sibling
                // fragment's decode work. Order-preserving, same guard, so
                // the decoded set is identical to decode-then-filter.
                let provenance = self.id_provenance();
                let ranged: Vec<u32>;
                let ids: &[u32] = match self.id_range.as_ref() {
                    Some(range) => {
                        let mut topo_ids = Vec::new();
                        let mut topo_vids = Vec::new();
                        table.scan_topology(
                            window,
                            &guard,
                            provenance,
                            &mut self.timings,
                            &mut topo_ids,
                            &mut topo_vids,
                        );
                        ranged = topo_ids
                            .into_iter()
                            .zip(topo_vids)
                            .filter_map(|(id, vid)| {
                                vid.as_int64()
                                    .filter(|v| (range.start..range.end).contains(v))
                                    .map(|_| id)
                            })
                            .collect();
                        &ranged
                    }
                    None => window,
                };
                let label_id = self.current_label;
                let base_ttl = label_id.and_then(|label| self.table_ttl(label));
                // A TTL table decodes its TTL column on top of the
                // projection so expiry is decidable; the column is
                // trimmed back below when the projection did not ask
                // for it.
                let (fetch_projection, added_ttl) = match (&self.projection, base_ttl.as_ref()) {
                    (Some(names), Some((_, col))) if !names.iter().any(|name| name == col) => {
                        let mut extended = names.clone();
                        extended.push(col.clone());
                        (Some(extended), true)
                    }
                    _ => (self.projection.clone(), false),
                };
                let records =
                    table.resolve_projected_batch(ids, &guard, fetch_projection.as_deref())?;
                let tag_name = label_id
                    .and_then(|l| names.get(&l))
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");

                for mut record in records.into_iter().flatten() {
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
                    // Expired rows are absent: skip before offset/limit
                    // accounting, then drop the helper TTL column again.
                    if Self::row_expired(&base_ttl, &record.properties, now) {
                        continue;
                    }
                    if added_ttl {
                        if let Some(names) = self.projection.as_deref() {
                            record
                                .properties
                                .retain(|(name, _)| names.iter().any(|n| n == name));
                        }
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
            Ok(batch)
        })?;
        self.scratch = scratch;

        Ok(batch)
    }

    /// Assemble the final [`VertexColumnBatch`] from the decoded union columns.
    ///
    /// When `prop_names` is non-empty only those columns are returned (in
    /// projection order, missing columns as all-null); an empty `prop_names`
    /// returns every decoded column.
    fn assemble_column_batch(
        prop_names: &[std::sync::Arc<str>],
        union_names: Vec<std::sync::Arc<str>>,
        columns: Vec<crate::cursor::ColumnValues>,
        internal_ids: Vec<u32>,
        label_ids: Vec<Option<linkrs_core::types::LabelId>>,
        vids: Vec<VertexId>,
        tag_names: Vec<String>,
    ) -> crate::cursor::VertexColumnBatch {
        let output_columns: Vec<crate::cursor::PropertyColumn> = if prop_names.is_empty() {
            union_names
                .into_iter()
                .zip(columns)
                .map(|(name, values)| crate::cursor::PropertyColumn {
                    name,
                    data_type: linkrs_core::types::DataType::Empty,
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
                        .position(|n| n.as_ref() == name.as_ref())
                        .and_then(|index| columns.get(index).cloned())
                        .unwrap_or_else(|| {
                            crate::cursor::ColumnValues::General(vec![None; row_count])
                        });
                    crate::cursor::PropertyColumn {
                        name: name.clone(),
                        data_type: linkrs_core::types::DataType::Empty,
                        values,
                    }
                })
                .collect()
        };
        crate::cursor::VertexColumnBatch {
            vids,
            internal_ids: internal_ids.into_iter().map(|id| id as i64).collect(),
            label_ids,
            tag_names,
            columns: output_columns,
            timings: ScanTimings::default(),
        }
    }
}
