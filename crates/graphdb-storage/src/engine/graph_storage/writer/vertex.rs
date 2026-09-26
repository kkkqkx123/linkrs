use std::collections::HashMap;

use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{EdgeIdentifier, LabelId, TagInfo, Timestamp, VertexId};
use graphdb_core::wal::redo::{
    DeleteEdgeRedo, DeleteVertexRedo, InsertVertexRedo, UpdateVertexPropRedo,
};
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{DataType, StorageError, StorageResult, Value, Vertex};
use graphdb_transaction::wal::TransactionWalEntry;
use graphdb_transaction::{MutationEntityKey, MutationResult};

use super::super::context::helpers;
use super::super::context::txn_staging::StagedIndexOp;
use super::super::context::GraphStorageContext;
use super::super::ops::{route_vertex_id, tag_label_id, RoutedVertexId};
use super::super::serial::scan_vertex_serial_column;
use super::batch::{PrecheckedBatchContext, SerialBatchState};
use crate::engine::data_store::EdgeTableKey;
use crate::index::types::EdgeIdentity;
use crate::vertex::{primary_key_mirror_value, IdKey};

/// Record one vertex insert for the transaction write set. Vertex rollback
/// is the staged buffer's, so no per-row undo entry is generated; the redo
/// entry keeps the WAL replay form.
pub(super) fn record_vertex_insert(
    ctx: &GraphStorageContext,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: None,
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

/// Record one vertex delete for the transaction write set. See
/// [`record_vertex_insert`] for why no undo entry is generated.
pub(super) fn record_vertex_remove(
    ctx: &GraphStorageContext,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: None,
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

/// Record one vertex property update for the transaction write set. See
/// [`record_vertex_insert`] for why no undo entry is generated.
pub(super) fn record_vertex_property_update(
    ctx: &GraphStorageContext,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: None,
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

pub(crate) fn insert_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
) -> StorageResult<VertexId> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    // The vertex type carries exactly one tag by construction, so no
    // multi-tag gate is needed: normalization runs before any timestamp is
    // allocated and a bad id fails the whole write up front.
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vertex.vid)?;
    let vertex = Vertex::new(vid, vertex.tag);

    let ts = ctx.get_write_timestamp()?;
    // Caller-owned staging scope created with the write timestamp; it
    // buffers the row through context and shard layers without touching
    // global state, the apply step below installs it, and rollback or crash
    // drop discards it.
    let mut scope = crate::vertex::WriteScope::new(ts);
    log::trace!("write scope opened ts={}", scope.write_ts());
    let staged = match stage_vertex_row_single(ctx, space, vertex, ts, &mut scope) {
        Ok(staged) => staged,
        Err(error) => {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    };

    // Online writes hold in the transaction buffer: absorb the staged row,
    // journal the transaction record and the index maintenance, and let the
    // commit point apply everything. Nothing global is touched past this
    // point, so a failure only rolls the buffer back to the statement mark.
    if ctx.is_online_write() {
        if let Err(error) = super::index_maintenance::check_vertex_unique_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            &staged.vertex_id,
            &staged.tag_name,
            &staged.props,
        ) {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        let (buffer, mark) = match ctx.txn_staging_mark(ts) {
            Ok(mark) => mark,
            Err(error) => {
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        };
        let outcome = ctx
            .absorb_write_scope(&mut scope, ts)
            .and_then(|()| record_vertex_insert(ctx, staged.vid, Some(staged.redo_entry.clone())))
            .and_then(|()| {
                ctx.stage_vertex_index_op(
                    ts,
                    StagedIndexOp::Insert {
                        space_id: space_info.space_id,
                        vid: staged.vertex_id.clone(),
                        tag: staged.tag_name.clone(),
                        properties: staged.props.clone(),
                    },
                )
            });
        if let Err(error) = outcome {
            ctx.rollback_staging_to(&buffer, mark);
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        return Ok(staged.vid);
    }

    // Apply: the commit hook installs the row and consumes the staging
    // record. A failed apply undoes its own partials inside the hook, the
    // row is not visible yet (the timestamp is still pending), and no index
    // entry exists, so the compensation is a scope discard plus abort.
    let internal_id = match ctx
        .commit_write_scope(staged.label_id, &mut scope, ts)
        .and_then(|mapping| {
            mapping
                .into_iter()
                .find(|(key, _)| *key == staged.key)
                .map(|(_, global_id)| global_id)
                .ok_or_else(|| {
                    StorageError::db_error(format!(
                        "commit apply lost staged vertex {:?}",
                        staged.vid
                    ))
                })
        }) {
        Ok(internal_id) => internal_id,
        Err(error) => {
            scope.clear();
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    };
    debug_assert!(scope.is_empty());

    // Index maintenance and the mutation record run after the row is
    // installed and before the timestamp publishes visibility. A failure
    // here leaves an applied row behind, so it is compensated by undoing
    // the install (dropping the identity and row slots for this
    // not-yet-visible timestamp) and clearing any index entries written.
    if let Err(error) = super::index_maintenance::update_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_info.space_id,
        &staged.vertex_id,
        &staged.tag_name,
        &staged.props,
        ts,
    ) {
        compensate_inserted_vertex(ctx, space_info.space_id, &staged, internal_id, ts);
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(error);
    }
    if let Err(error) = record_vertex_insert(ctx, staged.vid, Some(staged.redo_entry.clone())) {
        compensate_inserted_vertex(ctx, space_info.space_id, &staged, internal_id, ts);
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(error);
    }

    // Cache and domain bookkeeping feed from the commit mapping's id.
    match &staged.key {
        IdKey::Int(vid_int) => {
            ctx.cache_inserted_vertex_id(staged.label_id, &vid_int.to_string(), internal_id, ts);
            ctx.mark_vertex_modified(staged.label_id);
            ctx.observe_vertex_id_i64(staged.label_id, *vid_int);
        }
        IdKey::Text(id_str) => {
            ctx.cache_inserted_vertex_id(staged.label_id, id_str, internal_id, ts);
            ctx.mark_vertex_modified(staged.label_id);
            ctx.observe_vertex_id_string(staged.label_id);
        }
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(staged.vid)
}

/// Undo a post-apply insert failure: clear index entries written for the
/// row, then drop the applied row itself through the scope-apply undo hook.
fn compensate_inserted_vertex(
    ctx: &GraphStorageContext,
    space_id: u64,
    staged: &StagedVertexRow,
    internal_id: u32,
    ts: Timestamp,
) {
    let _ = super::index_maintenance::delete_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_id,
        &staged.vertex_id,
        &staged.tag_name,
        ts,
    );
    ctx.undo_applied_scope_inserts(staged.label_id, &[internal_id]);
}

/// Undo every row installed by the batch apply phase, label by label.
fn undo_applied_batch(ctx: &GraphStorageContext, applied: &[(LabelId, Vec<u32>)]) {
    for (label_id, ids) in applied {
        ctx.undo_applied_scope_inserts(*label_id, ids);
    }
}

/// Best-effort removal of every secondary index entry a batch insert could
/// have written. The batch vids are new, so entries of rows that never
/// reached the index phase are absent and the delete is a no-op.
fn clear_batch_vertex_indexes(
    ctx: &GraphStorageContext,
    space_id: u64,
    staged: &[StagedVertexRow],
    ts: Timestamp,
) {
    for row in staged {
        let _ = super::index_maintenance::delete_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_id,
            &row.vertex_id,
            &row.tag_name,
            ts,
        );
    }
}

fn stage_vertex_row_single(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
    ts: Timestamp,
    scope: &mut crate::vertex::WriteScope,
) -> StorageResult<StagedVertexRow> {
    let tag = &vertex.tag;
    let label_id = tag_label_id(ctx, space, &tag.name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;
    let props: Vec<(String, Value)> = tag
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let props = super::constraints::apply_tag_constraints(ctx, space, &tag.name, props)?;
    let redo = InsertVertexRedo {
        label: label_id,
        vid: vertex.vid,
        properties: props.clone(),
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::InsertVertex, ts, &redo)?;
    let key = match route_vertex_id(&vertex.vid)? {
        RoutedVertexId::Int(vid_int) => IdKey::Int(vid_int),
        RoutedVertexId::Text(id_str) => IdKey::Text(id_str),
    };
    match &key {
        IdKey::Int(vid_int) => {
            ctx.insert_vertex_by_i64_with_scope(label_id, *vid_int, &props, ts, scope)?;
        }
        IdKey::Text(id_str) => {
            ctx.insert_vertex_with_scope(label_id, id_str, &props, ts, scope)?;
        }
    }
    Ok(StagedVertexRow {
        label_id,
        vid: vertex.vid,
        key,
        vertex_id: Value::from(vertex.vid),
        tag_name: tag.name.clone(),
        props,
        redo_entry,
    })
}

pub(crate) fn update_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let tag = &vertex.tag;
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vertex.vid)?;

    let ts = ctx.get_write_timestamp()?;
    let label_id = tag_label_id(ctx, space, &tag.name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;

    // Online updates buffer in the transaction staging hold; the statement
    // mark lets a mid-statement failure unwind exactly this statement's
    // staged rows with nothing applied.
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
    let unwind = |ctx: &GraphStorageContext, error: StorageError| -> StorageError {
        if let Some((buffer, mark)) = &staging {
            ctx.rollback_staging_to(buffer, *mark);
        }
        ctx.abort_write_timestamp(ts);
        error
    };

    // Updates merge the full existing row, including the primary
    // key mirror, into the write set. Restating the mirror changes nothing
    // and is skipped; a divergent key is rejected by the update entry
    // (table layer offline, staging validation online).
    let pk_mirror: Option<(String, DataType, Value)> =
        ctx.data_store().with_vertex_tables(|tables| {
            tables.get(&label_id).and_then(|table| {
                let schema = table.schema();
                let pk = schema.properties.get(schema.primary_key_index)?;
                let key = match route_vertex_id(&vid).ok()? {
                    RoutedVertexId::Int(id) => IdKey::Int(id),
                    RoutedVertexId::Text(id) => IdKey::Text(id),
                };
                primary_key_mirror_value(&pk.data_type, &key)
                    .ok()
                    .map(|mirror| (pk.name.clone(), pk.data_type.clone(), mirror))
            })
        });

    let current_record = match route_vertex_id(&vid)? {
        RoutedVertexId::Int(id_int) => ctx.get_vertex_by_i64(label_id, id_int, ts),
        RoutedVertexId::Text(id_str) => ctx.get_vertex(label_id, &id_str, ts),
    };

    let mut merged_props: HashMap<String, Value> = current_record
        .as_ref()
        .map(|record| record.properties.iter().cloned().collect())
        .unwrap_or_default();
    for (prop_name, value) in &tag.properties {
        merged_props.insert(prop_name.clone(), value.clone());
    }

    for (prop_name, value) in &tag.properties {
        let restates_mirror = match pk_mirror.as_ref() {
            Some((pk_name, pk_type, mirror)) if prop_name == pk_name => {
                value == mirror || value.try_cast_to(pk_type).is_ok_and(|cast| &cast == mirror)
            }
            _ => false,
        };
        if restates_mirror {
            continue;
        }
        let redo = UpdateVertexPropRedo {
            label: label_id,
            vid,
            prop_name: prop_name.clone(),
            value: value.clone(),
        };
        let redo_entry = match ctx.append_wal_redo(WalOpType::UpdateVertexProp, ts, &redo) {
            Ok(entry) => entry,
            Err(error) => return Err(unwind(ctx, error)),
        };

        // The table layer rejects primary key columns and missing rows.
        let update_result = match route_vertex_id(&vid)? {
            RoutedVertexId::Int(id_int) => {
                ctx.update_vertex_property_by_i64(label_id, id_int, prop_name, value, ts)
            }
            RoutedVertexId::Text(id_str) => {
                ctx.update_vertex_property(label_id, &id_str, prop_name, value, ts)
            }
        };
        if let Err(error) = update_result {
            return Err(unwind(ctx, error));
        }
        if let Err(error) = record_vertex_property_update(ctx, vid, Some(redo_entry)) {
            return Err(unwind(ctx, error));
        }
    }

    let props: Vec<(String, Value)> = merged_props.into_iter().collect();
    let vid_value = Value::from(vid);
    if online {
        // The refreshed row values must not collide with another vertex's
        // unique entries at the statement; the index mutation itself
        // replays at commit apply, after the new bytes land.
        if let Err(error) = super::index_maintenance::check_vertex_unique_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            &vid_value,
            &tag.name,
            &props,
        ) {
            return Err(unwind(ctx, error));
        }
        if let Err(error) = ctx.stage_vertex_index_op(
            ts,
            StagedIndexOp::Update {
                space_id: space_info.space_id,
                vid: vid_value,
                tag: tag.name.clone(),
                properties: props,
            },
        ) {
            return Err(unwind(ctx, error));
        }
        return Ok(());
    }

    if let Err(error) = super::index_maintenance::refresh_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_info.space_id,
        &vid_value,
        &tag.name,
        &props,
        ts,
    ) {
        return Err(unwind(ctx, error));
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(())
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
    let routed = route_vertex_id(&vid)?;
    let ts = ctx.get_write_timestamp()?;

    let redo = DeleteVertexRedo {
        label: label_id,
        vid,
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

    let delete_result = match &routed {
        RoutedVertexId::Int(vid_int) => ctx.delete_vertex_by_i64(label_id, *vid_int, ts),
        RoutedVertexId::Text(id_str) => ctx.delete_vertex(label_id, id_str, ts),
    };
    if let Err(error) = delete_result {
        return Err(unwind(error));
    }
    if let Err(error) = record_vertex_remove(ctx, vid, Some(redo_entry)) {
        return Err(unwind(error));
    }

    let id_value = Value::from(vid);
    if online {
        // Index entry removal replays at commit apply.
        if let Err(error) = ctx.stage_vertex_index_op(
            ts,
            StagedIndexOp::Delete {
                space_id: space_info.space_id,
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
        space_info.space_id,
        &id_value,
        tag_name,
        ts,
    ) {
        return Err(unwind(error));
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(())
}

/// Delete a vertex together with every incident edge in table-scoped batches.
///
/// One write timestamp covers the whole vertex. Each edge-type table removes
/// its incident edges through the table-level batch entrance (one staging
/// precheck plus one commit per touched table), while the transaction layer
/// keeps per-edge redo entries, restore records and index maintenance, so
/// explicit transactions still roll back edge by edge. Commits and table log
/// entries scale with the touched tables, not the edge count. A failed table
/// batch aborts the timestamp with prior tables already committed, never a
/// half-batch; aborted stamps stay hidden through the pending gate.
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
    let id = VertexId::normalize_for_vid_type(&space_info.vid_type, *id)?;
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_write_timestamp()?;
    for edge_info in &edge_types {
        if let Err(error) = delete_incident_edges_of_type(ctx, space_id, &id, edge_info, ts) {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    }
    if let Err(error) = ctx.commit_write_timestamp_ordered(ts) {
        ctx.abort_write_timestamp(ts);
        return Err(error);
    }

    delete_vertex(ctx, space, tag_name, &id)
}

/// Batch-delete multiple vertices together with all their incident edges.
///
/// One write timestamp covers the entire batch. For each edge type, one
/// staging batch per physical table covers all vertices, reducing commits
/// from `N * tables` to `tables`. The per-edge transaction redo, restore
/// records and index maintenance keep the explicit transaction rollback
/// path intact. A failed table batch aborts the timestamp with prior
/// tables already committed; aborted stamps stay hidden through the
/// pending gate.
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
    let ids: Vec<VertexId> = ids
        .iter()
        .map(|id| VertexId::normalize_for_vid_type(&space_info.vid_type, *id))
        .collect::<StorageResult<_>>()?;
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_write_timestamp()?;

    // Phase 1: Cascade-delete edges for all vertices across all edge types.
    for edge_info in &edge_types {
        if let Err(error) = batch_delete_incident_edges_of_type(ctx, space_id, &ids, edge_info, ts)
        {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    }
    if let Err(error) = ctx.commit_write_timestamp_ordered(ts) {
        ctx.abort_write_timestamp(ts);
        return Err(error);
    }

    // Phase 2: Delete the vertices themselves.
    let mut deleted = 0usize;
    for id in &ids {
        delete_vertex(ctx, space, tag_name, id)?;
        deleted += 1;
    }
    Ok(deleted)
}

/// Batch-cascade one edge type across multiple vertices through per-table
/// batch entrances.
fn batch_delete_incident_edges_of_type(
    ctx: &GraphStorageContext,
    space_id: u64,
    ids: &[VertexId],
    edge_info: &graphdb_core::types::EdgeTypeInfo,
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
    edge_info: &graphdb_core::types::EdgeTypeInfo,
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

/// One validated, WAL-appended vertex insert awaiting table application.
///
/// Phase A of [`batch_insert_vertices`] stages rows in memory; phase B merges
/// them into the tables with shard-grouped writes; phase C finishes indexes
/// and caches. A staged row touches no table or index state, so dropping it
/// is a free abort.
struct StagedVertexRow {
    label_id: LabelId,
    vid: VertexId,
    key: IdKey,
    vertex_id: Value,
    tag_name: String,
    props: Vec<(String, Value)>,
    redo_entry: TransactionWalEntry,
}

/// Validate, constrain, and WAL-append one batch row without touching table
/// or index state. Uses the batch's pre-resolved schema context, so row
/// semantics match the former per-row path.
fn stage_vertex_row(
    ctx: &GraphStorageContext,
    space_id: u64,
    batch: &mut PrecheckedBatchContext<'_>,
    vertex: &Vertex,
    ts: Timestamp,
) -> StorageResult<StagedVertexRow> {
    let tag = &vertex.tag;
    let vid = VertexId::normalize_for_vid_type(batch.vid_type, vertex.vid)?;
    let tag_info = batch
        .tag_map
        .get(tag.name.as_str())
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;
    let label_id = tag_info.tag_id;
    let props: Vec<(String, Value)> = tag
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let props = super::constraints::apply_tag_constraints_prechecked(
        ctx,
        space_id,
        tag_info,
        batch.serial_state,
        props,
    )?;
    let redo = InsertVertexRedo {
        label: label_id,
        vid,
        properties: props.clone(),
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::InsertVertex, ts, &redo)?;
    let key = match route_vertex_id(&vid)? {
        RoutedVertexId::Int(vid_int) => IdKey::Int(vid_int),
        RoutedVertexId::Text(id_str) => IdKey::Text(id_str),
    };
    Ok(StagedVertexRow {
        label_id,
        vid,
        key,
        vertex_id: Value::from(vid),
        tag_name: tag.name.clone(),
        props,
        redo_entry,
    })
}

pub(crate) fn batch_insert_vertices(
    ctx: &GraphStorageContext,
    space: &str,
    vertices: Vec<Vertex>,
) -> StorageResult<Vec<VertexId>> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    // Resolve tags once per batch instead of once per row per use site
    // (validation, reserve counting, and insertion each re-looked them up).
    let tags = ctx.schema_manager().list_tags(space)?;
    let mut tag_map: HashMap<&str, &TagInfo> = HashMap::with_capacity(tags.len());
    for tag in &tags {
        tag_map.insert(tag.tag_name.as_str(), tag);
    }
    for vertex in &vertices {
        if !tag_map.contains_key(vertex.tag.name.as_str()) {
            return Err(StorageError::not_found(format!(
                "Tag {} not found",
                vertex.tag.name
            )));
        }
    }

    // Pre-count vertices per label and reserve capacity to avoid rehashing
    // during inserts. Each vertex carries exactly one label; the map only
    // batches capacity reservations, it is not multi-label bookkeeping.
    {
        let mut per_label_reserve_counts: HashMap<LabelId, usize> = HashMap::new();
        for vertex in &vertices {
            if let Some(info) = tag_map.get(vertex.tag.name.as_str()) {
                *per_label_reserve_counts.entry(info.tag_id).or_insert(0) += 1;
            }
        }
        for (label_id, count) in &per_label_reserve_counts {
            ctx.reserve_vertex_capacity(*label_id, *count);
        }
    }

    // One serial-column scan per touched column for the whole batch. The
    // per-row path scanned the full column for every explicit SERIAL value
    // (O(n log n) per row, O(n^2 log n) per batch); the batch path checks
    // explicit values against this snapshot plus the batch-local seen sets.
    let mut serial_state = SerialBatchState::new();
    for tag in tags.iter() {
        for prop_def in tag.properties.iter().filter(|p| p.serial) {
            let needs_scan = vertices.iter().any(|v| {
                v.tag.name == tag.tag_name && v.tag.properties.keys().any(|k| k == &prop_def.name)
            });
            if needs_scan {
                if let Some(scan) = scan_vertex_serial_column(ctx, tag.tag_id, &prop_def.name) {
                    serial_state.add_present(tag.tag_id, &prop_def.name, scan);
                }
            }
        }
    }

    // Fetch tag indexes once per batch instead of once per row.
    let tag_indexes = ctx
        .index_metadata_manager()
        .list_tag_indexes(space_info.space_id)?;

    let ts = ctx.get_write_timestamp()?;
    // Batch write scope created with the batch timestamp; every merge below
    // records into it, and the commit/rollback hooks destroy it. New write
    // entries must wire both hooks (review gate).
    let mut scope = crate::vertex::WriteScope::new(ts);
    log::trace!("batch write scope opened ts={}", scope.write_ts());
    let mut batch_ctx = PrecheckedBatchContext {
        tag_map: &tag_map,
        tag_indexes: &tag_indexes,
        serial_state: &mut serial_state,
        vid_type: &space_info.vid_type,
    };

    // Phase A (stage): validate, constrain, and WAL-append every row in
    // input order. No table or index state is touched, so a failure here
    // only aborts the timestamp; there is nothing to roll back.
    let mut staged: Vec<StagedVertexRow> = Vec::with_capacity(vertices.len());
    for vertex in &vertices {
        match stage_vertex_row(ctx, space_info.space_id, &mut batch_ctx, vertex, ts) {
            Ok(row) => staged.push(row),
            Err(e) => {
                ctx.abort_write_timestamp(ts);
                return Err(e);
            }
        }
    }

    // Scope capacity is enforced before any global mutation so no applied
    // row ever escapes scope ownership.
    if staged.len() > crate::vertex::MAX_WRITE_SCOPE_KEYS {
        ctx.abort_write_timestamp(ts);
        return Err(StorageError::capacity_exceeded());
    }

    // Phase B (stage): group staged rows by label and buffer each table's
    // rows without touching global state. Results stay aligned with the
    // input; every row is attempted so the caller can discard every staged
    // row when any row fails. The commit hook below applies the staged rows
    // of every label.
    let mut label_order: Vec<LabelId> = Vec::new();
    let mut by_label: HashMap<LabelId, Vec<usize>> = HashMap::new();
    for (pos, row) in staged.iter().enumerate() {
        by_label
            .entry(row.label_id)
            .or_insert_with(|| {
                label_order.push(row.label_id);
                Vec::new()
            })
            .push(pos);
    }
    let tables: Vec<(LabelId, std::sync::Arc<crate::vertex::ShardedVertexTable>)> =
        match ctx.data_store().with_vertex_tables(|tables| {
            label_order
                .iter()
                .map(|label_id| {
                    tables
                        .get(label_id)
                        .cloned()
                        .map(|t| (*label_id, t))
                        .ok_or_else(|| {
                            StorageError::label_not_found(format!("vertex label {}", label_id))
                        })
                })
                .collect::<StorageResult<Vec<_>>>()
        }) {
            Ok(tables) => tables,
            Err(e) => {
                ctx.abort_write_timestamp(ts);
                return Err(e);
            }
        };
    // `staged_marks[pos]` records whether the row was buffered: staging
    // touches no table state, so a failure here only aborts the timestamp.
    let mut staged_marks: Vec<Option<StorageResult<()>>> =
        (0..staged.len()).map(|_| None).collect();
    for (label_id, table) in &tables {
        let positions = &by_label[label_id];
        let mut str_order: Vec<usize> = Vec::new();
        let mut i64_order: Vec<usize> = Vec::new();
        for &pos in positions {
            match staged[pos].key {
                IdKey::Text(_) => str_order.push(pos),
                IdKey::Int(_) => i64_order.push(pos),
            }
        }
        if !str_order.is_empty() {
            let rows: Vec<(&str, &[(String, Value)])> = str_order
                .iter()
                .map(|&pos| {
                    let IdKey::Text(ref s) = staged[pos].key else {
                        unreachable!("str_order only holds text keys");
                    };
                    (s.as_str(), staged[pos].props.as_slice())
                })
                .collect();
            for (slot, result) in str_order
                .iter()
                .zip(table.insert_batch_str_with_scope(&rows, ts, &mut scope))
            {
                staged_marks[*slot] = Some(result);
            }
        }
        if !i64_order.is_empty() {
            let rows: Vec<(i64, &[(String, Value)])> = i64_order
                .iter()
                .map(|&pos| {
                    let IdKey::Int(n) = staged[pos].key else {
                        unreachable!("i64_order only holds int keys");
                    };
                    (n, staged[pos].props.as_slice())
                })
                .collect();
            for (slot, result) in i64_order
                .iter()
                .zip(table.insert_batch_i64_with_scope(&rows, ts, &mut scope))
            {
                staged_marks[*slot] = Some(result);
            }
        }
    }

    // Staging touches no table state, so a row failure only discards the
    // staged rows: no apply happened and no index entry exists yet.
    let mut first_error: Option<StorageError> = None;
    for mark in &mut staged_marks {
        if let Some(Err(e)) = mark.take() {
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    if let Some(e) = first_error {
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }

    // Online batch: the whole statement's scope folds into the transaction
    // buffer under one mark; records and index maintenance journal for
    // commit-time replay. A failure unwinds the buffer to the mark and
    // aborts the timestamp with nothing applied.
    if ctx.is_online_write() {
        let (buffer, mark) = match ctx.txn_staging_mark(ts) {
            Ok(mark) => mark,
            Err(error) => {
                scope.clear();
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        };
        let mut outcome = ctx.absorb_write_scope(&mut scope, ts);
        for row in staged.iter() {
            if outcome.is_err() {
                break;
            }
            outcome = super::index_maintenance::check_vertex_unique_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &row.vertex_id,
                &row.tag_name,
                &row.props,
            )
            .and_then(|()| record_vertex_insert(ctx, row.vid, Some(row.redo_entry.clone())))
            .and_then(|()| {
                ctx.stage_vertex_index_op(
                    ts,
                    StagedIndexOp::Insert {
                        space_id: space_info.space_id,
                        vid: row.vertex_id.clone(),
                        tag: row.tag_name.clone(),
                        properties: row.props.clone(),
                    },
                )
            });
        }
        if let Err(error) = outcome {
            ctx.rollback_staging_to(&buffer, mark);
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        return Ok(staged.iter().map(|row| row.vid).collect());
    }

    // Phase C (apply): commit every label's staged rows. A failed apply
    // undoes its own partial application inside the table hook; labels
    // applied earlier in this loop are tracked here so a mid-batch failure
    // leaves no applied row behind.
    let mut id_by_key: HashMap<(LabelId, IdKey), u32> = HashMap::new();
    let mut applied_by_label: Vec<(LabelId, Vec<u32>)> = Vec::new();
    let mut apply_failure: Option<StorageError> = None;
    for label_id in &label_order {
        match ctx.commit_write_scope(*label_id, &mut scope, ts) {
            Ok(mapping) => {
                let mut ids = Vec::with_capacity(mapping.len());
                for (key, global_id) in mapping {
                    id_by_key.insert((*label_id, key), global_id);
                    ids.push(global_id);
                }
                applied_by_label.push((*label_id, ids));
            }
            Err(e) => {
                apply_failure = Some(e);
                break;
            }
        }
    }
    let lost_vertex = if apply_failure.is_none() {
        staged.iter().find_map(|row| {
            if id_by_key.contains_key(&(row.label_id, row.key.clone())) {
                None
            } else {
                Some(row.vid)
            }
        })
    } else {
        None
    };
    if let Some(vid) = lost_vertex {
        apply_failure = Some(StorageError::db_error(format!(
            "commit apply lost staged vertex {:?}",
            vid
        )));
    }
    if let Some(e) = apply_failure {
        undo_applied_batch(ctx, &applied_by_label);
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }
    debug_assert!(scope.is_empty());
    let internal_ids: Vec<u32> = staged
        .iter()
        .map(|row| {
            *id_by_key
                .get(&(row.label_id, row.key.clone()))
                .expect("every staged row is resolved by the apply mapping")
        })
        .collect();

    // Phase D (indexes): per-row index maintenance after the rows are
    // installed and before the timestamp publishes visibility. A failure
    // compensates by clearing every index entry this batch could have
    // written (rows not yet reached are no-ops) and undoing the applied
    // rows.
    for row in staged.iter() {
        if let Err(e) = super::index_maintenance::update_vertex_indexes_with_list(
            ctx,
            batch_ctx.tag_indexes,
            space_info.space_id,
            &row.vertex_id,
            &row.tag_name,
            &row.props,
            ts,
        ) {
            clear_batch_vertex_indexes(ctx, space_info.space_id, &staged, ts);
            undo_applied_batch(ctx, &applied_by_label);
            scope.clear();
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
    }

    // Phase E: per-row transaction record, then the id-cache and domain
    // bookkeeping fed by the commit mapping.
    for (pos, row) in staged.iter().enumerate() {
        if let Err(e) = record_vertex_insert(ctx, row.vid, Some(row.redo_entry.clone())) {
            clear_batch_vertex_indexes(ctx, space_info.space_id, &staged, ts);
            undo_applied_batch(ctx, &applied_by_label);
            scope.clear();
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
        match &row.key {
            IdKey::Text(id_str) => {
                ctx.cache_inserted_vertex_id(row.label_id, id_str, internal_ids[pos], ts);
                ctx.mark_vertex_modified(row.label_id);
                ctx.observe_vertex_id_string(row.label_id);
            }
            IdKey::Int(vid_int) => {
                ctx.cache_inserted_vertex_id(
                    row.label_id,
                    &vid_int.to_string(),
                    internal_ids[pos],
                    ts,
                );
                ctx.mark_vertex_modified(row.label_id);
                ctx.observe_vertex_id_i64(row.label_id, *vid_int);
            }
        }
    }

    let ids: Vec<VertexId> = staged.iter().map(|row| row.vid).collect();

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(ids)
}
