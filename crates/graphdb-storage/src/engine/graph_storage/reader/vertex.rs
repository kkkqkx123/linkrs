use std::collections::HashMap;

use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    route_vertex_id, serialize_properties, vertex_record_to_vertex, RoutedVertexId,
};
use graphdb_core::types::{TagInfo, VertexId};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::{StorageError, StorageResult, Value, Vertex};

use crate::engine::graph_storage::reader::utils::*;

pub(crate) fn get_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    id: &VertexId,
) -> StorageResult<Option<Vertex>> {
    get_vertex_impl(ctx, space, tag, id, None)
}

pub(crate) fn get_vertex_projected(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    id: &VertexId,
    projection: &[String],
) -> StorageResult<Option<Vertex>> {
    get_vertex_impl(ctx, space, tag, id, Some(projection))
}

fn get_vertex_impl(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    id: &VertexId,
    projection: Option<&[String]>,
) -> StorageResult<Option<Vertex>> {
    record_vertex_read(ctx, *id);
    record_schema_read(ctx, space);
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let id = VertexId::normalize_for_vid_type(&space_info.vid_type, *id)?;
    let routed = route_vertex_id(&id)?;

    let tag_info = ctx
        .schema_manager()
        .get_tag(space, tag)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag)))?;
    let label_id = tag_info.tag_id;

    let ts = ctx.get_read_timestamp();
    let record = match &routed {
        RoutedVertexId::Int(id_int) => match projection {
            Some(proj) => ctx.get_vertex_by_i64_projected(label_id, *id_int, proj, ts),
            None => ctx.get_vertex_by_i64(label_id, *id_int, ts),
        },
        RoutedVertexId::Text(id_str) => match projection {
            Some(proj) => ctx.get_vertex_projected(label_id, id_str, proj, ts),
            None => ctx.get_vertex(label_id, id_str, ts),
        },
    };

    Ok(record.map(|record| {
        let props: HashMap<String, Value> = record.properties.iter().cloned().collect();
        Vertex::new(id, Tag::new(tag.to_string(), props))
    }))
}

pub(crate) fn scan_vertices(ctx: &GraphStorageContext, space: &str) -> StorageResult<Vec<Vertex>> {
    record_schema_read(ctx, space);
    let tags = ctx.schema_manager().list_tags(space)?;
    let ts = ctx.get_read_timestamp();

    // Single-label scan: every table row yields its own single-tag vertex.
    // The same id string stored under two labels denotes two independent
    // vertices and is never merged. Per-row visibility handling is unchanged.
    let mut out = Vec::new();

    const BATCH_SIZE: usize = 256;

    for tag in &tags {
        let tag_id = tag.tag_id;
        let tag_name = &tag.tag_name;
        let gate = ctx.pending_gate();
        ctx.data_store().with_vertex_tables(|tables| {
            if let Some(table) = tables.get(&tag_id) {
                let records = table.scan_shard_inconsistent(ts);
                for chunk in records.chunks(BATCH_SIZE) {
                    for record in chunk {
                        let (create_ts, delete_ts) = match table.row_timestamps(record.internal_id)
                        {
                            Some(stamps) => stamps,
                            None => continue,
                        };
                        if !gate.is_row_visible(ts, create_ts, delete_ts) {
                            continue;
                        }
                        let starts = table.row_picked_starts(record.internal_id, ts);
                        let record = if starts
                            .iter()
                            .any(|stamp| gate.is_foreign_pending(ts, *stamp))
                        {
                            match crate::engine::graph_storage::context::GraphStorageContext::resolve_on_table(
                                table, record.internal_id, ts, &gate,
                            ) {
                                Some((resolved, _, _, _)) => resolved,
                                None => continue,
                            }
                        } else {
                            record.clone()
                        };
                        record_vertex_read(ctx, record.vid);
                        let props: HashMap<String, Value> =
                            record.properties.iter().cloned().collect();
                        out.push(Vertex::new(
                            record.vid,
                            Tag::new(tag_name.clone(), props),
                        ));
                    }
                }
            }
        });
    }

    Ok(out)
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

    let ts = ctx.get_read_timestamp();
    let count = ctx.data_store().with_vertex_tables(|vertex_tables| {
        vertex_tables
            .get(&tag_info.tag_id)
            .map(|t| t.approximate_id_hole_stats(ts).0 as u64)
            .unwrap_or(0)
    });
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
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let ts = ctx.get_read_timestamp();
    let vid =
        VertexId::try_from(id).map_err(|error| StorageError::invalid_input(error.to_string()))?;
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vid)?;

    let label_id = tag_info.tag_id;
    let record = match route_vertex_id(&vid)? {
        RoutedVertexId::Int(id_int) => ctx.get_vertex_by_i64(label_id, id_int, ts),
        RoutedVertexId::Text(id_str) => ctx.get_vertex(label_id, &id_str, ts),
    };
    if let Some(record) = record {
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
