use crate::core::types::Timestamp;
use crate::core::{StorageResult, Value};
use crate::index::types::EdgeIdentity;
use crate::index::{EdgeIndexOps, VertexIndexOps};

use super::context::GraphStorageContext;

pub fn update_vertex_indexes_mvcc(
    ctx: &GraphStorageContext,
    space_id: u64,
    vertex_id: &Value,
    index_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    // Acquire the rebuild gate before the manager write lock. Rebuilds use
    // the same order, which keeps the snapshot/catch-up/publish protocol
    // free of a lock inversion.
    let rebuild_gate = ctx.index_data_manager().read().rebuild_gate();
    let _write_gate = rebuild_gate.read();
    ctx.index_data_manager()
        .write()
        .update_vertex_indexes_mvcc(space_id, vertex_id, index_name, props, ts)
}

pub fn delete_vertex_indexes_mvcc(
    ctx: &GraphStorageContext,
    space_id: u64,
    vertex_id: &Value,
    index_names: &[String],
    ts: Timestamp,
) -> StorageResult<()> {
    let rebuild_gate = ctx.index_data_manager().read().rebuild_gate();
    let _write_gate = rebuild_gate.read();
    ctx.index_data_manager().write().delete_vertex_indexes_mvcc(
        space_id,
        vertex_id,
        index_names,
        ts,
    )
}

pub fn update_edge_indexes_mvcc(
    ctx: &GraphStorageContext,
    edge: &EdgeIdentity<'_>,
    index_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    let rebuild_gate = ctx.index_data_manager().read().rebuild_gate();
    let _write_gate = rebuild_gate.read();
    ctx.index_data_manager()
        .write()
        .update_edge_indexes_mvcc(edge, index_name, props, ts)
}

pub fn delete_edge_indexes_mvcc(
    ctx: &GraphStorageContext,
    edge: &EdgeIdentity<'_>,
    index_names: &[String],
    ts: Timestamp,
) -> StorageResult<()> {
    let rebuild_gate = ctx.index_data_manager().read().rebuild_gate();
    let _write_gate = rebuild_gate.read();
    ctx.index_data_manager()
        .write()
        .delete_edge_indexes_mvcc(edge, index_names, ts)
}
