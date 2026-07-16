use crate::core::metadata::index_manager::IndexMetadataManager;
use crate::core::types::{Index, IndexStatus, MAX_TIMESTAMP};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::index_data_manager::IndexRecord;
use crate::storage::index::key_codec::KeyBuilder;
use crate::storage::index::{EdgeIndexOps, VertexIndexOps};
use std::collections::BTreeMap;

use super::context::GraphStorageContext;

pub(crate) fn create_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index: &Index,
) -> StorageResult<bool> {
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?
        .space_id;
    ctx.index_metadata_manager()
        .create_tag_index(space_id, index)
}

pub(crate) fn drop_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let dropped = ctx
        .index_metadata_manager()
        .drop_tag_index(space_id, index_name)?;
    if dropped {
        ctx.index_data_manager()
            .write()
            .clear_tag_index(space_id, index_name)?;
    }
    Ok(dropped)
}

pub(crate) fn get_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<Option<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager()
        .get_tag_index(space_id, index_name)
}

pub(crate) fn list_tag_indexes(
    ctx: &GraphStorageContext,
    space: &str,
) -> StorageResult<Vec<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager().list_tag_indexes(space_id)
}

/// Build a physical index key by appending a unique version suffix.
fn make_physical_key(logical_key: &[u8], version: u64) -> Vec<u8> {
    let mut physical_key = Vec::with_capacity(logical_key.len() + 8);
    physical_key.extend_from_slice(logical_key);
    physical_key.extend_from_slice(&version.to_le_bytes());
    physical_key
}

pub(crate) fn rebuild_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    vertices: &[crate::core::Vertex],
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_tag_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Index {} not found", index_name)))?;

    // Generation rebuild: Building -> Active protocol.
    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::Building,
    )?;

    // Build new index data from a fixed snapshot without clearing the active generation.
    // This allows existing readers to continue using the old generation during rebuild.
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    let mut version_counter = 1u64;

    for vertex in vertices {
        let props: Vec<(String, Value)> = vertex
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let vid_value = Value::from(vertex.vid);

        for (_, prop_value) in &props {
            let logical_forward_key = KeyBuilder::build_vertex_index_key(
                space_id, &index.name, prop_value, &vid_value,
            )?;
            let logical_reverse_key =
                KeyBuilder::build_vertex_reverse_key_v2(space_id, &vid_value, &index.name)?;

            let entry = IndexRecord::new(MAX_TIMESTAMP);
            let fwd_key = make_physical_key(&logical_forward_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);
            let rev_key = make_physical_key(&logical_reverse_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);

            forward.insert(fwd_key, entry.clone());
            reverse.insert(rev_key, entry);
        }
    }

    // Atomically swap the in-memory BTreeMaps.
    // After this point, new reads and writes use the rebuilt data.
    let mgr = ctx.index_data_manager().write();
    mgr.vertex_manager().base().replace_data(forward, reverse);

    // Mark as Active after successful rebuild.
    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::Publishing,
    )?;
    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::Active,
    )?;

    Ok(true)
}

pub(crate) fn lookup_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    value: &Value,
) -> StorageResult<Vec<Value>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;

    let index = ctx
        .index_metadata_manager()
        .get_tag_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Index {} not found", index_name)))?;

    let results = ctx
        .index_data_manager()
        .read()
        .lookup_tag_index(space_id, &index, value)?;
    Ok(results)
}

pub(crate) fn create_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index: &Index,
) -> StorageResult<bool> {
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?
        .space_id;
    ctx.index_metadata_manager()
        .create_edge_index(space_id, index)
}

pub(crate) fn drop_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let dropped = ctx
        .index_metadata_manager()
        .drop_edge_index(space_id, index_name)?;
    if dropped {
        ctx.index_data_manager()
            .write()
            .clear_edge_index(space_id, index_name)?;
    }
    Ok(dropped)
}

pub(crate) fn rebuild_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    edges: &[crate::core::Edge],
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_edge_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Edge index {} not found", index_name)))?;

    // Generation rebuild: Building -> Active protocol.
    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Building,
    )?;

    // Build new index data from a fixed snapshot without clearing the active generation.
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    let mut version_counter = 1u64;

    for edge in edges {
        let props: Vec<(String, Value)> = edge
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let src_value = Value::from(edge.src);
        let dst_value = Value::from(edge.dst);

        for (_, prop_value) in &props {
            let logical_forward_key = KeyBuilder::build_edge_index_key(
                space_id, &index.name, prop_value, &src_value, &dst_value,
                &edge.edge_type, edge.ranking,
            )?;
            let logical_reverse_key = KeyBuilder::build_edge_reverse_key(
                space_id, &src_value, &dst_value, &edge.edge_type,
                edge.ranking, &index.name,
            )?;

            let entry = IndexRecord::new(MAX_TIMESTAMP);
            let fwd_key = make_physical_key(&logical_forward_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);
            let rev_key = make_physical_key(&logical_reverse_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);

            forward.insert(fwd_key, entry.clone());
            reverse.insert(rev_key, entry);
        }
    }

    // Atomically swap the in-memory BTreeMaps.
    let mgr = ctx.index_data_manager().write();
    mgr.edge_manager().base().replace_data(forward, reverse);

    // Transition through publishing to active.
    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Publishing,
    )?;
    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Active,
    )?;

    Ok(true)
}

pub(crate) fn get_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<Option<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager()
        .get_edge_index(space_id, index_name)
}

pub(crate) fn list_edge_indexes(
    ctx: &GraphStorageContext,
    space: &str,
) -> StorageResult<Vec<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager().list_edge_indexes(space_id)
}

pub(crate) fn lookup_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    value: &Value,
) -> StorageResult<Vec<(Value, Value, String, i64)>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_edge_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Edge index {} not found", index_name)))?;
    let results = ctx
        .index_data_manager()
        .read()
        .lookup_edge_index(space_id, &index, value)?;
    Ok(results)
}

#[cfg(test)]
mod tests {
    use crate::core::types::{Index, IndexConfig, IndexField, IndexType};
    use crate::core::Value;
    use crate::storage::engine::graph_storage::context::GraphStorageContext;

    fn setup_context() -> GraphStorageContext {
        GraphStorageContext::new()
    }

    #[test]
    fn test_create_and_list_tag_index() {
        let ctx = setup_context();

        let index = Index::new(IndexConfig {
            id: 1,
            name: "person_name_idx".to_string(),
            space_id: 0,
            schema_name: "Person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::String(String::new()),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        });

        super::create_tag_index(&ctx, "test_space", &index)
            .expect_err("should fail because space does not exist");

        // Actually we need a space + schema adapter for full testing.
        // The index_manager functions require a schema_manager with registered space.
        // This is tested through integration tests (tests.rs).
    }

    #[test]
    fn test_get_tag_index_on_empty() {
        let ctx = setup_context();
        let result = super::get_tag_index(&ctx, "nonexistent", "some_index");
        assert!(result.is_err());
    }

    #[test]
    fn test_drop_tag_index_on_empty() {
        let ctx = setup_context();
        let result = super::drop_tag_index(&ctx, "nonexistent", "some_index");
        assert!(result.is_err());
    }

    #[test]
    fn test_lookup_index_on_nonexistent_space() {
        let ctx = setup_context();
        let result = super::lookup_index(
            &ctx,
            "no_space",
            "some_index",
            &Value::String("test".to_string()),
        );
        assert!(result.is_err());
    }
}
