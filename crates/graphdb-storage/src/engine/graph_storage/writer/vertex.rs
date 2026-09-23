use std::collections::HashMap;

use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{
    ColumnId, EdgeIdentifier, LabelId, TagInfo, Timestamp, VertexId,
};
use graphdb_core::wal::redo::{
    DeleteEdgeRedo, DeleteVertexRedo, InsertVertexRedo, UpdateVertexPropRedo,
};
use graphdb_core::wal::types::WalOpType;
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::{StorageError, StorageResult, Value, Vertex};
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

    // Fail fast before allocating a write timestamp: untyped ids and
    // non-single-tag vertices never reach storage.
    let tag = require_single_tag(&vertex)?.clone();
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vertex.vid)?;
    let vertex = Vertex::new(vid, vec![tag]);

    let ts = ctx.get_write_timestamp()?;
    let mut rollback = Vec::new();
    let result = insert_vertex_at_timestamp(
        ctx,
        space,
        space_info.space_id,
        vertex,
        ts,
        &mut rollback,
    );

    if result.is_err() {
        rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
    } else {
        for item in &rollback {
            record_vertex_insert(ctx, item.label_id, item.vid, Some(item.redo_entry.clone()))?;
        }
    }

    if result.is_ok() {
        ctx.commit_write_timestamp_ordered(ts)?;
    } else {
        ctx.abort_write_timestamp(ts);
    }

    result
}

/// Single-label gate for vertex writes: exactly one tag is required.
///
/// Zero tags (missing label) and multiple tags are rejected before any
/// timestamp is allocated, so a bad batch fails as a whole up front.
fn require_single_tag(vertex: &Vertex) -> StorageResult<&Tag> {
    match vertex.tags.as_slice() {
        [tag] => Ok(tag),
        [] => Err(StorageError::invalid_input(
            "Vertex must carry exactly one tag: missing label".to_string(),
        )),
        tags => {
            let names: Vec<&str> = tags.iter().map(|tag| tag.name.as_str()).collect();
            Err(StorageError::invalid_input(format!(
                "Multi-tag vertices are not supported: got tags [{}]",
                names.join(", ")
            )))
        }
    }
}

fn insert_vertex_at_timestamp(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    vertex: Vertex,
    ts: Timestamp,
    rollback: &mut Vec<InsertedVertexTag>,
) -> StorageResult<VertexId> {
    let tag = require_single_tag(&vertex)?;
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
            ctx.insert_vertex_by_i64(label_id, vid_int, &props, ts)?;
        }
        RoutedVertexId::Text(id_str) => {
            ctx.insert_vertex(label_id, &id_str, &props, ts)?;
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

/// Batch variant of [`insert_vertex_at_timestamp`] using pre-resolved schema
/// data: tag table (one `list_tags` per batch), pre-scanned SERIAL state (one
/// column scan per touched serial column), and pre-listed tag indexes (one
/// `list_tag_indexes` per batch). Row semantics match the per-row path.
fn insert_vertex_at_timestamp_prechecked(
    ctx: &GraphStorageContext,
    space_id: u64,
    batch: &mut PrecheckedBatchContext<'_>,
    vertex: Vertex,
    ts: Timestamp,
    rollback: &mut Vec<InsertedVertexTag>,
) -> StorageResult<VertexId> {
    let tag = require_single_tag(&vertex)?;
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

    match route_vertex_id(&vid)? {
        RoutedVertexId::Int(vid_int) => {
            ctx.insert_vertex_by_i64(label_id, vid_int, &props, ts)?;
        }
        RoutedVertexId::Text(id_str) => {
            ctx.insert_vertex(label_id, &id_str, &props, ts)?;
        }
    }

    let vid_value = Value::from(vid);
    rollback.push(InsertedVertexTag {
        label_id,
        vid,
        vertex_id: vid_value.clone(),
        tag_name: tag.name.clone(),
        redo_entry,
    });

    super::index_maintenance::update_vertex_indexes_with_list(
        ctx,
        batch.tag_indexes,
        space_id,
        &vid_value,
        &tag.name,
        &props,
        ts,
    )?;

    Ok(vid)
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

    let tag = require_single_tag(&vertex)?.clone();
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vertex.vid)?;

    let ts = ctx.get_write_timestamp()?;
    let label_id = tag_label_id(ctx, space, &tag.name)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found", tag.name))
    })?;

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
    id: &VertexId,
) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let tags = ctx.schema_manager().list_tags(space)?;
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, *id)?;
    let routed = route_vertex_id(&vid)?;
    let ts = ctx.get_write_timestamp()?;

    // Single-label delete: probe every label table for the id. Zero hits is
    // an explicit not-found; multiple hits violate the single-label
    // invariant and are rejected instead of fanning out across distinct
    // vertices that happen to share an id string.
    let mut owners: Vec<(LabelId, String)> = Vec::new();
    for tag in &tags {
        let hit = ctx.data_store().with_vertex_tables(|tables| {
            tables.get(&tag.tag_id).is_some_and(|table| match &routed {
                RoutedVertexId::Int(vid_int) => {
                    table.get_internal_id_by_i64(*vid_int, ts).is_some()
                }
                RoutedVertexId::Text(id_str) => {
                    table.get_internal_id(id_str, ts).is_some()
                }
            })
        });
        if hit {
            owners.push((tag.tag_id, tag.tag_name.clone()));
        }
    }

    let (label_id, tag_name) = match owners.as_slice() {
        [(label_id, tag_name)] => (*label_id, tag_name.clone()),
        [] => {
            ctx.abort_write_timestamp(ts);
            return Err(StorageError::vertex_not_found());
        }
        _ => {
            ctx.abort_write_timestamp(ts);
            let names: Vec<&str> = owners.iter().map(|(_, name)| name.as_str()).collect();
            return Err(StorageError::invalid_input(format!(
                "Vertex id {} matches multiple labels [{}]: label-scoped delete required",
                vid,
                names.join(", ")
            )));
        }
    };

    let redo = DeleteVertexRedo { label: label_id, vid };
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
        &tag_name,
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

    delete_vertex(ctx, space, &id)
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
        if let Err(error) = batch_delete_incident_edges_of_type(ctx, space_id, &ids, edge_info, ts) {
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
        delete_vertex(ctx, space, id)?;
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
        require_single_tag(vertex)?;
        for tag in &vertex.tags {
            if !tag_map.contains_key(tag.name.as_str()) {
                return Err(StorageError::not_found(format!(
                    "Tag {} not found",
                    tag.name
                )));
            }
        }
    }

    // Pre-count vertices per tag and reserve capacity to avoid rehashing during inserts.
    {
        let mut tag_counts: HashMap<LabelId, usize> = HashMap::new();
        for vertex in &vertices {
            for tag in &vertex.tags {
                if let Some(info) = tag_map.get(tag.name.as_str()) {
                    *tag_counts.entry(info.tag_id).or_insert(0) += 1;
                }
            }
        }
        for (label_id, count) in &tag_counts {
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
                v.tags.iter().any(|t| {
                    t.name == tag.tag_name && t.properties.keys().any(|k| k == &prop_def.name)
                })
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
    let mut ids = Vec::with_capacity(vertices.len());
    let mut rollback = Vec::new();
    let mut batch_ctx = PrecheckedBatchContext {
        tag_map: &tag_map,
        tag_indexes: &tag_indexes,
        serial_state: &mut serial_state,
        vid_type: &space_info.vid_type,
    };

    for vertex in vertices {
        let id = match insert_vertex_at_timestamp_prechecked(
            ctx,
            space_info.space_id,
            &mut batch_ctx,
            vertex,
            ts,
            &mut rollback,
        ) {
            Ok(id) => id,
            Err(e) => {
                rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
                ctx.abort_write_timestamp(ts);
                return Err(e);
            }
        };
        ids.push(id);
    }

    for item in &rollback {
        record_vertex_insert(ctx, item.label_id, item.vid, Some(item.redo_entry.clone()))?;
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(ids)
}

pub(crate) fn delete_tags(
    ctx: &GraphStorageContext,
    space: &str,
    vertex_id: &VertexId,
    tag_names: &[String],
) -> StorageResult<usize> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let ts = ctx.get_write_timestamp()?;
    let mut deleted_count = 0;

    let routed = route_vertex_id(vertex_id)?;

    for tag_name in tag_names {
        if let Some(label_id) = tag_label_id(ctx, space, tag_name)? {
            let redo = DeleteVertexRedo {
                label: label_id,
                vid: *vertex_id,
            };
            let redo_entry = ctx.append_wal_redo(WalOpType::DeleteVertex, ts, &redo)?;

            let result = match &routed {
                RoutedVertexId::Int(vid_int) => {
                    ctx.delete_vertex_by_i64(label_id, *vid_int, ts)
                }
                RoutedVertexId::Text(id_str) => ctx.delete_vertex(label_id, id_str, ts),
            };

            if result.is_ok() {
                record_vertex_remove(ctx, label_id, *vertex_id, Some(redo_entry))?;
                let vertex_id_value = Value::from(*vertex_id);
                super::index_maintenance::delete_vertex_indexes(
                    ctx,
                    ctx.index_metadata_manager(),
                    space_info.space_id,
                    &vertex_id_value,
                    tag_name,
                    ts,
                )?;
                deleted_count += 1;
            }
        }
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(deleted_count)
}
