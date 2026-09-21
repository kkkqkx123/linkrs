use std::collections::HashMap;

use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{ColumnId, EdgeIdentifier, LabelId, TagInfo, Timestamp, VertexId};
use graphdb_core::wal::redo::{DeleteVertexRedo, InsertVertexRedo, UpdateVertexPropRedo};
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{EdgeDirection, StorageError, StorageResult, Value, Vertex};
use graphdb_transaction::undo_log::{
    InsertVertexUndo, RemoveVertexUndo, UndoLogEntry, UpdateVertexPropUndo,
};
use graphdb_transaction::wal::TransactionWalEntry;
use graphdb_transaction::{MutationEntityKey, MutationResult};

use super::super::context::GraphStorageContext;
use super::super::ops::{endpoint_label_id, tag_label_id};
use super::super::reader;
use super::super::serial::scan_vertex_serial_column;
use super::batch::{InsertedVertexTag, PrecheckedBatchContext, SerialBatchState};

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
) -> StorageResult<VertexId> {
    for tag in &vertex.tags {
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

        super::index_maintenance::update_vertex_indexes(
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
    for tag in &vertex.tags {
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

        super::index_maintenance::update_vertex_indexes_with_list(
            ctx,
            batch.tag_indexes,
            space_id,
            &vid_value,
            &tag.name,
            &props,
            ts,
        )?;
    }

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
            super::index_maintenance::delete_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &id_value,
                &tag.tag_name,
                ts,
            )?;
        }
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(())
}

/// Delete a vertex together with every incident edge in bounded chunks.
///
/// Each chunk owns one write timestamp and commits independently, so a
/// mid-fanout failure aborts only the in-flight chunk instead of leaving
/// per-edge commits behind. Aborted stamps stay hidden through the pending
/// gate, and explicit transactions keep per-edge restore entries through the
/// mutation recorder. A failed chunk leaves prior committed chunks intact,
/// never a half-chunk.
pub(crate) fn delete_vertex_with_edges(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
) -> StorageResult<()> {
    const DELETE_VERTEX_FANOUT_CHUNK: usize = 256;
    let edges = reader::get_node_edges(ctx, space, id, EdgeDirection::Both)?;
    if edges.is_empty() {
        return delete_vertex(ctx, space, id);
    }
    for chunk in edges.chunks(DELETE_VERTEX_FANOUT_CHUNK) {
        let ts = ctx.get_write_timestamp()?;
        let mut failed: Option<StorageError> = None;
        for edge in chunk {
            let previous = reader::get_edge(
                ctx,
                space,
                &edge.src,
                &edge.dst,
                &edge.edge_type,
                edge.ranking,
            )?;
            match super::edge::delete_edge_at_timestamp(
                ctx,
                space,
                &edge.src,
                &edge.dst,
                &edge.edge_type,
                edge.ranking,
                ts,
            ) {
                Ok(redo_entry) => {
                    if let Some(previous) = previous {
                        if let Ok(edge_info) =
                            super::edge::resolve_edge_type(ctx, space, &edge.edge_type)
                        {
                            let src_label = endpoint_label_id(ctx, space, &edge_info.src_tag_name)?;
                            let dst_label = endpoint_label_id(ctx, space, &edge_info.dst_tag_name)?;
                            if let (Some(src_label), Some(dst_label)) = (src_label, dst_label) {
                                super::edge::record_edge_remove(
                                    ctx,
                                    EdgeIdentifier::new(
                                        src_label,
                                        edge.src,
                                        dst_label,
                                        edge.dst,
                                        edge_info.edge_type_id,
                                        edge.ranking,
                                    ),
                                    previous.props.into_iter().collect(),
                                    redo_entry,
                                )?;
                            }
                        }
                    }
                }
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failed {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        if let Err(error) = ctx.commit_write_timestamp_ordered(ts) {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
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

    // Resolve tags once per batch instead of once per row per use site
    // (validation, reserve counting, and insertion each re-looked them up).
    let tags = ctx.schema_manager().list_tags(space)?;
    let mut tag_map: HashMap<&str, &TagInfo> = HashMap::with_capacity(tags.len());
    for tag in &tags {
        tag_map.insert(tag.tag_name.as_str(), tag);
    }
    for vertex in &vertices {
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
