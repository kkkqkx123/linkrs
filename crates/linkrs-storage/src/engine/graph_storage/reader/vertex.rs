use std::collections::{HashMap, HashSet};

use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    route_vertex_id, serialize_properties, vertex_record_to_vertex, RoutedVertexId,
};
use linkrs_core::types::{TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{StorageError, StorageResult, Value, Vertex};

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
    projection: &[std::sync::Arc<str>],
) -> StorageResult<Option<Vertex>> {
    get_vertex_impl(ctx, space, tag, id, Some(projection))
}

fn get_vertex_impl(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    id: &VertexId,
    projection: Option<&[std::sync::Arc<str>]>,
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
        let props: HashMap<std::sync::Arc<str>, Value> =
            record.properties.iter().cloned().collect();
        Vertex::new(id, Tag::new(tag.to_string(), props))
    }))
}

pub(crate) fn scan_vertices(ctx: &GraphStorageContext, space: &str) -> StorageResult<Vec<Vertex>> {
    record_schema_read(ctx, space);
    let tags = ctx.schema_manager().list_tags(space)?;
    let ts = ctx.get_read_timestamp();

    // Single-label scan: every table row yields its own single-tag vertex.
    // The same id string stored under two labels denotes two independent
    // vertices and is never merged.
    let mut out = Vec::new();

    for tag in &tags {
        let Some(records) = ctx.scan_vertices(tag.tag_id, ts) else {
            continue;
        };
        for record in records {
            record_vertex_read(ctx, record.vid);
            let props: HashMap<std::sync::Arc<str>, Value> =
                record.properties.iter().cloned().collect();
            out.push(Vertex::new(
                record.vid,
                Tag::new(tag.tag_name.clone(), props),
            ));
        }
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
                .any(|(k, v)| *k.as_ref() == *prop && v == value)
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

pub(crate) fn get_vertices_batch(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    ids: &[VertexId],
) -> StorageResult<Vec<Option<Vertex>>> {
    get_vertices_batch_impl(ctx, space, tag, ids, None)
}

pub(crate) fn get_vertices_projected_batch(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    ids: &[VertexId],
    projection: &[std::sync::Arc<str>],
) -> StorageResult<Vec<Option<Vertex>>> {
    get_vertices_batch_impl(ctx, space, tag, ids, Some(projection))
}

fn get_vertices_batch_impl(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    ids: &[VertexId],
    projection: Option<&[std::sync::Arc<str>]>,
) -> StorageResult<Vec<Option<Vertex>>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    record_schema_read(ctx, space);
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let tag_info = ctx
        .schema_manager()
        .get_tag(space, tag)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag)))?;
    let label_id = tag_info.tag_id;
    let ts = ctx.get_read_timestamp();
    for id in ids {
        record_vertex_read(ctx, *id);
    }
    let staging = ctx.active_txn_staging();
    let mut normalized: Vec<Option<VertexId>> = Vec::with_capacity(ids.len());
    let mut keys: Vec<Option<crate::vertex::IdKey>> = Vec::with_capacity(ids.len());
    for id in ids {
        match VertexId::normalize_for_vid_type(&space_info.vid_type, *id) {
            Ok(vid) => match route_vertex_id(&vid) {
                Ok(_) => {
                    let key = match vid.as_int64() {
                        Some(i) => Some(crate::vertex::IdKey::Int(i)),
                        None => vid.as_str().map(|s| crate::vertex::IdKey::Text(s.to_string())),
                    };
                    normalized.push(Some(vid));
                    keys.push(key);
                }
                Err(_) => {
                    normalized.push(None);
                    keys.push(None);
                }
            },
            Err(_) => {
                normalized.push(None);
                keys.push(None);
            }
        }
    }
    let mut batchable: Vec<usize> = Vec::new();
    let mut staged: Vec<usize> = Vec::new();
    if let Some(buffer) = staging.as_ref() {
        let buffer = buffer.lock();
        for (idx, key) in keys.iter().enumerate() {
            let Some(key) = key else {
                continue;
            };
            if normalized[idx].is_none() {
                continue;
            }
            if buffer.has_pending_delete(label_id, key)
                || buffer.pending_update(label_id, key).is_some()
                || buffer.pending_property_deletes(label_id, key).is_some()
                || buffer.pending_insert_row(label_id, key).is_some()
            {
                staged.push(idx);
            } else {
                batchable.push(idx);
            }
        }
    } else {
        for (idx, key) in keys.iter().enumerate() {
            if normalized[idx].is_some() && key.is_some() {
                batchable.push(idx);
            }
        }
    }
    let mut out: Vec<Option<Vertex>> = vec![None; ids.len()];
    if !batchable.is_empty() {
        let guard = ctx.visibility_guard(ts);
        let internal_ids: Vec<Option<u32>> = ctx.data_store().with_vertex_tables(|tables| {
            let table = tables.get(&label_id)?;
            let mut resolved = Vec::with_capacity(batchable.len());
            for idx in &batchable {
                let key = keys.get(*idx)?.as_ref()?;
                let internal = match key {
                    crate::vertex::IdKey::Int(i) => table.get_internal_id_by_i64(*i, ts),
                    crate::vertex::IdKey::Text(s) => table.get_internal_id(s, ts),
                };
                resolved.push(internal);
            }
            Some(resolved)
        }).unwrap_or_default();
        if internal_ids.len() != batchable.len() {
            staged.extend(batchable.iter().copied());
            batchable.clear();
        }
        let mut batch_positions: Vec<usize> = Vec::new();
        let mut batch_internals: Vec<u32> = Vec::new();
        for (pos, internal) in batchable.iter().zip(internal_ids.iter()) {
            if let Some(internal) = internal {
                batch_positions.push(*pos);
                batch_internals.push(*internal);
            }
        }
        if !batch_internals.is_empty() {
            let records: Option<Vec<Option<crate::vertex::VertexRecord>>> =
                ctx.data_store().with_vertex_tables(|tables| {
                    let table = tables.get(&label_id)?;
                    table
                        .resolve_projected_batch(&batch_internals, &guard, projection)
                        .ok()
                });
            if let Some(records) = records {
                for (pos, record) in batch_positions.into_iter().zip(records.into_iter()) {
                    if let Some(record) = record {
                        let props: HashMap<std::sync::Arc<str>, Value> =
                            record.properties.iter().cloned().collect();
                        let vid = normalized[pos].expect("batchable has normalized vid");
                        out[pos] = Some(Vertex::new(vid, Tag::new(tag.to_string(), props)));
                    }
                }
            } else {
                staged.extend(batchable.iter().copied());
                batchable.clear();
            }
        }
    }
    let mut seen: HashSet<usize> = batchable.iter().copied().collect();
    let mut point_indices: Vec<usize> = Vec::with_capacity(staged.len());
    for idx in staged {
        if seen.insert(idx) {
            point_indices.push(idx);
        }
    }
    for (idx, vid) in normalized.iter().enumerate() {
        if vid.is_none() {
            continue;
        }
        if seen.insert(idx) {
            point_indices.push(idx);
        }
    }
    for idx in point_indices {
        let Some(vid) = normalized[idx] else {
            continue;
        };
        if out[idx].is_some() {
            continue;
        }
        let routed = match route_vertex_id(&vid) {
            Ok(routed) => routed,
            Err(_) => continue,
        };
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
        out[idx] = record.map(|record| {
            let props: HashMap<std::sync::Arc<str>, Value> =
                record.properties.iter().cloned().collect();
            Vertex::new(vid, Tag::new(tag.to_string(), props))
        });
    }
    Ok(out)
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
