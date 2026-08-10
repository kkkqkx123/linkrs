use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::query_registry::CancelToken;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::traversal::config::TraversalConfig;
use crate::query::executor::traversal::graph_reader::TraversalGraphReader;
use crate::query::executor::traversal::runtime::TraversalRuntime;
use crate::storage::QueryStorage;

use super::super::visited_set::VisitedSet;
use super::ExpandCtx;

/// Reusable buffer for building expand output rows without repeated allocation.
///
/// For each seed row, the buffer clones the seed row into `row_buf`, pushes
/// the edge and destination vertex values, then takes ownership of the
/// completed row via `std::mem::take`. This avoids per-row `Vec::clone`
/// allocation overhead.
struct ExpandOutputBuffer {
    row_buf: Vec<Value>,
    rows: Vec<Vec<Value>>,
}

impl ExpandOutputBuffer {
    fn new(seed_width: usize, capacity: usize) -> Self {
        Self {
            row_buf: Vec::with_capacity(seed_width + 2),
            rows: Vec::with_capacity(capacity),
        }
    }

    #[inline]
    fn push_row(&mut self, seed_row: &[Value], edge: Value, dst: Value) {
        self.row_buf.clear();
        self.row_buf.extend_from_slice(seed_row);
        self.row_buf.push(edge);
        self.row_buf.push(dst);
        let row = std::mem::take(&mut self.row_buf);
        self.rows.push(row);
    }

    fn finish(self) -> Vec<Vec<Value>> {
        self.rows
    }
}

/// Iterator over the visible rows of a chunk.
///
/// When a selection vector is attached (P2), only the selected rows are
/// yielded, preserving the absolute upstream row order. The output carries
/// `(row_index, &row)` so consumers that need the absolute index (e.g. for
/// `get_variable` on a per-row basis) keep working identically.
pub(super) struct VisibleRows<'a> {
    chunk: &'a DataChunk,
    pos: usize,
}

impl<'a> Iterator for VisibleRows<'a> {
    type Item = (usize, &'a Vec<Value>);

    fn next(&mut self) -> Option<Self::Item> {
        match self.chunk.selection() {
            Some(indices) => {
                let i = *indices.get(self.pos)?;
                self.pos += 1;
                Some((i, &self.chunk.rows[i]))
            }
            None => {
                let i = self.pos;
                let row = self.chunk.rows.get(i)?;
                self.pos += 1;
                Some((i, row))
            }
        }
    }
}

/// Yield the visible rows of `chunk` in upstream row order.
pub(super) fn visible_rows(chunk: &DataChunk) -> VisibleRows<'_> {
    VisibleRows { chunk, pos: 0 }
}

pub(super) fn row_passes_filter(
    row: &[Value],
    col_names: &[String],
    filter: &Option<Expression>,
) -> bool {
    let Some(expr) = filter else {
        return true;
    };

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut context = ValueRowContext::new(row.to_vec(), layout);
    matches!(
        ExpressionEvaluator::evaluate(expr, &mut context),
        Ok(Value::Bool(true))
    )
}

/// Pre-resolve the seed-variable slot for the fast expand paths.  Mirrors the
/// historical extraction priority (`"vid"`, `"src"`, the first col-name
/// template entry, then column 0) without building a per-row expression
/// context.
fn seed_slot(layout: &SlotLayout, col_names_template: &[String]) -> usize {
    if let Some(slot) = layout.slot_id("vid") {
        return slot;
    }
    if let Some(slot) = layout.slot_id("src") {
        return slot;
    }
    if let Some(name) = col_names_template.first() {
        if let Some(slot) = layout.slot_id(name) {
            return slot;
        }
    }
    0
}

/// Forward a seed row into an id_only expand output with the source column
/// replaced by a lightweight `Value::VertexId`, so intermediate hops never
/// deep-clone the full `Value::Vertex(Box)` (with its property maps) that a
/// storage scan puts in the entity column.
fn lightweight_seed_row(row: &[Value], src_slot: usize, vid: VertexId) -> Vec<Value> {
    if matches!(row.get(src_slot), Some(Value::VertexId(_))) {
        return row.to_vec();
    }
    let mut out = Vec::with_capacity(row.len());
    for (i, val) in row.iter().enumerate() {
        if i == src_slot {
            out.push(Value::VertexId(vid));
        } else {
            out.push(val.clone());
        }
    }
    out
}

/// Fast path for single-step expand (step_limit == 1, no filter).
///
/// Avoids TraversalRuntime construction (HashSet, VecDeque, TraversalConfig)
/// and directly calls storage for each seed vertex's edges.
/// Estimated ~4x speedup vs the generic `expand_on_chunk` path.
#[allow(clippy::too_many_arguments)]
pub(super) fn expand_single_step(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    emit_raw_ids: bool,
    lightweight_source: bool,
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);

    let mut seed_vids: Vec<VertexId> = Vec::new();
    let mut seed_rows: Vec<Vec<Value>> = Vec::new();

    for (_, row) in visible_rows(&chunk) {
        let vid_val = row
            .get(seed_slot)
            .or_else(|| row.first())
            .cloned()
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            seed_vids.push(vid);
            // Raw-id path: forward a lightweight seed row (the source column
            // replaced by `Value::VertexId`) so the output never deep-clones
            // the full `Value::Vertex(Box)` carried in from upstream.
            let seed_row = if emit_raw_ids && lightweight_source {
                lightweight_seed_row(row, seed_slot, vid)
            } else {
                row.clone()
            };
            seed_rows.push(seed_row);
        }
    }

    if seed_vids.is_empty() && !src_vids.is_empty() {
        for vid_val in &src_vids {
            if let Ok(vid) = VertexId::try_from(vid_val) {
                seed_vids.push(vid);
                seed_rows.push(Vec::new());
            }
        }
    }

    let seed_width = seed_rows.first().map_or(0, |r| r.len());
    let mut buf =
        ExpandOutputBuffer::new(seed_width, chunk.visible_count().saturating_mul(4).max(1));

    if emit_raw_ids {
        // Raw-id path: one batched storage read for the whole chunk, no
        // `Value::Vertex(Box)` / `Value::Edge(Box)` allocation.
        let neighbors =
            reader.neighbor_dst_ids_batch(space_name, &seed_vids, direction, edge_types)?;
        for (dst_ids, seed_row) in neighbors.iter().zip(seed_rows.iter()) {
            for dst in dst_ids {
                buf.push_row(
                    seed_row,
                    Value::Null(crate::core::NullType::Null),
                    Value::VertexId(*dst),
                );
            }
        }
        let out_rows = buf.finish();
        if out_rows.is_empty() {
            return Ok(None);
        }
        return Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)));
    }

    for (vid, seed_row) in seed_vids.iter().zip(seed_rows.iter()) {
        let edges = reader.get_node_edges(space_name, vid, direction)?;

        for edge in &edges {
            if !edge_types.is_empty() && !edge_types.contains(&edge.edge_type) {
                continue;
            }

            let dst_vid = match direction {
                EdgeDirection::Out => *edge.dst(),
                EdgeDirection::In => *edge.src(),
                EdgeDirection::Both => {
                    if edge.src() == vid {
                        *edge.dst()
                    } else {
                        *edge.src()
                    }
                }
            };

            let dst_vertex = reader
                .get_vertex(space_name, &dst_vid)?
                .unwrap_or_else(|| crate::core::Vertex::with_vid(dst_vid));
            buf.push_row(
                seed_row,
                Value::Edge(Box::new(edge.clone())),
                Value::Vertex(Box::new(dst_vertex)),
            );
        }
    }

    let out_rows = buf.finish();
    if out_rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}

/// Count-only fast path for single-step expand.
///
/// When the downstream is a simple COUNT(*) aggregate, this function avoids
/// materializing output rows entirely. It only counts the number of edges
/// matching the criteria for each seed vertex, via the batched out-degree
/// storage accessor.
pub(super) fn expand_count_only(
    chunk: DataChunk,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    ctx: &mut ExpandCtx,
) -> Result<i64, QueryError> {
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);

    let mut seed_vids: Vec<VertexId> = Vec::new();

    for (_, row) in visible_rows(&chunk) {
        let vid_val = row
            .get(seed_slot)
            .or_else(|| row.first())
            .cloned()
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            seed_vids.push(vid);
        }
    }

    if seed_vids.is_empty() && !src_vids.is_empty() {
        for vid_val in &src_vids {
            if let Ok(vid) = VertexId::try_from(vid_val) {
                seed_vids.push(vid);
            }
        }
    }

    let degrees = reader.out_degree_batch(space_name, &seed_vids, direction, edge_types)?;
    Ok(degrees.iter().map(|&d| d as i64).sum())
}

pub(super) fn expand_on_chunk(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    step_limit: u32,
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let filter_expr = ctx.filter_expr;
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);

    // Build the list of seed vertex IDs: from the chunk rows, or from explicit src_vids.
    let mut seed_vids: Vec<VertexId> = Vec::new();
    let mut seed_rows: Vec<Vec<Value>> = Vec::new();

    for (_, row) in visible_rows(&chunk) {
        let vid_val = row
            .get(seed_slot)
            .or_else(|| row.first())
            .cloned()
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            seed_vids.push(vid);
            seed_rows.push(row.clone());
        }
    }

    // If no valid vids came from the input chunk but src_vids are provided, use those.
    if seed_vids.is_empty() && !src_vids.is_empty() {
        for vid_val in &src_vids {
            if let Ok(vid) = VertexId::try_from(vid_val) {
                seed_vids.push(vid);
                seed_rows.push(Vec::new());
            }
        }
    }

    let mut out_rows = Vec::new();
    for (vid, row) in seed_vids.iter().zip(seed_rows.iter()) {
        let config = if step_limit > 1 {
            TraversalConfig {
                min_depth: step_limit,
                max_depth: step_limit,
                ..TraversalConfig::expand(space_name.to_string(), direction, edge_types.to_vec())
            }
        } else {
            TraversalConfig::expand(space_name.to_string(), direction, edge_types.to_vec())
        };
        let runtime_reader = TraversalGraphReader::new(reader);
        let mut runtime = TraversalRuntime::new(runtime_reader, config);
        if let Some(token) = ctx.cancel_token.clone() {
            runtime.set_cancel_token(token);
        }

        if let Ok(Some(vertex)) = reader.get_vertex(space_name, vid) {
            runtime.seed_from_vertex(vertex);
        } else {
            continue;
        }

        while let Some(event) = runtime.next_event() {
            let mut out_row = row.clone();
            if let Some(ref edge) = event.edge {
                out_row.push(Value::Edge(Box::new(edge.clone())));
            } else {
                out_row.push(Value::Null(crate::core::NullType::Null));
            }
            out_row.push(Value::Vertex(Box::new(event.vertex)));
            let mut out_col_names = ctx.col_names_template.clone();
            out_col_names.push("_expand_edge".to_string());
            out_col_names.push("_expand_dst".to_string());
            if row_passes_filter(&out_row, &out_col_names, filter_expr) {
                out_rows.push(out_row);
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}

pub(super) fn traverse_on_chunk(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    config: &TraversalConfig,
    visited: &mut VisitedSet,
    cancel_token: Option<CancelToken>,
) -> Result<Option<DataChunk>, QueryError> {
    let _col_names = chunk.col_names();
    let edge_type = config.edge_types.first().map(|s| s.as_str()).unwrap_or("");
    let dir_str = match config.direction {
        EdgeDirection::Out => "out",
        EdgeDirection::In => "in",
        EdgeDirection::Both => "both",
    };

    let mut out_rows = Vec::new();
    for (_, row) in visible_rows(&chunk) {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));
        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config.clone());
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            if let Ok(Some(vertex)) = reader.get_vertex(&config.space_name, &vid) {
                runtime.seed_from_vertex(vertex);
            } else {
                continue;
            }

            while let Some(event) = runtime.next_event() {
                let nid = event.vertex.vid();
                if !visited.insert(*nid) {
                    continue;
                }

                let mut out_row = row.clone();
                out_row.push(Value::Vertex(Box::new(event.vertex)));
                out_row.push(Value::string(edge_type));
                out_row.push(Value::string(dir_str));
                out_row.push(Value::BigInt(event.depth as i64));
                out_rows.push(out_row);
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}
