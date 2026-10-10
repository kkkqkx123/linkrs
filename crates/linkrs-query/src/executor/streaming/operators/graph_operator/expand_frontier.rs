use std::sync::Arc;

use crate::executor::streaming::chunk::{
    gather_typed_column, DataChunk, EdgeHeaderColumn, TypedColumn,
};
use crate::executor::streaming::slot::SlotLayout;
use crate::parser::ast::pattern::PathSemantic;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::{Edge, EdgeHeader, Value};

use super::expand_buffer::visible_rows;
use super::expand_columnar::{
    check_output_layout, check_rowless_lockstep, edge_neighbor, read_destinations,
};
use super::expand_dispatch::{
    ColumnarOutcome, bypass_column, closed_loop_dst_tag, dst_bypass_value, edge_bypass_value,
    edge_projection, expand_on_chunk, has_bypass_conflict, hop_bypass, is_closed_loop_storage,
};
use super::expand_seeds::{
    identity_seed_ids, materialize_rowless_rows, parse_seeds, seed_slot, seed_vid,
};
use super::ExpandCtx;

/// Fixed multi-hop frontier loop for `step_limit = k > 1`.
///
/// Intermediate hops run identifier-only through `neighbor_dst_ids_batch`
/// with duplicates preserved for walk semantics. The tail hop reuses the
/// single-step columnar assembly so edge and destination demands apply
/// unchanged. Only the tail depth assembles bypass columns; intermediate
/// depths stay pure identifiers.
pub(super) fn expand_multi_hop_frontier(
    chunk: &DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    step_limit: u32,
    ctx: &mut ExpandCtx,
) -> Result<ColumnarOutcome, QueryError> {
    if !ctx.closed_loop {
        return Ok(ColumnarOutcome::Declined);
    }
    if step_limit <= 1 || !src_vids.is_empty() || ctx.filter_expr.is_some() {
        return Ok(ColumnarOutcome::Declined);
    }
    if let Some(sem) = ctx.path_semantic.clone() {
        if !matches!(sem, PathSemantic::Walk) {
            return Ok(ColumnarOutcome::Declined);
        }
    }
    if !is_closed_loop_storage(
        reader,
        ctx.space_name,
        ctx.edge_types,
        ctx.direction,
        ctx.dst_tag,
    ) {
        return Ok(ColumnarOutcome::Declined);
    }
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);
    let seed_width = if chunk.rows.is_empty() {
        chunk.get_layout().len()
    } else {
        chunk.rows.first().map_or(chunk.get_layout().len(), |r| r.len())
    };
    let input_layout = chunk.get_layout();
    let (edge_bypass_opt, dst_bypass_opt) = hop_bypass(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &input_layout,
    );
    if ctx.skip_rows && (edge_bypass_opt.is_none() || dst_bypass_opt.is_none()) {
        return Err(QueryError::execution(
            "rowless expand without complete bypass columns violates the plan contract"
                .to_string(),
        ));
    }
    let conflict = has_bypass_conflict(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &input_layout,
    );
    let rowless_tail =
        ctx.skip_rows && edge_bypass_opt.is_some() && dst_bypass_opt.is_some() && !conflict;
    let edge_bypass = edge_bypass_opt.clone().unwrap_or_default();
    let dst_bypass = dst_bypass_opt.clone().unwrap_or_default();
    let input_typed = chunk.typed_columns.clone();
    let (seed_vids, seed_rows, seed_positions) = parse_seeds(chunk, seed_slot, &src_vids);
    if seed_vids.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    let mut frontier: Vec<(usize, VertexId)> = seed_vids
        .iter()
        .enumerate()
        .map(|(i, vid)| (i, *vid))
        .collect();
    for _ in 1..step_limit {
        if frontier.is_empty() {
            return Ok(ColumnarOutcome::Empty);
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
            return Ok(ColumnarOutcome::Empty);
        }
        if frontier.len() > 1_000_000 {
            return Err(QueryError::execution(
                "multi-hop frontier exceeded the in-memory fanout budget".to_string(),
            ));
        }
    }
    if frontier.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    let projection = edge_projection(ctx.edge_required_props.clone().as_ref().map(|v| v));
    let projection_ref = projection.as_deref();
    let edge_empty = matches!(ctx.edge_required_props.as_ref(), Some(v) if v.is_empty());
    let dst_empty = matches!(ctx.dst_required_props.as_ref(), Some(v) if v.is_empty());
    let rowless = rowless_tail;
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
        return Ok(ColumnarOutcome::Empty);
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
    let vertices = read_destinations(
        reader,
        ctx.space_name,
        tail_tag,
        &all_dst,
        dst_bypass_opt.as_ref(),
    )?;
    let mut kept: Vec<usize> = Vec::new();
    for (i, vertex) in vertices.iter().enumerate() {
        if vertex.is_some() {
            kept.push(i);
        }
    }
    if kept.is_empty() {
        return Ok(ColumnarOutcome::Empty);
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
        let mut typed: Vec<TypedColumn> =
            Vec::with_capacity(seed_width + 2 + edge_bypass.len() + dst_bypass.len());
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
        for prop in &edge_bypass {
            typed.push(bypass_column(
                kept.iter()
                    .map(|&i| edge_bypass_value(&pending[i].edge, prop))
                    .collect(),
            ));
        }
        for prop in &dst_bypass {
            typed.push(bypass_column(
                kept.iter()
                    .map(|&i| dst_bypass_value(vertices.get(i).and_then(|v| v.as_ref()), prop))
                    .collect(),
            ));
        }
        check_rowless_lockstep(&output_layout, &typed, kept.len())?;
        let mut out = DataChunk::new_with_layout(Vec::new(), output_layout);
        out.typed_columns = Some(typed);
        return Ok(ColumnarOutcome::Produced(out));
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
        let mut row = Vec::with_capacity(seed_row.len() + 2 + edge_bypass.len() + dst_bypass.len());
        row.extend_from_slice(seed_row);
        row.push(edge_value);
        row.push(dst_value);
        for prop in &edge_bypass {
            row.push(edge_bypass_value(&req.edge, prop));
        }
        for prop in &dst_bypass {
            row.push(dst_bypass_value(
                vertices.get(i).and_then(|v| v.as_ref()),
                prop,
            ));
        }
        out_rows.push(row);
    }
    if out_rows.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    check_output_layout(
        &output_layout,
        seed_width + 2 + edge_bypass.len() + dst_bypass.len(),
        out_rows.first().map(Vec::len),
    )?;
    Ok(ColumnarOutcome::Produced(DataChunk::new_with_layout(
        out_rows,
        output_layout,
    )))
}

/// Variable-length frontier loop for `step_limits` ranges.
///
/// Walks depths 1..=max with per-depth edge reads so every emitted depth
/// carries its own edge header and destination. Only depths listed in
/// `step_limits` are emitted, matching `[*min..max]` union semantics.
/// Bypass columns assemble only for emitted depths; non-emitted depths only
/// advance the frontier without reading properties.
pub(super) fn expand_variable_frontier(
    chunk: &DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    step_limits: &[u32],
    ctx: &mut ExpandCtx,
) -> Result<ColumnarOutcome, QueryError> {
    if !ctx.closed_loop {
        return Ok(ColumnarOutcome::Declined);
    }
    if step_limits.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    if !src_vids.is_empty() || ctx.filter_expr.is_some() {
        return Ok(ColumnarOutcome::Declined);
    }
    if let Some(sem) = ctx.path_semantic.clone() {
        if !matches!(sem, PathSemantic::Walk) {
            return Ok(ColumnarOutcome::Declined);
        }
    }
    if !is_closed_loop_storage(
        reader,
        ctx.space_name,
        ctx.edge_types,
        ctx.direction,
        ctx.dst_tag,
    ) {
        return Ok(ColumnarOutcome::Declined);
    }
    let max_depth = *step_limits.iter().max().unwrap_or(&0);
    if max_depth == 0 {
        return Ok(ColumnarOutcome::Empty);
    }
    if max_depth > 64 {
        return Ok(ColumnarOutcome::Declined);
    }
    let wanted: std::collections::HashSet<u32> = step_limits.iter().copied().collect();
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);
    let seed_width = if chunk.rows.is_empty() {
        chunk.get_layout().len()
    } else {
        chunk.rows.first().map_or(chunk.get_layout().len(), |r| r.len())
    };
    let input_layout = chunk.get_layout();
    let (edge_bypass_opt, dst_bypass_opt) = hop_bypass(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &input_layout,
    );
    if ctx.skip_rows && (edge_bypass_opt.is_none() || dst_bypass_opt.is_none()) {
        return Err(QueryError::execution(
            "rowless expand without complete bypass columns violates the plan contract"
                .to_string(),
        ));
    }
    let conflict = has_bypass_conflict(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &input_layout,
    );
    let rowless_emit =
        ctx.skip_rows && edge_bypass_opt.is_some() && dst_bypass_opt.is_some() && !conflict;
    let edge_bypass = edge_bypass_opt.clone().unwrap_or_default();
    let dst_bypass = dst_bypass_opt.clone().unwrap_or_default();
    let input_typed = chunk.typed_columns.clone();
    let (seed_vids, seed_rows, seed_positions) = parse_seeds(chunk, seed_slot, &src_vids);
    if seed_vids.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    let projection = edge_projection(ctx.edge_required_props.clone().as_ref().map(|v| v));
    let projection_ref = projection.as_deref();
    let edge_empty = matches!(ctx.edge_required_props.as_ref(), Some(v) if v.is_empty());
    let dst_empty = matches!(ctx.dst_required_props.as_ref(), Some(v) if v.is_empty());
    let rowless = rowless_emit;
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
        if !wanted.contains(&depth) {
            // Non-emitting depth: advance the frontier by identifiers only,
            // without decoding edge properties or building pending state.
            let ids: Vec<VertexId> = frontier.iter().map(|(_, vid)| *vid).collect();
            let batches = reader.neighbor_dst_ids_batch(
                ctx.space_name,
                &ids,
                ctx.direction,
                ctx.edge_types,
            )?;
            for ((orig_idx, _), neighbors) in frontier.iter().zip(batches.iter()) {
                for dst in neighbors {
                    next.push((*orig_idx, *dst));
                }
            }
        } else {
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
        return Ok(ColumnarOutcome::Empty);
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
    let vertices = read_destinations(
        reader,
        ctx.space_name,
        var_tag,
        &all_dst,
        dst_bypass_opt.as_ref(),
    )?;
    let mut kept: Vec<usize> = Vec::new();
    for (i, vertex) in vertices.iter().enumerate() {
        if vertex.is_some() {
            kept.push(i);
        }
    }
    if kept.is_empty() {
        return Ok(ColumnarOutcome::Empty);
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
        let mut typed: Vec<TypedColumn> =
            Vec::with_capacity(seed_width + 2 + edge_bypass.len() + dst_bypass.len());
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
        for prop in &edge_bypass {
            typed.push(bypass_column(
                kept.iter()
                    .map(|&i| edge_bypass_value(&pending[i].edge, prop))
                    .collect(),
            ));
        }
        for prop in &dst_bypass {
            typed.push(bypass_column(
                kept.iter()
                    .map(|&i| dst_bypass_value(vertices.get(i).and_then(|v| v.as_ref()), prop))
                    .collect(),
            ));
        }
        check_rowless_lockstep(&output_layout, &typed, kept.len())?;
        let mut out = DataChunk::new_with_layout(Vec::new(), output_layout);
        out.typed_columns = Some(typed);
        return Ok(ColumnarOutcome::Produced(out));
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
        let mut row = Vec::with_capacity(seed_row.len() + 2 + edge_bypass.len() + dst_bypass.len());
        row.extend_from_slice(seed_row);
        row.push(edge_value);
        row.push(dst_value);
        for prop in &edge_bypass {
            row.push(edge_bypass_value(&req.edge, prop));
        }
        for prop in &dst_bypass {
            row.push(dst_bypass_value(
                vertices.get(i).and_then(|v| v.as_ref()),
                prop,
            ));
        }
        out_rows.push(row);
    }
    if out_rows.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    check_output_layout(
        &output_layout,
        seed_width + 2 + edge_bypass.len() + dst_bypass.len(),
        out_rows.first().map(Vec::len),
    )?;
    Ok(ColumnarOutcome::Produced(DataChunk::new_with_layout(
        out_rows,
        output_layout,
    )))
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
    let chunk = materialize_rowless_rows(chunk);
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
