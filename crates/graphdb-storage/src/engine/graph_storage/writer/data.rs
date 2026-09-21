use std::collections::HashMap;

use crate::engine::params::{EdgeOperationParams, InsertEdgeParams};
use crate::index::types::EdgeIdentity;
use graphdb_core::types::{
    EdgeIdentifier, InsertEdgeInfo, InsertVertexInfo, UpdateInfo, UpdateOp, UpdateTarget, VertexId,
};
use graphdb_core::wal::redo::{DeleteEdgeRedo, DeleteVertexRedo, InsertEdgeRedo, InsertVertexRedo};
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{StorageError, StorageResult, Value};

use super::super::context::GraphStorageContext;
use super::super::ops::{endpoint_label_id, tag_label_id};
use super::super::reader;

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

    let props =
        super::constraints::apply_tag_constraints(ctx, space, &info.tag_name, info.props.clone())?;
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
            super::index_maintenance::update_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &info.vertex_id,
                &info.tag_name,
                &props,
                ts,
            )?;
            super::vertex::record_vertex_insert(ctx, label_id, vid, Some(redo_entry))?;
            Ok(true)
        }
        Err(ref e)
            if e.kind() == graphdb_core::error::storage::StorageErrorKind::VertexAlreadyExists =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    };
    if final_result.is_ok() {
        ctx.commit_write_timestamp_ordered(ts)?;
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
    let props = super::constraints::apply_edge_type_constraints(
        ctx,
        space,
        &info.edge_name,
        info.props.clone(),
    )?;
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
                    super::edge::record_edge_insert(
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
            if e.kind() == graphdb_core::error::storage::StorageErrorKind::EdgeAlreadyExists =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    };
    if final_result.is_ok() {
        ctx.commit_write_timestamp_ordered(ts)?;
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
            super::vertex::record_vertex_remove(ctx, label_id, vid, Some(redo_entry))?;
            super::index_maintenance::delete_vertex_indexes(
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

    ctx.commit_write_timestamp_ordered(ts)?;

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
                super::edge::record_edge_remove(
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

    ctx.commit_write_timestamp_ordered(ts)?;

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
                    if let (Some(graphdb_core::Value::Int(cv)), graphdb_core::Value::Int(add_val)) =
                        (current_val, &info.value)
                    {
                        graphdb_core::Value::Int(cv + add_val)
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
                    if let (Some(graphdb_core::Value::Int(cv)), graphdb_core::Value::Int(sub_val)) =
                        (current_val, &info.value)
                    {
                        graphdb_core::Value::Int(cv - sub_val)
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
        super::vertex::record_vertex_property_update(ctx, label_id, vid, prop, old_value, None)?;

        let mut merged_props: HashMap<String, Value> = current_record
            .as_ref()
            .map(|record| record.properties.iter().cloned().collect())
            .unwrap_or_default();
        merged_props.insert(prop.clone(), value);

        super::index_maintenance::refresh_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            id,
            label,
            &merged_props.into_iter().collect::<Vec<_>>(),
            ts,
        )?;
        ctx.commit_write_timestamp_ordered(ts)?;
        Ok(true)
    } else {
        ctx.abort_write_timestamp(ts);
        Err(StorageError::not_found(format!(
            "Label {} not found",
            label
        )))
    }
}
