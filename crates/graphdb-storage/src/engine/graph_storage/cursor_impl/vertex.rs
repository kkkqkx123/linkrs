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
                    let ranges = crate::cursor::ScanPredicate::merged_ranges(&self.predicate);
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
