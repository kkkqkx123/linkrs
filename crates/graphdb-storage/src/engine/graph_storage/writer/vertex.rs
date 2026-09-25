use std::collections::HashMap;

use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{ColumnId, EdgeIdentifier, LabelId, TagInfo, Timestamp, VertexId};
use graphdb_core::wal::redo::{
    DeleteEdgeRedo, DeleteVertexRedo, InsertVertexRedo, UpdateVertexPropRedo,
};
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{DataType, StorageError, StorageResult, Value, Vertex};
use graphdb_transaction::undo_log::{
    InsertVertexUndo, RemoveVertexUndo, UndoLogEntry, UpdateVertexPropUndo,
};
use graphdb_transaction::wal::TransactionWalEntry;
use graphdb_transaction::{MutationEntityKey, MutationResult};

use super::super::context::helpers;
use super::super::context::GraphStorageContext;
use super::super::ops::{route_vertex_id, tag_label_id, RoutedVertexId};
use super::super::serial::scan_vertex_serial_column;
use super::batch::{InsertedVertexTag, PrecheckedBatchContext, SerialBatchState};
use crate::engine::data_store::EdgeTableKey;
use crate::index::types::EdgeIdentity;
use crate::vertex::{primary_key_mirror_value, IdKey};

pub(super) fn record_vertex_insert(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: Some(UndoLogEntry::InsertVertex(InsertVertexUndo {
                v_label: label,
                vid,
            })),
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

pub(super) fn record_vertex_remove(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: Some(UndoLogEntry::RemoveVertex(RemoveVertexUndo {
                v_label: label,
                vid,
                related_edges: Vec::new(),
            })),
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

fn vertex_column_id(
    ctx: &GraphStorageContext,
    label: LabelId,
    property_name: &str,
) -> Option<ColumnId> {
    ctx.data_store().with_vertex_tables(|tables| {
        tables.get(&label).and_then(|table| {
            table
                .schema()
                .properties
                .iter()
                .position(|property| property.name == property_name)
                .and_then(|index| u32::try_from(index).ok())
                .map(ColumnId)
        })
    })
}

pub(super) fn record_vertex_property_update(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: VertexId,
    property_name: &str,
    old_value: Option<&Value>,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    let Some(col_id) = vertex_column_id(ctx, label, property_name) else {
        return Err(StorageError::column_not_found(property_name.to_string()));
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: Some(UndoLogEntry::UpdateVertexProp(UpdateVertexPropUndo {
                v_label: label,
                vid,
                col_id,
                old_value: old_value
                    .cloned()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            })),
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
    // global state, the commit hook below applies it, and rollback or crash
    // drop discards it.
    let mut scope = crate::vertex::WriteScope::new(ts);
    log::trace!("write scope opened ts={}", scope.write_ts());
    let mut rollback = Vec::new();
    let result = insert_vertex_at_timestamp(
        ctx,
        space,
        space_info.space_id,
        vertex,
        ts,
        &mut rollback,
        &mut scope,
    );

    if result.is_err() {
        rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
        // Rollback hook: discard the staged row; nothing was applied, so no
        // global write exists to compensate.
        for item in &rollback {
            ctx.rollback_write_scope(item.label_id, &mut scope, ts);
        }
        scope.clear();
    } else {
        let staged_tag = rollback.first().cloned();
        let mut apply_error: Option<StorageError> = None;
        let mut applied_id: Option<u32> = None;
        if let Some(tag) = staged_tag {
            match ctx.commit_write_scope(tag.label_id, &mut scope, ts) {
                Ok(mapping) => {
                    applied_id = mapping.into_iter().find_map(|(key, global_id)| {
                        let matches = match (route_vertex_id(&tag.vid), key) {
                            (Ok(RoutedVertexId::Int(a)), IdKey::Int(b)) => a == b,
                            (Ok(RoutedVertexId::Text(a)), IdKey::Text(b)) => a == b,
                            _ => false,
                        };
                        matches.then_some(global_id)
                    });
                    if applied_id.is_none() {
                        apply_error = Some(StorageError::db_error(format!(
                            "commit apply lost staged vertex {:?}",
                            tag.vid
                        )));
                    }
                }
                Err(error) => apply_error = Some(error),
            }
            if let Some(error) = apply_error {
                rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
                ctx.rollback_write_scope(tag.label_id, &mut scope, ts);
                scope.clear();
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        }
        for item in &rollback {
            record_vertex_insert(ctx, item.label_id, item.vid, Some(item.redo_entry.clone()))?;
            debug_assert!(scope.is_empty());
            let internal_id = applied_id.expect("applied id resolved from commit mapping");
            match route_vertex_id(&item.vid) {
                Ok(RoutedVertexId::Int(vid_int)) => {
                    ctx.cache_inserted_vertex_id(item.label_id, &vid_int.to_string(), internal_id, ts);
                    ctx.mark_vertex_modified(item.label_id);
                    ctx.observe_vertex_id_i64(item.label_id, vid_int);
                }
                Ok(RoutedVertexId::Text(id_str)) => {
                    ctx.cache_inserted_vertex_id(item.label_id, &id_str, internal_id, ts);
                    ctx.mark_vertex_modified(item.label_id);
                    ctx.observe_vertex_id_string(item.label_id);
                }
                Err(_) => {}
            }
        }
    }

    if result.is_ok() {
        ctx.commit_write_timestamp_ordered(ts)?;
    } else {
        ctx.abort_write_timestamp(ts);
    }

    result
}

fn insert_vertex_at_timestamp(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    vertex: Vertex,
    ts: Timestamp,
    rollback: &mut Vec<InsertedVertexTag>,
    scope: &mut crate::vertex::WriteScope,
) -> StorageResult<VertexId> {
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

    match route_vertex_id(&vertex.vid)? {
        RoutedVertexId::Int(vid_int) => {
            ctx.insert_vertex_by_i64_with_scope(label_id, vid_int, &props, ts, scope)?;
        }
        RoutedVertexId::Text(id_str) => {
            ctx.insert_vertex_with_scope(label_id, &id_str, &props, ts, scope)?;
        }
    }

    let vid_value = Value::from(vertex.vid);
    rollback.push(InsertedVertexTag {
        label_id,
        vid: vertex.vid,
        vertex_id: vid_value.clone(),
        tag_name: tag.name.clone(),
        redo_entry,
    });

    super::index_maintenance::update_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_id,
        &vid_value,
        &tag.name,
        &props,
        ts,
    )?;

    Ok(vertex.vid)
}

fn rollback_vertex_tags(
    ctx: &GraphStorageContext,
    space_id: u64,
    inserted: &[InsertedVertexTag],
    ts: Timestamp,
) {
    for item in inserted.iter().rev() {
        let _ = super::index_maintenance::delete_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_id,
            &item.vertex_id,
            &item.tag_name,
            ts,
        );
        match route_vertex_id(&item.vid) {
            Ok(RoutedVertexId::Int(vid_int)) => {
                let _ = ctx.delete_vertex_by_i64(item.label_id, vid_int, ts);
            }
            Ok(RoutedVertexId::Text(id)) => {
                let _ = ctx.delete_vertex(item.label_id, &id, ts);
            }
            Err(_) => {}
        }
    }
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

    // Updates merge the full existing row, including the primary
    // key mirror, into the write set. Restating the mirror changes nothing
    // and is skipped; a divergent key still falls through to the
    // table-layer rejection below.
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

    {
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
            let old_value = current_record.as_ref().and_then(|record| {
                record
                    .properties
                    .iter()
                    .find(|(name, _)| name == prop_name)
                    .map(|(_, value)| value)
            });
            let redo = UpdateVertexPropRedo {
                label: label_id,
                vid,
                prop_name: prop_name.clone(),
                value: value.clone(),
            };
            let redo_entry = ctx.append_wal_redo(WalOpType::UpdateVertexProp, ts, &redo)?;

            // The table layer rejects primary key columns and missing rows.
            match route_vertex_id(&vid)? {
                RoutedVertexId::Int(id_int) => {
                    ctx.update_vertex_property_by_i64(label_id, id_int, prop_name, value, ts)?;
                }
                RoutedVertexId::Text(id_str) => {
                    ctx.update_vertex_property(label_id, &id_str, prop_name, value, ts)?;
                }
            }
            record_vertex_property_update(
                ctx,
                label_id,
                vid,
                prop_name,
                old_value,
                Some(redo_entry),
            )?;
        }

        let props: Vec<(String, Value)> = merged_props.into_iter().collect();
        let vid_value = Value::from(vid);
        super::index_maintenance::refresh_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            &vid_value,
            &tag.name,
            &props,
            ts,
        )?;
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

    match &routed {
        RoutedVertexId::Int(vid_int) => {
            ctx.delete_vertex_by_i64(label_id, *vid_int, ts)?;
        }
        RoutedVertexId::Text(id_str) => {
            ctx.delete_vertex(label_id, id_str, ts)?;
        }
    }

    record_vertex_remove(ctx, label_id, vid, Some(redo_entry))?;
    let id_value = Value::from(vid);
    super::index_maintenance::delete_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_info.space_id,
        &id_value,
        tag_name,
        ts,
    )?;

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

    let mut rollback: Vec<InsertedVertexTag> = Vec::with_capacity(staged.len());
    let mut first_error: Option<StorageError> = None;
    for (pos, row) in staged.iter().enumerate() {
        match staged_marks[pos]
            .take()
            .expect("every staged row is buffered once")
        {
            Ok(()) => {
                rollback.push(InsertedVertexTag {
                    label_id: row.label_id,
                    vid: row.vid,
                    vertex_id: row.vertex_id.clone(),
                    tag_name: row.tag_name.clone(),
                    redo_entry: row.redo_entry.clone(),
                });
            }
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }
    if let Some(e) = first_error {
        rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
        for item in &rollback {
            ctx.rollback_write_scope(item.label_id, &mut scope, ts);
        }
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }

    // Phase C (finish): per-row index maintenance. Table rows are not
    // applied yet, so a failure here only discards the staged rows.
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
            rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
            for item in &rollback {
                ctx.rollback_write_scope(item.label_id, &mut scope, ts);
            }
            scope.clear();
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
    }

    // Phase D (apply): commit every label's staged rows. A failed apply
    // undoes its own partial application inside the table hook; the scope
    // records of every label are discarded and the timestamp aborted.
    let mut id_by_key: HashMap<(LabelId, IdKey), u32> = HashMap::new();
    for label_id in &label_order {
        match ctx.commit_write_scope(*label_id, &mut scope, ts) {
            Ok(mapping) => {
                for (key, global_id) in mapping {
                    id_by_key.insert((*label_id, key), global_id);
                }
            }
            Err(e) => {
                for item in &rollback {
                    ctx.rollback_write_scope(item.label_id, &mut scope, ts);
                }
                scope.clear();
                ctx.abort_write_timestamp(ts);
                return Err(e);
            }
        }
    }
    let mut internal_ids: Vec<u32> = vec![0; staged.len()];
    for (pos, row) in staged.iter().enumerate() {
        match id_by_key.get(&(row.label_id, row.key.clone())) {
            Some(global_id) => internal_ids[pos] = *global_id,
            None => {
                for item in &rollback {
                    ctx.rollback_write_scope(item.label_id, &mut scope, ts);
                }
                scope.clear();
                ctx.abort_write_timestamp(ts);
                return Err(StorageError::db_error(format!(
                    "commit apply lost staged vertex {:?}",
                    row.vid
                )));
            }
        }
    }

    // Phase E: the id-cache and domain bookkeeping that `insert_vertex`
    // performs inline, now fed by the commit mapping.
    for (pos, row) in staged.iter().enumerate() {
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

    let mut ids = Vec::with_capacity(staged.len());
    for row in &staged {
        ids.push(row.vid);
    }

    for item in &rollback {
        record_vertex_insert(ctx, item.label_id, item.vid, Some(item.redo_entry.clone()))?;
    }
    debug_assert!(scope.is_empty());

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(ids)
}
