use std::collections::HashMap;

use crate::core::metadata::IndexMetadataManager;
use crate::core::types::{
    EdgeTypeInfo, InsertEdgeInfo, InsertVertexInfo, LabelId, Timestamp, UpdateInfo, UpdateOp,
    UpdateTarget, VertexId,
};
use crate::core::{Edge, EdgeDirection, StorageError, StorageResult, Value, Vertex};
use crate::storage::engine::params::{EdgeOperationParams, InsertEdgeParams};
use crate::storage::index::index_data_manager::VertexIndexOps;
use crate::transaction::codec::value_to_bytes;
use crate::core::wal::redo::{
    DeleteEdgeRedo, DeleteVertexRedo, InsertEdgeRedo, InsertVertexRedo, UpdateVertexPropRedo,
};
use crate::core::wal::types::WalOpType;

use super::context::GraphStorageContext;
use super::ops::{edge_label_id, endpoint_label_id, tag_label_id};
use super::reader;

#[derive(Debug)]
struct InsertedVertexTag {
    label_id: LabelId,
    id: String,
    vid: VertexId,
    vertex_id: Value,
    tag_name: String,
}

#[derive(Debug)]
struct InsertedEdgeRecord {
    edge_label_id: LabelId,
    src_label_id: LabelId,
    dst_label_id: LabelId,
    src: VertexId,
    dst: VertexId,
    rank: i64,
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

    let ts = ctx.get_write_timestamp();
    let mut rollback = Vec::new();
    let result =
        insert_vertex_at_timestamp(ctx, space, space_info.space_id, vertex, ts, &mut rollback);

    if result.is_err() {
        rollback_vertex_tags(ctx, space_info.space_id, &rollback, ts);
    }

    ctx.version_manager().release_insert_timestamp(ts);

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
        let redo = InsertVertexRedo {
            label: label_id,
            vid: vertex.vid,
            properties: props
                .iter()
                .map(|(name, value)| (name.clone(), value_to_bytes(value)))
                .collect(),
        };
        ctx.append_wal_redo(WalOpType::InsertVertex, ts, &redo)?;

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

    let ts = ctx.get_write_timestamp();
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
                let redo = UpdateVertexPropRedo {
                    label: label_id,
                    vid: vertex.vid,
                    prop_name: prop_name.clone(),
                    value: value_to_bytes(value),
                };
                ctx.append_wal_redo(WalOpType::UpdateVertexProp, ts, &redo)?;

                if let Some(id_int) = vid_int {
                    ctx.update_vertex_property_by_i64(label_id, id_int, prop_name, value, ts)?;
                } else if let Some(id_str) = vertex.vid.as_str() {
                    ctx.update_vertex_property(label_id, id_str, prop_name, value, ts)?;
                } else {
                    let id_str = vertex.vid.to_string();
                    ctx.update_vertex_property(label_id, &id_str, prop_name, value, ts)?;
                }
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

    ctx.version_manager().release_insert_timestamp(ts);

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
    let ts = ctx.get_write_timestamp();
    let id_int = id.as_int64();

    for tag in &tags {
        let label_id = tag.tag_id;
        let redo = DeleteVertexRedo {
            label: label_id,
            vid: *id,
        };
        ctx.append_wal_redo(WalOpType::DeleteVertex, ts, &redo)?;

        if let Some(vid_int) = id_int {
            let _ = ctx.delete_vertex_by_i64(label_id, vid_int, ts);
        } else if let Some(id_str) = id.as_str() {
            let _ = ctx.delete_vertex(label_id, id_str, ts);
        } else {
            let id_str = id.to_string();
            let _ = ctx.delete_vertex(label_id, &id_str, ts);
        }

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

    ctx.version_manager().release_insert_timestamp(ts);

    Ok(())
}

pub(crate) fn delete_vertex_with_edges(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
) -> StorageResult<()> {
    let edges = reader::get_node_edges(ctx, space, id, EdgeDirection::Both)?;

    for edge in edges {
        let _ = delete_edge(
            ctx,
            space,
            &edge.src,
            &edge.dst,
            &edge.edge_type,
            edge.ranking,
        );
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

    let ts = ctx.get_write_timestamp();
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
                ctx.version_manager().release_insert_timestamp(ts);
                return Err(e);
            }
        };
        ids.push(id);
    }

    ctx.version_manager().release_insert_timestamp(ts);

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

    let ts = ctx.get_write_timestamp();
    let mut deleted_count = 0;

    let id_int = vertex_id.as_int64();
    let id_str_raw = vertex_id.as_str();

    for tag_name in tag_names {
        if let Some(label_id) = tag_label_id(ctx, space, tag_name)? {
            let redo = DeleteVertexRedo {
                label: label_id,
                vid: *vertex_id,
            };
            ctx.append_wal_redo(WalOpType::DeleteVertex, ts, &redo)?;

            let result = if let Some(vid_int) = id_int {
                ctx.delete_vertex_by_i64(label_id, vid_int, ts)
            } else if let Some(id_str) = id_str_raw {
                ctx.delete_vertex(label_id, id_str, ts)
            } else {
                let id_str = vertex_id.to_string();
                ctx.delete_vertex(label_id, &id_str, ts)
            };

            if result.is_ok() {
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

    ctx.version_manager().release_insert_timestamp(ts);

    Ok(deleted_count)
}

pub(crate) fn insert_edge(ctx: &GraphStorageContext, space: &str, edge: Edge) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let ts = ctx.get_write_timestamp();
    let mut rollback = Vec::new();
    let result = insert_edge_at_timestamp(ctx, space, space_info.space_id, edge, ts, &mut rollback);

    if result.is_err() {
        rollback_edges(ctx, space_info.space_id, &rollback, ts);
    }

    ctx.version_manager().release_insert_timestamp(ts);

    result
}

fn insert_edge_at_timestamp(
    ctx: &GraphStorageContext,
    space: &str,
    _space_id: u64,
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
    let redo = InsertEdgeRedo {
        src_label: src_label_id,
        src_vid: edge.src,
        dst_label: dst_label_id,
        dst_vid: edge.dst,
        edge_label: edge_label_id,
        rank: edge.ranking,
        properties: props
            .iter()
            .map(|(name, value)| (name.clone(), value_to_bytes(value)))
            .collect(),
    };
    ctx.append_wal_redo(WalOpType::InsertEdge, ts, &redo)?;

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
        rank: edge.ranking,
    });

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
    _space_id: u64,
    inserted: &[InsertedEdgeRecord],
    ts: Timestamp,
) {
    for item in inserted.iter().rev() {
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

pub(crate) fn delete_edge(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
) -> StorageResult<()> {
    let ts = ctx.get_write_timestamp();

    let edge_label_id = edge_label_id(ctx, space, edge_type)?
        .ok_or_else(|| StorageError::not_found(format!("Edge type {} not found", edge_type)))?;

    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let mut deleted = false;
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
            ctx.append_wal_redo(WalOpType::DeleteEdge, ts, &redo)?;

            ctx.delete_edge(
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

            deleted = true;
            break;
        }
    }

    if !deleted {
        return Err(StorageError::not_found(format!(
            "Edge type {} not found in space {}",
            edge_type, space
        )));
    }

    ctx.version_manager().release_insert_timestamp(ts);

    Ok(())
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

    let ts = ctx.get_write_timestamp();
    let mut rollback = Vec::new();

    for edge in edges {
        if let Err(e) =
            insert_edge_at_timestamp(ctx, space, space_info.space_id, edge, ts, &mut rollback)
        {
            rollback_edges(ctx, space_info.space_id, &rollback, ts);
            return Err(e);
        }
    }

    ctx.version_manager().release_insert_timestamp(ts);

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

    let ts = ctx.get_write_timestamp();

    let label_id = tag.tag_id;
    let vid = VertexId::try_from(&info.vertex_id)
        .map_err(|e| StorageError::invalid_input(e.to_string()))?;

    let result = if let Some(id_int) = vid.as_int64() {
        ctx.insert_vertex_by_i64(label_id, id_int, &info.props, ts)
    } else if let Some(id_str) = vid.as_str() {
        ctx.insert_vertex(label_id, id_str, &info.props, ts)
    } else {
        let id_str = vid.to_string();
        ctx.insert_vertex(label_id, &id_str, &info.props, ts)
    };
    let final_result = match result {
        Ok(_) => {
            update_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &info.vertex_id,
                &info.tag_name,
                &info.props,
                ts,
            )?;
            Ok(true)
        }
        Err(ref e)
            if e.kind() == crate::core::error::storage::StorageErrorKind::VertexAlreadyExists =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    };
    ctx.version_manager().release_insert_timestamp(ts);
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

    let ts = ctx.get_write_timestamp();

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
    let result = ctx.insert_edge(InsertEdgeParams {
        edge_label: edge_label_id,
        src_label: src_label_id,
        src_id: src_vid,
        dst_label: dst_label_id,
        dst_id: dst_vid,
        rank: info.rank,
        properties: &info.props,
        ts,
    });

    let final_result = match result {
        Ok(_) => Ok(true),
        Err(ref e)
            if e.kind() == crate::core::error::storage::StorageErrorKind::EdgeAlreadyExists =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    };
    ctx.version_manager().release_insert_timestamp(ts);
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
    let ts = ctx.get_write_timestamp();
    let mut deleted = false;

    for tag in tags {
        let label_id = tag.tag_id;
        if ctx.delete_vertex(label_id, vertex_id, ts).is_ok() {
            delete_vertex_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &Value::String(vertex_id.to_string()),
                &tag.tag_name,
                ts,
            )?;
            deleted = true;
        }
    }

    ctx.version_manager().release_insert_timestamp(ts);

    Ok(deleted)
}

pub(crate) fn delete_edge_data(
    ctx: &GraphStorageContext,
    space: &str,
    src: &str,
    dst: &str,
    rank: i64,
) -> StorageResult<bool> {
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    let ts = ctx.get_write_timestamp();
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
            .is_ok()
        {
            deleted = true;
        }
    }

    ctx.version_manager().release_insert_timestamp(ts);

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

    let ts = ctx.get_write_timestamp();

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
        ctx.version_manager().release_insert_timestamp(ts);
        Ok(true)
    } else {
        ctx.version_manager().release_insert_timestamp(ts);
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
    ts: u32,
) -> StorageResult<()> {
    let indexes = index_metadata_manager.list_tag_indexes(space_id)?;
    for index in indexes {
        if index.schema_name == tag_name {
            // Check unique constraint before inserting.
            // A unique index must not have an existing entry with the same
            // property value for a different vertex.
            if index.is_unique {
                let index_data = ctx.index_data_manager();
                for (_prop_name, prop_value) in props {
                    let existing = index_data
                        .read()
                        .lookup_tag_index(space_id, &index, prop_value)?;
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
    ts: u32,
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
    ts: u32,
) -> StorageResult<()> {
    let index_names = tag_index_names(index_metadata_manager, space_id, tag_name)?;
    if !index_names.is_empty() {
        ctx.delete_vertex_indexes_mvcc(space_id, vertex_id, &index_names, ts)?;
    }
    Ok(())
}
