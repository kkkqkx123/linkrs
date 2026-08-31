use std::collections::HashMap;

use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    serialize_properties, value_to_string, vertex_record_to_vertex,
};
use graphdb_core::types::{TagInfo, VertexId};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::{StorageError, StorageResult, Value, Vertex};

use crate::engine::graph_storage::reader::utils::*;

pub(crate) fn get_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
) -> StorageResult<Option<Vertex>> {
    get_vertex_impl(ctx, space, id, None)
}

pub(crate) fn get_vertex_projected(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
    projection: &[String],
) -> StorageResult<Option<Vertex>> {
    get_vertex_impl(ctx, space, id, Some(projection))
}

fn get_vertex_impl(
    ctx: &GraphStorageContext,
    space: &str,
    id: &VertexId,
    projection: Option<&[String]>,
) -> StorageResult<Option<Vertex>> {
    record_vertex_read(ctx, *id);
    record_schema_read(ctx, space);
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
            match projection {
                Some(proj) => ctx.get_vertex_by_i64_projected(label_id, id_int, proj, ts),
                None => ctx.get_vertex_by_i64(label_id, id_int, ts),
            }
        } else if let Some(id_str) = id.as_str() {
            match projection {
                Some(proj) => ctx.get_vertex_projected(label_id, id_str, proj, ts),
                None => ctx.get_vertex(label_id, id_str, ts),
            }
        } else {
            let id_str = id.to_string();
            match projection {
                Some(proj) => ctx.get_vertex_projected(label_id, &id_str, proj, ts),
                None => ctx.get_vertex(label_id, &id_str, ts),
            }
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
    record_schema_read(ctx, space);
    let tags = ctx.schema_manager().list_tags(space)?;
    let ts = ctx.get_read_timestamp();

    // Read per-tag in batches directly from vertex tables, merging by vertex ID.
    // This avoids the intermediate Vec<VertexRecord> allocation per tag that
    // ctx.scan_vertices() produces via table.scan(ts).collect().
    struct MergedVertex {
        vid: VertexId,
        internal_id: u32,
        tags: Vec<Tag>,
        properties: HashMap<String, Value>,
    }

    let mut merged: HashMap<VertexId, MergedVertex> = HashMap::new();

    const BATCH_SIZE: usize = 256;

    for tag in &tags {
        let tag_id = tag.tag_id;
        let tag_name = &tag.tag_name;
        // Lazily register the statement snapshot for this label.
        ctx.ensure_vertex_snapshot_registered(tag_id);
        ctx.data_store().with_vertex_tables(|tables| {
            if let Some(table) = tables.get(&tag_id) {
                let records = table.scan(ts);
                for chunk in records.chunks(BATCH_SIZE) {
                    for record in chunk {
                        record_vertex_read(ctx, record.vid);
                        let entry = merged.entry(record.vid).or_insert_with(|| MergedVertex {
                            vid: record.vid,
                            internal_id: record.internal_id,
                            tags: Vec::new(),
                            properties: HashMap::new(),
                        });
                        entry.internal_id = record.internal_id;
                        let props: HashMap<String, Value> =
                            record.properties.iter().cloned().collect();
                        entry.tags.push(Tag::new(tag_name.clone(), props.clone()));
                        entry.properties.extend(props);
                    }
                }
            }
        });
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
    record_schema_read(ctx, space);
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut vertices = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            record_vertex_read(ctx, record.vid);
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
    record_schema_read(ctx, space);
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let ts = ctx.get_read_timestamp();
    let mut vertices = Vec::new();

    let label_id = tag_info.tag_id;
    if let Some(iterator) = ctx.scan_vertices(label_id, ts) {
        for record in iterator {
            record_vertex_read(ctx, record.vid);
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

pub(crate) fn count_vertices_by_tag(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
) -> StorageResult<u64> {
    let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
        StorageError::not_found(format!("Tag {} not found in space {}", tag, space))
    })?;

    let count = ctx.data_store().with_vertex_tables(|vertex_tables| {
        vertex_tables
            .get(&tag_info.tag_id)
            .map(|t| t.total_count() as u64)
            .unwrap_or(0)
    });
    // Lazily register the statement snapshot for this label.
    ctx.ensure_vertex_snapshot_registered(tag_info.tag_id);
    Ok(count)
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
