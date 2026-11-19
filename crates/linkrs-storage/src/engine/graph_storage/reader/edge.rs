use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::edge::CsrBase;
use crate::edge::EdgeRecord;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    edge_record_to_edge, edge_record_to_edge_projected, endpoint_label_id, serialize_properties,
};
use crate::engine::params::EdgeOperationParams;
use crate::EdgeTopology;
use linkrs_core::types::{EdgeTypeInfo, LabelId, Timestamp, VertexId};
use linkrs_core::{Edge, EdgeDirection, StorageError, StorageResult, Value};

use crate::engine::graph_storage::reader::ttl_filter::edge_is_expired;
use crate::engine::graph_storage::reader::utils::*;

pub(crate) fn get_edge(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
) -> StorageResult<Option<Edge>> {
    get_edge_impl(ctx, space, src, dst, edge_type, rank, None)
}

pub(crate) fn get_edge_projected(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
    projection: &[Arc<str>],
) -> StorageResult<Option<Edge>> {
    get_edge_impl(ctx, space, src, dst, edge_type, rank, Some(projection))
}

fn get_edge_impl(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
    projection: Option<&[Arc<str>]>,
) -> StorageResult<Option<Edge>> {
    record_schema_read(ctx, space);
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let src = VertexId::normalize_for_vid_type(&space_info.vid_type, *src)?;
    let dst = VertexId::normalize_for_vid_type(&space_info.vid_type, *dst)?;
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let ts = ctx.get_read_timestamp();

    let edge_label_id = edge_info.edge_type_id;
    let src_label_id = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    let dst_label_id = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    record_edge_read(
        ctx,
        linkrs_core::types::EdgeIdentifier::new(
            src_label_id,
            src,
            dst_label_id,
            dst,
            edge_label_id,
            rank,
        ),
    );
    // Projection pushdown: `None` fetches all columns, `Some` decodes only the
    // requested columns (`Some(&[])` is topology-only with zero property decode).
    // Query empty projection means all columns and never reaches the projected
    // branch; topology-only is requested explicitly by batch/cursor paths.
    let params = EdgeOperationParams {
        edge_label: edge_label_id,
        src_label: src_label_id,
        src_id: src,
        dst_label: dst_label_id,
        dst_id: dst,
        rank,
    };
    let record = match projection {
        Some(projection) => ctx.get_edge_projected(&params, ts, Some(projection)),
        None => ctx.get_edge(&params, ts),
    };
    if let Some(record) = record {
        let edge = edge_record_to_edge_with_projection(record, edge_type, src, dst, projection);
        if edge_is_expired(&edge_info, &edge) {
            return Ok(None);
        }
        return Ok(Some(edge));
    }

    Ok(None)
}

/// Materialize an edge record, honoring an optional property projection.
fn edge_record_to_edge_with_projection(
    record: EdgeRecord,
    edge_type: &str,
    src_vid: VertexId,
    dst_vid: VertexId,
    projection: Option<&[Arc<str>]>,
) -> Edge {
    match projection {
        Some(projection) => {
            edge_record_to_edge_projected(record, edge_type, src_vid, dst_vid, projection)
        }
        None => edge_record_to_edge(record, edge_type, src_vid, dst_vid),
    }
}

/// Projected and limit-aware per-node edge fanout.
/// `projection`: `None` = all columns, `Some` = only listed columns
/// (`Some(&[])` = topology only). `limit`: `None` = unlimited, `Some(k)` =
/// first-`k` visible edges per direction branch via gate-aware limit pushdown.
/// `edge_types`: empty = all types, otherwise only matching tables are
/// visited so a single-type traversal never touches unrelated tables.
pub(crate) fn get_node_edges(
    ctx: &GraphStorageContext,
    space: &str,
    node_id: &VertexId,
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<Edge>> {
    get_node_edges_projected(ctx, space, node_id, direction, edge_types, None, None)
}

pub(crate) fn get_node_edges_projected(
    ctx: &GraphStorageContext,
    space: &str,
    node_id: &VertexId,
    direction: EdgeDirection,
    edge_types: &[String],
    projection: Option<&[Arc<str>]>,
    limit: Option<usize>,
) -> StorageResult<Vec<Edge>> {
    record_schema_read(ctx, space);
    record_vertex_read(ctx, *node_id);
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let node_vid = VertexId::normalize_for_vid_type(&space_info.vid_type, *node_id)?;
    let edge_types_all = ctx.schema_manager().list_edge_types(space)?;
    if edge_types_all.is_empty() {
        return Ok(Vec::new());
    }
    let ts = ctx.get_read_timestamp();
    let mut edges = Vec::new();
    for edge_info in &edge_types_all {
        if !edge_types.is_empty() && !edge_types.contains(&edge_info.edge_type_name) {
            continue;
        }
        let edge_label_id = edge_info.edge_type_id;
        let edge_type_name = &edge_info.edge_type_name;
        let Some(src_label_id) = endpoint_label_id(ctx, space, &edge_info.src_tag_name)? else {
            continue;
        };
        let Some(dst_label_id) = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? else {
            continue;
        };
        let remaining = limit.map(|l| l.saturating_sub(edges.len()));
        if remaining == Some(0) {
            break;
        }
        match direction {
            EdgeDirection::Out => {
                let records = match remaining {
                    Some(limit) => ctx
                        .out_edges_projected_limit(
                            edge_label_id,
                            src_label_id,
                            node_vid,
                            ts,
                            projection,
                            limit,
                        )
                        .unwrap_or_default(),
                    None => ctx
                        .out_edges_projected(edge_label_id, src_label_id, node_vid, ts, projection)
                        .unwrap_or_default(),
                };
                for record in records {
                    let dst = record.dst_vid;
                    let edge = edge_record_to_edge_with_projection(
                        record,
                        edge_type_name,
                        node_vid,
                        dst,
                        projection,
                    );
                    if edge_is_expired(edge_info, &edge) {
                        continue;
                    }
                    edges.push(edge);
                }
            }
            EdgeDirection::In => {
                let records = match remaining {
                    Some(limit) => ctx
                        .in_edges_projected_limit(
                            edge_label_id,
                            dst_label_id,
                            node_vid,
                            ts,
                            projection,
                            limit,
                        )
                        .unwrap_or_default(),
                    None => ctx
                        .in_edges_projected(edge_label_id, dst_label_id, node_vid, ts, projection)
                        .unwrap_or_default(),
                };
                for record in records {
                    let src = record.src_vid;
                    let edge = edge_record_to_edge_with_projection(
                        record,
                        edge_type_name,
                        src,
                        node_vid,
                        projection,
                    );
                    if edge_is_expired(edge_info, &edge) {
                        continue;
                    }
                    edges.push(edge);
                }
            }
            EdgeDirection::Both => {
                // A self-loop lives in both legs, so the out pass and the in
                // pass below would emit it twice. Deduplicate on the logical
                // edge key, matching the batch neighbor paths.
                let mut seen: HashSet<(VertexId, VertexId, i64)> = HashSet::new();
                let out_records = match remaining {
                    Some(limit) => ctx
                        .out_edges_projected_limit(
                            edge_label_id,
                            src_label_id,
                            node_vid,
                            ts,
                            projection,
                            limit,
                        )
                        .unwrap_or_default(),
                    None => ctx
                        .out_edges_projected(edge_label_id, src_label_id, node_vid, ts, projection)
                        .unwrap_or_default(),
                };
                for record in out_records {
                    if !seen.insert((node_vid, record.dst_vid, record.rank)) {
                        continue;
                    }
                    let dst = record.dst_vid;
                    let edge = edge_record_to_edge_with_projection(
                        record,
                        edge_type_name,
                        node_vid,
                        dst,
                        projection,
                    );
                    if edge_is_expired(edge_info, &edge) {
                        continue;
                    }
                    edges.push(edge);
                }
                let remaining = limit.map(|l| l.saturating_sub(edges.len()));
                if remaining == Some(0) {
                    break;
                }
                let in_records = match remaining {
                    Some(limit) => ctx
                        .in_edges_projected_limit(
                            edge_label_id,
                            dst_label_id,
                            node_vid,
                            ts,
                            projection,
                            limit,
                        )
                        .unwrap_or_default(),
                    None => ctx
                        .in_edges_projected(edge_label_id, dst_label_id, node_vid, ts, projection)
                        .unwrap_or_default(),
                };
                for record in in_records {
                    if !seen.insert((record.src_vid, node_vid, record.rank)) {
                        continue;
                    }
                    let src = record.src_vid;
                    let edge = edge_record_to_edge_with_projection(
                        record,
                        edge_type_name,
                        src,
                        node_vid,
                        projection,
                    );
                    if edge_is_expired(edge_info, &edge) {
                        continue;
                    }
                    edges.push(edge);
                }
            }
        }
        if let Some(l) = limit {
            if edges.len() >= l {
                edges.truncate(l);
                break;
            }
        }
    }
    Ok(edges)
}

/// Exact projected edge fanout for a batch of source vertices.
///
/// Schema resolution and edge-table locking are shared across the whole
/// batch; per-seed ordering, property projection and TTL filtering match
/// repeated [`get_node_edges_projected`] calls.
pub(crate) fn get_node_edges_projected_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
    projection: Option<&[Arc<str>]>,
) -> StorageResult<Vec<Vec<Edge>>> {
    record_schema_read(ctx, space);
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let normalized: Vec<VertexId> = src_ids
        .iter()
        .map(|id| VertexId::normalize_for_vid_type(&space_info.vid_type, *id))
        .collect::<Result<_, _>>()?;
    let edge_types_all = ctx.schema_manager().list_edge_types(space)?;
    if edge_types_all.is_empty() {
        return Ok(vec![Vec::new(); normalized.len()]);
    }
    let ts = ctx.get_read_timestamp();
    for id in &normalized {
        record_vertex_read(ctx, *id);
    }
    let mut out: Vec<Vec<Edge>> = vec![Vec::new(); normalized.len()];

    for edge_info in &edge_types_all {
        if !edge_types.is_empty() && !edge_types.contains(&edge_info.edge_type_name) {
            continue;
        }
        let Some(src_label_id) = endpoint_label_id(ctx, space, &edge_info.src_tag_name)? else {
            continue;
        };
        let Some(dst_label_id) = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? else {
            continue;
        };
        let edge_label_id = edge_info.edge_type_id;
        let edge_type_name = edge_info.edge_type_name.as_str();
        match direction {
            EdgeDirection::Out => {
                let batches = ctx.out_edges_projected_batch(
                    edge_label_id,
                    src_label_id,
                    &normalized,
                    ts,
                    projection,
                );
                for (index, records) in batches.into_iter().enumerate() {
                    for record in records {
                        let dst = record.dst_vid;
                        let edge = edge_record_to_edge_with_projection(
                            record,
                            edge_type_name,
                            normalized[index],
                            dst,
                            projection,
                        );
                        if !edge_is_expired(edge_info, &edge) {
                            out[index].push(edge);
                        }
                    }
                }
            }
            EdgeDirection::In => {
                let batches = ctx.in_edges_projected_batch(
                    edge_label_id,
                    dst_label_id,
                    &normalized,
                    ts,
                    projection,
                );
                for (index, records) in batches.into_iter().enumerate() {
                    for record in records {
                        let src = record.src_vid;
                        let edge = edge_record_to_edge_with_projection(
                            record,
                            edge_type_name,
                            src,
                            normalized[index],
                            projection,
                        );
                        if !edge_is_expired(edge_info, &edge) {
                            out[index].push(edge);
                        }
                    }
                }
            }
            EdgeDirection::Both => {
                let out_batches = ctx.out_edges_projected_batch(
                    edge_label_id,
                    src_label_id,
                    &normalized,
                    ts,
                    projection,
                );
                let in_batches = ctx.in_edges_projected_batch(
                    edge_label_id,
                    dst_label_id,
                    &normalized,
                    ts,
                    projection,
                );
                for (index, (out_records, in_records)) in
                    out_batches.into_iter().zip(in_batches).enumerate()
                {
                    let mut seen: HashSet<(VertexId, VertexId, i64)> = HashSet::new();
                    for record in out_records {
                        let dst = record.dst_vid;
                        let rank = record.rank;
                        if !seen.insert((normalized[index], dst, rank)) {
                            continue;
                        }
                        let edge = edge_record_to_edge_with_projection(
                            record,
                            edge_type_name,
                            normalized[index],
                            dst,
                            projection,
                        );
                        if !edge_is_expired(edge_info, &edge) {
                            out[index].push(edge);
                        }
                    }
                    for record in in_records {
                        let src = record.src_vid;
                        let rank = record.rank;
                        if !seen.insert((src, normalized[index], rank)) {
                            continue;
                        }
                        let edge = edge_record_to_edge_with_projection(
                            record,
                            edge_type_name,
                            src,
                            normalized[index],
                            projection,
                        );
                        if !edge_is_expired(edge_info, &edge) {
                            out[index].push(edge);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Topology-only batch adjacency read.
///
/// Mirrors the ordering, edge-type filtering and direction dedup of
/// `get_node_edges_projected_batch`, but converts CSR records straight into
/// shared topology identities. No property decode or `Edge` allocation is
/// performed, which is the shape columnar expand needs when only endpoints
/// and rank are consumed.
pub(crate) fn get_node_edge_headers_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    handles: Option<&[Option<(u32, LabelId)>]>,
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<Vec<EdgeTopology>>> {
    record_schema_read(ctx, space);
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let normalized: Vec<VertexId> = src_ids
        .iter()
        .map(|id| VertexId::normalize_for_vid_type(&space_info.vid_type, *id))
        .collect::<Result<_, _>>()?;
    let edge_types_all = ctx.schema_manager().list_edge_types(space)?;
    if edge_types_all.is_empty() {
        return Ok(vec![Vec::new(); normalized.len()]);
    }
    let ts = ctx.get_read_timestamp();
    for id in &normalized {
        record_vertex_read(ctx, *id);
    }
    let mut out: Vec<Vec<EdgeTopology>> = vec![Vec::new(); normalized.len()];
    // A traversal batch reaches a small set of destinations many times.
    // Translate each internal endpoint once per call instead of once per
    // CSR slot, while keeping the CSR visit order unchanged.
    let mut translated: HashMap<(LabelId, u32), VertexId> = HashMap::new();

    for edge_info in &edge_types_all {
        if !edge_types.is_empty() && !edge_types.contains(&edge_info.edge_type_name) {
            continue;
        }
        let Some(src_label_id) = endpoint_label_id(ctx, space, &edge_info.src_tag_name)? else {
            continue;
        };
        let Some(dst_label_id) = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? else {
            continue;
        };
        let edge_label_id = edge_info.edge_type_id;
        let edge_type: Arc<str> = Arc::from(edge_info.edge_type_name.as_str());
        match direction {
            EdgeDirection::Out => {
                let batches =
                    ctx.out_topology_batch(edge_label_id, src_label_id, &normalized, handles, ts);
                for (index, hots) in batches.into_iter().enumerate() {
                    for (far_label, hot) in hots {
                        let dst =
                            *translated
                                .entry((far_label, hot.endpoint))
                                .or_insert_with(|| {
                                    ctx.endpoint_to_external(
                                        far_label,
                                        VertexId::from_u32(hot.endpoint),
                                        ts,
                                    )
                                });
                        out[index].push(EdgeTopology {
                            src: normalized[index],
                            dst,
                            edge_type: Arc::clone(&edge_type),
                            ranking: hot.rank,
                        });
                    }
                }
            }
            EdgeDirection::In => {
                let batches = ctx.in_topology_batch(edge_label_id, dst_label_id, &normalized, ts);
                for (index, hots) in batches.into_iter().enumerate() {
                    for (far_label, hot) in hots {
                        let src =
                            *translated
                                .entry((far_label, hot.endpoint))
                                .or_insert_with(|| {
                                    ctx.endpoint_to_external(
                                        far_label,
                                        VertexId::from_u32(hot.endpoint),
                                        ts,
                                    )
                                });
                        out[index].push(EdgeTopology {
                            src,
                            dst: normalized[index],
                            edge_type: Arc::clone(&edge_type),
                            ranking: hot.rank,
                        });
                    }
                }
            }
            EdgeDirection::Both => {
                let out_batches =
                    ctx.out_topology_batch(edge_label_id, src_label_id, &normalized, handles, ts);
                let in_batches =
                    ctx.in_topology_batch(edge_label_id, dst_label_id, &normalized, ts);
                for (index, (out_hots, in_hots)) in
                    out_batches.into_iter().zip(in_batches).enumerate()
                {
                    let mut seen: HashSet<(VertexId, VertexId, i64)> = HashSet::new();
                    for (far_label, hot) in out_hots {
                        let dst =
                            *translated
                                .entry((far_label, hot.endpoint))
                                .or_insert_with(|| {
                                    ctx.endpoint_to_external(
                                        far_label,
                                        VertexId::from_u32(hot.endpoint),
                                        ts,
                                    )
                                });
                        let key = (normalized[index], dst, hot.rank);
                        if !seen.insert(key) {
                            continue;
                        }
                        out[index].push(EdgeTopology {
                            src: normalized[index],
                            dst,
                            edge_type: Arc::clone(&edge_type),
                            ranking: hot.rank,
                        });
                    }
                    for (far_label, hot) in in_hots {
                        let src =
                            *translated
                                .entry((far_label, hot.endpoint))
                                .or_insert_with(|| {
                                    ctx.endpoint_to_external(
                                        far_label,
                                        VertexId::from_u32(hot.endpoint),
                                        ts,
                                    )
                                });
                        let key = (src, normalized[index], hot.rank);
                        if !seen.insert(key) {
                            continue;
                        }
                        out[index].push(EdgeTopology {
                            src,
                            dst: normalized[index],
                            edge_type: Arc::clone(&edge_type),
                            ranking: hot.rank,
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Lightweight batch neighbor read used by de-materialized expand hops
/// (`id_only`/`count_only`).
///
/// Resolves the edge-type schema once for the whole batch and reads MVCC
/// neighbors directly from the node-group sharded CSR (skipping `EdgeRecord`
/// materialization and per-edge property decoding). Returns the
/// external destination/source `VertexId` per input source, in input order.
pub(crate) fn neighbor_dst_ids_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<Vec<VertexId>>> {
    record_schema_read(ctx, space);
    let edge_type_infos = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_read_timestamp();
    // Resolve the edge-type schema once for the whole batch.
    let mut resolved = Vec::new();
    for edge_info in &edge_type_infos {
        if !edge_types.is_empty() && !edge_types.contains(&edge_info.edge_type_name) {
            continue;
        }
        let Some(src_label_id) = endpoint_label_id(ctx, space, &edge_info.src_tag_name)? else {
            continue;
        };
        let Some(dst_label_id) = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? else {
            continue;
        };
        resolved.push((edge_info.edge_type_id, src_label_id, dst_label_id));
    }

    for src_id in src_ids {
        record_vertex_read(ctx, *src_id);
    }
    let mut staged: Vec<Vec<(LabelId, u32)>> = vec![Vec::new(); src_ids.len()];
    let mut seen: Vec<HashSet<(u32, u32, i64)>> =
        (0..src_ids.len()).map(|_| HashSet::new()).collect();
    for (edge_label_id, src_label_id, dst_label_id) in &resolved {
        stage_hot_neighbors(
            ctx,
            &mut staged,
            &mut seen,
            src_ids,
            *edge_label_id,
            *src_label_id,
            *dst_label_id,
            direction,
            ts,
        );
    }
    let mut results = Vec::with_capacity(src_ids.len());
    for staged_seed in &staged {
        let mut neighbors = Vec::with_capacity(staged_seed.len());
        for (far_label, endpoint) in staged_seed {
            if let Some(ext) = internal_to_external_vertex_id(ctx, *far_label, *endpoint, ts) {
                neighbors.push(ext);
            }
        }
        results.push(neighbors);
    }
    Ok(results)
}

/// Batch out-degree read for count-only expand tails. Counts distinct edges
/// per source with the schema resolved once for the whole batch.
pub(crate) fn out_degree_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<usize>> {
    record_schema_read(ctx, space);
    let edge_type_infos = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_read_timestamp();

    let mut resolved = Vec::new();
    for edge_info in &edge_type_infos {
        if !edge_types.is_empty() && !edge_types.contains(&edge_info.edge_type_name) {
            continue;
        }
        let Some(src_label_id) = endpoint_label_id(ctx, space, &edge_info.src_tag_name)? else {
            continue;
        };
        let Some(dst_label_id) = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? else {
            continue;
        };
        resolved.push((edge_info.edge_type_id, src_label_id, dst_label_id));
    }

    for src_id in src_ids {
        record_vertex_read(ctx, *src_id);
    }
    // Single-partition single-direction batches visit each edge exactly
    // once, so per-seed dedup sets only add allocation overhead. Count the
    // gated adjacency callbacks directly; multi-partition or both-direction
    // batches keep the dedup path since self-loops and cross-partition
    // duplicates genuinely repeat there.
    if resolved.len() == 1 && !matches!(direction, EdgeDirection::Both) {
        let (edge_label_id, src_label_id, dst_label_id) = resolved[0];
        let mut counts = vec![0usize; src_ids.len()];
        if matches!(direction, EdgeDirection::Out) {
            ctx.visit_out_nbrs_batched(
                edge_label_id,
                src_label_id,
                dst_label_id,
                src_ids,
                ts,
                |index, _, _, _| {
                    counts[index] += 1;
                },
            );
        } else {
            ctx.visit_in_nbrs_batched(
                edge_label_id,
                src_label_id,
                dst_label_id,
                src_ids,
                ts,
                |index, _, _, _| {
                    counts[index] += 1;
                },
            );
        }
        return Ok(counts);
    }
    let mut seen: Vec<HashSet<(u32, u32, i64)>> =
        (0..src_ids.len()).map(|_| HashSet::new()).collect();
    for (edge_label_id, src_label_id, dst_label_id) in &resolved {
        count_hot_neighbors_batched(
            ctx,
            &mut seen,
            src_ids,
            *edge_label_id,
            *src_label_id,
            *dst_label_id,
            direction,
            ts,
        );
    }
    Ok(seen.iter().map(HashSet::len).collect())
}

/// Fused two-hop degree read for count-only walks.
///
/// For one resolved edge type and one direction the first pass stages raw
/// internal neighbor endpoints, and the second pass counts their tail degrees
/// by row id. External ids are resolved once per batch to keep dangling-neighbor
/// visibility, but are not materialized into query rows. Other schemas fall
/// back to the portable neighbor plus degree calls so their dedup rules stay
/// unchanged.
pub(crate) fn neighbor_degree_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<usize>> {
    record_schema_read(ctx, space);
    let edge_type_infos = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_read_timestamp();
    let mut resolved = Vec::new();
    for edge_info in &edge_type_infos {
        if !edge_types.is_empty() && !edge_types.contains(&edge_info.edge_type_name) {
            continue;
        }
        let Some(src_label_id) = endpoint_label_id(ctx, space, &edge_info.src_tag_name)? else {
            continue;
        };
        let Some(dst_label_id) = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? else {
            continue;
        };
        resolved.push((edge_info.edge_type_id, src_label_id, dst_label_id));
    }
    for src_id in src_ids {
        record_vertex_read(ctx, *src_id);
    }
    if resolved.len() != 1 || matches!(direction, EdgeDirection::Both) {
        return portable_neighbor_degree(ctx, space, src_ids, direction, edge_types);
    }
    let (edge_label, src_label, dst_label) = resolved[0];
    let mut staged: Vec<Vec<(LabelId, u32)>> = vec![Vec::new(); src_ids.len()];
    if direction == EdgeDirection::Out {
        ctx.visit_out_nbrs_batched(
            edge_label,
            src_label,
            dst_label,
            src_ids,
            ts,
            |index, _, label, nbr| {
                staged[index].push((label, nbr.endpoint));
            },
        );
    } else {
        ctx.visit_in_nbrs_batched(
            edge_label,
            src_label,
            dst_label,
            src_ids,
            ts,
            |index, _, label, nbr| {
                staged[index].push((label, nbr.endpoint));
            },
        );
    }
    let neighbor_label = if direction == EdgeDirection::Out {
        dst_label
    } else {
        src_label
    };
    if staged
        .iter()
        .flatten()
        .any(|(label, _)| *label != neighbor_label)
    {
        return portable_neighbor_degree(ctx, space, src_ids, direction, edge_types);
    }
    // Keep the portable path's dangling-neighbor visibility with one batched
    // resolution, then record the same read keys when a recorder is active.
    let mut raw_offsets = Vec::with_capacity(src_ids.len() + 1);
    let mut raw_endpoints = Vec::new();
    for neighbors in &staged {
        raw_offsets.push(raw_endpoints.len());
        raw_endpoints.extend(neighbors.iter().map(|(_, endpoint)| *endpoint));
    }
    raw_offsets.push(raw_endpoints.len());
    let external = ctx.external_ids_by_internal_batch(neighbor_label, &raw_endpoints, ts);
    let recording = ctx.mutation_recorder().is_some();
    let mut offsets = Vec::with_capacity(src_ids.len() + 1);
    let mut endpoints = Vec::with_capacity(raw_endpoints.len());
    for (index, neighbors) in staged.iter().enumerate() {
        offsets.push(endpoints.len());
        let start = raw_offsets[index];
        for (position, (_, endpoint)) in neighbors.iter().enumerate() {
            if let Some(ext) = external[start + position] {
                if recording {
                    record_vertex_read(ctx, ext);
                }
                endpoints.push(*endpoint);
            }
        }
    }
    offsets.push(endpoints.len());
    let degrees = if direction == EdgeDirection::Out {
        ctx.count_out_nbrs_by_internal(edge_label, neighbor_label, &endpoints, ts)
    } else {
        ctx.count_in_nbrs_by_internal(edge_label, neighbor_label, &endpoints, ts)
    };
    let mut totals = Vec::with_capacity(src_ids.len());
    for pair in offsets.windows(2) {
        totals.push(degrees[pair[0]..pair[1]].iter().sum());
    }
    Ok(totals)
}

fn portable_neighbor_degree(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<usize>> {
    let neighbors = neighbor_dst_ids_batch(ctx, space, src_ids, direction, edge_types)?;
    let lengths: Vec<usize> = neighbors.iter().map(Vec::len).collect();
    let flat: Vec<VertexId> = neighbors.into_iter().flatten().collect();
    let degrees = out_degree_batch(ctx, space, &flat, direction, edge_types)?;
    let mut totals = Vec::with_capacity(src_ids.len());
    let mut offset = 0;
    for length in lengths {
        let sum: usize = degrees[offset..offset + length].iter().sum();
        totals.push(sum);
        offset += length;
    }
    Ok(totals)
}

/// Stage hot-CSR neighbors for a whole seed batch, one edge type at a time.
///
/// Batch counterpart of the former per-seed staging: each edge table is
/// locked once for all seeds instead of once per seed. Deduplicated by
/// `(src, dst, rank)` across tables into the per-seed `seen` sets, which the
/// caller keeps alive across edge types.
///
/// Two-phase to avoid holding an edge-table read lock while resolving
/// vertex ids: the adjacency callback only stages `(far_label, endpoint)`
/// pairs, and external resolution runs after the edge lock is released.
#[allow(clippy::too_many_arguments)]
fn stage_hot_neighbors(
    ctx: &GraphStorageContext,
    staged: &mut [Vec<(LabelId, u32)>],
    seen: &mut [HashSet<(u32, u32, i64)>],
    seeds: &[VertexId],
    edge_label_id: LabelId,
    src_label_id: LabelId,
    dst_label_id: LabelId,
    direction: EdgeDirection,
    ts: Timestamp,
) {
    if direction == EdgeDirection::Out || direction == EdgeDirection::Both {
        ctx.visit_out_nbrs_batched(
            edge_label_id,
            src_label_id,
            dst_label_id,
            seeds,
            ts,
            |index, src_internal, far_label, nbr| {
                if seen[index].insert((src_internal, nbr.endpoint, nbr.rank)) {
                    staged[index].push((far_label, nbr.endpoint));
                }
            },
        );
    }
    if direction == EdgeDirection::In || direction == EdgeDirection::Both {
        ctx.visit_in_nbrs_batched(
            edge_label_id,
            src_label_id,
            dst_label_id,
            seeds,
            ts,
            |index, dst_internal, far_label, nbr| {
                if seen[index].insert((nbr.endpoint, dst_internal, nbr.rank)) {
                    staged[index].push((far_label, nbr.endpoint));
                }
            },
        );
    }
}

/// Count hot-CSR neighbors for a whole seed batch into the per-seed `seen`
/// sets (dedup by full edge identity `(src_internal, dst_internal, rank)`).
///
/// Batch counterpart of the former per-seed counting with the same
/// single-lock-per-table behavior as [`stage_hot_neighbors`].
#[allow(clippy::too_many_arguments)]
fn count_hot_neighbors_batched(
    ctx: &GraphStorageContext,
    seen: &mut [HashSet<(u32, u32, i64)>],
    seeds: &[VertexId],
    edge_label_id: LabelId,
    src_label_id: LabelId,
    dst_label_id: LabelId,
    direction: EdgeDirection,
    ts: Timestamp,
) {
    if direction == EdgeDirection::Out || direction == EdgeDirection::Both {
        ctx.visit_out_nbrs_batched(
            edge_label_id,
            src_label_id,
            dst_label_id,
            seeds,
            ts,
            |index, src_internal, _far_label, nbr| {
                let dst_internal_vid = VertexId::from_u32(nbr.endpoint);
                if let Some(dst_internal) = dst_internal_vid.as_int64() {
                    seen[index].insert((src_internal, dst_internal as u32, nbr.rank));
                }
            },
        );
    }
    if direction == EdgeDirection::In || direction == EdgeDirection::Both {
        ctx.visit_in_nbrs_batched(
            edge_label_id,
            src_label_id,
            dst_label_id,
            seeds,
            ts,
            |index, dst_internal, _far_label, nbr| {
                let src_internal_vid = VertexId::from_u32(nbr.endpoint);
                if let Some(src_internal) = src_internal_vid.as_int64() {
                    seen[index].insert((src_internal as u32, dst_internal, nbr.rank));
                }
            },
        );
    }
}

/// Internal page size for the default full scan: bounds the intermediate
/// while the cursor stays the single scan implementation.
pub(crate) const SCAN_PAGE: usize = 1024;

pub(crate) fn scan_edges_by_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<Vec<Edge>> {
    record_schema_read(ctx, space);
    // Label ids for read-set recording below. The cursor returns external
    // vertex ids, which is also the form the write path records, so
    // read/write conflict keys finally meet instead of missing by form.
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;
    let edge_label_id = edge_info.edge_type_id;
    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    // The paginated cursor is the single scan implementation: one cursor
    // drains in bounded batches instead of materializing every record up
    // front. A fresh cursor per page would re-walk the offset each time;
    // draining one cursor keeps the full scan linear.
    let mut cursor = super::super::cursor_impl::edge::create_edge_cursor(
        Arc::new(ctx.clone()),
        space,
        &crate::cursor::ScanOptions {
            edge_type: Some(edge_type.to_string()),
            ..Default::default()
        },
    )?;
    let mut edges = Vec::new();
    loop {
        let batch = cursor.next_batch(SCAN_PAGE)?;
        if batch.is_empty() {
            break;
        }
        for edge in batch {
            if edge_is_expired(&edge_info, &edge) {
                continue;
            }
            record_edge_read(
                ctx,
                linkrs_core::types::EdgeIdentifier::new(
                    src_label_id,
                    edge.src,
                    dst_label_id,
                    edge.dst,
                    edge_label_id,
                    edge.ranking,
                ),
            );
            edges.push(edge);
        }
    }
    Ok(edges)
}

/// Table-sharded parallel full scan of one edge type.
///
/// Splits the type's edge tables across `shards` rayon shards (one cursor
/// per shard, each with its own table locks) and concatenates the shards in
/// key order, so the result multiset matches [`scan_edges_by_type`]. Read-set
/// recording runs once, serially, over the collected edges with the same
/// identifier mapping as the serial scan.
///
/// Shards share one read timestamp captured up front, so concurrent commits
/// cannot skew shards across snapshots. With fewer than two tables (or fewer
/// than two shards) this falls back to the serial scan: a single table has
/// no fan-out to parallelize, and its speedup row is expected to stay at 1.
pub(crate) fn scan_edges_by_type_parallel(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    shards: usize,
) -> StorageResult<Vec<Edge>> {
    use crate::engine::data_store::EdgeTableKey;
    use rayon::prelude::*;

    record_schema_read(ctx, space);
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;
    let edge_label_id = edge_info.edge_type_id;
    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    // Same table enumeration as the cursor target resolution: unconstrained
    // types (no src/dst tag) fan out over every table carrying the label, a
    // constrained type is exactly one table and stays serial.
    let mut keys: Vec<EdgeTableKey> = if src_label_id == 0 && dst_label_id == 0 {
        ctx.data_store().with_edge_tables(|tables| {
            tables
                .iter()
                .filter(|(_, arc)| arc.read().label() == edge_label_id)
                .map(|(key, _)| *key)
                .collect()
        })
    } else {
        vec![EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id)]
    };
    keys.sort_by_key(|key| (key.src_label, key.dst_label, key.edge_label));
    let width = shards.max(1).min(keys.len().max(1));
    if keys.len() < 2 || width < 2 {
        return scan_edges_by_type(ctx, space, edge_type);
    }
    // One snapshot for every shard: capture the read timestamp before the
    // fan-out so shards cannot observe different commit frontiers.
    let ts = ctx.get_read_timestamp();
    let chunk = keys.len().div_ceil(width);
    let key_groups: Vec<Vec<EdgeTableKey>> =
        keys.chunks(chunk).map(<[EdgeTableKey]>::to_vec).collect();
    let per_shard: Vec<Vec<Edge>> = key_groups
        .into_par_iter()
        .map(|group| {
            let options = crate::cursor::ScanOptions {
                edge_type: Some(edge_type.to_string()),
                edge_table_allowlist: Some(group),
                read_timestamp: Some(ts),
                ..Default::default()
            };
            let mut cursor = super::super::cursor_impl::edge::create_edge_cursor(
                Arc::new(ctx.clone()),
                space,
                &options,
            )?;
            let mut edges = Vec::new();
            loop {
                let batch = cursor.next_batch(SCAN_PAGE)?;
                if batch.is_empty() {
                    break;
                }
                edges.extend(batch);
            }
            Ok(edges)
        })
        .collect::<StorageResult<Vec<Vec<Edge>>>>()?;
    let mut edges = Vec::new();
    for shard in per_shard {
        for edge in shard {
            if edge_is_expired(&edge_info, &edge) {
                continue;
            }
            record_edge_read(
                ctx,
                linkrs_core::types::EdgeIdentifier::new(
                    src_label_id,
                    edge.src,
                    dst_label_id,
                    edge.dst,
                    edge_label_id,
                    edge.ranking,
                ),
            );
            edges.push(edge);
        }
    }
    Ok(edges)
}

pub(crate) fn scan_edges_by_type_paginated(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    offset: usize,
    limit: usize,
) -> StorageResult<Vec<Edge>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let edge_info = ctx.schema_manager().get_edge_type(space, edge_type)?;
    let mut cursor = crate::engine::graph_storage::cursor_impl::create_edge_cursor(
        Arc::new(ctx.clone()),
        space,
        &crate::cursor::ScanOptions {
            edge_type: Some(edge_type.to_string()),
            offset,
            limit: Some(limit),
            ..Default::default()
        },
    )?;
    let mut edges = Vec::with_capacity(limit);
    while edges.len() < limit {
        let batch = cursor.next_batch((limit - edges.len()).min(1024))?;
        if batch.is_empty() {
            break;
        }
        for edge in batch {
            if let Some(info) = edge_info.as_ref() {
                if edge_is_expired(info, &edge) {
                    continue;
                }
            }
            edges.push(edge);
            if edges.len() >= limit {
                break;
            }
        }
    }
    edges.truncate(limit);
    Ok(edges)
}

pub(crate) fn count_edges_by_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<u64> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let edge_label_id = edge_info.edge_type_id;

    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };

    // Snapshot-consistent count: the visibility gate keeps historical reads
    // and uncommitted writes out of the result, matching what scans observe.
    let ts = ctx.get_read_timestamp();
    let gate = ctx.pending_gate();
    let hot_count = if src_label_id == 0 && dst_label_id == 0 {
        let arcs = ctx.data_store().matching_edge_partition_arcs(edge_label_id);
        let mut hot_count = 0u64;
        for arc in &arcs {
            let table = ctx.data_store().read_edge_table(arc);
            if table.label() != edge_label_id {
                continue;
            }
            hot_count += table.visible_edge_count(ts, &gate);
        }
        hot_count
    } else {
        let key =
            crate::engine::data_store::EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
        ctx.data_store()
            .with_single_edge_table(&key, |t| Ok(t.visible_edge_count(ts, &gate)))
            .unwrap_or(0)
    };

    Ok(hot_count)
}

/// Physical edge total for one edge type, summed over the stored legs.
///
/// Sizing only (cardinality estimates, partition width): it ignores the
/// read timestamp and pending writes, so snapshot reads must use
/// [`EdgeStore::visible_edge_count`] or [`count_visible_edges`] instead.
/// Unknown endpoints or a missing type resolve like the visible counters
/// (empty endpoint table reads as zero), and a missing edge table reads
/// as zero. Lock discipline mirrors `count_edges_by_type`: one table lock
/// at a time, no snapshot capture.
pub(crate) fn approximate_edge_count(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<u64> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let edge_label_id = edge_info.edge_type_id;

    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };

    if src_label_id == 0 || dst_label_id == 0 {
        let arcs = ctx.data_store().matching_edge_partition_arcs(edge_label_id);
        let mut total = 0u64;
        for arc in &arcs {
            let table = ctx.data_store().read_edge_table(arc);
            if table.label() != edge_label_id {
                continue;
            }
            total += table.edge_count();
        }
        return Ok(total);
    }
    let key =
        crate::engine::data_store::EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
    Ok(ctx
        .data_store()
        .with_single_edge_table(&key, |t| Ok(t.edge_count()))
        .unwrap_or(0))
}

/// Exact visible-edge count for pure `COUNT` queries.
///
/// Matches the seed-driven degree sum the count-only expand computes:
/// every visible edge whose seed-side endpoint is a visible vertex. The
/// seed side is the source table for `Out` and the destination table for
/// `In`. Dangling seed-side endpoints are excluded exactly like the degree
/// sum skips unresolvable seeds, while the far side is never checked,
/// mirroring the degree sum ignoring far-side existence. A quiescent
/// hole-free endpoint table skips the per-edge liveness probe; otherwise
/// each edge is checked against the scan's own live-id set. TTL anywhere
/// on the path is refused loudly, and typeless endpoint tags are refused:
/// both hide rows this decode-free walk cannot observe.
pub(crate) fn count_visible_edges(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    direction: EdgeDirection,
) -> StorageResult<u64> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!("Edge type {edge_type} not found in space {space}"))
        })?;
    if linkrs_core::types::ttl::has_valid_ttl(edge_info.ttl_duration, edge_info.ttl_col.as_deref())
    {
        return Err(StorageError::not_supported(format!(
            "Metadata edge count cannot serve edge type with TTL configured: {edge_type}"
        )));
    }
    if edge_info.src_tag_name.is_empty() || edge_info.dst_tag_name.is_empty() {
        return Err(StorageError::not_supported(format!(
            "Metadata edge count requires typed endpoints: {edge_type}"
        )));
    }
    let check_out = match direction {
        EdgeDirection::Out => true,
        EdgeDirection::In => false,
        EdgeDirection::Both => {
            return Err(StorageError::not_supported(format!(
                "Metadata edge count refuses undirected walks: {edge_type}"
            )));
        }
    };
    let endpoint_tag = if check_out {
        &edge_info.src_tag_name
    } else {
        &edge_info.dst_tag_name
    };
    if let Some(tag_info) = ctx.schema_manager().get_tag(space, endpoint_tag)? {
        if linkrs_core::types::ttl::has_valid_ttl(
            tag_info.ttl_duration,
            tag_info.ttl_col.as_deref(),
        ) {
            return Err(StorageError::not_supported(format!(
                "Metadata edge count cannot serve endpoint tag with TTL configured: {endpoint_tag}"
            )));
        }
    }
    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };
    let ts = ctx.get_read_timestamp();
    let gate = ctx.pending_gate();
    let guard = ctx.visibility_guard(ts);
    let no_pending = !ctx.has_pending_writes();
    let endpoint_label = if check_out {
        src_label_id
    } else {
        dst_label_id
    };
    let (quiescent_full, live_ids) = ctx.data_store().with_vertex_tables(|vertex_tables| {
        let Some(table) = vertex_tables.get(&endpoint_label) else {
            // A missing endpoint table forces the exact walk, which then
            // finds no live endpoint and counts zero, matching the
            // seed-driven sum over an empty scan.
            return (false, Vec::new());
        };
        // Hole count first, quiescence fence second (same commit-ordering
        // argument as the vertex count). A quiescent hole-free table needs
        // no per-edge liveness probe: every non-empty row key is live.
        let (live, allocated) = table.approximate_id_hole_stats(ts);
        if table.max_start_ts() <= ts && live == allocated {
            return (true, Vec::new());
        }
        (false, table.live_ids(&guard))
    });
    let key = crate::engine::data_store::EdgeTableKey::new(
        src_label_id,
        dst_label_id,
        edge_info.edge_type_id,
    );
    // The walk always follows the query direction leg, exactly like the
    // seed-driven adjacency visit: a missing leg reads as empty, never as
    // the stored leg. Edge visibility stays independently snapshot-exact
    // in both modes.
    // A missing edge table reads as zero, mirroring the seed-driven sum
    // over an empty degree batch for a type with no partition yet.
    let count = ctx
        .data_store()
        .with_single_edge_table(&key, |table| {
            let leg = if check_out {
                &table.out_csr
            } else {
                &table.in_csr
            };
            let edge_quiescent = no_pending
                && table.mvcc.total_tombstone_count() == 0
                && table.mvcc.max_create_ts() <= ts;
            if edge_quiescent {
                log::debug!("edge metadata count quiescent fast path: {edge_type} {direction:?}");
                if quiescent_full {
                    return Ok(leg.edge_count());
                }
                return Ok(count_leg_seed_only(leg, &live_ids));
            }
            Ok(count_leg_visible(
                table,
                leg,
                ts,
                &gate,
                quiescent_full,
                &live_ids,
            ))
        })
        .unwrap_or(0);
    Ok(count)
}

/// Snapshot-exact visible count over one CSR leg, shared by the serial and
/// sharded walks below. The predicate matches the seed-driven degree sum:
/// per-edge visibility under the query snapshot plus, outside quiescent
/// hole-free endpoint tables, a seed-side liveness probe. Sharding only
/// partitions the disjoint group set, so every sharding sums to the same
/// total as the serial walk.
fn count_leg_visible(
    table: &crate::edge::edge_table::EdgeStore,
    leg: &crate::edge::node_group::CsrShardSet,
    ts: Timestamp,
    gate: &crate::mvcc_visibility::PendingGate<'_>,
    quiescent_full: bool,
    live_ids: &[u32],
) -> u64 {
    let groups = leg.group_ids();
    // One group (the common small-table case) has no fan-out to split;
    // tiny legs skip thread dispatch the same way.
    if groups.len() < 2 || table.edge_count() < 8192 {
        return count_leg_groups(table, leg, &groups, ts, gate, quiescent_full, live_ids);
    }
    use rayon::prelude::*;
    let width = rayon::current_num_threads().max(1).min(groups.len());
    let chunk = groups.len().div_ceil(width);
    groups
        .chunks(chunk)
        .par_bridge()
        .map(|gids| count_leg_groups(table, leg, gids, ts, gate, quiescent_full, live_ids))
        .sum()
}

/// Count the visible entries of the given groups with the shared leg
/// predicate. Groups are disjoint row sets, so callers sum chunk counts.
fn count_leg_groups(
    table: &crate::edge::edge_table::EdgeStore,
    leg: &crate::edge::node_group::CsrShardSet,
    gids: &[usize],
    ts: Timestamp,
    gate: &crate::mvcc_visibility::PendingGate<'_>,
    quiescent_full: bool,
    live_ids: &[u32],
) -> u64 {
    let mut count = 0u64;
    for gid in gids {
        let Some(iter) = leg.iter_group(*gid) else {
            continue;
        };
        for (row_vid, nbr) in iter {
            if !table.is_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            if !quiescent_full {
                let row = row_vid.as_int64().unwrap_or(-1);
                if row < 0 || live_ids.binary_search(&(row as u32)).is_err() {
                    continue;
                }
            }
            count += 1;
        }
    }
    count
}

/// Seed-filtered leg count for the edge-quiescent fast path.
///
/// Edge visibility is already fenced by the table quiescence check
/// (no creation past the snapshot, no tombstones, no pending writes),
/// so every physical entry is visible and only the seed-side liveness
/// probe remains. Callers with a quiescent hole-free endpoint table skip
/// this walk entirely via the physical leg length.
fn count_leg_seed_only(leg: &crate::edge::node_group::CsrShardSet, live_ids: &[u32]) -> u64 {
    let groups = leg.group_ids();
    if groups.len() < 2 || leg.edge_count() < 8192 {
        return count_leg_seed_groups(leg, &groups, live_ids);
    }
    use rayon::prelude::*;
    let width = rayon::current_num_threads().max(1).min(groups.len());
    let chunk = groups.len().div_ceil(width);
    groups
        .chunks(chunk)
        .par_bridge()
        .map(|gids| count_leg_seed_groups(leg, gids, live_ids))
        .sum()
}

/// Count physical entries of the given groups whose seed row is live.
fn count_leg_seed_groups(
    leg: &crate::edge::node_group::CsrShardSet,
    gids: &[usize],
    live_ids: &[u32],
) -> u64 {
    let mut count = 0u64;
    for gid in gids {
        let Some(iter) = leg.iter_group(*gid) else {
            continue;
        };
        for (row_vid, _nbr) in iter {
            let row = row_vid.as_int64().unwrap_or(-1);
            if row < 0 || live_ids.binary_search(&(row as u32)).is_err() {
                continue;
            }
            count += 1;
        }
    }
    count
}

/// Full edge materialization across all types, one type scan at a time.
///
/// Prefer `create_edge_cursor` for large spaces: cursors stream batches
/// without holding every decoded `Edge` at once. This helper stays for
/// small-space maintenance paths such as export and repair.
pub(crate) fn scan_all_edges(ctx: &GraphStorageContext, space: &str) -> StorageResult<Vec<Edge>> {
    record_schema_read(ctx, space);
    let _space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let mut edges = Vec::new();
    let edge_types = ctx.schema_manager().list_edge_types(space)?;

    for et in edge_types {
        let type_edges = scan_edges_by_type(ctx, space, &et.edge_type_name)?;
        edges.extend(type_edges);
    }

    Ok(edges)
}

pub(crate) fn get_edge_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    src: &Value,
    dst: &Value,
) -> StorageResult<Option<(EdgeTypeInfo, Vec<u8>)>> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let ts = ctx.get_read_timestamp();
    let src_vid = VertexId::try_from(src)?;
    let dst_vid = VertexId::try_from(dst)?;

    let edge_label_id = edge_info.edge_type_id;
    let src_label_id = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    let dst_label_id = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    if let Some(record) = ctx.get_edge(
        &EdgeOperationParams {
            edge_label: edge_label_id,
            src_label: src_label_id,
            src_id: src_vid,
            dst_label: dst_label_id,
            dst_id: dst_vid,
            rank: 0,
        },
        ts,
    ) {
        if crate::engine::graph_storage::ttl::record_expired_from_slice(
            edge_info.ttl_duration,
            edge_info.ttl_col.as_deref(),
            &record.properties,
        ) {
            return Ok(None);
        }
        let data = serialize_properties(&record.properties);
        return Ok(Some((edge_info, data)));
    }

    Ok(None)
}

pub(crate) fn scan_edges_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<Vec<(EdgeTypeInfo, Vec<u8>)>> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let edges = scan_edges_by_type(ctx, space, edge_type)?;
    let mut results = Vec::with_capacity(edges.len());
    for edge in edges {
        let mut props: Vec<(Arc<str>, Value)> = edge.props.into_iter().collect();
        props.sort_by(|a, b| a.0.cmp(&b.0));
        results.push((edge_info.clone(), serialize_properties(&props)));
    }
    Ok(results)
}
