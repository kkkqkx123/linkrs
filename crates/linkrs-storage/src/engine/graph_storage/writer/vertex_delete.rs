use linkrs_core::types::{EdgeIdentifier, LabelId, Timestamp, VertexId};
use linkrs_core::wal::redo::{DeleteEdgeRedo, DeleteVertexRedo};
use linkrs_core::wal::types::WalOpType;
use linkrs_core::{StorageError, StorageResult, Value};

use super::super::context::helpers;
use super::super::context::txn_staging::StagedIndexOp;
use super::super::context::GraphStorageContext;
use super::super::ops::{endpoint_label_id, route_vertex_id, tag_label_id, RoutedVertexId};
use crate::engine::data_store::EdgeTableKey;
use crate::index::types::EdgeIdentity;

/// Warn when a non-cascading vertex delete leaves incident edges behind.
///
/// Plain vertex deletion keeps incident edges by design, so the orphaned
/// rows stay visible until repaired. Each matching edge type is probed with
/// a limit of one, which costs a single row visit on connected vertices and
/// nothing when the space has no edge types at all.
fn warn_if_leaves_dangling_edges(
    ctx: &GraphStorageContext,
    space: &str,
    tag_name: &str,
    label_id: LabelId,
    vid: &VertexId,
) {
    let edge_types = match ctx.schema_manager().list_edge_types(space) {
        Ok(types) if !types.is_empty() => types,
        _ => return,
    };
    let ts = ctx.get_read_timestamp();
    for edge_info in &edge_types {
        let (Some(src_label), Some(dst_label)) = (
            endpoint_label_id(ctx, space, &edge_info.src_tag_name).unwrap_or(None),
            endpoint_label_id(ctx, space, &edge_info.dst_tag_name).unwrap_or(None),
        ) else {
            continue;
        };
        // Unconstrained endpoints resolve to label 0 and match every tag.
        let out_hit = (src_label == 0 || src_label == label_id)
            && ctx
                .out_edges_projected_limit(
                    edge_info.edge_type_id,
                    src_label,
                    *vid,
                    ts,
                    Some(&[]),
                    1,
                )
                .is_some_and(|records| !records.is_empty());
        let in_hit = !out_hit
            && (dst_label == 0 || dst_label == label_id)
            && ctx
                .in_edges_projected_limit(edge_info.edge_type_id, dst_label, *vid, ts, Some(&[]), 1)
                .is_some_and(|records| !records.is_empty());
        if out_hit || in_hit {
            log::warn!(
                "delete_vertex {}.{}:{} leaves incident {} edges behind; \
                 use delete_vertex_with_edges for cascade deletes or run \
                 repair_dangling_edges afterwards",
                space,
                tag_name,
                vid,
                edge_info.edge_type_name,
            );
            return;
        }
    }
}

pub(crate) fn delete_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    tag_name: &str,
    id: &VertexId,
) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let label_id = tag_label_id(ctx, space, tag_name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag_name)))?;
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, *id)?;
    warn_if_leaves_dangling_edges(ctx, space, tag_name, label_id, &vid);
    let routed = route_vertex_id(&vid)?;
    let ts = ctx.get_write_timestamp()?;
    delete_vertex_with_timestamp(
        ctx,
        VertexDeleteTarget {
            space_id: space_info.space_id,
            tag_name,
            label_id,
            vid: &vid,
            routed: &routed,
        },
        ts,
        true,
    )
}

/// Everything identifying one vertex deletion: its space/tag label plus the
/// identity in both logical and routed form.
struct VertexDeleteTarget<'a> {
    space_id: u64,
    tag_name: &'a str,
    label_id: LabelId,
    vid: &'a VertexId,
    routed: &'a RoutedVertexId,
}

/// Delete one vertex row with a caller-provided write timestamp.
///
/// Shared by plain, cascade and batch deletes so a cascade stamps its edges
/// and its vertices with one timestamp and settles it once: any failure
/// aborts the timestamp, hiding both the edge and the vertex writes instead
/// of leaving a half cascade. The online path still defers settling to the
/// transaction commit; `settle` only gates the offline commit, which the
/// batch caller performs once after its loop.
fn delete_vertex_with_timestamp(
    ctx: &GraphStorageContext,
    target: VertexDeleteTarget<'_>,
    ts: Timestamp,
    settle: bool,
) -> StorageResult<()> {
    let VertexDeleteTarget {
        space_id,
        tag_name,
        label_id,
        vid,
        routed,
    } = target;
    let redo = DeleteVertexRedo {
        label: label_id,
        vid: *vid,
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::DeleteVertex, ts, &redo)?;

    let online = ctx.is_online_write();
    let staging = if online {
        match ctx.txn_staging_mark(ts) {
            Ok(mark) => Some(mark),
            Err(error) => {
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        }
    } else {
        None
    };
    let unwind = |error: StorageError| -> StorageError {
        if let Some((buffer, mark)) = &staging {
            ctx.rollback_staging_to(buffer, *mark);
        }
        ctx.abort_write_timestamp(ts);
        error
    };

    let delete_result = match routed {
        RoutedVertexId::Int(vid_int) => ctx.delete_vertex_by_i64(label_id, *vid_int, ts),
        RoutedVertexId::Text(id_str) => ctx.delete_vertex(label_id, id_str, ts),
    };
    if let Err(error) = delete_result {
        return Err(unwind(error));
    }
    if let Err(error) = super::vertex::record_vertex_remove(ctx, *vid, Some(redo_entry)) {
        return Err(unwind(error));
    }

    let id_value = Value::from(*vid);
    if online {
        // Index entry removal replays at commit apply.
        if let Err(error) = ctx.stage_vertex_index_op(
            ts,
            StagedIndexOp::Delete {
                space_id,
                vid: id_value,
                tag: tag_name.to_string(),
            },
        ) {
            return Err(unwind(error));
        }
        return Ok(());
    }

    if let Err(error) = super::index_maintenance::delete_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_id,
        &id_value,
        tag_name,
        ts,
    ) {
        return Err(unwind(error));
    }

    if settle {
        ctx.commit_write_timestamp_ordered(ts)?;
    }

    Ok(())
}

/// Delete a vertex together with every incident edge in table-scoped batches.
///
/// One write timestamp covers the edges and the vertex: each edge-type table
/// removes its incident edges through the table-level batch entrance (one
/// staging precheck plus one commit per touched table), then the vertex row
/// is deleted with the same timestamp and settled once. Any failure aborts
/// the timestamp, hiding both the edge and the vertex writes instead of
/// leaving a half cascade; aborted stamps stay hidden through the pending
/// gate. The transaction layer keeps per-edge redo entries, restore records
/// and index maintenance, so explicit transactions still roll back edge by
/// edge. Commits and table log entries scale with the touched tables, not
/// the edge count.
pub(crate) fn delete_vertex_with_edges(
    ctx: &GraphStorageContext,
    space: &str,
    tag_name: &str,
    id: &VertexId,
) -> StorageResult<()> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let label_id = tag_label_id(ctx, space, tag_name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag_name)))?;
    let id = VertexId::normalize_for_vid_type(&space_info.vid_type, *id)?;
    let routed = route_vertex_id(&id)?;
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_write_timestamp()?;
    for edge_info in &edge_types {
        if let Err(error) = delete_incident_edges_of_type(ctx, space_id, &id, edge_info, ts) {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    }
    delete_vertex_with_timestamp(
        ctx,
        VertexDeleteTarget {
            space_id,
            tag_name,
            label_id,
            vid: &id,
            routed: &routed,
        },
        ts,
        true,
    )
}

/// Batch-delete multiple vertices together with all their incident edges.
///
/// One write timestamp covers the entire batch. For each edge type, one
/// staging batch per physical table covers all vertices, reducing commits
/// from `N * tables` to `tables`; the vertex rows are then deleted with the
/// same timestamp and settled once. Any failure aborts the timestamp, hiding
/// the whole batch instead of leaving a half cascade; aborted stamps stay
/// hidden through the pending gate. The per-edge transaction redo, restore
/// records and index maintenance keep the explicit transaction rollback
/// path intact.
pub(crate) fn batch_delete_vertices_with_edges(
    ctx: &GraphStorageContext,
    space: &str,
    tag_name: &str,
    ids: &[VertexId],
) -> StorageResult<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let label_id = tag_label_id(ctx, space, tag_name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag_name)))?;
    let ids: Vec<VertexId> = ids
        .iter()
        .map(|id| VertexId::normalize_for_vid_type(&space_info.vid_type, *id))
        .collect::<StorageResult<_>>()?;
    let routed: Vec<RoutedVertexId> = ids
        .iter()
        .map(route_vertex_id)
        .collect::<StorageResult<_>>()?;
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_write_timestamp()?;

    // Cascade-delete edges for all vertices across all edge types.
    for edge_info in &edge_types {
        if let Err(error) = batch_delete_incident_edges_of_type(ctx, space_id, &ids, edge_info, ts)
        {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    }

    // Delete the vertices themselves with the same timestamp, settling once.
    // The online path defers settling to the transaction commit.
    let mut deleted = 0usize;
    for (id, route) in ids.iter().zip(routed.iter()) {
        delete_vertex_with_timestamp(
            ctx,
            VertexDeleteTarget {
                space_id,
                tag_name,
                label_id,
                vid: id,
                routed: route,
            },
            ts,
            false,
        )?;
        deleted += 1;
    }
    if !ctx.is_online_write() {
        if let Err(error) = ctx.commit_write_timestamp_ordered(ts) {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    }
    Ok(deleted)
}

/// Batch-cascade one edge type across multiple vertices through per-table
/// batch entrances.
fn batch_delete_incident_edges_of_type(
    ctx: &GraphStorageContext,
    space_id: u64,
    ids: &[VertexId],
    edge_info: &linkrs_core::types::EdgeTypeInfo,
    ts: Timestamp,
) -> StorageResult<()> {
    let keys: Vec<EdgeTableKey> = ctx.data_store().with_edge_label_index(|index| {
        index
            .get(&edge_info.edge_type_id)
            .cloned()
            .unwrap_or_default()
    });
    for key in &keys {
        // Resolve internal IDs for all vertices in one pass.
        let vertex_rows: Vec<(Option<u32>, Option<u32>)> =
            ctx.data_store().with_vertex_tables(|vertex_tables| {
                ids.iter()
                    .map(|id| {
                        (
                            helpers::resolve_internal_id(
                                ctx,
                                vertex_tables,
                                key.src_label,
                                *id,
                                ts,
                            ),
                            helpers::resolve_internal_id(
                                ctx,
                                vertex_tables,
                                key.dst_label,
                                *id,
                                ts,
                            ),
                        )
                    })
                    .collect()
            });
        // Filter out vertices that resolve to neither direction.
        let filtered: Vec<_> = vertex_rows
            .iter()
            .filter(|(s, d)| s.is_some() || d.is_some())
            .copied()
            .collect();
        if filtered.is_empty() {
            continue;
        }
        let Some(table) = ctx.data_store().try_get_edge_table_mut(key) else {
            continue;
        };
        let deleted = table
            .write()
            .delete_incident_edges_of_vertices(&filtered, ts)?;
        if deleted.is_empty() {
            continue;
        }
        for edge in &deleted {
            let (Some(src_ext), Some(dst_ext)) = (
                ctx.get_external_id_by_internal_id(key.src_label, edge.src),
                ctx.get_external_id_by_internal_id(key.dst_label, edge.dst),
            ) else {
                return Err(StorageError::not_found(format!(
                    "deleted edge {} -> {} lost its endpoint mapping",
                    edge.src, edge.dst
                )));
            };
            let redo = DeleteEdgeRedo {
                src_label: key.src_label,
                src_vid: src_ext,
                dst_label: key.dst_label,
                dst_vid: dst_ext,
                edge_label: key.edge_label,
                rank: edge.rank,
            };
            let redo_entry = ctx.append_wal_redo(WalOpType::DeleteEdge, ts, &redo)?;
            super::edge::record_edge_remove(
                ctx,
                EdgeIdentifier::new(
                    key.src_label,
                    src_ext,
                    key.dst_label,
                    dst_ext,
                    key.edge_label,
                    edge.rank,
                ),
                edge.properties.clone(),
                Some(redo_entry),
            )?;
            let src_value = Value::from(src_ext);
            let dst_value = Value::from(dst_ext);
            let edge_identity = EdgeIdentity::new(
                space_id,
                &src_value,
                &dst_value,
                &edge_info.edge_type_name,
                edge.rank,
            );
            ctx.delete_all_edge_indexes_mvcc(&edge_identity, ts)?;
        }
        ctx.mark_edge_modified(key.edge_label);
    }
    Ok(())
}

/// Remove every incident edge of one vertex from every physical table of one
/// edge type through the table-level batch entrance.
///
/// Each touched table commits once with the shared timestamp; the per-edge
/// transaction redo, restore record and index maintenance keep the explicit
/// transaction rollback path intact. Tables holding no incident edge return
/// empty without committing.
fn delete_incident_edges_of_type(
    ctx: &GraphStorageContext,
    space_id: u64,
    id: &VertexId,
    edge_info: &linkrs_core::types::EdgeTypeInfo,
    ts: Timestamp,
) -> StorageResult<()> {
    let keys: Vec<EdgeTableKey> = ctx.data_store().with_edge_label_index(|index| {
        index
            .get(&edge_info.edge_type_id)
            .cloned()
            .unwrap_or_default()
    });
    for key in &keys {
        let (src_internal, dst_internal) = ctx.data_store().with_vertex_tables(|vertex_tables| {
            (
                helpers::resolve_internal_id(ctx, vertex_tables, key.src_label, *id, ts),
                helpers::resolve_internal_id(ctx, vertex_tables, key.dst_label, *id, ts),
            )
        });
        if src_internal.is_none() && dst_internal.is_none() {
            continue;
        }
        let Some(table) = ctx.data_store().try_get_edge_table_mut(key) else {
            continue;
        };
        let deleted =
            table
                .write()
                .delete_incident_edges_of_vertex(src_internal, dst_internal, ts)?;
        if deleted.is_empty() {
            continue;
        }
        for edge in &deleted {
            let (Some(src_ext), Some(dst_ext)) = (
                ctx.get_external_id_by_internal_id(key.src_label, edge.src),
                ctx.get_external_id_by_internal_id(key.dst_label, edge.dst),
            ) else {
                return Err(StorageError::not_found(format!(
                    "deleted edge {} -> {} lost its endpoint mapping",
                    edge.src, edge.dst
                )));
            };
            let redo = DeleteEdgeRedo {
                src_label: key.src_label,
                src_vid: src_ext,
                dst_label: key.dst_label,
                dst_vid: dst_ext,
                edge_label: key.edge_label,
                rank: edge.rank,
            };
            let redo_entry = ctx.append_wal_redo(WalOpType::DeleteEdge, ts, &redo)?;
            super::edge::record_edge_remove(
                ctx,
                EdgeIdentifier::new(
                    key.src_label,
                    src_ext,
                    key.dst_label,
                    dst_ext,
                    key.edge_label,
                    edge.rank,
                ),
                edge.properties.clone(),
                Some(redo_entry),
            )?;
            let src_value = Value::from(src_ext);
            let dst_value = Value::from(dst_ext);
            let edge_identity = EdgeIdentity::new(
                space_id,
                &src_value,
                &dst_value,
                &edge_info.edge_type_name,
                edge.rank,
            );
            ctx.delete_all_edge_indexes_mvcc(&edge_identity, ts)?;
        }
        ctx.mark_edge_modified(key.edge_label);
    }
    Ok(())
}
