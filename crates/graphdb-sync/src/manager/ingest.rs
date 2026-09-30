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
        let staged = self.expand_auto_embed_text(space_id, tag_name, properties, change_type)?;
        self.stage_intent(
            txn_id,
            OutboxPayload::Vertex {
                space_id,
                tag_name: tag_name.to_string(),
                vertex_id: vertex_id.clone(),
                properties: staged,
                change_type,
            },
        )?;
        Ok(())
    }

    #[cfg(not(feature = "vector"))]
    fn expand_auto_embed_text(
        &self,
        _space_id: u64,
        _tag_name: &str,
        properties: &[(String, Value)],
        _change_type: ChangeType,
    ) -> Result<Vec<(String, Value)>, SyncError> {
        Ok(properties.to_vec())
    }

    #[cfg(feature = "vector")]
    fn expand_auto_embed_text(
        &self,
        space_id: u64,
        tag_name: &str,
        properties: &[(String, Value)],
        change_type: ChangeType,
    ) -> Result<Vec<(String, Value)>, SyncError> {
        if !self.auto_embed_text() {
            return Ok(properties.to_vec());
        }
        if matches!(change_type, ChangeType::Delete) {
            return Ok(properties.to_vec());
        }
        let Some(coordinator) = self.vector_coordinator.as_ref() else {
            return Err(SyncError::VectorError(
                "auto_embed_text requires a configured vector coordinator".to_string(),
            ));
        };
        use std::collections::HashSet;
        let explicit_vectors: HashSet<&str> = properties
            .iter()
            .filter(|(_, value)| value.as_vector().is_some())
            .map(|(field, _)| field.as_str())
            .collect();
        let mut pending_fields = Vec::new();
        let mut pending_texts = Vec::new();
        for (field, value) in properties {
            if explicit_vectors.contains(field.as_str()) {
                continue;
            }
            let Some(text) = value.string_value() else {
                continue;
            };
            if !coordinator.index_exists(space_id, tag_name, field) {
                continue;
            }
            pending_fields.push(field.clone());
            pending_texts.push(text);
        }
        if pending_fields.is_empty() {
            return Ok(properties.to_vec());
        }
        #[cfg(not(feature = "embedding"))]
        {
            return Err(SyncError::VectorError(
                "auto_embed_text requires the embedding feature and service".to_string(),
            ));
        }
        #[cfg(feature = "embedding")]
        {
            if coordinator.embedding_service().is_none() {
                return Err(SyncError::VectorError(
                    "auto_embed_text requires a configured embedding service".to_string(),
                ));
            }
            let vectors = crate::runtime::block_on_ambient(coordinator.embed_texts(&pending_texts))
                .map_err(|error| SyncError::VectorError(error.to_string()))?
                .map_err(|error| SyncError::VectorError(error.to_string()))?;
            if vectors.len() != pending_fields.len() {
                return Err(SyncError::VectorError(
                    "embedding service returned mismatched vector count".to_string(),
                ));
            }
            let mut staged = properties.to_vec();
            for (field, vector) in pending_fields.into_iter().zip(vectors) {
                if let Some(info) = coordinator.index_info(space_id, tag_name, &field) {
                    let expected = info.config.vector_size;
                    if expected != 0 && vector.len() != expected {
                        return Err(SyncError::VectorError(format!(
                            "auto_embed dimension mismatch for {}.{}.{}: expected {} got {}",
                            space_id,
                            tag_name,
                            field,
                            expected,
                            vector.len()
                        )));
                    }
                }
                staged.push((field, Value::vector(vector)));
            }
            Ok(staged)
        }
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
