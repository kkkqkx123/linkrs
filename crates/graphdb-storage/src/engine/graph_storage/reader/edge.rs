use std::collections::HashSet;
use std::sync::Arc;

use crate::edge::{EdgeRecord, EdgeStore};
use crate::engine::data_store::EdgeTableKey;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    edge_record_to_edge, edge_record_to_edge_projected, endpoint_label_id, serialize_properties,
};
use crate::engine::params::EdgeOperationParams;
use graphdb_core::types::{EdgeTypeInfo, LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, EdgeDirection, StorageError, StorageResult, Value};

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
    projection: &[String],
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
    projection: Option<&[String]>,
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
        graphdb_core::types::EdgeIdentifier::new(
            src_label_id,
            src,
            dst_label_id,
            dst,
            edge_label_id,
            rank,
        ),
    );
    let src_str = src.to_string();
    let dst_str = dst.to_string();

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
        let edge =
            edge_record_to_edge_with_projection(&record, edge_type, &src_str, &dst_str, projection);
        return Ok(Some(edge));
    }

    Ok(None)
}

/// Materialize an edge record, honoring an optional property projection.
fn edge_record_to_edge_with_projection(
    record: &EdgeRecord,
    edge_type: &str,
    src_str: &str,
    dst_str: &str,
    projection: Option<&[String]>,
) -> Edge {
    match projection {
        Some(projection) => {
            edge_record_to_edge_projected(record, edge_type, src_str, dst_str, projection)
        }
        None => edge_record_to_edge(record, edge_type, src_str, dst_str),
    }
}

/// Projected and limit-aware per-node edge fanout.
/// `projection`: `None` = all columns, `Some` = only listed columns
/// (`Some(&[])` = topology only). `limit`: `None` = unlimited, `Some(k)` =
/// first-`k` visible edges per direction branch via gate-aware limit pushdown.
pub(crate) fn get_node_edges(
    ctx: &GraphStorageContext,
    space: &str,
    node_id: &VertexId,
    direction: EdgeDirection,
) -> StorageResult<Vec<Edge>> {
    get_node_edges_projected(ctx, space, node_id, direction, None, None)
}

pub(crate) fn get_node_edges_projected(
    ctx: &GraphStorageContext,
    space: &str,
    node_id: &VertexId,
    direction: EdgeDirection,
    projection: Option<&[String]>,
    limit: Option<usize>,
) -> StorageResult<Vec<Edge>> {
    record_schema_read(ctx, space);
    record_vertex_read(ctx, *node_id);
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let node_vid = VertexId::normalize_for_vid_type(&space_info.vid_type, *node_id)?;
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    if edge_types.is_empty() {
        return Ok(Vec::new());
    }
    let ts = ctx.get_read_timestamp();
    let node_str = vid_to_string(&node_vid);
    let mut edges = Vec::new();
    for edge_info in &edge_types {
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
                    let dst_internal = record.dst_vid.as_internal_u32();
                    let dst_external = match dst_internal {
                        Some(internal) if dst_label_id != 0 => ctx
                            .get_external_id(dst_label_id, internal, ts)
                            .or_else(|| {
                                ctx.get_external_id_by_internal_id(dst_label_id, internal)
                                    .map(|v| vid_to_string(&v))
                            })
                            .unwrap_or_else(|| vid_to_string(&record.dst_vid)),
                        Some(internal) => ctx
                            .get_external_id_any(internal, ts)
                            .unwrap_or_else(|| vid_to_string(&record.dst_vid)),
                        None => vid_to_string(&record.dst_vid),
                    };
                    let edge = edge_record_to_edge_with_projection(
                        &record,
                        edge_type_name,
                        &node_str,
                        &dst_external,
                        projection,
                    );
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
                    let src_internal = record.src_vid.as_internal_u32();
                    let src_external = match src_internal {
                        Some(internal) if src_label_id != 0 => ctx
                            .get_external_id(src_label_id, internal, ts)
                            .or_else(|| {
                                ctx.get_external_id_by_internal_id(src_label_id, internal)
                                    .map(|v| vid_to_string(&v))
                            })
                            .unwrap_or_else(|| vid_to_string(&record.src_vid)),
                        Some(internal) => ctx
                            .get_external_id_any(internal, ts)
                            .unwrap_or_else(|| vid_to_string(&record.src_vid)),
                        None => vid_to_string(&record.src_vid),
                    };
                    let edge = edge_record_to_edge_with_projection(
                        &record,
                        edge_type_name,
                        &src_external,
                        &node_str,
                        projection,
                    );
                    edges.push(edge);
                }
            }
            EdgeDirection::Both => {
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
                    let dst_internal = record.dst_vid.as_internal_u32();
                    let dst_external = match dst_internal {
                        Some(internal) if dst_label_id != 0 => ctx
                            .get_external_id(dst_label_id, internal, ts)
                            .or_else(|| {
                                ctx.get_external_id_by_internal_id(dst_label_id, internal)
                                    .map(|v| vid_to_string(&v))
                            })
                            .unwrap_or_else(|| vid_to_string(&record.dst_vid)),
                        Some(internal) => ctx
                            .get_external_id_any(internal, ts)
                            .unwrap_or_else(|| vid_to_string(&record.dst_vid)),
                        None => vid_to_string(&record.dst_vid),
                    };
                    let edge = edge_record_to_edge_with_projection(
                        &record,
                        edge_type_name,
                        &node_str,
                        &dst_external,
                        projection,
                    );
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
                    let src_internal = record.src_vid.as_internal_u32();
                    let src_external = match src_internal {
                        Some(internal) if src_label_id != 0 => ctx
                            .get_external_id(src_label_id, internal, ts)
                            .or_else(|| {
                                ctx.get_external_id_by_internal_id(src_label_id, internal)
                                    .map(|v| vid_to_string(&v))
                            })
                            .unwrap_or_else(|| vid_to_string(&record.src_vid)),
                        Some(internal) => ctx
                            .get_external_id_any(internal, ts)
                            .unwrap_or_else(|| vid_to_string(&record.src_vid)),
                        None => vid_to_string(&record.src_vid),
                    };
                    let edge = edge_record_to_edge_with_projection(
                        &record,
                        edge_type_name,
                        &src_external,
                        &node_str,
                        projection,
                    );
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

    let mut results = Vec::with_capacity(src_ids.len());
    for src_id in src_ids {
        record_vertex_read(ctx, *src_id);
        let mut neighbors: Vec<VertexId> = Vec::new();
        let mut seen: HashSet<(u32, u32, i64)> = HashSet::new();
        for (edge_label_id, src_label_id, dst_label_id) in &resolved {
            append_hot_neighbors(
                ctx,
                &mut neighbors,
                Some(&mut seen),
                src_id,
                *edge_label_id,
                *src_label_id,
                *dst_label_id,
                direction,
                ts,
            );
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

    let mut results = Vec::with_capacity(src_ids.len());
    for src_id in src_ids {
        record_vertex_read(ctx, *src_id);
        let mut seen: HashSet<(u32, u32, i64)> = HashSet::new();
        for (edge_label_id, src_label_id, dst_label_id) in &resolved {
            count_hot_neighbors(
                ctx,
                &mut seen,
                src_id,
                *edge_label_id,
                *src_label_id,
                *dst_label_id,
                direction,
                ts,
            );
        }
        results.push(seen.len());
    }
    Ok(results)
}

/// Append hot-CSR neighbors of `src_id` (direction-dependent endpoint) to
/// `neighbors`. Deduplicated by `(src, dst, rank)` across tables.
#[allow(clippy::too_many_arguments)]
fn append_hot_neighbors(
    ctx: &GraphStorageContext,
    neighbors: &mut Vec<VertexId>,
    mut seen: Option<&mut HashSet<(u32, u32, i64)>>,
    src_id: &VertexId,
    edge_label_id: LabelId,
    src_label_id: LabelId,
    dst_label_id: LabelId,
    direction: EdgeDirection,
    ts: Timestamp,
) {
    let mut unique = |key: (u32, u32, i64)| -> bool {
        match seen.as_deref_mut() {
            Some(seen) => seen.insert(key),
            None => true,
        }
    };
    match direction {
        EdgeDirection::Out => {
            ctx.visit_out_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |src_internal, nbr| {
                    let rank = nbr.rank;
                    let dst_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(dst_internal) = dst_internal_vid.as_int64() {
                        let dst_internal = dst_internal as u32;
                        if unique((src_internal, dst_internal, rank)) {
                            let ext = internal_to_external_vertex_id(
                                ctx,
                                dst_label_id,
                                dst_internal,
                                &dst_internal_vid,
                                ts,
                            );
                            neighbors.push(ext);
                        }
                    }
                },
            );
        }
        EdgeDirection::In => {
            ctx.visit_in_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |dst_internal, nbr| {
                    let rank = nbr.rank;
                    let src_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        let src_internal = src_internal as u32;
                        if unique((src_internal, dst_internal, rank)) {
                            let ext = internal_to_external_vertex_id(
                                ctx,
                                src_label_id,
                                src_internal,
                                &src_internal_vid,
                                ts,
                            );
                            neighbors.push(ext);
                        }
                    }
                },
            );
        }
        EdgeDirection::Both => {
            ctx.visit_out_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |src_internal, nbr| {
                    let rank = nbr.rank;
                    let dst_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(dst_internal) = dst_internal_vid.as_int64() {
                        let dst_internal = dst_internal as u32;
                        if unique((src_internal, dst_internal, rank)) {
                            let ext = internal_to_external_vertex_id(
                                ctx,
                                dst_label_id,
                                dst_internal,
                                &dst_internal_vid,
                                ts,
                            );
                            neighbors.push(ext);
                        }
                    }
                },
            );
            ctx.visit_in_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |dst_internal, nbr| {
                    let rank = nbr.rank;
                    let src_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        let src_internal = src_internal as u32;
                        if unique((src_internal, dst_internal, rank)) {
                            let ext = internal_to_external_vertex_id(
                                ctx,
                                src_label_id,
                                src_internal,
                                &src_internal_vid,
                                ts,
                            );
                            neighbors.push(ext);
                        }
                    }
                },
            );
        }
    }
}

/// Count hot-CSR neighbors of `src_id` into `seen` (dedup by full edge
/// identity `(src_internal, dst_internal, rank)`).
#[allow(clippy::too_many_arguments)]
fn count_hot_neighbors(
    ctx: &GraphStorageContext,
    seen: &mut HashSet<(u32, u32, i64)>,
    src_id: &VertexId,
    edge_label_id: LabelId,
    src_label_id: LabelId,
    dst_label_id: LabelId,
    direction: EdgeDirection,
    ts: Timestamp,
) {
    match direction {
        EdgeDirection::Out => {
            ctx.visit_out_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |src_internal, nbr| {
                    let rank = nbr.rank;
                    let dst_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(dst_internal) = dst_internal_vid.as_int64() {
                        seen.insert((src_internal, dst_internal as u32, rank));
                    }
                },
            );
        }
        EdgeDirection::In => {
            ctx.visit_in_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |dst_internal, nbr| {
                    let rank = nbr.rank;
                    let src_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        seen.insert((src_internal as u32, dst_internal, rank));
                    }
                },
            );
        }
        EdgeDirection::Both => {
            ctx.visit_out_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |src_internal, nbr| {
                    let rank = nbr.rank;
                    let dst_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(dst_internal) = dst_internal_vid.as_int64() {
                        seen.insert((src_internal, dst_internal as u32, rank));
                    }
                },
            );
            ctx.visit_in_nbrs(
                edge_label_id,
                src_label_id,
                dst_label_id,
                *src_id,
                ts,
                |dst_internal, nbr| {
                    let rank = nbr.rank;
                    let src_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        seen.insert((src_internal as u32, dst_internal, rank));
                    }
                },
            );
        }
    }
}

pub(crate) fn scan_edges_by_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<Vec<Edge>> {
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

    let ts = ctx.get_read_timestamp();
    let mut edges = Vec::new();

    let edge_label_id = edge_info.edge_type_id;

    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(edges),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(edges),
    };

    const BATCH_SIZE: usize = 256;

    // For unconstrained edge types (both tags empty), edges may be spread across
    // multiple edge tables. Use iter() directly instead of scan() to avoid
    // intermediate Vec<EdgeRecord> allocation per table.
    if src_label_id == 0 && dst_label_id == 0 {
        // Scatter-gather: collect the matching partition handles under a brief
        // catalog read lock, then scan each partition in parallel under its own
        // read lock. Results preserve partition order (indexed rayon collect).
        let matching: Vec<(EdgeTableKey, Arc<parking_lot::RwLock<EdgeStore>>)> =
            ctx.data_store().with_edge_tables(|edge_tables| {
                edge_tables
                    .iter()
                    .filter(|(_, arc)| arc.read().label() == edge_label_id)
                    .map(|(key, arc)| (*key, arc.clone()))
                    .collect()
            });

        use rayon::prelude::*;
        let per_partition: Vec<Vec<Edge>> = matching
            .par_iter()
            .map(|(_key, arc)| {
                let guard = arc.read();
                let mut iter = guard.iter(ts);
                let mut partition_edges = Vec::new();
                loop {
                    let batch: Vec<_> = iter.by_ref().take(BATCH_SIZE).collect();
                    if batch.is_empty() {
                        break;
                    }
                    for record in batch {
                        let src_internal = record.src_vid.as_internal_u32();
                        let dst_internal = record.dst_vid.as_internal_u32();

                        let tbl_src = guard.src_label();
                        let tbl_dst = guard.dst_label();

                        let src_external = match src_internal {
                            Some(internal) if tbl_src != 0 => ctx
                                .get_external_id(tbl_src, internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(tbl_src, internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| format!("{}", record.src_vid)),
                            Some(internal) => ctx
                                .get_external_id_any(internal, ts)
                                .unwrap_or_else(|| format!("{}", record.src_vid)),
                            None => format!("{}", record.src_vid),
                        };

                        let dst_external = match dst_internal {
                            Some(internal) if tbl_dst != 0 => ctx
                                .get_external_id(tbl_dst, internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(tbl_dst, internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| format!("{}", record.dst_vid)),
                            Some(internal) => ctx
                                .get_external_id_any(internal, ts)
                                .unwrap_or_else(|| format!("{}", record.dst_vid)),
                            None => format!("{}", record.dst_vid),
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type, &src_external, &dst_external);
                        partition_edges.push(edge);
                    }
                }
                partition_edges
            })
            .collect();

        let edges: Vec<Edge> = per_partition.into_iter().flatten().collect();
        return Ok(edges);
    }

    // Constrained path: access the specific edge table directly using iter()
    // instead of ctx.scan_edges() which collects into Vec.
    {
        let key = EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
        ctx.data_store().with_edge_tables(|edge_tables| {
            if let Some(arc) = edge_tables.get(&key) {
                let guard = arc.read();
                let mut iter = guard.iter(ts);
                loop {
                    let batch: Vec<_> = iter.by_ref().take(BATCH_SIZE).collect();
                    if batch.is_empty() {
                        break;
                    }
                    for record in batch {
                        record_edge_read(
                            ctx,
                            graphdb_core::types::EdgeIdentifier::new(
                                src_label_id,
                                record.src_vid,
                                dst_label_id,
                                record.dst_vid,
                                edge_label_id,
                                record.rank,
                            ),
                        );
                        let src_internal = record.src_vid.as_internal_u32();
                        let dst_internal = record.dst_vid.as_internal_u32();

                        let src_external = src_internal
                            .and_then(|internal| {
                                ctx.get_external_id(src_label_id, internal, ts).or_else(|| {
                                    ctx.get_external_id_by_internal_id(src_label_id, internal)
                                        .map(|v| vid_to_string(&v))
                                })
                            })
                            .unwrap_or_else(|| format!("{}", record.src_vid));

                        let dst_external = dst_internal
                            .and_then(|internal| {
                                ctx.get_external_id(dst_label_id, internal, ts).or_else(|| {
                                    ctx.get_external_id_by_internal_id(dst_label_id, internal)
                                        .map(|v| vid_to_string(&v))
                                })
                            })
                            .unwrap_or_else(|| format!("{}", record.dst_vid));

                        let edge =
                            edge_record_to_edge(&record, edge_type, &src_external, &dst_external);
                        edges.push(edge);
                    }
                }
            }
        });
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
        edges.extend(batch);
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

    let hot_count = if src_label_id == 0 && dst_label_id == 0 {
        ctx.data_store().with_edge_tables(|edge_tables| {
            edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label_id)
                .map(|t| t.edge_count())
                .sum()
        })
    } else {
        let key =
            crate::engine::data_store::EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
        ctx.data_store()
            .with_single_edge_table(&key, |t| Ok(t.edge_count()))
            .unwrap_or(0)
    };

    Ok(hot_count)
}

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
        let mut props: Vec<(String, Value)> = edge.props.into_iter().collect();
        props.sort_by(|a, b| a.0.cmp(&b.0));
        results.push((edge_info.clone(), serialize_properties(&props)));
    }
    Ok(results)
}
