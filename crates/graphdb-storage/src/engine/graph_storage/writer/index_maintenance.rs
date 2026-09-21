use crate::index::traits::VertexIndexOps;
use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{Index, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

use super::super::context::GraphStorageContext;

pub(super) fn tag_index_names(
    index_metadata_manager: &graphdb_core::metadata::IndexManager,
    space_id: u64,
    tag_name: &str,
) -> StorageResult<Vec<String>> {
    Ok(index_metadata_manager
        .list_tag_indexes(space_id)?
        .into_iter()
        .filter(|index| index.schema_name == tag_name)
        .map(|index| index.name)
        .collect())
}

pub(super) fn update_vertex_indexes(
    ctx: &GraphStorageContext,
    index_metadata_manager: &graphdb_core::metadata::IndexManager,
    space_id: u64,
    vertex_id: &Value,
    tag_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    let indexes = index_metadata_manager.list_tag_indexes(space_id)?;
    update_vertex_indexes_with_list(ctx, &indexes, space_id, vertex_id, tag_name, props, ts)
}

pub(super) fn update_vertex_indexes_with_list(
    ctx: &GraphStorageContext,
    indexes: &[Index],
    space_id: u64,
    vertex_id: &Value,
    tag_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    for index in indexes {
        if index.schema_name != tag_name {
            continue;
        }
        // The index manager derives indexed fields and included columns from
        // the complete entity property set. Keeping both sets here is
        // important: included columns must not become index keys, and an
        // update must refresh their covering values as well.
        let indexed_props: Vec<(String, Value)> = index
            .fields
            .iter()
            .filter_map(|field| props.iter().find(|(name, _)| name == &field.name).cloned())
            .collect();
        let included_changed = index
            .properties
            .iter()
            .any(|name| props.iter().any(|(changed, _)| changed == name));
        if indexed_props.is_empty() && !included_changed {
            continue;
        }
        // Check unique constraint before inserting. The pending-aware lookup
        // reads unpublished index deltas in-memory instead of forcing a
        // generation publish per statement, which would defeat delta
        // accumulation during batch loads with unique indexes.
        if index.is_unique {
            let index_data = ctx.index_data_manager();
            for (_prop_name, prop_value) in &indexed_props {
                let existing = index_data
                    .read()
                    .lookup_tag_index_pending_aware(space_id, index, prop_value)?;
                if !existing.is_empty() && !existing.contains(vertex_id) {
                    return Err(StorageError::conflict(format!(
                        "Unique index '{}' violated: value {:?} already exists",
                        index.name, prop_value
                    )));
                }
            }
        }
        ctx.update_vertex_indexes_mvcc(space_id, vertex_id, &index.name, props, ts)?;
    }
    Ok(())
}

pub(super) fn refresh_vertex_indexes(
    ctx: &GraphStorageContext,
    index_metadata_manager: &graphdb_core::metadata::IndexManager,
    space_id: u64,
    vertex_id: &Value,
    tag_name: &str,
    props: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    let index_names = tag_index_names(index_metadata_manager, space_id, tag_name)?;
    if index_names.is_empty() {
        return Ok(());
    }

    ctx.delete_vertex_indexes_mvcc(space_id, vertex_id, &index_names, ts)?;
    update_vertex_indexes(
        ctx,
        index_metadata_manager,
        space_id,
        vertex_id,
        tag_name,
        props,
        ts,
    )
}

pub(super) fn delete_vertex_indexes(
    ctx: &GraphStorageContext,
    index_metadata_manager: &graphdb_core::metadata::IndexManager,
    space_id: u64,
    vertex_id: &Value,
    tag_name: &str,
    ts: Timestamp,
) -> StorageResult<()> {
    let index_names = tag_index_names(index_metadata_manager, space_id, tag_name)?;
    if !index_names.is_empty() {
        ctx.delete_vertex_indexes_mvcc(space_id, vertex_id, &index_names, ts)?;
    }
    Ok(())
}
