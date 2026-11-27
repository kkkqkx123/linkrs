use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::client::traits_reader::TopologyTypeSchema;
use crate::edge::CsrBase;
use crate::edge::EdgeRecord;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    edge_record_to_edge, edge_record_to_edge_projected, endpoint_label_id, serialize_properties,
};
use crate::engine::params::EdgeOperationParams;
use crate::EdgeTopology;
use crate::{FrontierDegrees, PackedEdgeTopology, TopologySchema};
use linkrs_core::types::{EdgeTypeInfo, LabelId, Timestamp, VertexId};
use linkrs_core::{Edge, EdgeDirection, StorageError, StorageResult, Value};

use crate::engine::graph_storage::reader::ttl_filter::edge_is_expired;
use crate::engine::graph_storage::reader::utils::*;

use super::vertex::get_vertex_property_columns_with_handles_batch;

/// Existence-only reverse-CSR read for ordered destination scans.
///
/// One batched adjacency pass stages candidate source endpoints; a second
/// batched vertex read applies source visibility, label and TTL. No edges,
/// properties or destination rows are materialized.
pub(crate) fn incoming_edge_exists_batch(
    ctx: &GraphStorageContext,
    space: &str,
    dst_ids: &[VertexId],
    src_tag: &str,
    dst_tag: &str,
    edge_type: &str,
) -> StorageResult<Vec<bool>> {
    let mut exists = vec![false; dst_ids.len()];
    if dst_ids.is_empty() {
        return Ok(exists);
    }
    record_schema_read(ctx, space);
    let info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!("Edge type {edge_type} not found in space {space}"))
        })?;
    if info.src_tag_name != src_tag || info.dst_tag_name != dst_tag {
        return Err(StorageError::invalid_operation(format!(
            "Edge type {edge_type} endpoints do not match {src_tag} -> {dst_tag}"
        )));
    }
    let src_label = endpoint_label_id(ctx, space, &info.src_tag_name)?
        .ok_or_else(|| StorageError::label_not_found(src_tag.to_string()))?;
    let dst_label = endpoint_label_id(ctx, space, &info.dst_tag_name)?
        .ok_or_else(|| StorageError::label_not_found(dst_tag.to_string()))?;
    let ts = ctx.get_read_timestamp();
    let mut staged = Vec::new();
    let mut seen = HashSet::new();
    ctx.visit_in_nbrs_batched(
        info.edge_type_id,
        src_label,
        dst_label,
        dst_ids,
        ts,
        |index, _, label, nbr| {
            if label == src_label && seen.insert((index, nbr.endpoint, label)) {
                staged.push((index, nbr.endpoint, label));
            }
        },
    );
    if staged.is_empty() {
        return Ok(exists);
    }
    let internal_ids: Vec<u32> = staged.iter().map(|(_, internal, _)| *internal).collect();
    let external_ids = ctx.external_ids_by_internal_batch(src_label, &internal_ids, ts);
    let candidates: Vec<(usize, VertexId, (u32, LabelId))> = staged
        .into_iter()
        .zip(external_ids)
        .filter_map(|((index, internal, label), external)| {
            external.map(|external| (index, external, (internal, label)))
        })
        .collect();
    let external: Vec<VertexId> = candidates.iter().map(|(_, id, _)| *id).collect();
    let handles: Vec<Option<(u32, LabelId)>> = candidates
        .iter()
        .map(|(_, _, handle)| Some(*handle))
        .collect();
    let present = get_vertex_property_columns_with_handles_batch(
        ctx,
        space,
        src_tag,
        &external,
        &[],
        &handles,
    )?;
    for ((index, _, _), visible) in candidates.iter().zip(present) {
        if visible.is_some() {
            exists[*index] = true;
        }
    }
    Ok(exists)
}

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
    record_vertex_reads(ctx, &normalized);
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
fn resolve_topology_schema(
    ctx: &GraphStorageContext,
    space: &str,
    edge_types: &[String],
) -> StorageResult<TopologySchema> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let all = ctx.schema_manager().list_edge_types(space)?;
    let mut entries = Vec::new();
    for info in all
        .iter()
        .filter(|info| edge_types.is_empty() || edge_types.contains(&info.edge_type_name))
    {
        entries.push(TopologyTypeSchema {
            name: Arc::from(info.edge_type_name.as_str()),
            type_id: entries.len() as u32,
            edge_label: info.edge_type_id,
            src_label: endpoint_label_id(ctx, space, &info.src_tag_name)?,
            dst_label: endpoint_label_id(ctx, space, &info.dst_tag_name)?,
        });
    }
    Ok(TopologySchema {
        space: space.to_string(),
        vid_type: space_info.vid_type,
        layout_version: ctx.layout_version(),
        entries,
    })
}

fn resolve_far_ids<I>(
    ctx: &GraphStorageContext,
    endpoints: I,
    ts: Timestamp,
) -> HashMap<(LabelId, u32), VertexId>
where
    I: IntoIterator<Item = (LabelId, u32)>,
{
    let mut distinct: HashSet<(LabelId, u32)> = HashSet::new();
    for (label, endpoint) in endpoints {
        if label != 0 {
            distinct.insert((label, endpoint));
        }
    }
    let mut by_label: HashMap<LabelId, Vec<u32>> = HashMap::new();
    for (label, endpoint) in distinct {
        by_label.entry(label).or_default().push(endpoint);
    }
    let capacity: usize = by_label.values().map(Vec::len).sum();
    let mut resolved = HashMap::with_capacity(capacity);
    for (label, mut ids) in by_label {
        ids.sort_unstable();
        let external = ctx.external_ids_by_internal_batch(label, &ids, ts);
        for (endpoint, vid) in ids.into_iter().zip(external) {
            resolved.insert(
                (label, endpoint),
                vid.unwrap_or_else(|| VertexId::from_u32(endpoint)),
            );
        }
    }
    resolved
}

pub(crate) fn packed_node_edge_headers_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    handles: Option<&[Option<(u32, LabelId)>]>,
    direction: EdgeDirection,
    edge_types: &[String],
    schema: &TopologySchema,
) -> StorageResult<PackedEdgeTopology> {
    record_schema_read(ctx, space);
    let schema = if schema.matches(space, ctx.layout_version()) {
        Cow::Borrowed(schema)
    } else {
        Cow::Owned(resolve_topology_schema(ctx, space, edge_types)?)
    };
    let normalized: Vec<VertexId> = src_ids
        .iter()
        .map(|id| VertexId::normalize_for_vid_type(&schema.vid_type, *id))
        .collect::<Result<_, _>>()?;
    if schema.entries.is_empty() {
        let mut empty = PackedEdgeTopology {
            parents: normalized,
            ..PackedEdgeTopology::default()
        };
        empty.child_offsets.resize(empty.parents.len() + 1, 0);
        return Ok(empty);
    }
    let ts = ctx.get_read_timestamp();
    record_vertex_reads(ctx, &normalized);

    let mut packed = PackedEdgeTopology {
        parents: normalized.clone(),
        ..PackedEdgeTopology::default()
    };
    for entry in &schema.entries {
        packed.intern_edge_type(&entry.name);
    }

    let endpoint_types: Vec<(u32, LabelId, LabelId, LabelId)> = schema
        .entries
        .iter()
        .filter_map(|entry| {
            Some((
                entry.type_id,
                entry.edge_label,
                entry.src_label?,
                entry.dst_label?,
            ))
        })
        .collect();

    if direction != EdgeDirection::Both {
        let outward = direction == EdgeDirection::Out;
        let mut batches_by_type = Vec::with_capacity(endpoint_types.len());
        for (_, edge_label, src_label, dst_label) in &endpoint_types {
            let batches = if outward {
                ctx.out_topology_batch(
                    *edge_label,
                    *src_label,
                    &normalized,
                    handles,
                    ts,
                )
            } else {
                ctx.in_topology_batch(*edge_label, *dst_label, &normalized, ts)
            };
            batches_by_type.push(batches);
        }
        // Endpoint ids are resolved once per call under one table lock per label.
        let endpoints = batches_by_type.iter().flat_map(|batches| {
            batches
                .entries
                .iter()
                .map(|(label, hot)| (*label, hot.endpoint))
        });
        let far_ids = resolve_far_ids(ctx, endpoints, ts);
        let far_id = |label: LabelId, endpoint: u32| -> VertexId {
            if label == 0 {
                return VertexId::from_u32(endpoint);
            }
            far_ids
                .get(&(label, endpoint))
                .copied()
                .unwrap_or_else(|| VertexId::from_u32(endpoint))
        };
        let total: usize = batches_by_type
            .iter()
            .map(|batches| batches.entries.len())
            .sum();
        packed.src.reserve(total);
        packed.dst.reserve(total);
        packed.edge_type_ids.reserve(total);
        packed.rankings.reserve(total);
        packed.child_offsets.push(0);
        for parent in 0..normalized.len() {
            for ((type_id, _, _, _), batches) in endpoint_types.iter().zip(&batches_by_type) {
                for &(far_label, hot) in
                    &batches.entries[batches.offsets[parent]..batches.offsets[parent + 1]]
                {
                    let far = far_id(far_label, hot.endpoint);
                    let (src, dst) = if outward {
                        (normalized[parent], far)
                    } else {
                        (far, normalized[parent])
                    };
                    packed.src.push(src);
                    packed.dst.push(dst);
                    packed.edge_type_ids.push(*type_id);
                    packed.rankings.push(hot.rank);
                    packed
                        .destination_handles
                        .push((far_label != 0).then_some((hot.endpoint, far_label)));
                }
            }
            packed.child_offsets.push(packed.src.len());
        }
        return Ok(packed);
    }

    // Both directions need the shared dedup set, so they still collect CSR
    // slots first and scatter them into parent-major arrays afterwards.
    #[derive(Clone, Copy)]
    enum Side {
        Out,
        In,
    }
    struct Event {
        parent: usize,
        far_label: LabelId,
        endpoint: u32,
        rank: i64,
        type_id: u32,
        side: Side,
        handle: Option<(u32, LabelId)>,
    }
    let mut by_type = Vec::with_capacity(endpoint_types.len());
    let mut endpoint_pairs: Vec<(LabelId, u32)> = Vec::new();
    for (type_id, edge_label, src_label_id, dst_label_id) in &endpoint_types {
        let out_batches = ctx.out_topology_batch(
            *edge_label,
            *src_label_id,
            &normalized,
            handles,
            ts,
        );
        let in_batches =
            ctx.in_topology_batch(*edge_label, *dst_label_id, &normalized, ts);
        for batches in [&out_batches, &in_batches] {
            endpoint_pairs.extend(
                batches
                    .entries
                    .iter()
                    .map(|(label, hot)| (*label, hot.endpoint)),
            );
        }
        by_type.push((*type_id, out_batches, in_batches));
    }
    let far_ids = resolve_far_ids(ctx, endpoint_pairs, ts);
    let far_id = |label: LabelId, endpoint: u32| -> VertexId {
        if label == 0 {
            return VertexId::from_u32(endpoint);
        }
        far_ids
            .get(&(label, endpoint))
            .copied()
            .unwrap_or_else(|| VertexId::from_u32(endpoint))
    };

    let mut events: Vec<Event> = Vec::new();
    let mut seen: HashSet<(usize, VertexId, VertexId, i64)> = HashSet::new();

    for (type_id, out_batches, in_batches) in by_type {
        for parent in 0..normalized.len() {
            for &(far_label, hot) in
                &out_batches.entries[out_batches.offsets[parent]..out_batches.offsets[parent + 1]]
            {
                let dst = far_id(far_label, hot.endpoint);
                if !seen.insert((parent, normalized[parent], dst, hot.rank)) {
                    continue;
                }
                events.push(Event {
                    parent,
                    far_label,
                    endpoint: hot.endpoint,
                    rank: hot.rank,
                    type_id,
                    side: Side::Out,
                    handle: (far_label != 0).then_some((hot.endpoint, far_label)),
                });
            }
            for &(far_label, hot) in
                &in_batches.entries[in_batches.offsets[parent]..in_batches.offsets[parent + 1]]
            {
                let src = far_id(far_label, hot.endpoint);
                if !seen.insert((parent, src, normalized[parent], hot.rank)) {
                    continue;
                }
                events.push(Event {
                    parent,
                    far_label,
                    endpoint: hot.endpoint,
                    rank: hot.rank,
                    type_id,
                    side: Side::In,
                    handle: (far_label != 0).then_some((hot.endpoint, far_label)),
                });
            }
        }
    }

    packed.child_offsets.push(0);
    let mut counts = vec![0usize; normalized.len()];
    for event in &events {
        counts[event.parent] += 1;
    }
    for count in counts {
        let last = *packed.child_offsets.last().expect("offsets start at zero");
        packed.child_offsets.push(last + count);
    }

    let total = events.len();
    packed.src = vec![VertexId::default(); total];
    packed.dst = vec![VertexId::default(); total];
    packed.edge_type_ids = vec![0u32; total];
    packed.rankings = vec![0i64; total];
    packed.destination_handles = vec![None; total];
    let mut cursors = packed.child_offsets[..normalized.len()].to_vec();
    for event in events {
        let far = far_id(event.far_label, event.endpoint);
        let (src, dst) = match event.side {
            Side::Out => (normalized[event.parent], far),
            Side::In => (far, normalized[event.parent]),
        };
        let position = cursors[event.parent];
        cursors[event.parent] = position + 1;
        packed.src[position] = src;
        packed.dst[position] = dst;
        packed.edge_type_ids[position] = event.type_id;
        packed.rankings[position] = event.rank;
        packed.destination_handles[position] = event.handle;
    }
    Ok(packed)
}

pub(crate) fn get_node_edge_headers_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    handles: Option<&[Option<(u32, LabelId)>]>,
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<Vec<EdgeTopology>>> {
    let schema = resolve_topology_schema(ctx, space, edge_types)?;
    Ok(packed_node_edge_headers_batch(
        ctx, space, src_ids, handles, direction, edge_types, &schema,
    )?
    .to_batches())
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

    record_vertex_reads(ctx, src_ids);
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

    record_vertex_reads(ctx, src_ids);
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
pub(crate) fn frontier_degree_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<FrontierDegrees> {
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
        resolved.push((
            edge_info.edge_type_id,
            src_label_id,
            dst_label_id,
            edge_info.edge_type_name.clone(),
        ));
    }
    record_vertex_reads(ctx, src_ids);
    if resolved.len() != 1 || matches!(direction, EdgeDirection::Both) {
        return portable_frontier_degrees(ctx, space, src_ids, direction, edge_types);
    }
    let (edge_label, src_label, dst_label) = (resolved[0].0, resolved[0].1, resolved[0].2);
    let edge_type_name = &resolved[0].3;
    let neighbor_label = if direction == EdgeDirection::Out {
        dst_label
    } else {
        src_label
    };
    let mut offsets = vec![0usize; src_ids.len() + 1];
    let mut raw_endpoints = Vec::new();
    let mut foreign_label = false;
    let first_hop_start = std::time::Instant::now();
    if direction == EdgeDirection::Out {
        ctx.visit_out_nbrs_batched(
            edge_label,
            src_label,
            dst_label,
            src_ids,
            ts,
            |index, _, label, nbr| {
                foreign_label |= label != neighbor_label;
                raw_endpoints.push(nbr.endpoint);
                offsets[index + 1] += 1;
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
                foreign_label |= label != neighbor_label;
                raw_endpoints.push(nbr.endpoint);
                offsets[index + 1] += 1;
            },
        );
    }
    let first_hop_us = first_hop_start.elapsed().as_micros() as u64;
    if foreign_label {
        return portable_frontier_degrees(ctx, space, src_ids, direction, edge_types);
    }
    for index in 1..offsets.len() {
        offsets[index] += offsets[index - 1];
    }
    let direct: Vec<usize> = offsets.windows(2).map(|pair| pair[1] - pair[0]).collect();

    // Proven dangling-free endpoints skip the external-identity resolution
    // and the visibility filter entirely: inserts validate endpoints, the
    // token rules out deletes since, and the read-only/TTL gates below keep
    // write transactions and expiring tags on the checked path.
    let skip_visibility = !ctx.space_has_vertex_ttl(space)
        && !ctx.space_has_edge_ttl(space)
        && ctx.allows_dangling_skip()
        && ctx.is_edge_type_dangling_free(space, edge_type_name);
    let mut visible_offsets = Vec::with_capacity(src_ids.len() + 1);
    let mut endpoints = Vec::with_capacity(raw_endpoints.len());
    let resolve_start = std::time::Instant::now();
    if skip_visibility {
        endpoints.extend_from_slice(&raw_endpoints);
        visible_offsets.extend_from_slice(&offsets);
    } else {
        // Keep the portable path's dangling-neighbor visibility with one
        // batched resolution, then record the same read keys when a
        // recorder is active.
        let external = ctx.external_ids_by_internal_batch(neighbor_label, &raw_endpoints, ts);
        let recording = ctx.mutation_recorder().is_some();
        let mut read_vids: Vec<VertexId> = Vec::new();
        for pair in offsets.windows(2) {
            visible_offsets.push(endpoints.len());
            for position in pair[0]..pair[1] {
                if let Some(ext) = external[position] {
                    if recording {
                        read_vids.push(ext);
                    }
                    endpoints.push(raw_endpoints[position]);
                }
            }
        }
        visible_offsets.push(endpoints.len());
        if recording {
            record_vertex_reads(ctx, &read_vids);
        }
    }
    let resolve_us = resolve_start.elapsed().as_micros() as u64;
    // Read tail degrees in internal-endpoint order so scattered CSR head
    // reads become sequential runs. The per-seed sums are order-independent,
    // so the sorted reads scatter back into the seed-indexed totals.
    let sort_start = std::time::Instant::now();
    let mut order: Vec<(u32, usize)> = Vec::with_capacity(endpoints.len());
    for (seed_index, pair) in visible_offsets.windows(2).enumerate() {
        for position in pair[0]..pair[1] {
            order.push((endpoints[position], seed_index));
        }
    }
    order.sort_unstable_by_key(|&(endpoint, _)| endpoint);
    let sorted: Vec<u32> = order.iter().map(|&(endpoint, _)| endpoint).collect();
    let sort_us = sort_start.elapsed().as_micros() as u64;
    let tail_start = std::time::Instant::now();
    // The endpoint-sorted order places repeat visits of one endpoint (one
    // per seed sharing it) adjacently. Read each distinct endpoint once and
    // scatter the same degree to every seed pair: identical sums with fewer
    // CSR head reads, and no extra allocation since adjacency comes free
    // from the sort above.
    let mut unique: Vec<u32> = Vec::with_capacity(sorted.len());
    let mut last_endpoint: Option<u32> = None;
    for endpoint in &sorted {
        if last_endpoint != Some(*endpoint) {
            unique.push(*endpoint);
            last_endpoint = Some(*endpoint);
        }
    }
    let unique_degrees = if direction == EdgeDirection::Out {
        ctx.count_out_nbrs_by_internal(edge_label, neighbor_label, &unique, ts)
    } else {
        ctx.count_in_nbrs_by_internal(edge_label, neighbor_label, &unique, ts)
    };
    let mut two_hop = vec![0usize; src_ids.len()];
    let mut cursor = 0usize;
    let mut last_scattered: Option<u32> = None;
    let mut current_degree = 0usize;
    for &(endpoint, seed_index) in &order {
        if last_scattered != Some(endpoint) {
            current_degree = unique_degrees[cursor];
            cursor += 1;
            last_scattered = Some(endpoint);
        }
        two_hop[seed_index] += current_degree;
    }
    let tail_us = tail_start.elapsed().as_micros() as u64;
    Ok(FrontierDegrees {
        direct,
        two_hop,
        dangling_skipped: skip_visibility,
        first_hop_us,
        sort_us,
        resolve_us,
        tail_us,
    })
}

/// Two-hop tail degrees derived from the shared frontier staging.
pub(crate) fn neighbor_degree_batch(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<Vec<usize>> {
    Ok(frontier_degree_batch(ctx, space, src_ids, direction, edge_types)?.two_hop)
}

fn portable_frontier_degrees(
    ctx: &GraphStorageContext,
    space: &str,
    src_ids: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> StorageResult<FrontierDegrees> {
    let direct = out_degree_batch(ctx, space, src_ids, direction, edge_types)?;
    let neighbors = neighbor_dst_ids_batch(ctx, space, src_ids, direction, edge_types)?;
    let lengths: Vec<usize> = neighbors.iter().map(Vec::len).collect();
    let flat: Vec<VertexId> = neighbors.into_iter().flatten().collect();
    let degrees = out_degree_batch(ctx, space, &flat, direction, edge_types)?;
    let mut two_hop = Vec::with_capacity(src_ids.len());
    let mut offset = 0;
    for length in lengths {
        let sum: usize = degrees[offset..offset + length].iter().sum();
        two_hop.push(sum);
        offset += length;
    }
    Ok(FrontierDegrees {
        direct,
        two_hop,
        dangling_skipped: false,
        first_hop_us: 0,
        sort_us: 0,
        resolve_us: 0,
        tail_us: 0,
    })
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
