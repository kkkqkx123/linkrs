use graphdb_core::metadata::index_manager::IndexMetadataManager;
use graphdb_core::StorageResult;

use super::super::context::GraphStorageContext;
use super::edge_index::drop_edge_index;
use super::tag_index::drop_tag_index;

/// Cascade helper: drop every tag index bound to one tag, including runtime
/// state and checkpoint directories. Previously only metadata was removed.
pub(crate) fn drop_tag_indexes_by_tag_cascade(
    ctx: &GraphStorageContext,
    space: &str,
    tag_name: &str,
) -> StorageResult<usize> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let names: Vec<String> = ctx
        .index_metadata_manager()
        .list_tag_indexes(space_id)?
        .into_iter()
        .filter(|idx| idx.schema_name == tag_name)
        .map(|idx| idx.name)
        .collect();
    let mut removed = 0;
    for name in names {
        if drop_tag_index(ctx, space, &name)? {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Cascade helper: drop every edge index bound to one edge type.
pub(crate) fn drop_edge_indexes_by_type_cascade(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<usize> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let names: Vec<String> = ctx
        .index_metadata_manager()
        .list_edge_indexes(space_id)?
        .into_iter()
        .filter(|idx| idx.schema_name == edge_type)
        .map(|idx| idx.name)
        .collect();
    let mut removed = 0;
    for name in names {
        if drop_edge_index(ctx, space, &name)? {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Cascade helper: drop all tag and edge indexes of one space.
pub(crate) fn drop_space_indexes_cascade(
    ctx: &GraphStorageContext,
    space: &str,
) -> StorageResult<usize> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let mut removed = 0;
    for idx in ctx.index_metadata_manager().list_tag_indexes(space_id)? {
        if drop_tag_index(ctx, space, &idx.name)? {
            removed += 1;
        }
    }
    for idx in ctx.index_metadata_manager().list_edge_indexes(space_id)? {
        if drop_edge_index(ctx, space, &idx.name)? {
            removed += 1;
        }
    }
    Ok(removed)
}
