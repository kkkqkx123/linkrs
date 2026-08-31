use crate::engine::graph_storage::context::helpers;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::endpoint_label_id;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

/// Resolve a VertexId to its internal u32 CSR index via vertex tables.
pub(crate) fn vertex_id_to_internal(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: &VertexId,
    ts: Timestamp,
) -> Option<u32> {
    ctx.data_store()
        .with_vertex_tables(|tables| helpers::resolve_internal_id(ctx, tables, label, *vid, ts))
}

pub(crate) fn vid_to_string(vid: &VertexId) -> String {
    if let Some(s) = vid.as_str() {
        s.to_string()
    } else if let Some(i) = vid.as_int64() {
        i.to_string()
    } else {
        format!("{:?}", vid.as_bytes())
    }
}

/// Parse an external ID string back into a VertexId.
pub(crate) fn vid_from_str(id: &str) -> VertexId {
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
pub(crate) fn external_id_string(
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

pub(crate) fn record_vertex_read(ctx: &GraphStorageContext, vid: VertexId) {
    if let Some(recorder) = ctx.mutation_recorder() {
        recorder.record_vertex_read(vid);
    }
}

pub(crate) fn record_edge_read(
    ctx: &GraphStorageContext,
    edge: graphdb_core::types::EdgeIdentifier,
) {
    if let Some(recorder) = ctx.mutation_recorder() {
        recorder.record_edge_read(edge);
    }
}

pub(crate) fn record_schema_read(ctx: &GraphStorageContext, space: &str) {
    if let Some(recorder) = ctx.mutation_recorder() {
        recorder.record_schema_read(space);
    }
}

/// Get the minimum snapshot timestamp for a cold snapshot, or u64::MAX if none.
pub(crate) fn snapshot_min_ts(ctx: &GraphStorageContext, label: LabelId) -> Timestamp {
    ctx.cold_snapshots()
        .read()
        .get(&label)
        .and_then(|snapshots| snapshots.iter().map(|s| s.snapshot_ts()).min())
        .unwrap_or(u64::MAX)
}

/// Resolve an internal vertex-table id to its external `VertexId` without the
/// string round-trip when a direct raw lookup is available.
pub(crate) fn internal_to_external_vertex_id(
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

/// Resolve the edge table labels for a named edge type.
pub(crate) fn resolve_edge_table_labels(
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
