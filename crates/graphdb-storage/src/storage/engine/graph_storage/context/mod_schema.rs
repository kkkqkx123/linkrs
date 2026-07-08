use crate::core::StorageResult;
use crate::core::types::LabelId;
use crate::storage::engine::params::CreateEdgeTypeParams;
use crate::storage::edge::EdgeStrategy;
use crate::storage::types::StoragePropertyDef;

use super::GraphStorageContext;

impl GraphStorageContext {
    pub fn create_vertex_type(
        &self,
        name: &str,
        properties: Vec<StoragePropertyDef>,
        primary_key: &str,
    ) -> StorageResult<LabelId> {
        super::super::schema_engine::create_vertex_type(self, name, properties, primary_key)
    }

    pub fn create_vertex_type_with_id(
        &self,
        storage_name: &str,
        user_name: &str,
        label_id: LabelId,
        properties: Vec<StoragePropertyDef>,
        primary_key: &str,
    ) -> StorageResult<LabelId> {
        super::super::schema_engine::create_vertex_type_with_id(
            self,
            storage_name,
            user_name,
            label_id,
            properties,
            primary_key,
        )
    }

    pub fn create_edge_type(
        &self,
        name: &str,
        src_label: LabelId,
        dst_label: LabelId,
        properties: Vec<StoragePropertyDef>,
        oe_strategy: EdgeStrategy,
        ie_strategy: EdgeStrategy,
    ) -> StorageResult<LabelId> {
        super::super::schema_engine::create_edge_type(
            self,
            name,
            src_label,
            dst_label,
            properties,
            oe_strategy,
            ie_strategy,
        )
    }

    pub fn create_edge_type_with_id(
        &self,
        params: CreateEdgeTypeParams,
        label_id: LabelId,
    ) -> StorageResult<LabelId> {
        super::super::schema_engine::create_edge_type_with_id(self, params, label_id)
    }

    pub fn drop_vertex_type(&self, name: &str) -> StorageResult<()> {
        super::super::schema_engine::drop_vertex_type(self, name)
    }

    pub fn drop_edge_type(&self, name: &str) -> StorageResult<()> {
        super::super::schema_engine::drop_edge_type(self, name)
    }

    pub fn add_vertex_property(
        &self,
        label: LabelId,
        prop: StoragePropertyDef,
    ) -> StorageResult<()> {
        super::super::schema_engine::add_vertex_property(self, label, prop)
    }

    pub fn delete_vertex_property(&self, label: LabelId, prop_name: &str) -> StorageResult<()> {
        super::super::schema_engine::delete_vertex_property(self, label, prop_name)
    }

    pub fn rename_vertex_property(
        &self,
        label: LabelId,
        old_name: &str,
        new_name: &str,
    ) -> StorageResult<()> {
        super::super::schema_engine::rename_vertex_property(self, label, old_name, new_name)
    }

    pub fn add_edge_property(
        &self,
        edge_label: LabelId,
        prop: StoragePropertyDef,
    ) -> StorageResult<()> {
        super::super::schema_engine::add_edge_property(self, edge_label, prop)
    }

    pub fn delete_edge_property(&self, edge_label: LabelId, prop_name: &str) -> StorageResult<()> {
        super::super::schema_engine::delete_edge_property(self, edge_label, prop_name)
    }

    pub fn rename_edge_property(
        &self,
        edge_label: LabelId,
        old_name: &str,
        new_name: &str,
    ) -> StorageResult<()> {
        super::super::schema_engine::rename_edge_property(self, edge_label, old_name, new_name)
    }
}
