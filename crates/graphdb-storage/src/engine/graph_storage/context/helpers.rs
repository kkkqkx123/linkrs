use std::collections::HashMap;
use std::sync::Arc;

use crate::vertex::{IdKey, ShardedVertexTable};
use graphdb_core::types::{LabelId, Timestamp, VertexId};

use super::vertex_ops::id_key_of;
use super::GraphStorageContext;

/// The `(label, reserved id)` binding of a row the active transaction
/// staged for insert, answered before any global lookup so an edge write in
/// the same transaction can resolve an endpoint whose row only exists in
/// this transaction's staging. A `label` of 0 scans all label tables.
fn staged_insert_binding(
    ctx: &GraphStorageContext,
    vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
    label: LabelId,
    key: &IdKey,
) -> Option<(LabelId, u32)> {
    let buffer = ctx.active_txn_staging()?;
    let buffer = buffer.lock();
    if label != 0 {
        buffer
            .pending_insert_id(label, key)
            .map(|reserved| (label, reserved))
    } else {
        vertex_tables.keys().find_map(|lbl| {
            buffer
                .pending_insert_id(*lbl, key)
                .map(|reserved| (*lbl, reserved))
        })
    }
}

pub fn resolve_internal_id(
    ctx: &GraphStorageContext,
    vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
    label: LabelId,
    id: VertexId,
    ts: Timestamp,
) -> Option<u32> {
    // A row this transaction staged for deletion stops resolving as soon as
    // the delete is buffered, well before the tombstone lands in the table.
    let key = id_key_of(&id);
    if let Some(key) = &key {
        if label == 0 {
            if vertex_tables
                .keys()
                .any(|lbl| ctx.staged_delete_vetoes(*lbl, key))
            {
                return None;
            }
        } else if ctx.staged_delete_vetoes(label, key) {
            return None;
        }
        // A staged insert resolves to its reserved global id.
        if let Some((_, reserved)) = staged_insert_binding(ctx, vertex_tables, label, key) {
            return Some(reserved);
        }
    }
    if let Some(int_id) = id.as_int64() {
        resolve_internal_id_from_i64(vertex_tables, label, int_id, ts)
    } else if let Some(str_id) = id.as_str() {
        resolve_internal_id_from_str(vertex_tables, label, str_id, ts)
    } else {
        None
    }
}

pub fn resolve_internal_id_any(
    ctx: &GraphStorageContext,
    vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
    label: LabelId,
    id: VertexId,
) -> Option<u32> {
    if let Some(key) = id_key_of(&id) {
        if let Some((_, reserved)) = staged_insert_binding(ctx, vertex_tables, label, &key) {
            return Some(reserved);
        }
    }
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
    vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
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
    vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
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
    ctx: &GraphStorageContext,
    vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
    id: &VertexId,
    ts: Timestamp,
) -> Option<LabelId> {
    if let Some(key) = id_key_of(id) {
        if let Some((label, _)) = staged_insert_binding(ctx, vertex_tables, 0, &key) {
            return Some(label);
        }
    }
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
