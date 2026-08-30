use crate::engine::graph_storage::GraphStorageContext;
use crate::index::{EdgeIndexOps, VertexIndexOps};
use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{Index, Timestamp};
use graphdb_core::StorageResult;
use graphdb_transaction::wal::{
    CreateEdgeIndexRedo, CreateTagIndexRedo, DropEdgeIndexRedo, DropTagIndexRedo,
};

pub(crate) fn replay_create_tag_index(
    ctx: &GraphStorageContext,
    redo: &CreateTagIndexRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let index = Index::new(graphdb_core::types::IndexConfig {
        id: 0,
        name: redo.index_name.clone(),
        space_id: redo.space_id,
        schema_name: String::new(),
        fields: redo
            .fields
            .iter()
            .map(|(name, _typ)| {
                graphdb_core::types::IndexField::new(
                    name.clone(),
                    graphdb_core::Value::string(""),
                    true,
                )
            })
            .collect(),
        properties: redo.properties.clone(),
        index_type: graphdb_core::types::IndexType::TagIndex,
        is_unique: redo.is_unique,
        covering: false,
        partial_condition: None,
    });
    match ctx
        .index_metadata_manager()
        .create_tag_index(redo.space_id, &index)
    {
        Ok(_) => {}
        Err(e) => log::warn!("create_tag_index replay: {}", e),
    }
    let stored = ctx
        .index_metadata_manager()
        .get_tag_index(redo.space_id, &redo.index_name)?
        .unwrap_or(index);
    let data_mgr = ctx.index_data_manager().write();
    let _ = data_mgr.register_native_index(redo.space_id, &stored);
    Ok(())
}

pub(crate) fn replay_drop_tag_index(
    ctx: &GraphStorageContext,
    redo: &DropTagIndexRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let _ = ctx
        .index_metadata_manager()
        .drop_tag_index(redo.space_id, &redo.index_name);
    let data_mgr = ctx.index_data_manager().write();
    let _ = data_mgr.clear_tag_index(redo.space_id, &redo.index_name);
    data_mgr.unregister_native_index(redo.space_id, &redo.index_name);
    Ok(())
}

pub(crate) fn replay_create_edge_index(
    ctx: &GraphStorageContext,
    redo: &CreateEdgeIndexRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let index = Index::new(graphdb_core::types::IndexConfig {
        id: 0,
        name: redo.index_name.clone(),
        space_id: redo.space_id,
        schema_name: String::new(),
        fields: redo
            .fields
            .iter()
            .map(|(name, _typ)| {
                graphdb_core::types::IndexField::new(
                    name.clone(),
                    graphdb_core::Value::string(""),
                    true,
                )
            })
            .collect(),
        properties: redo.properties.clone(),
        index_type: graphdb_core::types::IndexType::EdgeIndex,
        is_unique: redo.is_unique,
        covering: false,
        partial_condition: None,
    });
    match ctx
        .index_metadata_manager()
        .create_edge_index(redo.space_id, &index)
    {
        Ok(_) => {}
        Err(e) => log::warn!("create_edge_index replay: {}", e),
    }
    let stored = ctx
        .index_metadata_manager()
        .get_edge_index(redo.space_id, &redo.index_name)?
        .unwrap_or(index);
    let data_mgr = ctx.index_data_manager().write();
    let _ = data_mgr.register_native_index(redo.space_id, &stored);
    Ok(())
}

pub(crate) fn replay_drop_edge_index(
    ctx: &GraphStorageContext,
    redo: &DropEdgeIndexRedo,
    _ts: Timestamp,
) -> StorageResult<()> {
    let _ = ctx
        .index_metadata_manager()
        .drop_edge_index(redo.space_id, &redo.index_name);
    let data_mgr = ctx.index_data_manager().write();
    let _ = data_mgr.clear_edge_index(redo.space_id, &redo.index_name);
    data_mgr.unregister_native_index(redo.space_id, &redo.index_name);
    Ok(())
}
