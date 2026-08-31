use std::collections::HashSet;
use std::sync::Arc;

use crate::cold::{ColdIndexEntry, ColdSnapshot};
use crate::edge::Nbr;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::edge_record_to_edge;
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, EdgeDirection, StorageResult};

use crate::engine::graph_storage::reader::utils::*;

/// Look up a single edge in cold snapshots by source/dest VertexId.
///
/// Returns the matched neighbor together with the owning snapshot and the
/// internal indices of both endpoints. Snapshots are probed newest-first.
/// The cold CSR indexes vertices by vertex-table internal IDs, matching the
/// hot edge table, so `vertex_id_to_internal` applies to both.
pub(crate) fn query_cold_edge(
    ctx: &GraphStorageContext,
    edge_label: LabelId,
    src: VertexId,
    dst: VertexId,
    src_label: LabelId,
    dst_label: LabelId,
    ts: Timestamp,
) -> Option<(Arc<crate::cold::ColdSnapshot>, Nbr, u32, VertexId)> {
    let cold = ctx.cold_snapshots().read();
    let snapshots = cold.get(&edge_label)?;
    let src_internal = vertex_id_to_internal(ctx, src_label, &src, ts)?;
    let dst_internal = vertex_id_to_internal(ctx, dst_label, &dst, ts)?;
    for snapshot in snapshots.iter().rev() {
        if ts < snapshot.snapshot_ts() {
            continue;
        }
        if let Some(nbr) = snapshot.get_edge_to_dst(src_internal, dst_internal) {
            return Some((
                snapshot.clone(),
                nbr,
                src_internal,
                VertexId::from_int64(dst_internal as i64),
            ));
        }
    }
    None
}

/// Query cold snapshots for node edges and append to `edges`.
///
/// The cold CSR is indexed by vertex-table internal IDs, matching the hot
/// edge table, so `vertex_id_to_internal` works for both. Neighbor entries
/// are decoded from their `(endpoint_internal, rank)` encoding before being
/// resolved to external IDs. Dedup happens in external-ID space so edges
/// present in both hot and cold data (or in several snapshots) are not
/// returned twice. Snapshots whose timestamp is newer than the read
/// timestamp are skipped.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_cold_node_edges(
    ctx: &GraphStorageContext,
    edges: &mut Vec<Edge>,
    edge_label: LabelId,
    edge_type_name: &str,
    node_id: &VertexId,
    src_label: LabelId,
    dst_label: LabelId,
    direction: EdgeDirection,
    ts: Timestamp,
) -> StorageResult<()> {
    let cold = ctx.cold_snapshots().read();
    let Some(snapshots) = cold.get(&edge_label) else {
        return Ok(());
    };

    let node_str = vid_to_string(node_id);
    let mut dedup: HashSet<(VertexId, VertexId, i64)> = HashSet::with_capacity(edges.len());
    for e in edges.iter() {
        dedup.insert((e.src, e.dst, e.ranking));
    }

    for snapshot in snapshots.iter().filter(|s| ts >= s.snapshot_ts()) {
        match direction {
            EdgeDirection::Out => {
                let Some(internal) = vertex_id_to_internal(ctx, src_label, node_id, ts) else {
                    continue;
                };
                for nbr in snapshot.get_out_edges(internal) {
                    let rank = nbr.rank;
                    let dst_internal = nbr.endpoint;
                    let dst_internal_vid = VertexId::from_int64(dst_internal as i64);
                    let dst_ext =
                        external_id_string(ctx, dst_label, dst_internal, &dst_internal_vid, ts);
                    if dedup.insert((*node_id, vid_from_str(&dst_ext), rank)) {
                        let record = snapshot.nbr_to_edge_record(&nbr, *node_id, dst_internal_vid);
                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &node_str, &dst_ext);
                        edges.push(edge);
                    }
                }
            }
            EdgeDirection::In => {
                let Some(internal) = vertex_id_to_internal(ctx, dst_label, node_id, ts) else {
                    continue;
                };
                for nbr in snapshot.get_in_edges(internal) {
                    let rank = nbr.rank;
                    let src_internal = nbr.endpoint;
                    let src_internal_vid = VertexId::from_int64(src_internal as i64);
                    let src_ext =
                        external_id_string(ctx, src_label, src_internal, &src_internal_vid, ts);
                    if dedup.insert((vid_from_str(&src_ext), *node_id, rank)) {
                        let record = snapshot.nbr_to_edge_record(&nbr, src_internal_vid, *node_id);
                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &src_ext, &node_str);
                        edges.push(edge);
                    }
                }
            }
            EdgeDirection::Both => {
                if let Some(internal) = vertex_id_to_internal(ctx, src_label, node_id, ts) {
                    for nbr in snapshot.get_out_edges(internal) {
                        let rank = nbr.rank;
                        let dst_internal = nbr.endpoint;
                        let dst_internal_vid = VertexId::from_int64(dst_internal as i64);
                        let dst_ext =
                            external_id_string(ctx, dst_label, dst_internal, &dst_internal_vid, ts);
                        if dedup.insert((*node_id, vid_from_str(&dst_ext), rank)) {
                            let record =
                                snapshot.nbr_to_edge_record(&nbr, *node_id, dst_internal_vid);
                            let edge =
                                edge_record_to_edge(&record, edge_type_name, &node_str, &dst_ext);
                            edges.push(edge);
                        }
                    }
                }
                if let Some(internal) = vertex_id_to_internal(ctx, dst_label, node_id, ts) {
                    for nbr in snapshot.get_in_edges(internal) {
                        let rank = nbr.rank;
                        let src_internal = nbr.endpoint;
                        let src_internal_vid = VertexId::from_int64(src_internal as i64);
                        let src_ext =
                            external_id_string(ctx, src_label, src_internal, &src_internal_vid, ts);
                        if dedup.insert((vid_from_str(&src_ext), *node_id, rank)) {
                            let record =
                                snapshot.nbr_to_edge_record(&nbr, src_internal_vid, *node_id);
                            let edge =
                                edge_record_to_edge(&record, edge_type_name, &src_ext, &node_str);
                            edges.push(edge);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Append cold-snapshot neighbors of `src_id` with the same dedup as the hot
/// path, mirroring [`append_cold_node_edges`] but without materializing
/// `Edge` records.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_cold_neighbors(
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
    let cold = ctx.cold_snapshots().read();
    let Some(snapshots) = cold.get(&edge_label_id) else {
        return;
    };
    for snapshot in snapshots.iter().filter(|s| ts >= s.snapshot_ts()) {
        match direction {
            EdgeDirection::Out => {
                let Some(src_internal) = vertex_id_to_internal(ctx, src_label_id, src_id, ts)
                else {
                    continue;
                };
                for nbr in snapshot.get_out_edges(src_internal) {
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
                }
            }
            EdgeDirection::In => {
                let Some(dst_internal) = vertex_id_to_internal(ctx, dst_label_id, src_id, ts)
                else {
                    continue;
                };
                for nbr in snapshot.get_in_edges(dst_internal) {
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
                }
            }
            EdgeDirection::Both => {
                if let Some(src_internal) = vertex_id_to_internal(ctx, src_label_id, src_id, ts) {
                    for nbr in snapshot.get_out_edges(src_internal) {
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
                    }
                }
                if let Some(dst_internal) = vertex_id_to_internal(ctx, dst_label_id, src_id, ts) {
                    for nbr in snapshot.get_in_edges(dst_internal) {
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
                    }
                }
            }
        }
    }
}

/// Count cold-snapshot neighbors of `src_id` into `seen`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn count_cold_neighbors(
    ctx: &GraphStorageContext,
    seen: &mut HashSet<(u32, u32, i64)>,
    src_id: &VertexId,
    edge_label_id: LabelId,
    src_label_id: LabelId,
    dst_label_id: LabelId,
    direction: EdgeDirection,
    ts: Timestamp,
) {
    let cold = ctx.cold_snapshots().read();
    let Some(snapshots) = cold.get(&edge_label_id) else {
        return;
    };
    for snapshot in snapshots.iter().filter(|s| ts >= s.snapshot_ts()) {
        match direction {
            EdgeDirection::Out => {
                let Some(src_internal) = vertex_id_to_internal(ctx, src_label_id, src_id, ts)
                else {
                    continue;
                };
                for nbr in snapshot.get_out_edges(src_internal) {
                    let rank = nbr.rank;
                    let dst_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(dst_internal) = dst_internal_vid.as_int64() {
                        seen.insert((src_internal, dst_internal as u32, rank));
                    }
                }
            }
            EdgeDirection::In => {
                let Some(dst_internal) = vertex_id_to_internal(ctx, dst_label_id, src_id, ts)
                else {
                    continue;
                };
                for nbr in snapshot.get_in_edges(dst_internal) {
                    let rank = nbr.rank;
                    let src_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        seen.insert((src_internal as u32, dst_internal, rank));
                    }
                }
            }
            EdgeDirection::Both => {
                if let Some(src_internal) = vertex_id_to_internal(ctx, src_label_id, src_id, ts) {
                    for nbr in snapshot.get_out_edges(src_internal) {
                        let rank = nbr.rank;
                        let dst_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                        if let Some(dst_internal) = dst_internal_vid.as_int64() {
                            seen.insert((src_internal, dst_internal as u32, rank));
                        }
                    }
                }
                if let Some(dst_internal) = vertex_id_to_internal(ctx, dst_label_id, src_id, ts) {
                    for nbr in snapshot.get_in_edges(dst_internal) {
                        let rank = nbr.rank;
                        let src_internal_vid = VertexId::from_int64(nbr.endpoint as i64);
                        if let Some(src_internal) = src_internal_vid.as_int64() {
                            seen.insert((src_internal as u32, dst_internal, rank));
                        }
                    }
                }
            }
        }
    }
}

/// Materialize an edge from a cold property-index hit.
pub(crate) fn cold_index_entry_to_edge(
    ctx: &GraphStorageContext,
    snapshot: &Arc<ColdSnapshot>,
    entry: &ColdIndexEntry,
    edge_type: &str,
    src_label: LabelId,
    dst_label: LabelId,
    ts: Timestamp,
) -> Edge {
    let src_vid = VertexId::from_int64(entry.src_internal as i64);
    let dst_vid = VertexId::from_int64(entry.dst_internal as i64);
    let nbr = Nbr::new(entry.dst_internal, entry.rank, EdgeId(0));
    let record = snapshot.nbr_to_edge_record(&nbr, src_vid, dst_vid);
    let src_ext = external_id_string(ctx, src_label, entry.src_internal, &src_vid, ts);
    let dst_ext = external_id_string(ctx, dst_label, entry.dst_internal, &dst_vid, ts);
    edge_record_to_edge(&record, edge_type, &src_ext, &dst_ext)
}

/// Append cold snapshot edges to a scan result.
///
/// Each cold record carries the CSR source index and the decoded destination
/// VertexId; both endpoints are resolved to external IDs in the same way as
/// the hot scan path. Dedup uses external-ID space so edges that still exist
/// in hot data (or repeat across snapshots) are not returned twice.
pub(crate) fn append_cold_scan_edges(
    ctx: &GraphStorageContext,
    mut edges: Vec<Edge>,
    edge_label: LabelId,
    edge_type: &str,
    src_label: LabelId,
    dst_label: LabelId,
    ts: Timestamp,
) -> Vec<Edge> {
    let cold = ctx.cold_snapshots().read();
    let Some(snapshots) = cold.get(&edge_label) else {
        return edges;
    };

    let mut dedup: HashSet<(VertexId, VertexId, i64)> = HashSet::with_capacity(edges.len());
    for e in &edges {
        dedup.insert((e.src, e.dst, e.ranking));
    }

    for snapshot in snapshots.iter().filter(|s| ts >= s.snapshot_ts()) {
        for record in snapshot.scan_edges() {
            let src_internal = record.src_internal;
            let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;
            let rank = record.rank;
            let src_ext = external_id_string(
                ctx,
                src_label,
                src_internal,
                &VertexId::from_int64(src_internal as i64),
                ts,
            );
            let dst_ext = external_id_string(ctx, dst_label, dst_internal, &record.dst_vid, ts);
            let key = (vid_from_str(&src_ext), vid_from_str(&dst_ext), rank);
            if dedup.insert(key) {
                let edge_record = snapshot.nbr_to_edge_record(
                    &record.nbr,
                    VertexId::from_int64(src_internal as i64),
                    record.dst_vid,
                );
                let edge = edge_record_to_edge(&edge_record, edge_type, &src_ext, &dst_ext);
                edges.push(edge);
            }
        }
    }
    edges
}
