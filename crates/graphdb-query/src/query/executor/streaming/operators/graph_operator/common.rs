use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::ValueRowContext;
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

/// Fast path for single-step expand (step_limit == 1, no filter).
///
/// Avoids TraversalRuntime construction (HashSet, VecDeque, TraversalConfig)
/// and directly calls storage for each seed vertex's edges.
/// Estimated ~4x speedup vs the generic `expand_on_chunk` path.
pub(super) fn expand_single_step(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;

    let mut seed_vids: Vec<VertexId> = Vec::new();
    let mut seed_rows: Vec<Vec<Value>> = Vec::new();

    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let src_name = ctx.col_names_template.first().map(|s| s.as_str());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| context.get_variable("src"))
            .or_else(|| src_name.and_then(|name| context.get_variable(name)))
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            seed_vids.push(vid);
            seed_rows.push(row.clone());
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
    let mut buf = ExpandOutputBuffer::new(seed_width, chunk.rows.len() * 4);

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
/// matching the criteria for each seed vertex.
pub(super) fn expand_count_only(
    chunk: DataChunk,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    ctx: &mut ExpandCtx,
) -> Result<i64, QueryError> {
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;

    let mut seed_vids: Vec<VertexId> = Vec::new();

    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let src_name = ctx.col_names_template.first().map(|s| s.as_str());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| context.get_variable("src"))
            .or_else(|| src_name.and_then(|name| context.get_variable(name)))
            .or_else(|| row.first().cloned())
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

    let mut total_count: i64 = 0;
    for vid in &seed_vids {
        let edges = reader.get_node_edges(space_name, vid, direction)?;
        for edge in &edges {
            if edge_types.is_empty() || edge_types.contains(&edge.edge_type) {
                total_count += 1;
            }
        }
    }

    Ok(total_count)
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

    // Build the list of seed vertex IDs: from the chunk rows, or from explicit src_vids.
    let mut seed_vids: Vec<VertexId> = Vec::new();
    let mut seed_rows: Vec<Vec<Value>> = Vec::new();

    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let src_name = ctx.col_names_template.first().map(|s| s.as_str());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| context.get_variable("src"))
            .or_else(|| src_name.and_then(|name| context.get_variable(name)))
            .or_else(|| row.first().cloned())
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
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<Option<DataChunk>, QueryError> {
    let _col_names = chunk.col_names();
    let edge_type = config.edge_types.first().map(|s| s.as_str()).unwrap_or("");
    let dir_str = match config.direction {
        EdgeDirection::Out => "out",
        EdgeDirection::In => "in",
        EdgeDirection::Both => "both",
    };

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
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
