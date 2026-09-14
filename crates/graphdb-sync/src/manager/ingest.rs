//! Graph mutation entry points translating storage callbacks into staged intents.
use super::types::{EdgeProps, EdgeRef, IndexCreateRequest};
use super::*;
use crate::outbox::OutboxPayload;
use crate::types::ChangeType;
use graphdb_core::types::TransactionId;
use graphdb_core::Value;
#[cfg_attr(
    not(any(feature = "fulltext", feature = "vector")),
    allow(unused_variables)
)]
impl super::SyncManager {
    pub fn on_vertex_change_with_txn(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        tag_name: &str,
        vertex_id: &Value,
        properties: &[(String, Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::Vertex {
                space_id,
                tag_name: tag_name.to_string(),
                vertex_id: vertex_id.clone(),
                properties: properties.to_vec(),
                change_type,
            },
        )?;
        Ok(())
    }

    pub fn on_index_create(
        &self,
        txn_id: TransactionId,
        request: IndexCreateRequest,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::CreateIndex {
                space_id: request.space_id,
                index_name: request.index_name,
                schema_name: request.schema_name,
                index_type: request.index_type,
                fields: request.fields,
                properties: request.properties,
            },
        )?;
        Ok(())
    }

    pub fn on_index_drop(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        index_name: &str,
        schema_name: &str,
        index_type: &str,
        fields: &[String],
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::DropIndex {
                space_id,
                index_name: index_name.to_string(),
                schema_name: schema_name.to_string(),
                index_type: index_type.to_string(),
                fields: fields.to_vec(),
            },
        )?;
        Ok(())
    }

    pub fn on_space_drop(&self, txn_id: TransactionId, space_id: u64) -> Result<(), SyncError> {
        self.stage_intent(txn_id, OutboxPayload::DropSpace { space_id })?;
        Ok(())
    }

    pub fn on_tag_drop(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        tag_name: &str,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::DropTag {
                space_id,
                tag_name: tag_name.to_string(),
            },
        )?;
        Ok(())
    }

    pub fn on_edge_insert(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        edge: &graphdb_core::Edge,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::EdgeInsert {
                space_id,
                edge: edge.clone(),
            },
        )?;
        Ok(())
    }

    pub fn on_edge_delete(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        src: &Value,
        dst: &Value,
        edge_type: &str,
        ranking: i64,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::EdgeDelete {
                space_id,
                src: src.clone(),
                dst: dst.clone(),
                edge_type: edge_type.to_string(),
                ranking,
            },
        )?;
        Ok(())
    }

    pub fn on_edge_update(
        &self,
        _txn_id: TransactionId,
        _space_id: u64,
        _edge: EdgeRef<'_>,
        _props: EdgeProps<'_>,
    ) -> Result<(), SyncError> {
        Ok(())
    }
}
