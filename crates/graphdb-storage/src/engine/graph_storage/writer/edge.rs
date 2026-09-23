use graphdb_core::types::{EdgeIdentifier, EdgeTypeInfo, Timestamp, VertexId};
use graphdb_core::wal::redo::{DeleteEdgeRedo, InsertEdgeRedo};
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{Edge, StorageError, StorageResult, Value};
use graphdb_transaction::undo_log::{InsertEdgeUndo, RestoreEdgeUndo, UndoLogEntry};
use graphdb_transaction::wal::TransactionWalEntry;
use graphdb_transaction::{MutationEntityKey, MutationResult};

use crate::engine::params::{EdgeOperationParams, InsertEdgeParams};
use crate::index::types::EdgeIdentity;

use super::super::context::GraphStorageContext;
use super::super::ops::{edge_label_id, endpoint_label_id};
use super::super::reader;
use super::batch::InsertedEdgeRecord;

pub(super) fn record_edge_insert(
    ctx: &GraphStorageContext,
    edge: EdgeIdentifier,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Edge(edge)],
            undo_entry: Some(UndoLogEntry::InsertEdge(InsertEdgeUndo {
                src_label: edge.src_label,
                src_vid: edge.src_vid,
                dst_label: edge.dst_label,
                dst_vid: edge.dst_vid,
                edge_label: edge.edge_label,
                rank: edge.rank,
            })),
            redo_entry,
            modified_table: Some("edge".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

pub(super) fn record_edge_remove(
    ctx: &GraphStorageContext,
    edge: EdgeIdentifier,
    properties: Vec<(String, Value)>,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Edge(edge)],
            undo_entry: Some(UndoLogEntry::RestoreEdge(RestoreEdgeUndo {
                src_label: edge.src_label,
                src_vid: edge.src_vid,
                dst_label: edge.dst_label,
                dst_vid: edge.dst_vid,
                edge_label: edge.edge_label,
                rank: edge.rank,
                properties,
            })),
            redo_entry,
            modified_table: Some("edge".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

pub(crate) fn insert_edge(ctx: &GraphStorageContext, space: &str, edge: Edge) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let src = VertexId::normalize_for_vid_type(&space_info.vid_type, edge.src)?;
    let dst = VertexId::normalize_for_vid_type(&space_info.vid_type, edge.dst)?;
    let edge = Edge::new(
        src,
        dst,
        edge.edge_type.clone(),
        edge.ranking(),
        edge.props.clone(),
    );

    let ts = ctx.get_write_timestamp()?;
    let mut rollback = Vec::new();
    let result = insert_edge_at_timestamp(ctx, space, space_info.space_id, edge, ts, &mut rollback);

    if result.is_err() {
        rollback_edges(ctx, space_info.space_id, &rollback, ts);
    } else {
        for item in &rollback {
            record_edge_insert(
                ctx,
                EdgeIdentifier::new(
                    item.src_label_id,
                    item.src,
                    item.dst_label_id,
                    item.dst,
                    item.edge_label_id,
                    item.rank,
                ),
                Some(item.redo_entry.clone()),
            )?;
        }
    }

    if result.is_ok() {
        ctx.commit_write_timestamp_ordered(ts)?;
    } else {
        ctx.abort_write_timestamp(ts);
    }

    result
}

fn insert_edge_at_timestamp(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    edge: Edge,
    ts: Timestamp,
    rollback: &mut Vec<InsertedEdgeRecord>,
) -> StorageResult<()> {
    let edge_type = resolve_edge_type(ctx, space, &edge.edge_type)?;
    let edge_label_id = edge_type.edge_type_id;
    let src_label_id =
        endpoint_label_id(ctx, space, &edge_type.src_tag_name)?.ok_or_else(|| {
            StorageError::not_found(format!("Source tag {} not found", edge_type.src_tag_name))
        })?;
    let dst_label_id =
        endpoint_label_id(ctx, space, &edge_type.dst_tag_name)?.ok_or_else(|| {
            StorageError::not_found(format!(
                "Destination tag {} not found",
                edge_type.dst_tag_name
            ))
        })?;

    let props: Vec<(String, Value)> = edge
        .props
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let props =
        super::constraints::apply_edge_type_constraints(ctx, space, &edge.edge_type, props)?;
    let src_value = Value::from(edge.src);
    let dst_value = Value::from(edge.dst);
    let redo = InsertEdgeRedo {
        src_label: src_label_id,
        src_vid: edge.src,
        dst_label: dst_label_id,
        dst_vid: edge.dst,
        edge_label: edge_label_id,
        rank: edge.ranking,
        properties: props.clone(),
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::InsertEdge, ts, &redo)?;

    ctx.insert_edge(InsertEdgeParams {
        edge_label: edge_label_id,
        src_label: src_label_id,
        src_id: edge.src,
        dst_label: dst_label_id,
        dst_id: edge.dst,
        rank: edge.ranking,
        properties: &props,
        ts,
    })?;

    rollback.push(InsertedEdgeRecord {
        edge_label_id,
        src_label_id,
        dst_label_id,
        src: edge.src,
        dst: edge.dst,
        edge_type: edge.edge_type.clone(),
        rank: edge.ranking,
        redo_entry,
    });

    let edge_identity = EdgeIdentity::new(
        space_id,
        &src_value,
        &dst_value,
        &edge.edge_type,
        edge.ranking,
    );
    ctx.update_all_edge_indexes_mvcc(&edge_identity, &props, ts)?;

    Ok(())
}

pub(super) fn resolve_edge_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<EdgeTypeInfo> {
    ctx.schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| StorageError::not_found(format!("Edge type {} not found", edge_type)))
}

pub(super) fn rollback_edges(
    ctx: &GraphStorageContext,
    space_id: u64,
    inserted: &[InsertedEdgeRecord],
    ts: Timestamp,
) {
    for item in inserted.iter().rev() {
        let src_value = Value::from(item.src);
        let dst_value = Value::from(item.dst);
        let edge_identity =
            EdgeIdentity::new(space_id, &src_value, &dst_value, &item.edge_type, item.rank);
        let _ = ctx.delete_all_edge_indexes_mvcc(&edge_identity, ts);
        let _ = ctx.delete_edge(
            &EdgeOperationParams {
                edge_label: item.edge_label_id,
                src_label: item.src_label_id,
                src_id: item.src,
                dst_label: item.dst_label_id,
                dst_id: item.dst,
                rank: item.rank,
            },
            ts,
        );
    }
}

pub(crate) fn delete_edge_at_timestamp(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
    ts: Timestamp,
) -> StorageResult<Option<TransactionWalEntry>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let edge_label_id = edge_label_id(ctx, space, edge_type)?
        .ok_or_else(|| StorageError::not_found(format!("Edge type {} not found", edge_type)))?;

    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let mut deleted = false;
    let mut redo_entry = None;
    for et in edge_types {
        if et.edge_type_name == edge_type {
            let src_label_id = match endpoint_label_id(ctx, space, &et.src_tag_name)? {
                Some(id) => id,
                None => break,
            };
            let dst_label_id = match endpoint_label_id(ctx, space, &et.dst_tag_name)? {
                Some(id) => id,
                None => break,
            };
            let redo = DeleteEdgeRedo {
                src_label: src_label_id,
                src_vid: *src,
                dst_label: dst_label_id,
                dst_vid: *dst,
                edge_label: edge_label_id,
                rank,
            };
            redo_entry = Some(ctx.append_wal_redo(WalOpType::DeleteEdge, ts, &redo)?);

            let deleted_edge = ctx.delete_edge(
                &EdgeOperationParams {
                    edge_label: edge_label_id,
                    src_label: src_label_id,
                    src_id: *src,
                    dst_label: dst_label_id,
                    dst_id: *dst,
                    rank,
                },
                ts,
            )?;

            if deleted_edge {
                let src_value = Value::from(*src);
                let dst_value = Value::from(*dst);
                let edge_identity =
                    EdgeIdentity::new(space_id, &src_value, &dst_value, edge_type, rank);
                ctx.delete_all_edge_indexes_mvcc(&edge_identity, ts)?;
                deleted = true;
            }
            break;
        }
    }

    if !deleted {
        // Deleting a nonexistent edge is a no-op.
        return Ok(None);
    }

    Ok(redo_entry)
}

pub(crate) fn delete_edge(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let src = VertexId::normalize_for_vid_type(&space_info.vid_type, *src)?;
    let dst = VertexId::normalize_for_vid_type(&space_info.vid_type, *dst)?;
    let previous = reader::get_edge(ctx, space, &src, &dst, edge_type, rank)?;
    let ts = ctx.get_write_timestamp()?;
    let result = delete_edge_at_timestamp(ctx, space, &src, &dst, edge_type, rank, ts);
    if result.is_ok() {
        if let Some(previous) = previous {
            let edge_info = resolve_edge_type(ctx, space, edge_type)?;
            let src_label = endpoint_label_id(ctx, space, &edge_info.src_tag_name)?
                .ok_or_else(|| StorageError::not_found("Source tag not found"))?;
            let dst_label = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)?
                .ok_or_else(|| StorageError::not_found("Destination tag not found"))?;
            record_edge_remove(
                ctx,
                EdgeIdentifier::new(src_label, src, dst_label, dst, edge_info.edge_type_id, rank),
                previous.props.into_iter().collect(),
                result.as_ref().ok().and_then(Clone::clone),
            )?;
        }
    }
    if result.is_ok() {
        ctx.commit_write_timestamp_ordered(ts)?;
    } else {
        ctx.abort_write_timestamp(ts);
    }
    result.map(|_| ())
}

/// Atomically replace an edge's properties: delete the old edge and insert the
/// new one under a single write timestamp. If the insert fails, the old edge is
/// restored from a pre-delete read, ensuring no data loss.
pub(crate) fn update_edge(ctx: &GraphStorageContext, space: &str, edge: Edge) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    // Save edge identity for rollback
    let src = VertexId::normalize_for_vid_type(&space_info.vid_type, edge.src)?;
    let dst = VertexId::normalize_for_vid_type(&space_info.vid_type, edge.dst)?;
    let edge_type = edge.edge_type.clone();
    let ranking = edge.ranking;
    let edge = Edge::new(src, dst, edge_type.clone(), ranking, edge.props.clone());

    // Read current properties for rollback
    let current_props =
        super::super::reader::get_edge(ctx, space, &src, &dst, &edge_type, ranking)?
            .map(|e| e.props)
            .unwrap_or_default();
    let edge_info = resolve_edge_type(ctx, space, &edge_type)?;
    let src_label = endpoint_label_id(ctx, space, &edge_info.src_tag_name)?
        .ok_or_else(|| StorageError::not_found("Source tag not found"))?;
    let dst_label = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)?
        .ok_or_else(|| StorageError::not_found("Destination tag not found"))?;

    let ts = ctx.get_write_timestamp()?;

    // Delete the old edge
    let delete_redo =
        match delete_edge_at_timestamp(ctx, space, &src, &dst, &edge_type, ranking, ts) {
            Ok(entry) => entry,
            Err(e) => {
                ctx.abort_write_timestamp(ts);
                return Err(e);
            }
        };

    // Insert the new edge
    let mut rollback = Vec::new();
    match insert_edge_at_timestamp(ctx, space, space_info.space_id, edge, ts, &mut rollback) {
        Ok(()) => {
            let edge_id = EdgeIdentifier::new(
                src_label,
                src,
                dst_label,
                dst,
                edge_info.edge_type_id,
                ranking,
            );
            let inserted_redo = rollback.first().map(|record| record.redo_entry.clone());
            record_edge_insert(ctx, edge_id, inserted_redo)?;
            record_edge_remove(
                ctx,
                edge_id,
                current_props.clone().into_iter().collect(),
                delete_redo,
            )?;
            ctx.commit_write_timestamp_ordered(ts)?;
            Ok(())
        }
        Err(e) => {
            // Rollback: undo the failed insert, then re-insert the old edge
            rollback_edges(ctx, space_info.space_id, &rollback, ts);
            let old_edge = Edge {
                src,
                dst,
                edge_type,
                ranking,
                props: current_props,
            };
            let _ = insert_edge_at_timestamp(
                ctx,
                space,
                space_info.space_id,
                old_edge,
                ts,
                &mut Vec::new(),
            );
            ctx.abort_write_timestamp(ts);
            Err(e)
        }
    }
}
