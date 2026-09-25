use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use crate::cursor::{FlatVertexRecord, ScanOptions, VertexCursor};
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::vertex::ShardedVertexTable;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::{StorageError, StorageResult, Value, Vertex};

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
    /// Semi-mask allowlist: decode exactly these global internal IDs
    /// instead of enumerating live IDs. Only valid with a tag filter.
    allowlist: Option<Vec<u32>>,
    /// Whether the allowlist has been loaded into `pending_ids` once.
    allowlist_loaded: bool,
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

        // Internal ID spaces are per tag table: an allowlist without a tag
        // restriction has no well-defined decoding domain.
        if options.internal_id_allowlist.is_some() && options.tag.is_none() {
            return Err(StorageError::invalid_operation(
                "internal_id_allowlist requires a tag filter: internal IDs are per-table",
            ));
        }

        let exhausted = match &options.internal_id_allowlist {
            Some(ids) => ids.is_empty(),
            None => ctx.data_store().with_vertex_tables(|tables| {
                tags.labels.iter().all(|label_id| {
                    tables
                        .get(label_id)
                        .is_none_or(|t| t.approximate_id_hole_stats(ts).0 == 0)
                })
            }),
        };

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
            allowlist: options.internal_id_allowlist.clone(),
            allowlist_loaded: false,
        })
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
    ) {
        self.current_table = None;
        self.current_label = None;
        self.pending_ids.clear();
        self.pending_idx = 0;
        if let Some(ids) = self.allowlist.clone() {
            if self.allowlist_loaded {
                self.exhausted = true;
                return;
            }
            self.allowlist_loaded = true;
            for label_id in &self.tags.labels {
                if let Some(table) = tables.get(label_id) {
                    self.current_label = Some(*label_id);
                    self.pending_ids = ids;
                    self.current_table = Some(Arc::clone(table));
                    return;
                }
            }
            self.exhausted = true;
            return;
        }
        while self.current_table_idx < self.tags.labels.len() {
            let label_id = self.tags.labels[self.current_table_idx];
            self.current_table_idx += 1;
            if let Some(table) = tables.get(&label_id) {
                let ids = table.live_ids(guard);
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
            let _ = internal_id;
            let props_map: HashMap<String, Value> = props.into_iter().collect();
            Vertex::new(vid, Tag::new(tag_name, props_map))
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
        let (gate_vm, gate_own) = self.ctx.gate_inputs();
        let ts = self.ts;
        let result = data_store.with_vertex_tables(|tables| {
            let guard = crate::mvcc_visibility::VisibilityGuard::new(
                ts,
                crate::mvcc_visibility::PendingGate::new(&gate_vm, gate_own),
            );
            let mut vids: Vec<VertexId> = Vec::new();
            let mut internal_ids: Vec<u32> = Vec::new();
            let mut tag_names: Vec<String> = Vec::new();
            // Union of decoded column names, grown as tables are processed.
            let mut union_names: Vec<String> = Vec::new();
            let mut columns: Vec<crate::cursor::ColumnValues> = Vec::new();

            while internal_ids.len() < batch_size && !self.exhausted {
                if self.current_table.is_none() {
                    self.load_next_table(tables, &guard);
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

                // Zone-map pruning over the candidate window: rows dropped
                // here are exactly those the pushed predicates would reject
                // after decoding, so skipping their decode is a pure
                // optimization with identical results.
                let candidates: Vec<u32> = if self.predicate.is_empty() {
                    ids.to_vec()
                } else {
                    let ranges = crate::cursor::ScanPredicate::merged_ranges(&self.predicate);
                    let mask = table.zone_prune_mask(ids, &ranges);
                    ids.iter()
                        .zip(mask.iter())
                        .filter_map(|(&id, &keep)| keep.then_some(id))
                        .collect()
                };

                // Guarded decode: a property covered by a foreign uncommitted
                // write is read at the version below it, so the scan never
                // yields an uncommitted value. Rows with no visible version are
                // dropped and the columns stay aligned with the surviving ids.
                let (mut run_internal, mut run_vids, mut decoded) =
                    table.scan_columns(&candidates, &guard, &run_names);
                if run_internal.is_empty() {
                    continue;
                }

                // The external-id range and offset skipping are applied to the
                // surviving rows, in scan order.
                let mut selection: Vec<usize> = Vec::new();
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
                    selection.push(row);
                }
                if selection.len() != run_internal.len() {
                    for (_, column) in decoded.iter_mut() {
                        column.select(&selection);
                    }
                    let pruned_ids = std::mem::take(&mut run_internal);
                    let pruned_vids = std::mem::take(&mut run_vids);
                    run_internal = selection.iter().map(|&row| pruned_ids[row]).collect();
                    run_vids = selection.iter().map(|&row| pruned_vids[row]).collect();
                }
                let run_rows = run_internal.len();
                if run_rows == 0 {
                    continue;
                }

                // Merge the run into the batch's column union.
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

        // Apply pushed predicates over the decoded columns. Filtering
        // produces a selection vector (index sequence, not a boolean mask)
        // and surviving rows are gathered via that selection.
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
                let selection: Vec<usize> = keep
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &k)| if k { Some(i) } else { None })
                    .collect();
                let mut kept_ids = Vec::with_capacity(selection.len());
                let mut kept_vids = Vec::with_capacity(selection.len());
                let mut kept_tags = Vec::with_capacity(selection.len());
                for &row in &selection {
                    kept_ids.push(internal_ids[row]);
                    kept_vids.push(vids[row]);
                    kept_tags.push(tag_names[row].clone());
                }
                for column in columns.iter_mut() {
                    column.select(&selection);
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
        let (gate_vm, gate_own) = self.ctx.gate_inputs();
        let ts = self.ts;
        let batch = data_store.with_vertex_tables(|tables| {
            let guard = crate::mvcc_visibility::VisibilityGuard::new(
                ts,
                crate::mvcc_visibility::PendingGate::new(&gate_vm, gate_own),
            );
            let mut batch = Vec::new();

            while batch.len() < batch_size && !self.exhausted {
                if self.current_table.is_none() {
                    self.load_next_table(tables, &guard);
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
                let records =
                    table.resolve_projected_batch(ids, &guard, self.projection.as_deref());
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
                    data_type: graphdb_core::types::DataType::Empty,
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
                        data_type: graphdb_core::types::DataType::Empty,
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
