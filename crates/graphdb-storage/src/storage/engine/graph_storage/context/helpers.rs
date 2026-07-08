use crate::core::types::{LabelId, Timestamp, VertexId};
use crate::storage::vertex::VertexTable;
use std::collections::HashMap;

pub fn resolve_internal_id(
    _ctx: &super::GraphStorageContext,
    vertex_tables: &HashMap<LabelId, VertexTable>,
    label: LabelId,
    id: VertexId,
    ts: Timestamp,
) -> Option<u32> {
    if let Some(int_id) = id.as_int64() {
        resolve_internal_id_from_i64(vertex_tables, label, int_id, ts)
    } else if let Some(str_id) = id.as_str() {
        resolve_internal_id_from_str(vertex_tables, label, str_id, ts)
    } else {
        None
    }
}

pub fn resolve_internal_id_any(
    vertex_tables: &HashMap<LabelId, VertexTable>,
    label: LabelId,
    id: VertexId,
) -> Option<u32> {
    if let Some(int_id) = id.as_int64() {
        if label == 0 {
            vertex_tables
                .values()
                .find_map(|t| t.get_internal_id_by_i64_raw(int_id))
        } else {
            vertex_tables
                .get(&label)?
                .get_internal_id_by_i64_raw(int_id)
        }
    } else if let Some(str_id) = id.as_str() {
        if label == 0 {
            vertex_tables
                .values()
                .find_map(|t| t.get_internal_id_raw(str_id))
        } else {
            vertex_tables.get(&label)?.get_internal_id_raw(str_id)
        }
    } else {
        None
    }
}

fn resolve_internal_id_from_i64(
    vertex_tables: &HashMap<LabelId, VertexTable>,
    label: LabelId,
    id: i64,
    ts: Timestamp,
) -> Option<u32> {
    if label == 0 {
        vertex_tables
            .values()
            .find_map(|t| t.get_internal_id_by_i64(id, ts))
    } else {
        vertex_tables.get(&label)?.get_internal_id_by_i64(id, ts)
    }
}

fn resolve_internal_id_from_str(
    vertex_tables: &HashMap<LabelId, VertexTable>,
    label: LabelId,
    id: &str,
    ts: Timestamp,
) -> Option<u32> {
    if label == 0 {
        vertex_tables
            .values()
            .find_map(|t| t.get_internal_id(id, ts))
    } else {
        vertex_tables.get(&label)?.get_internal_id(id, ts)
    }
}

pub fn resolve_internal_id_label(
    vertex_tables: &HashMap<LabelId, VertexTable>,
    id: &VertexId,
    ts: Timestamp,
) -> Option<LabelId> {
    if let Some(int_id) = id.as_int64() {
        vertex_tables
            .iter()
            .find_map(|(lbl, t)| t.get_internal_id_by_i64(int_id, ts).map(|_| *lbl))
    } else if let Some(str_id) = id.as_str() {
        vertex_tables
            .iter()
            .find_map(|(lbl, t)| t.get_internal_id(str_id, ts).map(|_| *lbl))
    } else {
        None
    }
}
