use std::sync::Arc;

use crate::executor::streaming::chunk::{
    gather_typed_column, DataChunk, EdgeHeaderColumn, TypedColumn,
};
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::{Edge, EdgeDirection, EdgeHeader, Value, Vertex};

use super::expand_dispatch::{
    ColumnarOutcome, closed_loop_dst_tag, edge_projection, has_bypass_conflict, hop_bypass,
    bypass_column, dst_bypass_value, edge_bypass_value,
};
use super::expand_seeds::{parse_seeds, seed_slot};
use super::ExpandCtx;

/// Destination batch read honoring the bypass projection.
///
/// Non-empty destination demands decode only the demanded properties while
/// keeping the full batch's timestamp, input order and visibility: one entry
/// per id, `None` for missing vertices (dangling edges still drop the row).
/// Anything else keeps the full batch so boxed entity slots observe unchanged
/// records. Columnar paths never carry a hop filter when this runs, so the
/// projection only ever trims downstream-demanded properties the demand audit
/// already proved complete.
pub(super) fn read_destinations(
    reader: &dyn QueryStorage,
    space_name: &str,
    dst_tag: &str,
    ids: &[VertexId],
    dst_props: Option<&Vec<String>>,
) -> Result<Vec<Option<Vertex>>, QueryError> {
    if let Some(props) = dst_props {
        if !props.is_empty() {
            let mut sorted = props.clone();
            sorted.sort();
            sorted.dedup();
            let projection: Vec<std::sync::Arc<str>> = sorted
                .iter()
                .map(|s| std::sync::Arc::from(s.as_str()))
                .collect();
            return Ok(reader.get_vertices_projected_batch(
                space_name,
                dst_tag,
                ids,
                &projection,
            )?);
        }
    }
    Ok(reader.get_vertices_batch(space_name, dst_tag, ids)?)
}

/// Explicit layout lockstep check: typed arms and row width must match the
/// plan layout exactly instead of silently misaligning.
pub(super) fn check_output_layout(
    output_layout: &SlotLayout,
    typed_arms: usize,
    row_width: Option<usize>,
) -> Result<(), QueryError> {
    if typed_arms != output_layout.len() {
        return Err(QueryError::execution(format!(
            "expand typed arms {} do not match layout width {}",
            typed_arms,
            output_layout.len()
        )));
    }
    if let Some(width) = row_width {
        if width != output_layout.len() {
            return Err(QueryError::execution(format!(
                "expand row width {} does not match layout width {}",
                width,
                output_layout.len()
            )));
        }
    }
    Ok(())
}

/// Runtime lockstep check for rowless construction: every typed arm must
/// carry exactly `retained` rows and the arm count must match the layout.
/// Replaces scattered debug assertions so release builds fail explicitly
/// instead of misaligning downstream consumers.
pub(super) fn check_rowless_lockstep(
    output_layout: &SlotLayout,
    typed: &[TypedColumn],
    retained: usize,
) -> Result<(), QueryError> {
    if typed.len() != output_layout.len() {
        return Err(QueryError::execution(format!(
            "expand typed arms {} do not match layout width {}",
            typed.len(),
            output_layout.len()
        )));
    }
    for (idx, col) in typed.iter().enumerate() {
        if col.len() != retained {
            return Err(QueryError::execution(format!(
                "expand typed arm {idx} length {} does not match retained rows {retained}",
                col.len()
            )));
        }
    }
    Ok(())
}

/// Runtime lockstep check for row outputs that also carry typed arms: row
/// width, typed arm count and every arm length must agree with the layout
/// and the retained row count.
pub(super) fn check_row_lockstep(
    output_layout: &SlotLayout,
    typed: &[TypedColumn],
    rows: &[Vec<Value>],
) -> Result<(), QueryError> {
    let retained = rows.len();
    let width = rows.first().map(Vec::len);
    check_output_layout(output_layout, typed.len(), width)?;
    for (idx, col) in typed.iter().enumerate() {
        if col.len() != retained {
            return Err(QueryError::execution(format!(
                "expand typed arm {idx} length {} does not match retained rows {retained}",
                col.len()
            )));
        }
    }
    Ok(())
}

/// Destination id on one side of an edge for the hop direction.
pub(super) fn edge_neighbor(edge: &Edge, seed: &VertexId, direction: EdgeDirection) -> VertexId {
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
/// with identity shortcuts for undemanded slots. Demanded properties ride
/// flat bypass columns (`{var}.{prop}`) in layout order in both modes.
/// Declines shapes the columnar path does not serve so callers fall back to
/// the row path instead of dropping input.
pub(super) fn expand_single_step_columnar(
    chunk: &DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    ctx: &mut ExpandCtx,
) -> Result<ColumnarOutcome, QueryError> {
    if !ctx.closed_loop {
        return Ok(ColumnarOutcome::Declined);
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
    let rowless =
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
        return Ok(ColumnarOutcome::Empty);
    }
    let dst_tag_owned =
        closed_loop_dst_tag(reader, space_name, edge_types, direction, ctx.dst_tag)
            .unwrap_or_else(|| ctx.dst_tag.to_string());
    let dst_tag = dst_tag_owned.as_str();
    let vertices = read_destinations(
        reader,
        space_name,
        dst_tag,
        &all_dst,
        dst_bypass_opt.as_ref(),
    )?;
    let mut kept: Vec<usize> = Vec::with_capacity(pending.len());
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
            gather_indices.push(seed_positions[req.seed_idx]);
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
        edge_src.push(*req.edge.src());
        edge_dst.push(*req.edge.dst());
        edge_types_out.push(req.edge.edge_type().to_string());
        rankings.push(req.edge.ranking());
        dst_ids.push(req.dst);
        gather_indices.push(seed_positions[req.seed_idx]);
    }
    if out_rows.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    check_output_layout(
        &output_layout,
        seed_width + 2 + edge_bypass.len() + dst_bypass.len(),
        out_rows.first().map(Vec::len),
    )?;
    let mut out = DataChunk::new_with_layout(out_rows, Arc::clone(&output_layout));
    // Seed arms without an input typed column mirror the row view exactly, so
    // row-based and typed consumers observe the same seed values.
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
    let bypass_base = seed_width + 2;
    for (j, _) in edge_bypass.iter().enumerate() {
        typed.push(bypass_column(
            out.rows
                .iter()
                .map(|r| {
                    r.get(bypass_base + j)
                        .cloned()
                        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                })
                .collect(),
        ));
    }
    for (j, _) in dst_bypass.iter().enumerate() {
        typed.push(bypass_column(
            out.rows
                .iter()
                .map(|r| {
                    r.get(bypass_base + edge_bypass.len() + j)
                        .cloned()
                        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                })
                .collect(),
        ));
    }
    check_row_lockstep(&output_layout, &typed, &out.rows)?;
    out.typed_columns = Some(typed);
    Ok(ColumnarOutcome::Produced(out))
}
