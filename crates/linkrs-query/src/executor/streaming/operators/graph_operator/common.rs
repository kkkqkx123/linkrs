use std::collections::HashMap;
use std::sync::Arc;

use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::{
    gather_typed_column, DataChunk, EdgeHeaderColumn, TypedColumn,
};
use crate::executor::streaming::context::ValueRowContext;
use crate::executor::streaming::query_registry::CancelToken;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::traversal::config::TraversalConfig;
use crate::executor::traversal::graph_reader::TraversalGraphReader;
use crate::executor::traversal::runtime::TraversalRuntime;
use crate::parser::ast::pattern::PathSemantic;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::{Edge, EdgeDirection, EdgeHeader, Value};

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
/// When a selection vector is attached, only the selected rows are
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

/// Read the seed id array when the seed slot carries a vertex identity
/// column, so seed parsing skips cloning and parsing the row value.
/// Typed columns stay in lockstep with rows across selection and gather
/// transforms, so the absolute row index addresses the id array directly.
pub(super) fn identity_seed_ids(chunk: &DataChunk, seed_slot: usize) -> Option<&[VertexId]> {
    match chunk.typed_column(seed_slot) {
        Some(TypedColumn::VertexIdentity(ids)) => Some(ids),
        _ => None,
    }
}

/// Resolve the seed id for one visible row: identity scan output reads the
/// dense id array positionally, anything else parses the row value with the
/// historical seed-slot priority. Returns `None` for unparseable seeds.
fn seed_vid(
    identity_seeds: Option<&[VertexId]>,
    index: usize,
    row: &[Value],
    seed_slot: usize,
) -> Option<VertexId> {
    if let Some(ids) = identity_seeds {
        return Some(ids[index]);
    }
    let vid_val = row
        .get(seed_slot)
        .or_else(|| row.first())
        .cloned()
        .unwrap_or(Value::Null(linkrs_core::NullType::Null));
    VertexId::try_from(&vid_val).ok()
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
    let identity_seeds = identity_seed_ids(&chunk, seed_slot);

    for (index, row) in visible_rows(&chunk) {
        if let Some(vid) = seed_vid(identity_seeds, index, row, seed_slot) {
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
                    Value::Null(linkrs_core::NullType::Null),
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

    // Materialized path: per-seed edge fanout keeps the `Edge` objects
    // the edge slot must bind, then destination vertices resolve with one
    // batched read per tag group instead of one point lookup per edge.
    struct PendingExpand {
        seed_idx: usize,
        edge: Edge,
        dst: VertexId,
        tag: String,
    }

    let mut pending: Vec<PendingExpand> = Vec::new();
    let mut tag_groups: HashMap<String, Vec<VertexId>> = HashMap::new();
    for (seed_idx, vid) in seed_vids.iter().enumerate() {
        let edges = reader.get_node_edges(space_name, vid, direction, edge_types)?;
        for edge in edges {
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
            let Some(neighbor_tag) = crate::executor::traversal::graph_reader::resolve_neighbor_tag(
                reader,
                space_name,
                &edge,
                &dst_vid,
                ctx.dst_tag,
            ) else {
                continue;
            };
            tag_groups
                .entry(neighbor_tag.clone())
                .or_default()
                .push(dst_vid);
            pending.push(PendingExpand {
                seed_idx,
                edge,
                dst: dst_vid,
                tag: neighbor_tag,
            });
        }
    }

    // Batch-read vertices per tag group.
    let mut vertex_map: HashMap<(String, VertexId), Value> = HashMap::new();
    for (tag, ids) in &tag_groups {
        let vertices = reader.get_vertices_batch(space_name, tag, ids)?;
        for (id, vertex) in ids.iter().zip(vertices.iter()) {
            if let Some(v) = vertex {
                vertex_map.insert((tag.clone(), *id), Value::Vertex(Box::new(v.clone())));
            }
        }
    }

    for req in &pending {
        if let Some(dst_vertex) = vertex_map.get(&(req.tag.clone(), req.dst)) {
            buf.push_row(
                &seed_rows[req.seed_idx],
                Value::Edge(Box::new(req.edge.clone())),
                dst_vertex.clone(),
            );
        }
    }

    let out_rows = buf.finish();
    if out_rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}

/// Verify the planner closed-loop claim against storage schemas. Every edge
/// type must declare endpoint labels and the planned `dst_tag` must match the
/// neighbor side. An empty plan tag is allowed when all edge types agree on
/// one neighbor label, which the executor then derives from schema.
/// Anything else falls back to the row path.
pub(super) fn is_closed_loop_storage(
    reader: &dyn QueryStorage,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    dst_tag: &str,
) -> bool {
    closed_loop_dst_tag(reader, space_name, edge_types, direction, dst_tag).is_some()
}

/// The single agreed neighbor label for a closed loop, or `None` when the
/// schemas disagree or are untyped. An empty plan tag falls back to the schema
/// label so anonymous endpoints stay closed-loop.
pub(super) fn closed_loop_dst_tag(
    reader: &dyn QueryStorage,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    dst_tag: &str,
) -> Option<String> {
    if edge_types.is_empty() {
        return None;
    }
    let mut agreed: Option<String> = None;
    for edge_type in edge_types {
        let Ok(Some(info)) = reader.get_edge_type(space_name, edge_type) else {
            return None;
        };
        if info.src_tag_name.is_empty() || info.dst_tag_name.is_empty() {
            return None;
        }
        let neighbor = match direction {
            EdgeDirection::Out => info.dst_tag_name.clone(),
            EdgeDirection::In => info.src_tag_name.clone(),
            EdgeDirection::Both => {
                if info.src_tag_name != info.dst_tag_name {
                    return None;
                }
                info.src_tag_name.clone()
            }
        };
        if !dst_tag.is_empty() && neighbor != dst_tag {
            return None;
        }
        match agreed.as_ref() {
            None => agreed = Some(neighbor),
            Some(prev) if prev == &neighbor => {}
            _ => return None,
        }
    }
    agreed
}

/// Convert the planner edge demand into a storage projection. `None` means the
/// whole edge (all properties), `Some([])` means topology only.
fn edge_projection(edge_required: Option<&Vec<String>>) -> Option<Vec<std::sync::Arc<str>>> {
    match edge_required {
        None => None,
        Some(props) => Some(props.iter().map(|s| std::sync::Arc::from(s.as_str())).collect()),
    }
}

/// Parse seeds from both row and rowless inputs. Returns the seed ids, the
/// materialized seed rows for row outputs, and the absolute input positions
/// for typed gathering in rowless outputs.
fn parse_seeds(
    chunk: &DataChunk,
    seed_slot: usize,
    src_vids: &[Value],
) -> (Vec<VertexId>, Vec<Vec<Value>>, Vec<usize>) {
    let mut vids = Vec::new();
    let mut rows = Vec::new();
    let mut positions = Vec::new();
    let identity = identity_seed_ids(chunk, seed_slot);
    if !chunk.rows.is_empty() {
        for (index, row) in visible_rows(chunk) {
            if let Some(vid) = seed_vid(identity, index, row, seed_slot) {
                vids.push(vid);
                rows.push(row.clone());
                positions.push(index);
            }
        }
    } else if let Some(n) = chunk.typed_len() {
        let selected: Vec<usize> = chunk
            .selection()
            .map(|s| s.to_vec())
            .unwrap_or_else(|| (0..n).collect());
        let width = chunk.get_layout().len();
        for pos in selected {
            let vid_opt = if let Some(ids) = identity {
                ids.get(pos).copied()
            } else {
                chunk
                    .get_typed_by_slot(pos, seed_slot)
                    .and_then(|v| VertexId::try_from(&v).ok())
            };
            if let Some(vid) = vid_opt {
                vids.push(vid);
                let mut row = Vec::with_capacity(width);
                for slot in 0..width {
                    row.push(
                        chunk
                            .get_typed_by_slot(pos, slot)
                            .unwrap_or(Value::Null(linkrs_core::NullType::Null)),
                    );
                }
                rows.push(row);
                positions.push(pos);
            }
        }
    }
    if vids.is_empty() && !src_vids.is_empty() {
        for vid_val in src_vids {
            if let Ok(vid) = VertexId::try_from(vid_val) {
                vids.push(vid);
                rows.push(Vec::new());
                positions.push(0);
            }
        }
    }
    (vids, rows, positions)
}

/// Destination id on one side of an edge for the hop direction.
fn edge_neighbor(edge: &Edge, seed: &VertexId, direction: EdgeDirection) -> VertexId {
    match direction {
        EdgeDirection::Out => *edge.dst(),
        EdgeDirection::In => *edge.src(),
        EdgeDirection::Both => {
            if edge.src() == seed {
                *edge.dst()
            } else {
                *edge.src()
            }
        }
    }
}

/// Single-step columnar assembly for closed-loop materialized hops.
///
/// Uses a projected edge read (topology only when no edge properties are
/// demanded), filters dangling destinations with one batched vertex read per
/// tag, and either skips the row view (rowless typed output) or keeps rows
/// with identity shortcuts for undemanded slots. Falls back to `None` when
/// there is no output; callers fall back to the legacy row path when the
/// closed-loop check fails before calling here.
pub(super) fn expand_single_step_columnar(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    if !ctx.closed_loop {
        return Ok(None);
    }
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);
    let seed_width = if chunk.rows.is_empty() {
        chunk
            .typed_len()
            .map(|_| chunk.get_layout().len())
            .unwrap_or(0)
    } else {
        chunk.rows.first().map_or(chunk.get_layout().len(), |r| r.len())
    };
    let input_typed = chunk.typed_columns.clone();
    let (seed_vids, seed_rows, seed_positions) = parse_seeds(&chunk, seed_slot, &src_vids);
    if seed_vids.is_empty() {
        return Ok(None);
    }
    let projection = edge_projection(ctx.edge_required_props.clone().as_ref().map(|v| v));
    let projection_ref = projection.as_deref();
    let edge_empty = matches!(ctx.edge_required_props.as_ref(), Some(v) if v.is_empty());
    let dst_empty = matches!(ctx.dst_required_props.as_ref(), Some(v) if v.is_empty());
    let rowless = ctx.skip_rows && edge_empty && dst_empty;

    struct PendingEdge {
        seed_idx: usize,
        edge: Edge,
        dst: VertexId,
    }
    let mut pending: Vec<PendingEdge> = Vec::new();
    let mut all_dst: Vec<VertexId> = Vec::new();
    for (seed_idx, vid) in seed_vids.iter().enumerate() {
        let edges = reader.get_node_edges_projected(
            space_name,
            vid,
            direction,
            edge_types,
            projection_ref,
            None,
        )?;
        for edge in edges {
            let dst = edge_neighbor(&edge, vid, direction);
            pending.push(PendingEdge {
                seed_idx,
                edge,
                dst,
            });
            all_dst.push(dst);
        }
    }
    if pending.is_empty() {
        return Ok(None);
    }
    let dst_tag_owned =
        closed_loop_dst_tag(reader, space_name, edge_types, direction, ctx.dst_tag)
            .unwrap_or_else(|| ctx.dst_tag.to_string());
    let dst_tag = dst_tag_owned.as_str();
    let vertices = reader.get_vertices_batch(space_name, dst_tag, &all_dst)?;
    let mut kept: Vec<usize> = Vec::with_capacity(pending.len());
    for (i, vertex) in vertices.iter().enumerate() {
        if vertex.is_some() {
            kept.push(i);
        }
    }
    if kept.is_empty() {
        return Ok(None);
    }

    if rowless {
        let mut edge_src = Vec::with_capacity(kept.len());
        let mut edge_dst = Vec::with_capacity(kept.len());
        let mut edge_types_out = Vec::with_capacity(kept.len());
        let mut rankings = Vec::with_capacity(kept.len());
        let mut dst_ids = Vec::with_capacity(kept.len());
        let mut gather_indices = Vec::with_capacity(kept.len());
        for &i in &kept {
            let req = &pending[i];
            edge_src.push(*req.edge.src());
            edge_dst.push(*req.edge.dst());
            edge_types_out.push(req.edge.edge_type().to_string());
            rankings.push(req.edge.ranking());
            dst_ids.push(req.dst);
            gather_indices.push(seed_positions[req.seed_idx]);
        }
        let mut typed: Vec<TypedColumn> = Vec::with_capacity(seed_width + 2);
        for slot in 0..seed_width {
            if let Some(ref cols) = input_typed {
                if let Some(col) = cols.get(slot) {
                    typed.push(gather_typed_column(col, &gather_indices));
                    continue;
                }
            }
            typed.push(TypedColumn::Fallback(
                kept.iter()
                    .map(|&i| {
                        seed_rows[pending[i].seed_idx]
                            .get(slot)
                            .cloned()
                            .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                    })
                    .collect(),
            ));
        }
        typed.push(TypedColumn::EdgeHeader(EdgeHeaderColumn::from_parts(
            edge_src,
            edge_dst,
            edge_types_out,
            rankings,
        )));
        typed.push(TypedColumn::VertexIdentity(dst_ids));
        debug_assert!(
            typed.iter().all(|c| c.len() == kept.len()),
            "expand typed arms must stay in lockstep like rows"
        );
        let mut out = DataChunk::new_with_layout(Vec::new(), output_layout);
        out.typed_columns = Some(typed);
        return Ok(Some(out));
    }

    let mut out_rows: Vec<Vec<Value>> = Vec::with_capacity(kept.len());
    let mut edge_src = Vec::with_capacity(kept.len());
    let mut edge_dst = Vec::with_capacity(kept.len());
    let mut edge_types_out = Vec::with_capacity(kept.len());
    let mut rankings = Vec::with_capacity(kept.len());
    let mut dst_ids = Vec::with_capacity(kept.len());
    let mut gather_indices = Vec::with_capacity(kept.len());
    // Destination assembly reuses the batched existence read above: it was
    // issued with the schema-resolved tag and is aligned with `pending`, so
    // no per-row point lookup is needed for any demand shape.
    for &i in &kept {
        let req = &pending[i];
        let seed_row = &seed_rows[req.seed_idx];
        let edge_value = if edge_empty {
            Value::edge_header(EdgeHeader::new(
                *req.edge.src(),
                *req.edge.dst(),
                req.edge.edge_type().to_string(),
                req.edge.ranking(),
            ))
        } else {
            Value::Edge(Box::new(req.edge.clone()))
        };
        let dst_value = if dst_empty {
            Value::VertexId(req.dst)
        } else {
            match vertices.get(i).and_then(|v| v.clone()) {
                Some(v) => Value::Vertex(Box::new(v)),
                None => continue,
            }
        };
        let mut row = Vec::with_capacity(seed_row.len() + 2);
        row.extend_from_slice(seed_row);
        row.push(edge_value);
        row.push(dst_value);
        out_rows.push(row);
        edge_src.push(*req.edge.src());
        edge_dst.push(*req.edge.dst());
        edge_types_out.push(req.edge.edge_type().to_string());
        rankings.push(req.edge.ranking());
        dst_ids.push(req.dst);
        gather_indices.push(seed_positions[req.seed_idx]);
    }
    if out_rows.is_empty() {
        return Ok(None);
    }
    let mut out = DataChunk::new_with_layout(out_rows, Arc::clone(&output_layout));
    // Seed arms without an input typed column mirror the row view exactly, so
    // row-based and typed consumers observe the same seed values.
    let mut typed: Vec<TypedColumn> = Vec::with_capacity(seed_width + 2);
    for slot in 0..seed_width {
        if let Some(ref cols) = input_typed {
            if let Some(col) = cols.get(slot) {
                typed.push(gather_typed_column(col, &gather_indices));
                continue;
            }
        }
        typed.push(TypedColumn::Fallback(
            out.rows
                .iter()
                .map(|r| {
                    r.get(slot)
                        .cloned()
                        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                })
                .collect(),
        ));
    }
    if edge_empty {
        typed.push(TypedColumn::EdgeHeader(EdgeHeaderColumn::from_parts(
            edge_src,
            edge_dst,
            edge_types_out,
            rankings,
        )));
    } else {
        typed.push(TypedColumn::Fallback(
            out.rows.iter().map(|r| r[seed_width].clone()).collect(),
        ));
    }
    if dst_empty {
        typed.push(TypedColumn::VertexIdentity(dst_ids));
    } else {
        typed.push(TypedColumn::Fallback(
            out.rows.iter().map(|r| r[seed_width + 1].clone()).collect(),
        ));
    }
    debug_assert!(
        typed.iter().all(|c| c.len() == out.rows.len()),
        "expand typed arms must stay in lockstep like rows"
    );
    out.typed_columns = Some(typed);
    Ok(Some(out))
}

/// Fixed multi-hop frontier loop for `step_limit = k > 1`.
///
/// Intermediate hops run identifier-only through `neighbor_dst_ids_batch`
/// with duplicates preserved for walk semantics. The tail hop reuses the
/// single-step columnar assembly so edge and destination demands apply
/// unchanged.
pub(super) fn expand_multi_hop_frontier(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    step_limit: u32,
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    if !ctx.closed_loop {
        return Ok(None);
    }
    if step_limit <= 1 || !src_vids.is_empty() || ctx.filter_expr.is_some() {
        return Ok(None);
    }
    if let Some(sem) = ctx.path_semantic.clone() {
        if !matches!(sem, PathSemantic::Walk) {
            return Ok(None);
        }
    }
    if !is_closed_loop_storage(
        reader,
        ctx.space_name,
        ctx.edge_types,
        ctx.direction,
        ctx.dst_tag,
    ) {
        return Ok(None);
    }
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);
    let seed_width = if chunk.rows.is_empty() {
        chunk.get_layout().len()
    } else {
        chunk.rows.first().map_or(chunk.get_layout().len(), |r| r.len())
    };
    let input_typed = chunk.typed_columns.clone();
    let (seed_vids, seed_rows, seed_positions) = parse_seeds(&chunk, seed_slot, &src_vids);
    if seed_vids.is_empty() {
        return Ok(None);
    }
    let mut frontier: Vec<(usize, VertexId)> = seed_vids
        .iter()
        .enumerate()
        .map(|(i, vid)| (i, *vid))
        .collect();
    for _ in 1..step_limit {
        if frontier.is_empty() {
            return Ok(None);
        }
        let ids: Vec<VertexId> = frontier.iter().map(|(_, vid)| *vid).collect();
        let batches =
            reader.neighbor_dst_ids_batch(ctx.space_name, &ids, ctx.direction, ctx.edge_types)?;
        let mut next: Vec<(usize, VertexId)> = Vec::with_capacity(frontier.len().saturating_mul(2));
        for ((seed_idx, _), neighbors) in frontier.iter().zip(batches.iter()) {
            for dst in neighbors {
                next.push((*seed_idx, *dst));
            }
        }
        frontier = next;
        if frontier.is_empty() {
            return Ok(None);
        }
        if frontier.len() > 1_000_000 {
            return Err(QueryError::execution(
                "multi-hop frontier exceeded the in-memory fanout budget".to_string(),
            ));
        }
    }
    if frontier.is_empty() {
        return Ok(None);
    }
    let projection = edge_projection(ctx.edge_required_props.clone().as_ref().map(|v| v));
    let projection_ref = projection.as_deref();
    let edge_empty = matches!(ctx.edge_required_props.as_ref(), Some(v) if v.is_empty());
    let dst_empty = matches!(ctx.dst_required_props.as_ref(), Some(v) if v.is_empty());
    let rowless = ctx.skip_rows && edge_empty && dst_empty;
    struct TailEdge {
        orig_idx: usize,
        edge: Edge,
        dst: VertexId,
    }
    let mut pending: Vec<TailEdge> = Vec::new();
    let mut all_dst: Vec<VertexId> = Vec::new();
    for (orig_idx, vid) in frontier.iter() {
        let edges = reader.get_node_edges_projected(
            ctx.space_name,
            vid,
            ctx.direction,
            ctx.edge_types,
            projection_ref,
            None,
        )?;
        for edge in edges {
            let dst = edge_neighbor(&edge, vid, ctx.direction);
            pending.push(TailEdge {
                orig_idx: *orig_idx,
                edge,
                dst,
            });
            all_dst.push(dst);
        }
    }
    if pending.is_empty() {
        return Ok(None);
    }
    let tail_tag_owned = closed_loop_dst_tag(
        reader,
        ctx.space_name,
        ctx.edge_types,
        ctx.direction,
        ctx.dst_tag,
    )
    .unwrap_or_else(|| ctx.dst_tag.to_string());
    let tail_tag = tail_tag_owned.as_str();
    let vertices = reader.get_vertices_batch(ctx.space_name, tail_tag, &all_dst)?;
    let mut kept: Vec<usize> = Vec::new();
    for (i, vertex) in vertices.iter().enumerate() {
        if vertex.is_some() {
            kept.push(i);
        }
    }
    if kept.is_empty() {
        return Ok(None);
    }
    if rowless {
        let mut edge_src = Vec::with_capacity(kept.len());
        let mut edge_dst = Vec::with_capacity(kept.len());
        let mut edge_types_out = Vec::with_capacity(kept.len());
        let mut rankings = Vec::with_capacity(kept.len());
        let mut dst_ids = Vec::with_capacity(kept.len());
        let mut gather_indices = Vec::with_capacity(kept.len());
        for &i in &kept {
            let req = &pending[i];
            edge_src.push(*req.edge.src());
            edge_dst.push(*req.edge.dst());
            edge_types_out.push(req.edge.edge_type().to_string());
            rankings.push(req.edge.ranking());
            dst_ids.push(req.dst);
            gather_indices.push(seed_positions[req.orig_idx]);
        }
        let mut typed: Vec<TypedColumn> = Vec::with_capacity(seed_width + 2);
        for slot in 0..seed_width {
            if let Some(ref cols) = input_typed {
                if let Some(col) = cols.get(slot) {
                    typed.push(gather_typed_column(col, &gather_indices));
                    continue;
                }
            }
            typed.push(TypedColumn::Fallback(
                kept.iter()
                    .map(|&i| {
                        seed_rows[pending[i].orig_idx]
                            .get(slot)
                            .cloned()
                            .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                    })
                    .collect(),
            ));
        }
        typed.push(TypedColumn::EdgeHeader(EdgeHeaderColumn::from_parts(
            edge_src,
            edge_dst,
            edge_types_out,
            rankings,
        )));
        typed.push(TypedColumn::VertexIdentity(dst_ids));
        debug_assert!(
            typed.iter().all(|c| c.len() == kept.len()),
            "expand typed arms must stay in lockstep like rows"
        );
        let mut out = DataChunk::new_with_layout(Vec::new(), output_layout);
        out.typed_columns = Some(typed);
        return Ok(Some(out));
    }
    let mut out_rows: Vec<Vec<Value>> = Vec::with_capacity(kept.len());
    // Tail destinations reuse the batched read above, which already ran under
    // the schema-resolved tag. This also keeps anonymous endpoints working:
    // the plan tag may be empty while the batch tag is resolved.
    for &i in &kept {
        let req = &pending[i];
        let seed_row = &seed_rows[req.orig_idx];
        let edge_value = if edge_empty {
            Value::edge_header(EdgeHeader::new(
                *req.edge.src(),
                *req.edge.dst(),
                req.edge.edge_type().to_string(),
                req.edge.ranking(),
            ))
        } else {
            Value::Edge(Box::new(req.edge.clone()))
        };
        let dst_value = if dst_empty {
            Value::VertexId(req.dst)
        } else {
            match vertices.get(i).and_then(|v| v.clone()) {
                Some(v) => Value::Vertex(Box::new(v)),
                None => continue,
            }
        };
        let mut row = Vec::with_capacity(seed_row.len() + 2);
        row.extend_from_slice(seed_row);
        row.push(edge_value);
        row.push(dst_value);
        out_rows.push(row);
    }
    if out_rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}

/// Variable-length frontier loop for `step_limits` ranges.
///
/// Walks depths 1..=max with per-depth edge reads so every emitted depth
/// carries its own edge header and destination. Only depths listed in
/// `step_limits` are emitted, matching `[*min..max]` union semantics.
pub(super) fn expand_variable_frontier(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    step_limits: &[u32],
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    if !ctx.closed_loop {
        return Ok(None);
    }
    if step_limits.is_empty() || !src_vids.is_empty() || ctx.filter_expr.is_some() {
        return Ok(None);
    }
    if let Some(sem) = ctx.path_semantic.clone() {
        if !matches!(sem, PathSemantic::Walk) {
            return Ok(None);
        }
    }
    if !is_closed_loop_storage(
        reader,
        ctx.space_name,
        ctx.edge_types,
        ctx.direction,
        ctx.dst_tag,
    ) {
        return Ok(None);
    }
    let max_depth = *step_limits.iter().max().unwrap_or(&0);
    if max_depth == 0 || max_depth > 64 {
        return Ok(None);
    }
    let wanted: std::collections::HashSet<u32> = step_limits.iter().copied().collect();
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);
    let seed_width = if chunk.rows.is_empty() {
        chunk.get_layout().len()
    } else {
        chunk.rows.first().map_or(chunk.get_layout().len(), |r| r.len())
    };
    let input_typed = chunk.typed_columns.clone();
    let (seed_vids, seed_rows, seed_positions) = parse_seeds(&chunk, seed_slot, &src_vids);
    if seed_vids.is_empty() {
        return Ok(None);
    }
    let projection = edge_projection(ctx.edge_required_props.clone().as_ref().map(|v| v));
    let projection_ref = projection.as_deref();
    let edge_empty = matches!(ctx.edge_required_props.as_ref(), Some(v) if v.is_empty());
    let dst_empty = matches!(ctx.dst_required_props.as_ref(), Some(v) if v.is_empty());
    let rowless = ctx.skip_rows && edge_empty && dst_empty;
    struct VarEdge {
        orig_idx: usize,
        edge: Edge,
        dst: VertexId,
    }
    let mut frontier: Vec<(usize, VertexId)> = seed_vids
        .iter()
        .enumerate()
        .map(|(i, vid)| (i, *vid))
        .collect();
    let mut pending: Vec<VarEdge> = Vec::new();
    let mut all_dst: Vec<VertexId> = Vec::new();
    for depth in 1..=max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next: Vec<(usize, VertexId)> = Vec::new();
        let mut depth_edges: Vec<VarEdge> = Vec::new();
        for (orig_idx, vid) in frontier.iter() {
            let edges = reader.get_node_edges_projected(
                ctx.space_name,
                vid,
                ctx.direction,
                ctx.edge_types,
                projection_ref,
                None,
            )?;
            for edge in edges {
                let dst = edge_neighbor(&edge, vid, ctx.direction);
                depth_edges.push(VarEdge {
                    orig_idx: *orig_idx,
                    edge,
                    dst,
                });
                next.push((*orig_idx, dst));
            }
        }
        if wanted.contains(&depth) {
            for e in depth_edges.into_iter() {
                all_dst.push(e.dst);
                pending.push(e);
            }
        }
        frontier = next;
        if frontier.len() > 1_000_000 {
            return Err(QueryError::execution(
                "variable-length frontier exceeded the in-memory fanout budget".to_string(),
            ));
        }
        if depth == max_depth {
            break;
        }
        if frontier.is_empty() {
            break;
        }
    }
    if pending.is_empty() {
        return Ok(None);
    }
    let var_tag_owned = closed_loop_dst_tag(
        reader,
        ctx.space_name,
        ctx.edge_types,
        ctx.direction,
        ctx.dst_tag,
    )
    .unwrap_or_else(|| ctx.dst_tag.to_string());
    let var_tag = var_tag_owned.as_str();
    let vertices = reader.get_vertices_batch(ctx.space_name, var_tag, &all_dst)?;
    let mut kept: Vec<usize> = Vec::new();
    for (i, vertex) in vertices.iter().enumerate() {
        if vertex.is_some() {
            kept.push(i);
        }
    }
    if kept.is_empty() {
        return Ok(None);
    }
    if rowless {
        let mut edge_src = Vec::with_capacity(kept.len());
        let mut edge_dst = Vec::with_capacity(kept.len());
        let mut edge_types_out = Vec::with_capacity(kept.len());
        let mut rankings = Vec::with_capacity(kept.len());
        let mut dst_ids = Vec::with_capacity(kept.len());
        let mut gather_indices = Vec::with_capacity(kept.len());
        for &i in &kept {
            let req = &pending[i];
            edge_src.push(*req.edge.src());
            edge_dst.push(*req.edge.dst());
            edge_types_out.push(req.edge.edge_type().to_string());
            rankings.push(req.edge.ranking());
            dst_ids.push(req.dst);
            gather_indices.push(seed_positions[req.orig_idx]);
        }
        let mut typed: Vec<TypedColumn> = Vec::with_capacity(seed_width + 2);
        for slot in 0..seed_width {
            if let Some(ref cols) = input_typed {
                if let Some(col) = cols.get(slot) {
                    typed.push(gather_typed_column(col, &gather_indices));
                    continue;
                }
            }
            typed.push(TypedColumn::Fallback(
                kept.iter()
                    .map(|&i| {
                        seed_rows[pending[i].orig_idx]
                            .get(slot)
                            .cloned()
                            .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                    })
                    .collect(),
            ));
        }
        typed.push(TypedColumn::EdgeHeader(EdgeHeaderColumn::from_parts(
            edge_src,
            edge_dst,
            edge_types_out,
            rankings,
        )));
        typed.push(TypedColumn::VertexIdentity(dst_ids));
        debug_assert!(
            typed.iter().all(|c| c.len() == kept.len()),
            "expand typed arms must stay in lockstep like rows"
        );
        let mut out = DataChunk::new_with_layout(Vec::new(), output_layout);
        out.typed_columns = Some(typed);
        return Ok(Some(out));
    }
    let mut out_rows: Vec<Vec<Value>> = Vec::with_capacity(kept.len());
    // Destinations reuse the batched read above, which already ran under the
    // schema-resolved tag. This also keeps anonymous endpoints working: the
    // plan tag may be empty while the batch tag is resolved.
    for &i in &kept {
        let req = &pending[i];
        let seed_row = &seed_rows[req.orig_idx];
        let edge_value = if edge_empty {
            Value::edge_header(EdgeHeader::new(
                *req.edge.src(),
                *req.edge.dst(),
                req.edge.edge_type().to_string(),
                req.edge.ranking(),
            ))
        } else {
            Value::Edge(Box::new(req.edge.clone()))
        };
        let dst_value = if dst_empty {
            Value::VertexId(req.dst)
        } else {
            match vertices.get(i).and_then(|v| v.clone()) {
                Some(v) => Value::Vertex(Box::new(v)),
                None => continue,
            }
        };
        let mut row = Vec::with_capacity(seed_row.len() + 2);
        row.extend_from_slice(seed_row);
        row.push(edge_value);
        row.push(dst_value);
        out_rows.push(row);
    }
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
    let identity_seeds = identity_seed_ids(&chunk, seed_slot);

    for (index, row) in visible_rows(&chunk) {
        if let Some(vid) = seed_vid(identity_seeds, index, row, seed_slot) {
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

/// Row-path union for `step_limits` ranges when the frontier loop declines.
///
/// Replays the generic single-depth path once per wanted depth and
/// concatenates the per-depth rows, preserving `[*min..max]` union semantics
/// on shapes the identifier frontier cannot serve (open schema, literal
/// seeds, constrained path semantics). Depths are deduplicated; each replay
/// is an independent chunk so no cross-depth state leaks.
pub(super) fn expand_variable_row_union(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    step_limits: &[u32],
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let mut wanted: Vec<u32> = step_limits.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() {
        return Ok(None);
    }
    let mut all_rows: Vec<Vec<Value>> = Vec::new();
    for depth in wanted {
        if depth == 0 {
            continue;
        }
        if let Some(out) = expand_on_chunk(
            chunk.clone(),
            Arc::clone(&output_layout),
            reader,
            src_vids.clone(),
            depth,
            &mut *ctx,
        )? {
            all_rows.extend(out.rows);
        }
    }
    if all_rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(DataChunk::new_with_layout(all_rows, output_layout)))
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
    // Seeds prefer full vertex values (preserving tags); bare ids without
    // tags are illegal under single-label semantics.
    let mut seed_vids: Vec<VertexId> = Vec::new();
    let mut seed_rows: Vec<Vec<Value>> = Vec::new();
    let mut seed_vertices: Vec<Option<linkrs_core::Vertex>> = Vec::new();

    for (_, row) in visible_rows(&chunk) {
        let vid_val = row
            .get(seed_slot)
            .or_else(|| row.first())
            .cloned()
            .unwrap_or(Value::Null(linkrs_core::NullType::Null));

        if let Value::Vertex(vertex) = &vid_val {
            seed_vids.push(vertex.vid);
            seed_rows.push(row.clone());
            seed_vertices.push(Some((**vertex).clone()));
        } else if let Ok(vid) = VertexId::try_from(&vid_val) {
            seed_vids.push(vid);
            seed_rows.push(row.clone());
            seed_vertices.push(None);
        }
    }

    // Literal seeds carry no label: resolve them in the seed domain
    // determined by the edge schemas. Seeds missing from that domain walk
    // nothing; ambiguous domains error instead of guessing a label.
    if seed_vids.is_empty() && !src_vids.is_empty() {
        let scope_tag = if src_vids.iter().any(|v| !matches!(v, Value::Vertex(_))) {
            Some(crate::executor::traversal::graph_reader::resolve_seed_tag(
                reader,
                space_name,
                edge_types,
                direction,
                ctx.dst_tag,
            )?)
        } else {
            None
        };
        for vid_val in &src_vids {
            if let Value::Vertex(vertex) = vid_val {
                seed_vids.push(vertex.vid);
                seed_rows.push(Vec::new());
                seed_vertices.push(Some((**vertex).clone()));
            } else if let Ok(vid) = VertexId::try_from(vid_val) {
                let Some(scope) = scope_tag.as_deref() else {
                    continue;
                };
                let Some(vertex) = reader.get_vertex(space_name, scope, &vid)? else {
                    continue;
                };
                seed_vids.push(vid);
                seed_rows.push(Vec::new());
                seed_vertices.push(Some(vertex));
            }
        }
    }

    let mut out_rows = Vec::new();
    for ((vid, row), seed_vertex) in seed_vids
        .iter()
        .zip(seed_rows.iter())
        .zip(seed_vertices.iter())
    {
        let _ = vid;
        let mut config =
            TraversalConfig::expand(space_name.to_string(), direction, edge_types.to_vec());
        if step_limit > 1 {
            config.min_depth = step_limit;
            config.max_depth = step_limit;
        }
        config.vertex_tag = ctx.dst_tag.to_string();
        config.path_semantic = ctx.path_semantic.clone();
        match config.path_semantic {
            // Walk/Trail/Acyclic allow or constrain repeats per path, so
            // the runtime must not apply global vertex dedup. Trail and
            // Acyclic are enforced per path inside TraversalRuntime.
            Some(PathSemantic::Walk)
            | Some(PathSemantic::Trail)
            | Some(PathSemantic::Acyclic)
            | None => {
                if config.path_semantic.is_some() {
                    config.visited_policy = crate::executor::traversal::config::VisitedPolicy::None;
                }
            }
            // Shortest variants rely on a global first-visit order: the
            // first time a vertex is reached is via a shortest path, so
            // global dedup is the algorithm rather than an optimization.
            // The weighted variant runs Dijkstra inside the runtime, which
            // also needs global dedup.
            Some(PathSemantic::Shortest)
            | Some(PathSemantic::AllShortest)
            | Some(PathSemantic::WeightedShortest(_)) => {
                config.visited_policy = crate::executor::traversal::config::VisitedPolicy::Global;
                config.order = crate::executor::traversal::config::TraversalOrder::Bfs;
            }
        }
        let runtime_reader = TraversalGraphReader::new(reader);
        let mut runtime = TraversalRuntime::new(runtime_reader, config);
        if let Some(token) = ctx.cancel_token.clone() {
            runtime.set_cancel_token(token);
        }

        if let Some(vertex) = seed_vertex.clone() {
            runtime.seed_from_vertex(vertex);
        } else {
            return Err(QueryError::execution(
                "Traversal seed requires a vertex value with tag; bare id is illegal".to_string(),
            ));
        }

        while let Some(event) = runtime.next_event() {
            let mut out_row = row.clone();
            if let Some(ref edge) = event.edge {
                out_row.push(Value::Edge(Box::new(edge.clone())));
            } else {
                out_row.push(Value::Null(linkrs_core::NullType::Null));
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

pub(super) fn _traverse_on_chunk(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    config: &TraversalConfig,
    visited: &mut VisitedSet,
    cancel_token: Option<CancelToken>,
) -> Result<Option<DataChunk>, QueryError> {
    traverse_on_chunk_with_semantic(
        chunk,
        output_layout,
        reader,
        config,
        visited,
        true,
        cancel_token,
    )
}

pub(super) fn traverse_on_chunk_with_semantic(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    config: &TraversalConfig,
    visited: &mut VisitedSet,
    skip_visited: bool,
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
            .unwrap_or(Value::Null(linkrs_core::NullType::Null));
        let seed_vertex = match &vid_val {
            Value::Vertex(vertex) => Some((**vertex).clone()),
            _ => None,
        };
        let Some(seed) = seed_vertex else {
            return Err(QueryError::execution(
                "Traversal seed requires a vertex value with tag; bare id is illegal".to_string(),
            ));
        };
        {
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime_config = config.clone();
            // Keep the declared semantic for the runtime: Trail/Acyclic
            // are enforced per path, Shortest uses BFS first-visit and
            // WeightedShortest uses Dijkstra (both need global dedup).
            // The operator-level `skip_visited` (global VisitedSet) is
            // applied separately below so per-path semantics are never
            // silently replaced by global dedup.
            match runtime_config.path_semantic {
                Some(PathSemantic::Walk)
                | Some(PathSemantic::Trail)
                | Some(PathSemantic::Acyclic)
                | None => {
                    if runtime_config.path_semantic.is_some() {
                        runtime_config.visited_policy =
                            crate::executor::traversal::config::VisitedPolicy::None;
                    }
                }
                Some(PathSemantic::Shortest)
                | Some(PathSemantic::AllShortest)
                | Some(PathSemantic::WeightedShortest(_)) => {
                    runtime_config.visited_policy =
                        crate::executor::traversal::config::VisitedPolicy::Global;
                    runtime_config.order = crate::executor::traversal::config::TraversalOrder::Bfs;
                }
            }
            let mut runtime = TraversalRuntime::new(runtime_reader, runtime_config);
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            runtime.seed_from_vertex(seed);

            while let Some(event) = runtime.next_event() {
                let nid = event.vertex.vid();
                if skip_visited && !visited.insert(*nid) {
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
