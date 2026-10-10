use std::collections::HashMap;
use std::sync::Arc;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::{Edge, EdgeDirection, Value};

use super::expand_buffer::{visible_rows, ExpandOutputBuffer};
use super::expand_columnar::check_output_layout;
use super::expand_dispatch::{dst_bypass_value, edge_bypass_value, hop_bypass};
use super::expand_seeds::{
    identity_seed_ids, lightweight_seed_row, materialize_rowless_rows, seed_slot, seed_vid,
};
use super::ExpandCtx;

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
    let chunk = materialize_rowless_rows(chunk);
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let emit_raw_ids = ctx.emit_raw_ids;
    let lightweight_source = ctx.lightweight_source;
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
        // `Value::Vertex(Box)` / `Value::Edge(Box)` allocation. Any bypass
        // demand violates the plan contract: raw ids carry no properties.
        let edge_empty = matches!(ctx.edge_required_props.as_ref(), Some(v) if v.is_empty());
        let dst_empty = matches!(ctx.dst_required_props.as_ref(), Some(v) if v.is_empty());
        if !edge_empty || !dst_empty {
            return Err(QueryError::execution(
                "raw-id expand with property demands violates the plan contract".to_string(),
            ));
        }
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

    let mut out_rows = buf.finish();
    // Legacy row fallback also honors the bypass layout: demanded properties
    // ride the same flat suffix so the evaluator's compound-slot reads hit
    // whether the hop ran columnar or fell back here.
    let (edge_bypass_opt, dst_bypass_opt) = hop_bypass(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &chunk.get_layout(),
    );
    let edge_bypass = edge_bypass_opt.unwrap_or_default();
    let dst_bypass = dst_bypass_opt.unwrap_or_default();
    if !edge_bypass.is_empty() || !dst_bypass.is_empty() {
        let mut kept: Vec<&PendingExpand> = Vec::with_capacity(out_rows.len());
        for req in &pending {
            if vertex_map.contains_key(&(req.tag.clone(), req.dst)) {
                kept.push(req);
            }
        }
        if out_rows.len() != kept.len() {
            return Err(QueryError::execution(format!(
                "legacy expand rows {} do not match kept destinations {}",
                out_rows.len(),
                kept.len()
            )));
        }
        for (row, req) in out_rows.iter_mut().zip(kept.iter()) {
            for prop in &edge_bypass {
                row.push(edge_bypass_value(&req.edge, prop));
            }
            let dst_vertex = vertex_map.get(&(req.tag.clone(), req.dst));
            for prop in &dst_bypass {
                let value = match dst_vertex {
                    Some(Value::Vertex(vertex)) => dst_bypass_value(Some(vertex), prop),
                    _ => dst_bypass_value(None, prop),
                };
                row.push(value);
            }
        }
        check_output_layout(
            &output_layout,
            output_layout.len(),
            out_rows.first().map(Vec::len),
        )?;
    }
    if out_rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}
