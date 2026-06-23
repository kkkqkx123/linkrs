use std::collections::HashMap;

use crate::core::types::VertexId;
use crate::core::types::{EdgeTypeInfo, LabelId, TagInfo};
use crate::core::vertex_edge_path::Tag;
use crate::core::{Edge, EdgeDirection, StorageError, StorageResult, Value, Vertex};
use crate::storage::engine::params::EdgeOperationParams;

use super::context::GraphStorageContext;
use super::ops::{
    edge_record_to_edge, endpoint_label_id, serialize_properties, value_to_string,
    vertex_record_to_vertex,
};

/// Convert a VertexId to its external string representation.
/// For string IDs, returns the raw string without quotes.
/// For integer IDs, returns the integer as a string.
fn vid_to_string(vid: &VertexId) -> String {
    if let Some(s) = vid.as_str() {
        s.to_string()
    } else if let Some(i) = vid.as_int64() {
        i.to_string()
    } else {
        format!("{:?}", vid.as_bytes())
    }
}

pub(crate) fn get_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
) -> StorageResult<Option<Vertex>> {
    let _space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let tags = ctx.schema_manager().list_tags(space)?;
    if tags.is_empty() {
        return Ok(None);
    }

    let ts = ctx.get_read_timestamp();
    let mut all_tags: Vec<Tag> = Vec::new();
    let mut merged_properties: HashMap<String, Value> = HashMap::new();
    let mut internal_id = 0u32;

    for tag in &tags {
        let label_id = tag.tag_id;
        let record = if let Some(id_int) = id.as_int64() {
            ctx.get_vertex_by_i64(label_id, id_int, ts)
        } else if let Some(id_str) = id.as_str() {
            ctx.get_vertex(label_id, id_str, ts)
        } else {
            let id_str = id.to_string();
            ctx.get_vertex(label_id, &id_str, ts)
        };

        if let Some(record) = record {
            internal_id = record.internal_id;
            let props: HashMap<String, Value> = record.properties.iter().cloned().collect();
            all_tags.push(Tag::new(tag.tag_name.clone(), props.clone()));
            merged_properties.extend(props);
        }
    }

    if all_tags.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Vertex {
            vid: *id,
            id: internal_id as i64,
            tags: all_tags,
            properties: merged_properties,
        }))
    }
}

pub(crate) fn scan_vertices(ctx: &GraphStorageContext, space: &str) -> StorageResult<Vec<Vertex>> {
    let tags = ctx.schema_manager().list_tags(space)?;
    let ts = ctx.get_read_timestamp();

    // Group records by vertex ID to merge multi-tag vertices
    struct MergedVertex {
        vid: VertexId,
        internal_id: u32,
        tags: Vec<Tag>,
        properties: HashMap<String, Value>,
    }

    let mut merged: HashMap<VertexId, MergedVertex> = HashMap::new();

    for tag in &tags {
        if let Some(iterator) = ctx.scan_vertices(tag.tag_id, ts) {
            for record in iterator {
                let entry = merged.entry(record.vid).or_insert(MergedVertex {
                    vid: record.vid,
                    internal_id: record.internal_id,
                    tags: Vec::new(),
                    properties: HashMap::new(),
                });
                entry.internal_id = record.internal_id;
                let props: HashMap<String, Value> = record.properties.iter().cloned().collect();
                entry
                    .tags
                    .push(Tag::new(tag.tag_name.clone(), props.clone()));
                entry.properties.extend(props);
            }
        }
    }

    Ok(merged
        .into_values()
        .map(|mv| Vertex {
            vid: mv.vid,
            id: mv.internal_id as i64,
            tags: mv.tags,
            properties: mv.properties,
        })
        .collect())
}

pub(crate) fn scan_vertices_by_tag(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
) -> StorageResult<Vec<Vertex>> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut vertices = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            let vertex = vertex_record_to_vertex(&record, tag);
            vertices.push(vertex);
        }
    }

    Ok(vertices)
}

pub(crate) fn scan_vertices_by_prop(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    prop: &str,
    value: &Value,
) -> StorageResult<Vec<Vertex>> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut vertices = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            if record
                .properties
                .iter()
                .any(|(k, v)| k == prop && v == value)
            {
                let vertex = vertex_record_to_vertex(&record, tag);
                vertices.push(vertex);
            }
        }
    }

    Ok(vertices)
}

pub(crate) fn get_edge(
    ctx: &GraphStorageContext,
    space: &str,
    src: &VertexId,
    dst: &VertexId,
    edge_type: &str,
    rank: i64,
) -> StorageResult<Option<Edge>> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let ts = ctx.get_read_timestamp();

    let edge_label_id = edge_info.edge_type_id;
    let src_label_id = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    let dst_label_id = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    let src_str = src.to_string();
    let dst_str = dst.to_string();

    if let Some(record) = ctx.get_edge(
        &EdgeOperationParams {
            edge_label: edge_label_id,
            src_label: src_label_id,
            src_id: *src,
            dst_label: dst_label_id,
            dst_id: *dst,
            rank,
        },
        ts,
    ) {
        let edge = edge_record_to_edge(&record, edge_type, &src_str, &dst_str);
        return Ok(Some(edge));
    }

    Ok(None)
}

pub(crate) fn get_node_edges(
    ctx: &GraphStorageContext,
    space: &str,
    node_id: &VertexId,
    direction: EdgeDirection,
) -> StorageResult<Vec<Edge>> {
    let edge_types = ctx.schema_manager().list_edge_types(space)?;
    if edge_types.is_empty() {
        return Ok(Vec::new());
    }

    let ts = ctx.get_read_timestamp();
    let node_str = vid_to_string(node_id);
    let mut edges = Vec::new();

    for edge_info in &edge_types {
        let edge_label_id = edge_info.edge_type_id;
        let edge_type_name = &edge_info.edge_type_name;

        let src_label_id = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
            Some(id) => id,
            None => continue,
        };
        let dst_label_id = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
            Some(id) => id,
            None => continue,
        };

        match direction {
            EdgeDirection::Out => {
                if let Some(out_edges) =
                    ctx.out_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in out_edges {
                        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;
                        let dst_external = if dst_label_id != 0 {
                            ctx.get_external_id(dst_label_id, dst_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(dst_label_id, dst_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        } else {
                            ctx.get_external_id_any(dst_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &node_str, &dst_external);
                        edges.push(edge);
                    }
                }
            }
            EdgeDirection::In => {
                if let Some(in_edges) =
                    ctx.in_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in in_edges {
                        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
                        let src_external = if src_label_id != 0 {
                            ctx.get_external_id(src_label_id, src_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(src_label_id, src_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        } else {
                            ctx.get_external_id_any(src_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &src_external, &node_str);
                        edges.push(edge);
                    }
                }
            }
            EdgeDirection::Both => {
                if let Some(out_edges) =
                    ctx.out_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in out_edges {
                        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;
                        let dst_external = if dst_label_id != 0 {
                            ctx.get_external_id(dst_label_id, dst_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(dst_label_id, dst_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        } else {
                            ctx.get_external_id_any(dst_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.dst_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &node_str, &dst_external);
                        edges.push(edge);
                    }
                }
                if let Some(in_edges) =
                    ctx.in_edges(edge_label_id, src_label_id, dst_label_id, *node_id, ts)
                {
                    for record in in_edges {
                        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
                        let src_external = if src_label_id != 0 {
                            ctx.get_external_id(src_label_id, src_internal, ts)
                                .or_else(|| {
                                    ctx.get_external_id_by_internal_id(src_label_id, src_internal)
                                        .map(|v| vid_to_string(&v))
                                })
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        } else {
                            ctx.get_external_id_any(src_internal, ts)
                                .unwrap_or_else(|| vid_to_string(&record.src_vid))
                        };

                        let edge =
                            edge_record_to_edge(&record, edge_type_name, &src_external, &node_str);
                        edges.push(edge);
                    }
                }
            }
        }
    }

    Ok(edges)
}

pub(crate) fn scan_edges_by_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<Vec<Edge>> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let ts = ctx.get_read_timestamp();
    let mut edges = Vec::new();

    let edge_label_id = edge_info.edge_type_id;

    let src_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(edges),
    };
    let dst_label_id: LabelId = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(edges),
    };

    // For unconstrained edge types (both tags empty), edges may be spread across
    // multiple edge tables (per-label tables from recent inserts + the original
    // (0, 0, edge_label) table from legacy inserts). We iterate every edge table
    // for this edge type and resolve internal IDs using each table's known
    // src_label/dst_label. The (0, 0, edge_label) table mixes vertices from
    // different tables whose internal IDs may collide, so we fall back to
    // get_external_id_any for those (best-effort).
    if src_label_id == 0 && dst_label_id == 0 {
        let edge_tables = ctx.data_store().edge_tables().read();
        for table in edge_tables.values().filter(|t| t.label() == edge_label_id) {
            let tbl_src = table.src_label();
            let tbl_dst = table.dst_label();
            for record in table.scan(ts) {
                let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
                let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;

                let src_external = if tbl_src != 0 {
                    ctx.get_external_id(tbl_src, src_internal, ts)
                        .or_else(|| {
                            ctx.get_external_id_by_internal_id(tbl_src, src_internal)
                                .map(|v| vid_to_string(&v))
                        })
                        .unwrap_or_else(|| format!("{}", record.src_vid))
                } else {
                    ctx.get_external_id_any(src_internal, ts)
                        .unwrap_or_else(|| format!("{}", record.src_vid))
                };

                let dst_external = if tbl_dst != 0 {
                    ctx.get_external_id(tbl_dst, dst_internal, ts)
                        .or_else(|| {
                            ctx.get_external_id_by_internal_id(tbl_dst, dst_internal)
                                .map(|v| vid_to_string(&v))
                        })
                        .unwrap_or_else(|| format!("{}", record.dst_vid))
                } else {
                    ctx.get_external_id_any(dst_internal, ts)
                        .unwrap_or_else(|| format!("{}", record.dst_vid))
                };

                let edge = edge_record_to_edge(&record, edge_type, &src_external, &dst_external);
                edges.push(edge);
            }
        }
        return Ok(edges);
    }

    let records = ctx.scan_edges(src_label_id, dst_label_id, edge_label_id, ts);

    for record in records {
        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;

        let src_external = ctx
            .get_external_id(src_label_id, src_internal, ts)
            .or_else(|| {
                ctx.get_external_id_by_internal_id(src_label_id, src_internal)
                    .map(|v| vid_to_string(&v))
            })
            .unwrap_or_else(|| format!("{}", record.src_vid));

        let dst_external = ctx
            .get_external_id(dst_label_id, dst_internal, ts)
            .or_else(|| {
                ctx.get_external_id_by_internal_id(dst_label_id, dst_internal)
                    .map(|v| vid_to_string(&v))
            })
            .unwrap_or_else(|| format!("{}", record.dst_vid));

        let edge = edge_record_to_edge(&record, edge_type, &src_external, &dst_external);
        edges.push(edge);
    }

    Ok(edges)
}

pub(crate) fn scan_all_edges(ctx: &GraphStorageContext, space: &str) -> StorageResult<Vec<Edge>> {
    let _space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let mut edges = Vec::new();
    let edge_types = ctx.schema_manager().list_edge_types(space)?;

    for et in edge_types {
        let type_edges = scan_edges_by_type(ctx, space, &et.edge_type_name)?;
        edges.extend(type_edges);
    }

    Ok(edges)
}

pub(crate) fn get_vertex_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    id: &Value,
) -> StorageResult<Option<(TagInfo, Vec<u8>)>> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let id_str = value_to_string(id);

    let label_id = tag_info.tag_id;
    if let Some(record) = ctx.get_vertex(label_id, &id_str, ts) {
        let data = serialize_properties(&record.properties);
        return Ok(Some((tag_info, data)));
    }

    Ok(None)
}

pub(crate) fn get_edge_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    src: &Value,
    dst: &Value,
) -> StorageResult<Option<(EdgeTypeInfo, Vec<u8>)>> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let ts = ctx.get_read_timestamp();
    let src_vid = VertexId::try_from(src)?;
    let dst_vid = VertexId::try_from(dst)?;

    let edge_label_id = edge_info.edge_type_id;
    let src_label_id = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    let dst_label_id = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
        Some(id) => id,
        None => return Ok(None),
    };
    if let Some(record) = ctx.get_edge(
        &EdgeOperationParams {
            edge_label: edge_label_id,
            src_label: src_label_id,
            src_id: src_vid,
            dst_label: dst_label_id,
            dst_id: dst_vid,
            rank: 0,
        },
        ts,
    ) {
        let data = serialize_properties(&record.properties);
        return Ok(Some((edge_info, data)));
    }

    Ok(None)
}

pub(crate) fn scan_vertices_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
) -> StorageResult<Vec<(TagInfo, Vec<u8>)>> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut results = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            let data = serialize_properties(&record.properties);
            results.push((tag_info.clone(), data));
        }
    }

    Ok(results)
}

pub(crate) fn scan_edges_with_schema(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<Vec<(EdgeTypeInfo, Vec<u8>)>> {
    let edge_info = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| {
            StorageError::not_found(format!(
                "Edge type {} not found in space {}",
                edge_type, space
            ))
        })?;

    let ts = ctx.get_read_timestamp();
    let mut results = Vec::new();

    let edge_label_id = edge_info.edge_type_id;
    let src_label_id: LabelId;
    let dst_label_id: LabelId;

    if edge_info.src_tag_name.is_empty() || edge_info.dst_tag_name.is_empty() {
        let records = ctx.scan_edges_by_label(edge_label_id, ts);
        for record in records {
            let data = serialize_properties(&record.properties);
            results.push((edge_info.clone(), data));
        }
    } else {
        src_label_id = match endpoint_label_id(ctx, space, &edge_info.src_tag_name)? {
            Some(id) => id,
            None => return Ok(results),
        };
        dst_label_id = match endpoint_label_id(ctx, space, &edge_info.dst_tag_name)? {
            Some(id) => id,
            None => return Ok(results),
        };
        let records = ctx.scan_edges(src_label_id, dst_label_id, edge_label_id, ts);
        for record in records {
            let data = serialize_properties(&record.properties);
            results.push((edge_info.clone(), data));
        }
    }

    Ok(results)
}
