use crate::core::types::Timestamp;
use crate::core::{StorageResult, Value};
use crate::storage::index::VertexIndexOps;

use super::context::GraphStorageContext;

pub fn update_vertex_indexes_mvcc(
    ctx: &GraphStorageContext,
    space_id: u64,
    vertex_id: &Value,
    index_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
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
    ctx.index_data_manager().write().delete_vertex_indexes_mvcc(
        space_id,
        vertex_id,
        index_names,
        ts,
    )
}
