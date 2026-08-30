use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::cold::ColdSnapshot;
use crate::cursor::{EdgeCursor, ScanOptions};
use crate::edge::Nbr;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::cursor_impl::edge::{
    make_vid, resolve_vertex_id, GraphEdgeCursor,
};
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, StorageError, StorageResult, Value};

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

            let rank = nbr.rank;
            let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
            let dst_internal = nbr.endpoint;
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
