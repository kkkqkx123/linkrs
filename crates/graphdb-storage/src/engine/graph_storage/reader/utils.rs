use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::endpoint_label_id;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

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

/// Resolve an internal vertex-table id to its external `VertexId`.
///
/// Raw lookup first so edge endpoints still resolve past a vertex deletion,
/// then the timestamp-valid lookup. Returns `None` when the row has no
/// external key. There is no fallback value and no label-less probing:
/// callers must pass the owning label.
pub(crate) fn internal_to_external_vertex_id(
    ctx: &GraphStorageContext,
    label: LabelId,
    internal: u32,
    ts: Timestamp,
) -> Option<VertexId> {
    if let Some(vid) = ctx.get_external_id_by_internal_id(label, internal) {
        return Some(vid);
    }
    ctx.get_external_vertex_id(label, internal, ts)
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
