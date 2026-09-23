use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::endpoint_label_id;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

pub(crate) fn vid_to_string(vid: &VertexId) -> String {
    if let Some(s) = vid.as_str() {
        s.to_string()
    } else if let Some(i) = vid.as_int64() {
        i.to_string()
    } else if let Some(u) = vid.as_u64() {
        u.to_string()
    } else {
        format!("{:?}", vid.as_bytes())
    }
}

/// Parse an external ID string back into a VertexId.
///
/// Strict user-visible behavior without silent truncation: numeric strings
/// become integer ids through the rejecting constructor, negative integers
/// fall back to text, and overlong text yields an empty id that later stages
/// reject instead of a truncated key.
pub(crate) fn vid_from_str(id: &str) -> VertexId {
    if let Ok(parsed) = id.parse::<i64>() {
        if let Ok(vid) = VertexId::try_from_int64(parsed) {
            return vid;
        }
    }
    VertexId::try_from_string(id).unwrap_or_default()
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
