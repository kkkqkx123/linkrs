use std::sync::Arc;

use crate::edge::EdgeStrategy;
use crate::engine::params::CreateEdgeTypeParams;
use crate::types::StoragePropertyDef;
use graphdb_core::event_dispatch::SubscriptionId;
use graphdb_core::types::LabelId;
use graphdb_core::StorageResult;

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

    /// Register a schema-change observer.
    ///
    /// `SchemaManager` and `IndexManager` share one registry, so a single
    /// registration receives both table/space DDL and index DDL. External
    /// subsystems (fulltext rebuild, vector sync, cache
    /// invalidation, monitoring) should subscribe here.
    pub fn register_schema_callback(
        &self,
        callback: graphdb_core::metadata::SchemaChangeCallback,
    ) -> SubscriptionId {
        self.schema_manager().register_schema_callback(callback)
    }

    /// Shared schema-event registry (single subscription receives both
    /// table/space DDL and index DDL). Used by the central `HookBus`.
    pub fn shared_schema_callbacks(
        &self,
    ) -> Arc<
        graphdb_core::event_dispatch::EventSubscriptions<graphdb_core::metadata::SchemaChangeEvent>,
    > {
        self.schema_manager().shared_schema_callbacks()
    }

    /// Remove a previously registered schema-change observer.
    pub fn unregister_schema_callback(&self, id: SubscriptionId) -> bool {
        self.schema_manager().unregister_schema_callback(id)
    }

    /// Number of registered schema-change observers.
    pub fn schema_callback_count(&self) -> usize {
        self.schema_manager().schema_callback_count()
    }
}
