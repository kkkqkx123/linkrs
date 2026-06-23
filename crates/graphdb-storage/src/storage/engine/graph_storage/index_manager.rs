use crate::core::metadata::index_manager::IndexMetadataManager;
use crate::core::types::Index;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::VertexIndexOps;

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

    let ts = ctx.get_write_timestamp();
    for vertex in vertices {
        let props: Vec<(String, Value)> = vertex
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let vid_value = Value::from(vertex.vid);
        ctx.update_vertex_indexes_mvcc(space_id, &vid_value, &index.name, &props, ts)?;
    }

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
