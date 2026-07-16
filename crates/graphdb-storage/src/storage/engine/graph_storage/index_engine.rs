use crate::core::types::Timestamp;
use crate::core::{StorageResult, Value};
use crate::storage::index::{EdgeIndexOps, VertexIndexOps};

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

pub fn update_edge_indexes_mvcc(
    ctx: &GraphStorageContext,
    space_id: u64,
    edge_src: &Value,
    edge_dst: &Value,
    edge_type: &str,
    ranking: i64,
    index_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    ctx.index_data_manager().write().update_edge_indexes_mvcc(
        space_id, edge_src, edge_dst, edge_type, ranking, index_name, props, ts,
    )
}

pub fn delete_edge_indexes_mvcc(
    ctx: &GraphStorageContext,
    space_id: u64,
    edge_src: &Value,
    edge_dst: &Value,
    edge_type: &str,
    ranking: i64,
    index_names: &[String],
    ts: Timestamp,
) -> StorageResult<()> {
    ctx.index_data_manager().write().delete_edge_indexes_mvcc(
        space_id,
        edge_src,
        edge_dst,
        edge_type,
        ranking,
        index_names,
        ts,
    )
}
