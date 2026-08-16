use std::collections::HashMap;

use crate::core::metadata::IndexMetadataManager;
use crate::core::types::{
    ColumnId, EdgeIdentifier, EdgeTypeInfo, InsertEdgeInfo, InsertVertexInfo, LabelId, Timestamp,
    UpdateInfo, UpdateOp, UpdateTarget, VertexId,
};
use crate::core::wal::redo::{
    DeleteEdgeRedo, DeleteVertexRedo, InsertEdgeRedo, InsertVertexRedo, UpdateVertexPropRedo,
};
use crate::core::wal::types::WalOpType;
use crate::core::{Edge, EdgeDirection, StorageError, StorageResult, Value, Vertex};
use crate::storage::engine::params::{EdgeOperationParams, InsertEdgeParams};
use crate::storage::index::traits::VertexIndexOps;
use crate::storage::index::types::EdgeIdentity;
use crate::transaction::undo_log::{
    InsertEdgeUndo, InsertVertexUndo, RemoveVertexUndo, RestoreEdgeUndo, UndoLogEntry,
    UpdateVertexPropUndo,
};
use crate::transaction::wal::TransactionWalEntry;
use crate::transaction::{MutationEntityKey, MutationResult};

use super::context::GraphStorageContext;
use super::ops::{edge_label_id, endpoint_label_id, tag_label_id};
use super::reader;
use super::serial::{scan_edge_serial_column, scan_vertex_serial_column};

#[derive(Debug)]
struct InsertedVertexTag {
    label_id: LabelId,
    id: String,
    vid: VertexId,
    vertex_id: Value,
    tag_name: String,
    redo_entry: TransactionWalEntry,
}

#[derive(Debug)]
struct InsertedEdgeRecord {
    edge_label_id: LabelId,
    src_label_id: LabelId,
    dst_label_id: LabelId,
    src: VertexId,
    dst: VertexId,
    edge_type: String,
    rank: i64,
    redo_entry: TransactionWalEntry,
}

fn record_vertex_insert(
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

fn record_vertex_remove(
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

fn record_vertex_property_update(
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
                    .unwrap_or(Value::Null(crate::core::value::null::NullType::Null)),
            })),
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

fn record_edge_insert(
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
                oe_offset: -1,
                ie_offset: -1,
            })),
            redo_entry,
            modified_table: Some("edge".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

fn record_edge_remove(
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

pub(crate) fn insert_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
) -> StorageResult<VertexId> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let ts = ctx.get_write_timestamp()?;
    let mut rollback = Vec::new();
    let result =
        insert_vertex_at_timestamp(ctx, space, space_info.space_id, vertex, ts, &mut rollback);

    if result.is_err() {
        rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
    } else {
        for item in &rollback {
            record_vertex_insert(ctx, item.label_id, item.vid, Some(item.redo_entry.clone()))?;
        }
    }

    if result.is_ok() {
        ctx.commit_write_timestamp(ts);
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
) -> StorageResult<VertexId> {
    for tag in &vertex.tags {
        let label_id = tag_label_id(ctx, space, &tag.name)?
            .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;
        let props: Vec<(String, Value)> = tag
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let props = apply_tag_constraints(ctx, space, &tag.name, props)?;
        let redo = InsertVertexRedo {
            label: label_id,
            vid: vertex.vid,
            properties: props.clone(),
        };
        let redo_entry = ctx.append_wal_redo(WalOpType::InsertVertex, ts, &redo)?;

        if let Some(vid_int) = vertex.vid.as_int64() {
            ctx.insert_vertex_by_i64(label_id, vid_int, &props, ts)?;
        } else if let Some(id_str) = vertex.vid.as_str() {
            ctx.insert_vertex(label_id, id_str, &props, ts)?;
        } else {
            let id_str = vertex.vid.to_string();
            ctx.insert_vertex(label_id, &id_str, &props, ts)?;
        }

        let vid_value = Value::from(vertex.vid);
        rollback.push(InsertedVertexTag {
            label_id,
            id: vertex.vid.to_string(),
            vid: vertex.vid,
            vertex_id: vid_value.clone(),
            tag_name: tag.name.clone(),
            redo_entry,
        });

        update_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_id,
            &vid_value,
            &tag.name,
            &props,
            ts,
        )?;
    }

    Ok(vertex.vid)
}

/// Apply tag schema constraints (DEFAULT values, NOT NULL and SERIAL) to a
/// property list before persisting a vertex tag.
fn apply_tag_constraints(
    ctx: &GraphStorageContext,
    space: &str,
    tag_name: &str,
    props: Vec<(String, Value)>,
) -> StorageResult<Vec<(String, Value)>> {
    let tag = ctx
        .schema_manager()
        .get_tag(space, tag_name)?
        .ok_or_else(|| StorageError::label_not_found(tag_name.to_string()))?;
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .map(|space| space.space_id)
        .unwrap_or(0);
    let mut result = props;
    for prop_def in &tag.properties {
        if let Some((_, value)) = result.iter().find(|(name, _)| name == &prop_def.name) {
            if !prop_def.nullable && value.is_null() {
                return Err(StorageError::null_value_not_allowed(&prop_def.name));
            }
            if prop_def.serial {
                validate_explicit_serial_value(
                    ctx,
                    space_id,
                    tag_name,
                    tag.tag_id,
                    &prop_def.name,
                    value,
                    scan_vertex_serial_column,
                )?;
            }
            continue;
        }
        if prop_def.serial {
            // Auto-allocate the next value for this tag's serial column.
            let key = super::serial::SerialKey::new(space_id, tag_name.to_string());
            let next = ctx.serial_allocator().next(&key);
            result.push((prop_def.name.clone(), Value::BigInt(next as i64)));
            continue;
        }
        if let Some(default) = &prop_def.default {
            result.push((prop_def.name.clone(), default.clone()));
        } else if !prop_def.nullable {
            return Err(StorageError::null_value_not_allowed(&prop_def.name));
        }
    }
    Ok(result)
}

/// Apply edge schema constraints (DEFAULT values, NOT NULL and SERIAL) to a
/// property list before persisting an edge.
fn apply_edge_type_constraints(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    props: Vec<(String, Value)>,
) -> StorageResult<Vec<(String, Value)>> {
    let et = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))?;
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .map(|space| space.space_id)
        .unwrap_or(0);
    let mut result = props;
    for prop_def in &et.properties {
        if let Some((_, value)) = result.iter().find(|(name, _)| name == &prop_def.name) {
            if !prop_def.nullable && value.is_null() {
                return Err(StorageError::null_value_not_allowed(&prop_def.name));
            }
            if prop_def.serial {
                validate_explicit_serial_value(
                    ctx,
                    space_id,
                    edge_type,
                    et.edge_type_id,
                    &prop_def.name,
                    value,
                    scan_edge_serial_column,
                )?;
            }
            continue;
        }
        if prop_def.serial {
            // Auto-allocate the next value for this edge type's serial column.
            let key = super::serial::SerialKey::new(space_id, edge_type.to_string());
            let next = ctx.serial_allocator().next(&key);
            result.push((prop_def.name.clone(), Value::BigInt(next as i64)));
            continue;
        }
        if let Some(default) = &prop_def.default {
            result.push((prop_def.name.clone(), default.clone()));
        } else if !prop_def.nullable {
            return Err(StorageError::null_value_not_allowed(&prop_def.name));
        }
    }
    Ok(result)
}

/// Validate an explicitly supplied SERIAL value and advance the counter.
///
/// Explicit values are rejected when they collide with an already-allocated
/// value in the column's occupied interval. On success the counter is advanced
/// past the supplied value so later auto-allocations never collide with it.
fn validate_explicit_serial_value(
    ctx: &GraphStorageContext,
    space_id: u64,
    table_name: &str,
    label: LabelId,
    prop_name: &str,
    value: &Value,
    scan_column: fn(&GraphStorageContext, LabelId, &str) -> Option<super::serial::SerialColumnScan>,
) -> StorageResult<()> {
    let Some(integer) = serial_value_as_i64(value) else {
        // Non-integer values are rejected later by the column type coercion.
        return Ok(());
    };
    if integer >= 0 {
        if let Some(scan) = scan_column(ctx, label, prop_name) {
            if scan.contains(integer) {
                return Err(StorageError::invalid_operation(format!(
                    "Duplicate value {} for SERIAL column '{}': the value is already allocated",
                    integer, prop_name
                )));
            }
        }
        ctx.serial_allocator().advance_to(
            &super::serial::SerialKey::new(space_id, table_name),
            integer as u64,
        );
    }
    Ok(())
}

fn serial_value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::BigInt(v) => Some(*v),
        Value::Int(v) => Some(*v as i64),
        Value::SmallInt(v) => Some(*v as i64),
        _ => None,
    }
}

fn rollback_vertex_tags(
    ctx: &GraphStorageContext,
    space_id: u64,
    inserted: &[InsertedVertexTag],
    ts: Timestamp,
) {
    for item in inserted.iter().rev() {
        let _ = delete_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_id,
            &item.vertex_id,
            &item.tag_name,
            ts,
        );
        if let Some(vid_int) = item.vid.as_int64() {
            let _ = ctx.delete_vertex_by_i64(item.label_id, vid_int, ts);
        } else {
            let _ = ctx.delete_vertex(item.label_id, &item.id, ts);
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

    let ts = ctx.get_write_timestamp()?;
    let vid_int = vertex.vid.as_int64();

    for tag in &vertex.tags {
        if let Some(label_id) = tag_label_id(ctx, space, &tag.name)? {
            let current_record = if let Some(id_int) = vid_int {
                ctx.get_vertex_by_i64(label_id, id_int, ts)
            } else if let Some(id_str) = vertex.vid.as_str() {
                ctx.get_vertex(label_id, id_str, ts)
            } else {
                let id_str = vertex.vid.to_string();
                ctx.get_vertex(label_id, &id_str, ts)
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
                    vid: vertex.vid,
                    prop_name: prop_name.clone(),
                    value: value.clone(),
                };
                let redo_entry = ctx.append_wal_redo(WalOpType::UpdateVertexProp, ts, &redo)?;

                if let Some(id_int) = vid_int {
                    ctx.update_vertex_property_by_i64(label_id, id_int, prop_name, value, ts)?;
                } else if let Some(id_str) = vertex.vid.as_str() {
                    ctx.update_vertex_property(label_id, id_str, prop_name, value, ts)?;
                } else {
                    let id_str = vertex.vid.to_string();
                    ctx.update_vertex_property(label_id, &id_str, prop_name, value, ts)?;
                }
                record_vertex_property_update(
                    ctx,
                    label_id,
                    vertex.vid,
                    prop_name,
                    old_value,
                    Some(redo_entry),
                )?;
            }

            let props: Vec<(String, Value)> = merged_props.into_iter().collect();
            let vid_value = Value::from(vertex.vid);
            refresh_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &vid_value,
                &tag.name,
                &props,
                ts,
            )?;
        }
    }

    ctx.commit_write_timestamp(ts);

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
    let ts = ctx.get_write_timestamp()?;
    let id_int = id.as_int64();

    for tag in &tags {
        let label_id = tag.tag_id;
        let redo = DeleteVertexRedo {
            label: label_id,
            vid: *id,
        };
        let redo_entry = ctx.append_wal_redo(WalOpType::DeleteVertex, ts, &redo)?;

        let delete_result = if let Some(vid_int) = id_int {
            ctx.delete_vertex_by_i64(label_id, vid_int, ts)
        } else if let Some(id_str) = id.as_str() {
            ctx.delete_vertex(label_id, id_str, ts)
        } else {
            let id_str = id.to_string();
            ctx.delete_vertex(label_id, &id_str, ts)
        };

        if delete_result.is_ok() {
            record_vertex_remove(ctx, label_id, *id, Some(redo_entry))?;
            let id_value = Value::from(*id);
            delete_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &id_value,
                &tag.tag_name,
                ts,
            )?;
        }
    }

    ctx.commit_write_timestamp(ts);

    Ok(())
}

pub(crate) fn delete_vertex_with_edges(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
) -> StorageResult<()> {
    let edges = reader::get_node_edges(ctx, space, id, EdgeDirection::Both)?;

    for edge in edges {
        delete_edge(
            ctx,
            space,
            &edge.src,
            &edge.dst,
            &edge.edge_type,
            edge.ranking,
        )?;
    }

    delete_vertex(ctx, space, id)
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

    validate_vertex_batch(ctx, space, &vertices)?;

    // Pre-count vertices per tag and reserve capacity to avoid rehashing during inserts.
    {
        let mut tag_counts: HashMap<LabelId, usize> = HashMap::new();
        for vertex in &vertices {
            for tag in &vertex.tags {
                if let Ok(Some(label_id)) = tag_label_id(ctx, space, &tag.name) {
                    *tag_counts.entry(label_id).or_insert(0) += 1;
                }
            }
        }
        for (label_id, count) in &tag_counts {
            ctx.reserve_vertex_capacity(*label_id, *count);
        }
    }

    let ts = ctx.get_write_timestamp()?;
    let mut ids = Vec::with_capacity(vertices.len());
    let mut rollback = Vec::new();

    for vertex in vertices {
        let id = match insert_vertex_at_timestamp(
            ctx,
            space,
            space_info.space_id,
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

    ctx.commit_write_timestamp(ts);

    Ok(ids)
}

fn validate_vertex_batch(
    ctx: &GraphStorageContext,
    space: &str,
    vertices: &[Vertex],
) -> StorageResult<()> {
    for vertex in vertices {
        for tag in &vertex.tags {
            if tag_label_id(ctx, space, &tag.name)?.is_none() {
                return Err(StorageError::not_found(format!(
                    "Tag {} not found",
                    tag.name
                )));
            }
        }
    }
    Ok(())
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

    let id_int = vertex_id.as_int64();
    let id_str_raw = vertex_id.as_str();

    for tag_name in tag_names {
        if let Some(label_id) = tag_label_id(ctx, space, tag_name)? {
            let redo = DeleteVertexRedo {
                label: label_id,
                vid: *vertex_id,
            };
            let redo_entry = ctx.append_wal_redo(WalOpType::DeleteVertex, ts, &redo)?;

            let result = if let Some(vid_int) = id_int {
                ctx.delete_vertex_by_i64(label_id, vid_int, ts)
            } else if let Some(id_str) = id_str_raw {
                ctx.delete_vertex(label_id, id_str, ts)
            } else {
                let id_str = vertex_id.to_string();
                ctx.delete_vertex(label_id, &id_str, ts)
            };

            if result.is_ok() {
                record_vertex_remove(ctx, label_id, *vertex_id, Some(redo_entry))?;
                let vertex_id_value = Value::from(*vertex_id);
                delete_vertex_indexes(
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

    ctx.commit_write_timestamp(ts);

    Ok(deleted_count)
}

pub(crate) fn insert_edge(ctx: &GraphStorageContext, space: &str, edge: Edge) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

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
        ctx.commit_write_timestamp(ts);
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
    let props = apply_edge_type_constraints(ctx, space, &edge.edge_type, props)?;
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

fn resolve_edge_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<EdgeTypeInfo> {
    ctx.schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| StorageError::not_found(format!("Edge type {} not found", edge_type)))
}

fn rollback_edges(
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
    let previous = reader::get_edge(ctx, space, src, dst, edge_type, rank)?;
    let ts = ctx.get_write_timestamp()?;
    let result = delete_edge_at_timestamp(ctx, space, src, dst, edge_type, rank, ts);
    if result.is_ok() {
        if let Some(previous) = previous {
            let edge_info = resolve_edge_type(ctx, space, edge_type)?;
            let src_label = endpoint_label_id(ctx, space, &edge_info.src_tag_name)?
                .ok_or_else(|| StorageError::not_found("Source tag not found"))?;
            let dst_label = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)?
                .ok_or_else(|| StorageError::not_found("Destination tag not found"))?;
            record_edge_remove(
                ctx,
                EdgeIdentifier::new(
                    src_label,
                    *src,
                    dst_label,
                    *dst,
                    edge_info.edge_type_id,
                    rank,
                ),
                previous.props.into_iter().collect(),
                result.as_ref().ok().and_then(Clone::clone),
            )?;
        }
    }
    if result.is_ok() {
        ctx.commit_write_timestamp(ts);
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
    let src = edge.src;
    let dst = edge.dst;
    let edge_type = edge.edge_type.clone();
    let ranking = edge.ranking;

    // Read current properties for rollback
    let current_props = super::reader::get_edge(ctx, space, &src, &dst, &edge_type, ranking)?
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
            ctx.commit_write_timestamp(ts);
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

pub(crate) fn batch_insert_edges(
    ctx: &GraphStorageContext,
    space: &str,
    edges: Vec<Edge>,
) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    validate_edge_batch(ctx, space, &edges)?;

    let ts = ctx.get_write_timestamp()?;
    let mut rollback = Vec::new();

    for edge in edges {
        if let Err(e) =
            insert_edge_at_timestamp(ctx, space, space_info.space_id, edge, ts, &mut rollback)
        {
            rollback_edges(ctx, space_info.space_id, &rollback, ts);
            return Err(e);
        }
    }

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

    ctx.commit_write_timestamp(ts);

    Ok(())
}

fn validate_edge_batch(
    ctx: &GraphStorageContext,
    space: &str,
    edges: &[Edge],
) -> StorageResult<()> {
    for edge in edges {
        let edge_type = resolve_edge_type(ctx, space, &edge.edge_type)?;
        if endpoint_label_id(ctx, space, &edge_type.src_tag_name)?.is_none() {
            return Err(StorageError::not_found(format!(
                "Source tag {} not found",
                edge_type.src_tag_name
            )));
        }
        if endpoint_label_id(ctx, space, &edge_type.dst_tag_name)?.is_none() {
            return Err(StorageError::not_found(format!(
                "Destination tag {} not found",
                edge_type.dst_tag_name
            )));
        }
    }
    Ok(())
}

pub(crate) fn insert_vertex_data(
    ctx: &GraphStorageContext,
    space: &str,
    info: &InsertVertexInfo,
) -> StorageResult<bool> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let tag = ctx
        .schema_manager()
        .get_tag(space, &info.tag_name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", info.tag_name)))?;

    if info.space_id != space_info.space_id {
        return Err(StorageError::db_error("Space ID mismatch".to_string()));
    }

    let ts = ctx.get_write_timestamp()?;

    let label_id = tag.tag_id;
    let vid = VertexId::try_from(&info.vertex_id)
        .map_err(|e| StorageError::invalid_input(e.to_string()))?;

    let props = apply_tag_constraints(ctx, space, &info.tag_name, info.props.clone())?;
    let redo = InsertVertexRedo {
        label: label_id,
        vid,
        properties: props.clone(),
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::InsertVertex, ts, &redo)?;
    let result = if let Some(id_int) = vid.as_int64() {
        ctx.insert_vertex_by_i64(label_id, id_int, &props, ts)
    } else if let Some(id_str) = vid.as_str() {
        ctx.insert_vertex(label_id, id_str, &props, ts)
    } else {
        let id_str = vid.to_string();
        ctx.insert_vertex(label_id, &id_str, &props, ts)
    };
    let final_result = match result {
        Ok(_) => {
            update_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &info.vertex_id,
                &info.tag_name,
                &props,
                ts,
            )?;
            record_vertex_insert(ctx, label_id, vid, Some(redo_entry))?;
            Ok(true)
        }
        Err(ref e)
            if e.kind() == crate::core::error::storage::StorageErrorKind::VertexAlreadyExists =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    };
    if final_result.is_ok() {
        ctx.commit_write_timestamp(ts);
    } else {
        ctx.abort_write_timestamp(ts);
    }
    final_result
}

pub(crate) fn insert_edge_data(
    ctx: &GraphStorageContext,
    space: &str,
    info: &InsertEdgeInfo,
) -> StorageResult<bool> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let edge_type = ctx
        .schema_manager()
        .get_edge_type(space, &info.edge_name)?
        .ok_or_else(|| {
            StorageError::not_found(format!("Edge type {} not found", info.edge_name))
        })?;

    if info.space_id != space_info.space_id {
        return Err(StorageError::db_error("Space ID mismatch".to_string()));
    }

    let ts = ctx.get_write_timestamp()?;

    let edge_label_id = edge_type.edge_type_id;
    let src_vid = VertexId::try_from(&info.src_vertex_id)
        .map_err(|e| StorageError::invalid_input(e.to_string()))?;
    let dst_vid = VertexId::try_from(&info.dst_vertex_id)
        .map_err(|e| StorageError::invalid_input(e.to_string()))?;
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
    let props = apply_edge_type_constraints(ctx, space, &info.edge_name, info.props.clone())?;
    let redo = InsertEdgeRedo {
        src_label: src_label_id,
        src_vid,
        dst_label: dst_label_id,
        dst_vid,
        edge_label: edge_label_id,
        rank: info.rank,
        properties: props.clone(),
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::InsertEdge, ts, &redo)?;
    let result = ctx.insert_edge(InsertEdgeParams {
        edge_label: edge_label_id,
        src_label: src_label_id,
        src_id: src_vid,
        dst_label: dst_label_id,
        dst_id: dst_vid,
        rank: info.rank,
        properties: &props,
        ts,
    });

    let final_result = match result {
        Ok(_) => {
            let src_value = Value::from(src_vid);
            let dst_value = Value::from(dst_vid);
            let edge_identity = EdgeIdentity::new(
                space_info.space_id,
                &src_value,
                &dst_value,
                &info.edge_name,
                info.rank,
            );
            match ctx.update_all_edge_indexes_mvcc(&edge_identity, &props, ts) {
                Ok(()) => {
                    record_edge_insert(
                        ctx,
                        EdgeIdentifier::new(
                            src_label_id,
                            src_vid,
                            dst_label_id,
                            dst_vid,
                            edge_label_id,
                            info.rank,
                        ),
                        Some(redo_entry),
                    )?;
                    Ok(true)
                }
                Err(error) => {
                    let _ = ctx.delete_edge(
                        &EdgeOperationParams {
                            edge_label: edge_label_id,
                            src_label: src_label_id,
                            src_id: src_vid,
                            dst_label: dst_label_id,
                            dst_id: dst_vid,
                            rank: info.rank,
                        },
                        ts,
                    );
                    Err(error)
                }
            }
        }
        Err(ref e)
            if e.kind() == crate::core::error::storage::StorageErrorKind::EdgeAlreadyExists =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    };
    if final_result.is_ok() {
        ctx.commit_write_timestamp(ts);
    } else {
        ctx.abort_write_timestamp(ts);
    }
    final_result
}

pub(crate) fn delete_vertex_data(
    ctx: &GraphStorageContext,
    space: &str,
    vertex_id: &str,
) -> StorageResult<bool> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let tags = ctx.schema_manager().list_tags(space)?;
    let ts = ctx.get_write_timestamp()?;
    let mut deleted = false;
    let vid = vertex_id
        .parse::<i64>()
        .map(VertexId::from_int64)
        .unwrap_or_else(|_| VertexId::from_string(vertex_id));

    for tag in tags {
        let label_id = tag.tag_id;
        if ctx.delete_vertex(label_id, vertex_id, ts).is_ok() {
            let redo = DeleteVertexRedo {
                label: label_id,
                vid,
            };
            let redo_entry = ctx.append_wal_redo(WalOpType::DeleteVertex, ts, &redo)?;
            record_vertex_remove(ctx, label_id, vid, Some(redo_entry))?;
            delete_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &Value::string(vertex_id),
                &tag.tag_name,
                ts,
            )?;
            deleted = true;
        }
    }

    ctx.commit_write_timestamp(ts);

    Ok(deleted)
}

pub(crate) fn delete_edge_data(
    ctx: &GraphStorageContext,
    space: &str,
    src: &str,
    dst: &str,
    rank: i64,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_write_timestamp()?;
    let mut deleted = false;

    for et in edge_types {
        let edge_label_id = et.edge_type_id;
        let src_label_id = match endpoint_label_id(ctx, space, &et.src_tag_name)? {
            Some(id) => id,
            None => continue,
        };
        let dst_label_id = match endpoint_label_id(ctx, space, &et.dst_tag_name)? {
            Some(id) => id,
            None => continue,
        };
        let src_vid = src
            .parse::<i64>()
            .map(VertexId::from_int64)
            .unwrap_or_else(|_| VertexId::from_string(src));
        let dst_vid = dst
            .parse::<i64>()
            .map(VertexId::from_int64)
            .unwrap_or_else(|_| VertexId::from_string(dst));
        let previous = reader::get_edge(ctx, space, &src_vid, &dst_vid, &et.edge_type_name, rank)?;
        let redo_entry = previous
            .as_ref()
            .map(|_| {
                let redo = DeleteEdgeRedo {
                    src_label: src_label_id,
                    src_vid,
                    dst_label: dst_label_id,
                    dst_vid,
                    edge_label: edge_label_id,
                    rank,
                };
                ctx.append_wal_redo(WalOpType::DeleteEdge, ts, &redo)
            })
            .transpose()?;
        if ctx
            .delete_edge(
                &EdgeOperationParams {
                    edge_label: edge_label_id,
                    src_label: src_label_id,
                    src_id: src_vid,
                    dst_label: dst_label_id,
                    dst_id: dst_vid,
                    rank,
                },
                ts,
            )
            .is_ok_and(|deleted_edge| deleted_edge)
        {
            if let Some(previous) = previous {
                record_edge_remove(
                    ctx,
                    EdgeIdentifier::new(
                        src_label_id,
                        src_vid,
                        dst_label_id,
                        dst_vid,
                        edge_label_id,
                        rank,
                    ),
                    previous.props.into_iter().collect(),
                    redo_entry,
                )?;
            }
            let src_value = Value::from(src_vid);
            let dst_value = Value::from(dst_vid);
            let edge_identity =
                EdgeIdentity::new(space_id, &src_value, &dst_value, &et.edge_type_name, rank);
            ctx.delete_all_edge_indexes_mvcc(&edge_identity, ts)?;
            deleted = true;
        }
    }

    ctx.commit_write_timestamp(ts);

    Ok(deleted)
}

pub(crate) fn update_data(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    info: &UpdateInfo,
) -> StorageResult<bool> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    if space_info.space_id != space_id {
        return Err(StorageError::db_error("Space ID mismatch".to_string()));
    }

    let ts = ctx.get_write_timestamp()?;

    let UpdateTarget {
        space_name,
        label,
        id,
        prop,
    } = &info.update_target;

    if space_name != space {
        return Err(StorageError::db_error(
            "Space name mismatch in update target".to_string(),
        ));
    }

    if let Some(label_id) = tag_label_id(ctx, space, label)? {
        let vid = VertexId::try_from(id).map_err(|e| StorageError::invalid_input(e.to_string()))?;
        let id_str = vid.to_string();
        let current_record = if let Some(id_int) = vid.as_int64() {
            ctx.get_vertex_by_i64(label_id, id_int, ts)
        } else {
            ctx.get_vertex(label_id, &id_str, ts)
        };
        let value = match &info.update_op {
            UpdateOp::Set => info.value.clone(),
            UpdateOp::Add => {
                if let Some(current) = current_record.as_ref() {
                    let current_val = current
                        .properties
                        .iter()
                        .find(|(k, _)| k == prop)
                        .map(|(_, v)| v);
                    if let (Some(crate::core::Value::Int(cv)), crate::core::Value::Int(add_val)) =
                        (current_val, &info.value)
                    {
                        crate::core::Value::Int(cv + add_val)
                    } else {
                        info.value.clone()
                    }
                } else {
                    info.value.clone()
                }
            }
            UpdateOp::Subtract => {
                if let Some(current) = current_record.as_ref() {
                    let current_val = current
                        .properties
                        .iter()
                        .find(|(k, _)| k == prop)
                        .map(|(_, v)| v);
                    if let (Some(crate::core::Value::Int(cv)), crate::core::Value::Int(sub_val)) =
                        (current_val, &info.value)
                    {
                        crate::core::Value::Int(cv - sub_val)
                    } else {
                        info.value.clone()
                    }
                } else {
                    info.value.clone()
                }
            }
            _ => info.value.clone(),
        };

        ctx.update_vertex_property(label_id, &id_str, prop, &value, ts)?;
        let old_value = current_record.as_ref().and_then(|record| {
            record
                .properties
                .iter()
                .find(|(name, _)| name == prop)
                .map(|(_, value)| value)
        });
        record_vertex_property_update(ctx, label_id, vid, prop, old_value, None)?;

        let mut merged_props: HashMap<String, Value> = current_record
            .as_ref()
            .map(|record| record.properties.iter().cloned().collect())
            .unwrap_or_default();
        merged_props.insert(prop.clone(), value);

        refresh_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            id,
            label,
            &merged_props.into_iter().collect::<Vec<_>>(),
            ts,
        )?;
        ctx.commit_write_timestamp(ts);
        Ok(true)
    } else {
        ctx.abort_write_timestamp(ts);
        Err(StorageError::not_found(format!(
            "Label {} not found",
            label
        )))
    }
}

fn tag_index_names(
    index_metadata_manager: &crate::core::metadata::IndexManager,
    space_id: u64,
    tag_name: &str,
) -> StorageResult<Vec<String>> {
    Ok(index_metadata_manager
        .list_tag_indexes(space_id)?
        .into_iter()
        .filter(|index| index.schema_name == tag_name)
        .map(|index| index.name)
        .collect())
}

fn update_vertex_indexes(
    ctx: &GraphStorageContext,
    index_metadata_manager: &crate::core::metadata::IndexManager,
    space_id: u64,
    vertex_id: &Value,
    tag_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    let indexes = index_metadata_manager.list_tag_indexes(space_id)?;
    for index in indexes {
        if index.schema_name != tag_name {
            continue;
        }
        // The index manager derives indexed fields and included columns from
        // the complete entity property set. Keeping both sets here is
        // important: included columns must not become index keys, and an
        // update must refresh their covering values as well.
        let indexed_props: Vec<(String, Value)> = index
            .fields
            .iter()
            .filter_map(|field| props.iter().find(|(name, _)| name == &field.name).cloned())
            .collect();
        let included_changed = index
            .properties
            .iter()
            .any(|name| props.iter().any(|(changed, _)| changed == name));
        if indexed_props.is_empty() && !included_changed {
            continue;
        }
        // Check unique constraint before inserting. The pending-aware lookup
        // (P2) reads unpublished index deltas in-memory instead of forcing a
        // generation publish per statement, which would defeat delta
        // accumulation during batch loads with unique indexes.
        if index.is_unique {
            let index_data = ctx.index_data_manager();
            for (_prop_name, prop_value) in &indexed_props {
                let existing = index_data
                    .read()
                    .lookup_tag_index_pending_aware(space_id, &index, prop_value)?;
                if !existing.is_empty() && !existing.contains(vertex_id) {
                    return Err(StorageError::conflict(format!(
                        "Unique index '{}' violated: value {:?} already exists",
                        index.name, prop_value
                    )));
                }
            }
        }
        ctx.update_vertex_indexes_mvcc(space_id, vertex_id, &index.name, props, ts)?;
    }
    Ok(())
}

fn refresh_vertex_indexes(
    ctx: &GraphStorageContext,
    index_metadata_manager: &crate::core::metadata::IndexManager,
    space_id: u64,
    vertex_id: &Value,
    tag_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    let index_names = tag_index_names(index_metadata_manager, space_id, tag_name)?;
    if index_names.is_empty() {
        return Ok(());
    }

    ctx.delete_vertex_indexes_mvcc(space_id, vertex_id, &index_names, ts)?;
    update_vertex_indexes(
        ctx,
        index_metadata_manager,
        space_id,
        vertex_id,
        tag_name,
        props,
        ts,
    )
}

fn delete_vertex_indexes(
    ctx: &GraphStorageContext,
    index_metadata_manager: &crate::core::metadata::IndexManager,
    space_id: u64,
    vertex_id: &Value,
    tag_name: &str,
    ts: Timestamp,
) -> StorageResult<()> {
    let index_names = tag_index_names(index_metadata_manager, space_id, tag_name)?;
    if !index_names.is_empty() {
        ctx.delete_vertex_indexes_mvcc(space_id, vertex_id, &index_names, ts)?;
    }
    Ok(())
}
