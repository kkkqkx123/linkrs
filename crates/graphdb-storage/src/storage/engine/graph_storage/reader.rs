use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::core::types::{EdgeId, EdgeTypeInfo, LabelId, TagInfo, Timestamp, VertexId};
use crate::core::vertex_edge_path::Tag;
use crate::core::{Edge, EdgeDirection, StorageError, StorageResult, Value, Vertex};
use crate::storage::cold::{ColdIndexEntry, ColdSnapshot};
use crate::storage::edge::edge_table::core::TimeTravelEdgeStore;
use crate::storage::edge::EdgeStore;
use crate::storage::edge::Nbr;
use crate::storage::engine::data_store::EdgeTableKey;
use crate::storage::engine::params::EdgeOperationParams;

use super::context::helpers;
use super::context::GraphStorageContext;
use super::ops::{
    edge_record_to_edge, endpoint_label_id, serialize_properties, value_to_string,
    vertex_record_to_vertex,
};

/// Resolve a VertexId to its internal u32 CSR index via vertex tables.
fn vertex_id_to_internal(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: &VertexId,
    ts: Timestamp,
) -> Option<u32> {
    ctx.data_store()
        .with_vertex_tables(|tables| helpers::resolve_internal_id(ctx, tables, label, *vid, ts))
}

fn vid_to_string(vid: &VertexId) -> String {
    if let Some(s) = vid.as_str() {
        s.to_string()
    } else if let Some(i) = vid.as_int64() {
        i.to_string()
    } else {
        format!("{:?}", vid.as_bytes())
    }
}

/// Parse an external ID string back into a VertexId.
fn vid_from_str(id: &str) -> VertexId {
    if let Ok(parsed) = id.parse::<i64>() {
        VertexId::from_int64(parsed)
    } else {
        VertexId::from_string(id)
    }
}

/// Resolve a vertex table internal index to its external ID string.
///
/// Mirrors the hot-path resolution: try the timestamp-valid lookup first,
/// then the raw lookup, then fall back to the raw internal value.
fn external_id_string(
    ctx: &GraphStorageContext,
    label: LabelId,
    internal: u32,
    fallback: &VertexId,
    ts: Timestamp,
) -> String {
    if label != 0 {
        ctx.get_external_id(label, internal, ts)
            .or_else(|| {
                ctx.get_external_id_by_internal_id(label, internal)
                    .map(|v| vid_to_string(&v))
            })
            .unwrap_or_else(|| vid_to_string(fallback))
    } else {
        ctx.get_external_id_any(internal, ts)
            .unwrap_or_else(|| vid_to_string(fallback))
    }
}

fn record_vertex_read(ctx: &GraphStorageContext, vid: VertexId) {
    if let Some(recorder) = ctx.mutation_recorder() {
        recorder.record_vertex_read(vid);
    }
}

fn record_edge_read(ctx: &GraphStorageContext, edge: crate::core::types::EdgeIdentifier) {
    if let Some(recorder) = ctx.mutation_recorder() {
        recorder.record_edge_read(edge);
    }
}

fn record_schema_read(ctx: &GraphStorageContext, space: &str) {
    if let Some(recorder) = ctx.mutation_recorder() {
        recorder.record_schema_read(space);
    }
}

pub(crate) fn get_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
) -> StorageResult<Option<Vertex>> {
    get_vertex_impl(ctx, space, id, None)
}

pub(crate) fn get_vertex_projected(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
    projection: &[String],
) -> StorageResult<Option<Vertex>> {
    get_vertex_impl(ctx, space, id, Some(projection))
}

fn get_vertex_impl(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
    projection: Option<&[String]>,
) -> StorageResult<Option<Vertex>> {
    record_vertex_read(ctx, *id);
    record_schema_read(ctx, space);
    let _space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let tags = ctx.schema_manager().list_tags(space)?;
    if tags.is_empty() {
        return Ok(None);
    }

    let ts = ctx.get_read_timestamp();
    let mut all_tags: Vec<Tag> = Vec::new();
    let mut merged_properties: HashMap<String, Value> = HashMap::new();
    let mut internal_id = 0u32;

    for tag in &tags {
        let label_id = tag.tag_id;
        let record = if let Some(id_int) = id.as_int64() {
            match projection {
                Some(proj) => ctx.get_vertex_by_i64_projected(label_id, id_int, proj, ts),
                None => ctx.get_vertex_by_i64(label_id, id_int, ts),
            }
        } else if let Some(id_str) = id.as_str() {
            match projection {
                Some(proj) => ctx.get_vertex_projected(label_id, id_str, proj, ts),
                None => ctx.get_vertex(label_id, id_str, ts),
            }
        } else {
            let id_str = id.to_string();
            match projection {
                Some(proj) => ctx.get_vertex_projected(label_id, &id_str, proj, ts),
                None => ctx.get_vertex(label_id, &id_str, ts),
            }
        };

        if let Some(record) = record {
            internal_id = record.internal_id;
            let props: HashMap<String, Value> = record.properties.iter().cloned().collect();
            all_tags.push(Tag::new(tag.tag_name.clone(), props.clone()));
            merged_properties.extend(props);
        }
    }

    if all_tags.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Vertex {
            vid: *id,
            id: internal_id as i64,
            tags: all_tags,
            properties: merged_properties,
        }))
    }
}

pub(crate) fn scan_vertices(ctx: &GraphStorageContext, space: &str) -> StorageResult<Vec<Vertex>> {
    record_schema_read(ctx, space);
    let tags = ctx.schema_manager().list_tags(space)?;
    let ts = ctx.get_read_timestamp();

    // Read per-tag in batches directly from vertex tables, merging by vertex ID.
    // This avoids the intermediate Vec<VertexRecord> allocation per tag that
    // ctx.scan_vertices() produces via table.scan(ts).collect().
    struct MergedVertex {
        vid: VertexId,
        internal_id: u32,
        tags: Vec<Tag>,
        properties: HashMap<String, Value>,
    }

    let mut merged: HashMap<VertexId, MergedVertex> = HashMap::new();

    const BATCH_SIZE: usize = 256;

    for tag in &tags {
        let tag_id = tag.tag_id;
        let tag_name = &tag.tag_name;
        // Lazily register the statement snapshot for this label.
        ctx.ensure_vertex_snapshot_registered(tag_id);
        ctx.data_store().with_vertex_tables(|tables| {
            if let Some(table) = tables.get(&tag_id) {
                let records = table.scan(ts);
                for chunk in records.chunks(BATCH_SIZE) {
                    for record in chunk {
                        record_vertex_read(ctx, record.vid);
                        let entry = merged.entry(record.vid).or_insert_with(|| MergedVertex {
                            vid: record.vid,
                            internal_id: record.internal_id,
                            tags: Vec::new(),
                            properties: HashMap::new(),
                        });
                        entry.internal_id = record.internal_id;
                        let props: HashMap<String, Value> =
                            record.properties.iter().cloned().collect();
                        entry.tags.push(Tag::new(tag_name.clone(), props.clone()));
                        entry.properties.extend(props);
                    }
                }
            }
        });
    }

    Ok(merged
        .into_values()
        .map(|mv| Vertex {
            vid: mv.vid,
            id: mv.internal_id as i64,
            tags: mv.tags,
            properties: mv.properties,
        })
        .collect())
}

pub(crate) fn scan_vertices_by_tag(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
) -> StorageResult<Vec<Vertex>> {
    record_schema_read(ctx, space);
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut vertices = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            record_vertex_read(ctx, record.vid);
            let vertex = vertex_record_to_vertex(&record, tag);
            vertices.push(vertex);
        }
    }

    Ok(vertices)
}

pub(crate) fn scan_vertices_by_prop(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    prop: &str,
    value: &Value,
) -> StorageResult<Vec<Vertex>> {
    record_schema_read(ctx, space);
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut vertices = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            record_vertex_read(ctx, record.vid);
            if record
                .properties
                .iter()
                .any(|(k, v)| k == prop && v == value)
            {
                let vertex = vertex_record_to_vertex(&record, tag);
                vertices.push(vertex);
            }
        }
    }

    Ok(vertices)
}

pub(crate) fn get_edge(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
) -> StorageResult<Option<Edge>> {
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
        crate::core::types::EdgeIdentifier::new(
            src_label_id,
            *src,
            dst_label_id,
            *dst,
            edge_label_id,
            rank,
        ),
    );
    let src_str = src.to_string();
    let dst_str = dst.to_string();

    if let Some(record) = ctx.get_edge(
        &EdgeOperationParams {
            edge_label: edge_label_id,
            src_label: src_label_id,
            src_id: *src,
            dst_label: dst_label_id,
            dst_id: *dst,
            rank,
        },
        ts,
    ) {
        let edge = edge_record_to_edge(&record, edge_type, &src_str, &dst_str);
        return Ok(Some(edge));
    }

    // Fallback: check cold snapshots if hot missed
    if ts >= snapshot_min_ts(ctx, edge_label_id) {
        if let Some((snapshot, nbr, src_internal, dst_internal_vid)) = query_cold_edge(
            ctx,
            edge_label_id,
            *src,
            *dst,
            src_label_id,
            dst_label_id,
            ts,
        ) {
            let record = snapshot.nbr_to_edge_record(
                &nbr,
                VertexId::from_int64(src_internal as i64),
                dst_internal_vid,
            );
            let edge = edge_record_to_edge(&record, edge_type, &src_str, &dst_str);
            return Ok(Some(edge));
        }
    }

    Ok(None)
}

/// Get the minimum snapshot timestamp for a cold snapshot, or u64::MAX if none.
fn snapshot_min_ts(ctx: &GraphStorageContext, label: LabelId) -> Timestamp {
    ctx.cold_snapshots()
        .read()
        .get(&label)
        .and_then(|snapshots| snapshots.iter().map(|s| s.snapshot_ts()).min())
        .unwrap_or(u64::MAX)
}

/// Look up a single edge in cold snapshots by source/dest VertexId.
///
/// Returns the matched neighbor together with the owning snapshot and the
/// internal indices of both endpoints. Snapshots are probed newest-first.
/// The cold CSR indexes vertices by vertex-table internal IDs, matching the
/// hot edge table, so `vertex_id_to_internal` applies to both.
fn query_cold_edge(
    ctx: &GraphStorageContext,
    edge_label: LabelId,
    src: VertexId,
    dst: VertexId,
    src_label: LabelId,
    dst_label: LabelId,
    ts: Timestamp,
) -> Option<(Arc<crate::storage::cold::ColdSnapshot>, Nbr, u32, VertexId)> {
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

pub(crate) fn get_node_edges(
    ctx: &GraphStorageContext,
    space: &str,
    node_id: &VertexId,
    direction: EdgeDirection,
) -> StorageResult<Vec<Edge>> {
    record_schema_read(ctx, space);
    record_vertex_read(ctx, *node_id);
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    if edge_types.is_empty() {
        return Ok(Vec::new());
    }

    let ts = ctx.get_read_timestamp();
    let node_str = vid_to_string(node_id);
    let mut edges = Vec::new();

    for edge_info in &edge_types {
        let edge_label_id = edge_info.edge_type_id;
        let edge_type_name = &edge_info.edge_type_name;

        let src_label_id = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
            Some(id) => id,
            None => continue,
        };
        let dst_label_id = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
            Some(id) => id,
            None => continue,
        };

        match direction {
            EdgeDirection::Out => {
                if let Some(out_edges) =
                    ctx.out_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in out_edges {
                        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;
                        let dst_external = if dst_label_id != 0 {
                            ctx.get_external_id(dst_label_id, dst_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(dst_label_id, dst_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        } else {
                            ctx.get_external_id_any(dst_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &node_str, &dst_external);
                        edges.push(edge);
                    }
                }
            }
            EdgeDirection::In => {
                if let Some(in_edges) =
                    ctx.in_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in in_edges {
                        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
                        let src_external = if src_label_id != 0 {
                            ctx.get_external_id(src_label_id, src_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(src_label_id, src_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        } else {
                            ctx.get_external_id_any(src_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &src_external, &node_str);
                        edges.push(edge);
                    }
                }
            }
            EdgeDirection::Both => {
                if let Some(out_edges) =
                    ctx.out_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in out_edges {
                        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;
                        let dst_external = if dst_label_id != 0 {
                            ctx.get_external_id(dst_label_id, dst_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(dst_label_id, dst_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        } else {
                            ctx.get_external_id_any(dst_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &node_str, &dst_external);
                        edges.push(edge);
                    }
                }
                if let Some(in_edges) =
                    ctx.in_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in in_edges {
                        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
                        let src_external = if src_label_id != 0 {
                            ctx.get_external_id(src_label_id, src_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(src_label_id, src_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        } else {
                            ctx.get_external_id_any(src_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &src_external, &node_str);
                        edges.push(edge);
                    }
                }
            }
        }

        // Append cold snapshot edges for this edge type
        append_cold_node_edges(
            ctx,
            &mut edges,
            edge_label_id,
            edge_type_name,
            node_id,
            src_label_id,
            dst_label_id,
            direction,
            ts,
        )?;
    }

    Ok(edges)
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
fn append_cold_node_edges(
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
                    let (dst_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                    let dst_internal = dst_internal_vid.as_int64().unwrap_or(0) as u32;
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
                    let (src_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                    let src_internal = src_internal_vid.as_int64().unwrap_or(0) as u32;
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
                        let (dst_internal_vid, rank) =
                            TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                        let dst_internal = dst_internal_vid.as_int64().unwrap_or(0) as u32;
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
                        let (src_internal_vid, rank) =
                            TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                        let src_internal = src_internal_vid.as_int64().unwrap_or(0) as u32;
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

/// Resolve an internal vertex-table id to its external `VertexId` without the
/// string round-trip when a direct raw lookup is available.
fn internal_to_external_vertex_id(
    ctx: &GraphStorageContext,
    label: LabelId,
    internal: u32,
    fallback: &VertexId,
    ts: Timestamp,
) -> VertexId {
    if label != 0 {
        ctx.get_external_id_by_internal_id(label, internal)
            .unwrap_or_else(|| {
                vid_from_str(&external_id_string(ctx, label, internal, fallback, ts))
            })
    } else {
        vid_from_str(&external_id_string(ctx, 0, internal, fallback, ts))
    }
}

/// Lightweight batch neighbor read used by de-materialized expand hops
/// (`id_only`/`count_only`).
///
/// Resolves the edge-type schema once for the whole batch and reads MVCC
/// neighbors directly from the CSR (skipping `EdgeRecord` materialization and
/// per-edge property decoding).  Cold snapshots are merged with the same
/// `(neighbor_internal, rank)` dedup as [`get_node_edges`].  Returns the
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
    let has_cold = !ctx.cold_snapshots().read().is_empty();
    for src_id in src_ids {
        record_vertex_read(ctx, *src_id);
        let mut neighbors: Vec<VertexId> = Vec::new();
        // Dedup is only required to merge cold snapshots against hot data;
        // without cold snapshots each edge is returned at most once by the
        // merged CSR path, so the set is skipped entirely.
        let mut seen: Option<HashSet<(u32, u32, i64)>> = has_cold.then(HashSet::new);
        for (edge_label_id, src_label_id, dst_label_id) in &resolved {
            append_hot_neighbors(
                ctx,
                &mut neighbors,
                seen.as_mut(),
                src_id,
                *edge_label_id,
                *src_label_id,
                *dst_label_id,
                direction,
                ts,
            );
            append_cold_neighbors(
                ctx,
                &mut neighbors,
                seen.as_mut(),
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

/// Batch out-degree read for count-only expand tails.  Counts distinct edges
/// (`(neighbor_internal, rank)` deduped across hot and cold) per source, with
/// the schema resolved once for the whole batch.
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
            count_cold_neighbors(
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
/// `neighbors`, deduped by the full `(src_internal, dst_internal, rank)` edge
/// identity (matching [`get_node_edges`] semantics).  When `seen` is `None`
/// (no cold snapshots to merge) every edge is accepted.
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
            if let Some((src_internal, nbrs)) =
                ctx.out_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (dst_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
        }
        EdgeDirection::In => {
            if let Some((dst_internal, nbrs)) =
                ctx.in_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (src_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
        EdgeDirection::Both => {
            if let Some((src_internal, nbrs)) =
                ctx.out_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (dst_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
            if let Some((dst_internal, nbrs)) =
                ctx.in_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (src_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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

/// Append cold-snapshot neighbors of `src_id` with the same dedup as the hot
/// path, mirroring [`append_cold_node_edges`] but without materializing
/// `Edge` records.
#[allow(clippy::too_many_arguments)]
fn append_cold_neighbors(
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
                    let (dst_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
                    let (src_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
                        let (dst_internal_vid, rank) =
                            TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
                        let (src_internal_vid, rank) =
                            TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
            if let Some((src_internal, nbrs)) =
                ctx.out_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (dst_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                    if let Some(dst_internal) = dst_internal_vid.as_int64() {
                        seen.insert((src_internal, dst_internal as u32, rank));
                    }
                }
            }
        }
        EdgeDirection::In => {
            if let Some((dst_internal, nbrs)) =
                ctx.in_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (src_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        seen.insert((src_internal as u32, dst_internal, rank));
                    }
                }
            }
        }
        EdgeDirection::Both => {
            if let Some((src_internal, nbrs)) =
                ctx.out_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (dst_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                    if let Some(dst_internal) = dst_internal_vid.as_int64() {
                        seen.insert((src_internal, dst_internal as u32, rank));
                    }
                }
            }
            if let Some((dst_internal, nbrs)) =
                ctx.in_nbrs(edge_label_id, src_label_id, dst_label_id, *src_id, ts)
            {
                for nbr in nbrs {
                    let (src_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        seen.insert((src_internal as u32, dst_internal, rank));
                    }
                }
            }
        }
    }
}

/// Count cold-snapshot neighbors of `src_id` into `seen`.
#[allow(clippy::too_many_arguments)]
fn count_cold_neighbors(
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
                    let (dst_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
                    let (src_internal_vid, rank) =
                        TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                    if let Some(src_internal) = src_internal_vid.as_int64() {
                        seen.insert((src_internal as u32, dst_internal, rank));
                    }
                }
            }
            EdgeDirection::Both => {
                if let Some(src_internal) = vertex_id_to_internal(ctx, src_label_id, src_id, ts) {
                    for nbr in snapshot.get_out_edges(src_internal) {
                        let (dst_internal_vid, rank) =
                            TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                        if let Some(dst_internal) = dst_internal_vid.as_int64() {
                            seen.insert((src_internal, dst_internal as u32, rank));
                        }
                    }
                }
                if let Some(dst_internal) = vertex_id_to_internal(ctx, dst_label_id, src_id, ts) {
                    for nbr in snapshot.get_in_edges(dst_internal) {
                        let (src_internal_vid, rank) =
                            TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                        if let Some(src_internal) = src_internal_vid.as_int64() {
                            seen.insert((src_internal as u32, dst_internal, rank));
                        }
                    }
                }
            }
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
                    .filter(|(_, arc)| arc.read().0.label() == edge_label_id)
                    .map(|(key, arc)| (*key, arc.clone()))
                    .collect()
            });

        // Lazily register the statement snapshots for every matching partition
        // before scanning so GC cannot reclaim versions under this statement.
        for (key, _) in &matching {
            ctx.ensure_edge_snapshot_registered(*key);
        }

        use rayon::prelude::*;
        let per_partition: Vec<Vec<Edge>> = matching
            .par_iter()
            .map(|(_key, arc)| {
                let guard = arc.read();
                let mut iter = guard.0.iter(ts);
                let mut partition_edges = Vec::new();
                loop {
                    let batch: Vec<_> = iter.by_ref().take(BATCH_SIZE).collect();
                    if batch.is_empty() {
                        break;
                    }
                    for record in batch {
                        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
                        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;

                        let tbl_src = guard.0.src_label();
                        let tbl_dst = guard.0.dst_label();

                        let src_external = if tbl_src != 0 {
                            ctx.get_external_id(tbl_src, src_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(tbl_src, src_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| format!("{}", record.src_vid))
                        } else {
                            ctx.get_external_id_any(src_internal, ts)
                                .unwrap_or_else(|| format!("{}", record.src_vid))
                        };

                        let dst_external = if tbl_dst != 0 {
                            ctx.get_external_id(tbl_dst, dst_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(tbl_dst, dst_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| format!("{}", record.dst_vid))
                        } else {
                            ctx.get_external_id_any(dst_internal, ts)
                                .unwrap_or_else(|| format!("{}", record.dst_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type, &src_external, &dst_external);
                        partition_edges.push(edge);
                    }
                }
                partition_edges
            })
            .collect();

        let mut edges: Vec<Edge> = per_partition.into_iter().flatten().collect();
        edges = append_cold_scan_edges(ctx, edges, edge_label_id, edge_type, 0, 0, ts);
        return Ok(edges);
    }

    // Constrained path: access the specific edge table directly using iter()
    // instead of ctx.scan_edges() which collects into Vec.
    {
        let key = EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
        // Lazily register the statement snapshot for this partition.
        ctx.ensure_edge_snapshot_registered(key);
        ctx.data_store().with_edge_tables(|edge_tables| {
            if let Some(arc) = edge_tables.get(&key) {
                let guard = arc.read();
                let mut iter = guard.0.iter(ts);
                loop {
                    let batch: Vec<_> = iter.by_ref().take(BATCH_SIZE).collect();
                    if batch.is_empty() {
                        break;
                    }
                    for record in batch {
                        record_edge_read(
                            ctx,
                            crate::core::types::EdgeIdentifier::new(
                                src_label_id,
                                record.src_vid,
                                dst_label_id,
                                record.dst_vid,
                                edge_label_id,
                                record.rank,
                            ),
                        );
                        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
                        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;

                        let src_external = ctx
                            .get_external_id(src_label_id, src_internal, ts)
                            .or_else(|| {
                                ctx.get_external_id_by_internal_id(src_label_id, src_internal)
                                    .map(|v| vid_to_string(&v))
                            })
                            .unwrap_or_else(|| format!("{}", record.src_vid));

                        let dst_external = ctx
                            .get_external_id(dst_label_id, dst_internal, ts)
                            .or_else(|| {
                                ctx.get_external_id_by_internal_id(dst_label_id, dst_internal)
                                    .map(|v| vid_to_string(&v))
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

    edges = append_cold_scan_edges(
        ctx,
        edges,
        edge_label_id,
        edge_type,
        src_label_id,
        dst_label_id,
        ts,
    );
    Ok(edges)
}

/// Resolve the edge table labels for a named edge type.
fn resolve_edge_table_labels(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<(LabelId, LabelId, LabelId)> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;
    let src_label = endpoint_label_id(ctx, space, &edge_info.src_tag_name)?.unwrap_or(0);
    let dst_label = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)?.unwrap_or(0);
    Ok((src_label, dst_label, edge_info.edge_type_id))
}

/// Enable the per-table edge property index for `edge_type`.
pub(crate) fn enable_edge_property_index(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    pool_capacity: u64,
) -> StorageResult<bool> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    if src_label != 0 && dst_label != 0 {
        ctx.enable_edge_property_index(src_label, dst_label, edge_label, pool_capacity)?;
    } else {
        // Unconstrained endpoint tags: enable on every table of this edge type.
        ctx.data_store()
            .with_edge_tables(|tables| -> StorageResult<()> {
                let matching: Vec<_> = tables
                    .values()
                    .filter(|arc| arc.read().0.label() == edge_label)
                    .cloned()
                    .collect();
                for arc in matching {
                    arc.write().0.enable_property_index(pool_capacity)?;
                }
                Ok(())
            })?;
    }
    Ok(true)
}

/// Whether the per-table edge property index is enabled for `edge_type`.
pub(crate) fn has_edge_property_index(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<bool> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    if src_label != 0 && dst_label != 0 {
        Ok(ctx.has_edge_property_index(src_label, dst_label, edge_label))
    } else {
        Ok(ctx.data_store().with_edge_tables(|tables| {
            tables
                .values()
                .filter(|arc| arc.read().0.label() == edge_label)
                .any(|arc| arc.read().0.has_property_index())
        }))
    }
}

/// Drop the per-table edge property index for `edge_type`.
pub(crate) fn disable_edge_property_index(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<()> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    if src_label != 0 && dst_label != 0 {
        ctx.disable_edge_property_index(src_label, dst_label, edge_label)?;
    } else {
        ctx.data_store()
            .with_edge_tables(|tables| -> StorageResult<()> {
                for arc in tables
                    .values()
                    .filter(|arc| arc.read().0.label() == edge_label)
                {
                    arc.write().0.disable_property_index();
                }
                Ok(())
            })?;
    }
    Ok(())
}

/// Look up edges of `edge_type` whose `prop_name` value falls in `[lower, upper)`.
///
/// Bounds are encoded with the ordered codec; the inclusion flags control
/// whether the boundary values themselves are part of the range.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lookup_edges_by_property_range(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    prop_name: &str,
    lower: Option<&Value>,
    upper: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> StorageResult<Vec<Edge>> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    let codec = crate::core::value::ordered_codec::OrderedCodec::new();
    // Degenerate range [v, v) with an exclusive upper bound is interpreted as
    // a prefix/equality bound: everything from v up to the next value boundary.
    let prefix_bounds = include_lower && !include_upper && lower.is_some() && upper == lower;
    let value_lower = match lower {
        Some(value) => {
            let encoded = codec.encode(value)?;
            if include_lower {
                encoded
            } else {
                crate::core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&encoded)
            }
        }
        None => Vec::new(),
    };
    let value_upper = match upper {
        Some(value) => {
            let encoded = codec.encode(value)?;
            if prefix_bounds || include_upper {
                crate::core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&encoded)
            } else {
                encoded
            }
        }
        None => Vec::new(),
    };

    let ts = ctx.get_read_timestamp();
    let mut edges = Vec::new();

    let records = if src_label != 0 && dst_label != 0 {
        ctx.lookup_edges_by_property_range(
            src_label,
            dst_label,
            edge_label,
            prop_name,
            &value_lower,
            &value_upper,
            ts,
        )
    } else {
        ctx.data_store().with_edge_tables(|tables| {
            let matching: Vec<_> = tables
                .values()
                .filter(|arc| arc.read().0.label() == edge_label)
                .cloned()
                .collect();
            let mut records = Vec::new();
            for arc in matching {
                let table = arc.read();
                records.extend(
                    table
                        .0
                        .lookup_edges_by_property_range(prop_name, &value_lower, &value_upper)
                        .into_iter()
                        .filter_map(|(src, dst, rank)| table.0.get_edge(src, dst, rank, ts)),
                );
            }
            records
        })
    };

    for record in &records {
        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;
        let src_external = if src_label != 0 {
            ctx.get_external_id(src_label, src_internal, ts)
                .or_else(|| {
                    ctx.get_external_id_by_internal_id(src_label, src_internal)
                        .map(|v| vid_to_string(&v))
                })
                .unwrap_or_else(|| format!("{}", record.src_vid))
        } else {
            ctx.get_external_id_any(src_internal, ts)
                .unwrap_or_else(|| format!("{}", record.src_vid))
        };
        let dst_external = if dst_label != 0 {
            ctx.get_external_id(dst_label, dst_internal, ts)
                .or_else(|| {
                    ctx.get_external_id_by_internal_id(dst_label, dst_internal)
                        .map(|v| vid_to_string(&v))
                })
                .unwrap_or_else(|| format!("{}", record.dst_vid))
        } else {
            ctx.get_external_id_any(dst_internal, ts)
                .unwrap_or_else(|| format!("{}", record.dst_vid))
        };
        edges.push(edge_record_to_edge(
            record,
            edge_type,
            &src_external,
            &dst_external,
        ));
    }

    // Cold snapshot property index: same encoded bounds as the hot index.
    // Dedup happens in internal-ID space (the CSR row indices shared by the
    // hot lookup records and the cold index entries).
    let cold = ctx.cold_snapshots().read();
    if let Some(snapshots) = cold.get(&edge_label) {
        let mut seen: HashSet<(u32, u32, i64)> = records
            .iter()
            .map(|r| {
                (
                    r.src_vid.as_int64().unwrap_or(0) as u32,
                    r.dst_vid.as_int64().unwrap_or(0) as u32,
                    r.rank,
                )
            })
            .collect();
        for snapshot in snapshots.iter().filter(|s| ts >= s.snapshot_ts()) {
            let Some(index) = snapshot.property_index() else {
                continue;
            };
            if !index.has_property(prop_name) {
                continue;
            }
            for entry in index.lookup(prop_name, &value_lower, &value_upper) {
                let key = (entry.src_internal, entry.dst_internal, entry.rank);
                if seen.insert(key) {
                    edges.push(cold_index_entry_to_edge(
                        ctx, snapshot, &entry, edge_type, src_label, dst_label, ts,
                    ));
                }
            }
        }
    }

    Ok(edges)
}

/// Materialize an edge from a cold property-index hit.
fn cold_index_entry_to_edge(
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
    let nbr = Nbr::new(
        TimeTravelEdgeStore::edge_endpoint_key(entry.dst_internal, entry.rank),
        EdgeId(0),
        entry.prop_offset,
        snapshot.snapshot_ts(),
    );
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
fn append_cold_scan_edges(
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
    let mut cursor = super::cursor_impl::create_edge_cursor(
        Arc::new(ctx.clone()),
        space,
        &crate::storage::cursor::ScanOptions {
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

pub(crate) fn count_vertices_by_tag(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
) -> StorageResult<u64> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let count = ctx.data_store().with_vertex_tables(|vertex_tables| {
        vertex_tables
            .get(&tag_info.tag_id)
            .map(|t| t.total_count() as u64)
            .unwrap_or(0)
    });
    // Lazily register the statement snapshot for this label.
    ctx.ensure_vertex_snapshot_registered(tag_info.tag_id);
    Ok(count)
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

    let ts = ctx.get_read_timestamp();

    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(0),
    };

    let hot_count = if src_label_id == 0 && dst_label_id == 0 {
        // Lazily register the statement snapshots for every matching partition.
        for key in ctx.data_store().with_edge_tables(|edge_tables| {
            edge_tables
                .keys()
                .copied()
                .filter(|key| key.edge_label == edge_label_id)
                .collect::<Vec<_>>()
        }) {
            ctx.ensure_edge_snapshot_registered(key);
        }
        ctx.data_store().with_edge_tables(|edge_tables| {
            edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label_id)
                .map(|t| t.edge_count())
                .sum()
        })
    } else {
        let key = crate::storage::engine::data_store::EdgeTableKey::new(
            src_label_id,
            dst_label_id,
            edge_label_id,
        );
        ctx.ensure_edge_snapshot_registered(key);
        ctx.data_store()
            .with_single_edge_table(&key, |t| Ok(t.edge_count()))
            .unwrap_or(0)
    };

    let cold_count = ctx
        .cold_snapshots()
        .read()
        .get(&edge_label_id)
        .map(|snapshots| {
            snapshots
                .iter()
                .filter(|s| ts >= s.snapshot_ts())
                .map(|s| s.edge_count())
                .sum::<u64>()
        })
        .unwrap_or(0);

    Ok(hot_count + cold_count)
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

pub(crate) fn get_vertex_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    id: &Value,
) -> StorageResult<Option<(TagInfo, Vec<u8>)>> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let id_str = value_to_string(id);

    let label_id = tag_info.tag_id;
    if let Some(record) = ctx.get_vertex(label_id, &id_str, ts) {
        let data = serialize_properties(&record.properties);
        return Ok(Some((tag_info, data)));
    }

    Ok(None)
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

    // Fallback: check cold snapshots if hot missed
    if ts >= snapshot_min_ts(ctx, edge_label_id) {
        if let Some((snapshot, nbr, src_internal, dst_internal_vid)) = query_cold_edge(
            ctx,
            edge_label_id,
            src_vid,
            dst_vid,
            src_label_id,
            dst_label_id,
            ts,
        ) {
            let record = snapshot.nbr_to_edge_record(
                &nbr,
                VertexId::from_int64(src_internal as i64),
                dst_internal_vid,
            );
            let data = serialize_properties(&record.properties);
            return Ok(Some((edge_info, data)));
        }
    }

    Ok(None)
}

pub(crate) fn scan_vertices_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
) -> StorageResult<Vec<(TagInfo, Vec<u8>)>> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut results = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            let data = serialize_properties(&record.properties);
            results.push((tag_info.clone(), data));
        }
    }

    Ok(results)
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
