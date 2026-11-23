use std::collections::{HashMap, HashSet};

use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    route_vertex_id, serialize_properties, vertex_record_to_vertex, RoutedVertexId,
};
use linkrs_core::types::{TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{StorageError, StorageResult, Value, Vertex};

use crate::engine::graph_storage::reader::ttl_filter::vertex_is_expired;
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

    Ok(record
        .map(|record| {
            let props: HashMap<std::sync::Arc<str>, Value> =
                record.properties.iter().cloned().collect();
            Vertex::new(id, Tag::new(tag.to_string(), props))
        })
        .filter(|vertex| !vertex_is_expired(&tag_info, vertex)))
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
            let vertex = Vertex::new(record.vid, Tag::new(tag.tag_name.clone(), props));
            if vertex_is_expired(tag, &vertex) {
                continue;
            }
            out.push(vertex);
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
            if vertex_is_expired(&tag_info, &vertex) {
                continue;
            }
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
                if vertex_is_expired(&tag_info, &vertex) {
                    continue;
                }
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

/// Exact visible-vertex count for pure `COUNT` queries.
///
/// Snapshot-exact counterpart of [`count_vertices_by_tag`]: when the table
/// is quiescent (no creation stamp past the read timestamp) the hole
/// statistics equal a full tag scan, so the count is O(shards). Otherwise
/// it enumerates the same guard-filtered live ids the scan cursor walks.
/// The hole count is always read before the quiescence fence: a commit
/// landing between the two trips the fence into the exact walk, while a
/// later commit cannot affect the read. A `None` tag sums every tag of the
/// space. Missing tags contribute zero, mirroring the cursor skipping
/// unknown tables. TTL is refused loudly: expired vertices are filtered
/// at decode time, which this decode-free count cannot observe. Callers
/// gate out write transactions beforehand, so no staged rows can hide
/// from this table walk.
pub(crate) fn count_visible_vertices(
    ctx: &GraphStorageContext,
    space: &str,
    tag: Option<&str>,
) -> StorageResult<u64> {
    let tag_names: Vec<String> = match tag {
        Some(name) => vec![name.to_string()],
        None => ctx
            .schema_manager()
            .list_tags(space)?
            .into_iter()
            .map(|info| info.tag_name)
            .collect(),
    };
    let mut label_ids = Vec::with_capacity(tag_names.len());
    for name in &tag_names {
        let Some(info) = ctx.schema_manager().get_tag(space, name)? else {
            continue;
        };
        if linkrs_core::types::ttl::has_valid_ttl(info.ttl_duration, info.ttl_col.as_deref()) {
            return Err(StorageError::not_supported(format!(
                "Metadata vertex count cannot serve tag with TTL configured: {name}"
            )));
        }
        label_ids.push(info.tag_id);
    }
    let ts = ctx.get_read_timestamp();
    let guard = ctx.visibility_guard(ts);
    let total = ctx.data_store().with_vertex_tables(|vertex_tables| {
        label_ids
            .iter()
            .filter_map(|id| vertex_tables.get(id))
            .map(|table| {
                let (live, _) = table.approximate_id_hole_stats(ts);
                if table.max_start_ts() <= ts {
                    live as u64
                } else {
                    table.live_ids(&guard).len() as u64
                }
            })
            .sum()
    });
    Ok(total)
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
                        None => vid
                            .as_str()
                            .map(|s| crate::vertex::IdKey::Text(s.to_string())),
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
        let internal_ids: Vec<Option<u32>> = ctx
            .data_store()
            .with_vertex_tables(|tables| {
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
            })
            .unwrap_or_default();
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
                for (pos, record) in batch_positions.into_iter().zip(records) {
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
    let out: Vec<Option<Vertex>> = out
        .into_iter()
        .map(|opt| opt.filter(|vertex| !vertex_is_expired(&tag_info, vertex)))
        .collect();
    Ok(out)
}

/// Batch property-column reads without constructing entity values.
///
/// Used by expand hops whose destination demand is a flat property column:
/// visibility and TTL still come from the same snapshot, but only the named
/// properties are decoded. An empty property list performs existence-only
/// reads and returns present vertices with an empty value vector.
pub(crate) fn get_vertex_property_columns_batch(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    ids: &[VertexId],
    properties: &[std::sync::Arc<str>],
) -> StorageResult<Vec<Option<Vec<Value>>>> {
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

    let extract = |record: &crate::vertex::VertexRecord| {
        properties
            .iter()
            .map(|name| {
                record
                    .properties
                    .iter()
                    .find(|(stored, _)| stored == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or(Value::Null(linkrs_core::NullType::Null))
            })
            .collect::<Vec<_>>()
    };
    let expired = |record: &crate::vertex::VertexRecord| {
        crate::engine::graph_storage::ttl::record_expired_from_slice(
            tag_info.ttl_duration,
            tag_info.ttl_col.as_deref(),
            &record.properties,
        )
    };

    let staging = ctx.active_txn_staging();
    let mut normalized: Vec<Option<VertexId>> = Vec::with_capacity(ids.len());
    let mut keys: Vec<Option<crate::vertex::IdKey>> = Vec::with_capacity(ids.len());
    for id in ids {
        match VertexId::normalize_for_vid_type(&space_info.vid_type, *id) {
            Ok(vid) => match route_vertex_id(&vid) {
                Ok(_) => {
                    let key = match vid.as_int64() {
                        Some(i) => Some(crate::vertex::IdKey::Int(i)),
                        None => vid
                            .as_str()
                            .map(|s| crate::vertex::IdKey::Text(s.to_string())),
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
            let Some(key) = key else { continue };
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

    let mut out: Vec<Option<Vec<Value>>> = vec![None; ids.len()];
    if !batchable.is_empty() {
        let guard = ctx.visibility_guard(ts);
        let internal_ids: Vec<Option<u32>> = ctx
            .data_store()
            .with_vertex_tables(|tables| {
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
            })
            .unwrap_or_default();
        if internal_ids.len() != batchable.len() {
            staged.extend(batchable.iter().copied());
            batchable.clear();
        }
        let mut positions = Vec::new();
        let mut internals = Vec::new();
        for (pos, internal) in batchable.iter().zip(internal_ids.iter()) {
            if let Some(internal) = internal {
                positions.push(*pos);
                internals.push(*internal);
            }
        }
        if !internals.is_empty() {
            let records: Option<Vec<Option<crate::vertex::VertexRecord>>> =
                ctx.data_store().with_vertex_tables(|tables| {
                    let table = tables.get(&label_id)?;
                    table
                        .resolve_projected_batch(&internals, &guard, Some(properties))
                        .ok()
                });
            if let Some(records) = records {
                for (pos, record) in positions.into_iter().zip(records) {
                    if let Some(record) = record {
                        if !expired(&record) {
                            out[pos] = Some(extract(&record));
                        }
                    }
                }
            } else {
                staged.extend(batchable.iter().copied());
                batchable.clear();
            }
        }
    }

    let mut seen: HashSet<usize> = batchable.iter().copied().collect();
    let mut point_indices = Vec::with_capacity(staged.len());
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
        let Some(vid) = normalized[idx] else { continue };
        if out[idx].is_some() {
            continue;
        }
        let record = match route_vertex_id(&vid)? {
            RoutedVertexId::Int(id_int) => {
                ctx.get_vertex_by_i64_projected(label_id, id_int, properties, ts)
            }
            RoutedVertexId::Text(id_str) => {
                ctx.get_vertex_projected(label_id, &id_str, properties, ts)
            }
        };
        if let Some(record) = record {
            if !expired(&record) {
                out[idx] = Some(extract(&record));
            }
        }
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
        if crate::engine::graph_storage::ttl::record_expired_from_slice(
            tag_info.ttl_duration,
            tag_info.ttl_col.as_deref(),
            &record.properties,
        ) {
            return Ok(None);
        }
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
            if crate::engine::graph_storage::ttl::record_expired_from_slice(
                tag_info.ttl_duration,
                tag_info.ttl_col.as_deref(),
                &record.properties,
            ) {
                continue;
            }
            let data = serialize_properties(&record.properties);
            results.push((tag_info.clone(), data));
        }
    }

    Ok(results)
}
