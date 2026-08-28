use crate::engine::data_store::EdgeTableKey;
use crate::engine::graph_storage::GraphStorageContext;
use crate::engine::transaction::{
    EdgeTypeLabelParams, RevertDeleteEdgeParams, TransactionOps, UpdateEdgePropertyUndoParams,
};
use graphdb_core::types::{
    ColumnId, EdgeDeletionContext, EdgeIdentifier, EdgeKey, LabelId, Timestamp, UndoLogError,
    UndoLogResult, UndoTarget, VertexIdentifier,
};

fn checked_internal_vertex_id(vid: &graphdb_core::types::VertexId) -> UndoLogResult<u32> {
    let value = vid.as_int64().ok_or_else(|| {
        UndoLogError::UndoFailed(format!(
            "Cannot encode non-integer vertex ID in an edge undo record: {vid}"
        ))
    })?;
    u32::try_from(value).map_err(|_| {
        UndoLogError::UndoFailed(format!(
            "Vertex ID {value} does not fit in the edge undo record"
        ))
    })
}

impl UndoTarget for GraphStorageContext {
    fn delete_vertex_type(&self, label: LabelId) -> UndoLogResult<()> {
        self.data_store()
            .drop_vertex_type_by_label(label)
            .map_err(|error| UndoLogError::UndoFailed(error.to_string()))?;
        self.mark_vertex_modified(label);
        Ok(())
    }

    fn delete_edge_type(&self, edge_key: EdgeKey) -> UndoLogResult<()> {
        self.data_store()
            .drop_edge_partition(crate::engine::data_store::EdgeTableKey::new(
                edge_key.src_label,
                edge_key.dst_label,
                edge_key.edge_label,
            ))
            .map_err(|error| UndoLogError::UndoFailed(error.to_string()))?;
        self.mark_edge_modified(edge_key.edge_label);
        Ok(())
    }

    fn delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()> {
        self.data_store()
            .with_vertex_tables_mut_result(|vertex_tables| {
                TransactionOps::delete_vertex(vertex_tables, vertex.label, vertex.vid, ts)
            })?;
        self.mark_vertex_modified(vertex.label);
        Ok(())
    }

    fn delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
        let edge = edge_ctx.edge_id;
        let params = crate::engine::params::EdgeOperationParams {
            edge_label: edge.edge_label,
            src_label: edge.src_label,
            src_id: edge.src_vid,
            dst_label: edge.dst_label,
            dst_id: edge.dst_vid,
            rank: edge.rank,
        };
        self.delete_edge_by_offset(
            &params,
            edge_ctx.oe_offset,
            edge_ctx.ie_offset,
            edge_ctx.timestamp,
        )
        .map_err(|error| UndoLogError::UndoFailed(error.to_string()))?;
        self.mark_edge_modified(edge_ctx.edge_id.edge_label);
        Ok(())
    }

    fn restore_edge(
        &self,
        edge: EdgeIdentifier,
        properties: Vec<(String, graphdb_core::Value)>,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        self.insert_edge(crate::engine::params::InsertEdgeParams {
            edge_label: edge.edge_label,
            src_label: edge.src_label,
            src_id: edge.src_vid,
            dst_label: edge.dst_label,
            dst_id: edge.dst_vid,
            rank: edge.rank,
            properties: &properties,
            ts,
        })
        .map_err(|error| UndoLogError::UndoFailed(error.to_string()))?;
        self.mark_edge_modified(edge.edge_label);
        Ok(())
    }

    fn undo_update_vertex_property(
        &self,
        vertex: VertexIdentifier,
        col_id: ColumnId,
        value: graphdb_core::Value,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        self.data_store()
            .with_vertex_tables_mut_result(|vertex_tables| {
                TransactionOps::update_vertex_property_undo(
                    vertex_tables,
                    vertex.label,
                    vertex.vid,
                    col_id,
                    value,
                    ts,
                )
            })?;
        self.mark_vertex_modified(vertex.label);
        Ok(())
    }

    fn undo_update_edge_property(
        &self,
        edge_id: EdgeIdentifier,
        col_id: ColumnId,
        value: graphdb_core::Value,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let params = UpdateEdgePropertyUndoParams {
            src_vid: checked_internal_vertex_id(&edge_id.src_vid)?,
            dst_vid: checked_internal_vertex_id(&edge_id.dst_vid)?,
            rank: edge_id.rank,
        };
        let key = EdgeTableKey::new(edge_id.src_label, edge_id.dst_label, edge_id.edge_label);
        self.data_store()
            .with_single_edge_table_mut(&key, |table| {
                TransactionOps::update_edge_property_undo_single(
                    table,
                    params,
                    col_id.0 as u16,
                    value,
                    ts,
                )
                .map_err(|e| graphdb_core::StorageError::db_error(e.to_string()))
            })
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        self.mark_edge_modified(edge_id.edge_label);
        Ok(())
    }

    fn revert_delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()> {
        self.data_store()
            .with_vertex_tables_mut_result(|vertex_tables| {
                TransactionOps::revert_delete_vertex(vertex_tables, vertex.label, vertex.vid, ts)
            })?;
        self.mark_vertex_modified(vertex.label);
        Ok(())
    }

    fn revert_delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
        let params = RevertDeleteEdgeParams {
            src_vid: checked_internal_vertex_id(&edge_ctx.edge_id.src_vid)?,
            dst_vid: checked_internal_vertex_id(&edge_ctx.edge_id.dst_vid)?,
            rank: edge_ctx.edge_id.rank,
        };
        let key = EdgeTableKey::new(
            edge_ctx.edge_id.src_label,
            edge_ctx.edge_id.dst_label,
            edge_ctx.edge_id.edge_label,
        );
        self.data_store()
            .with_single_edge_table_mut(&key, |table| {
                TransactionOps::revert_delete_edge_single(
                    table,
                    params,
                    edge_ctx.oe_offset,
                    edge_ctx.ie_offset,
                    edge_ctx.timestamp,
                )
                .map_err(|e| graphdb_core::StorageError::db_error(e.to_string()))
            })
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        self.mark_edge_modified(edge_ctx.edge_id.edge_label);
        Ok(())
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> UndoLogResult<()> {
        let label_id = {
            let catalog = self.data_store().catalog_write_set();
            TransactionOps::revert_delete_vertex_properties(
                &catalog.vertex_tables,
                &catalog.vertex_label_names,
                label_name,
                prop_names,
            )?;
            catalog.vertex_label_names.get(label_name).copied()
        };
        if let Some(label) = label_id {
            self.mark_vertex_modified(label);
        }
        Ok(())
    }

    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> UndoLogResult<()> {
        let edge_label_id = {
            let mut catalog = self.data_store().catalog_write_set();
            let edge_labels = EdgeTypeLabelParams {
                src_label,
                dst_label,
                edge_label,
            };
            TransactionOps::revert_delete_edge_properties(
                &mut catalog.edge_tables,
                &mut catalog.edge_label_names,
                &catalog.vertex_tables,
                prop_names,
                &edge_labels,
            )?;
            catalog.edge_label_names.get(edge_label).copied()
        };
        if let Some(label) = edge_label_id {
            self.mark_edge_modified(label);
        }
        Ok(())
    }

    fn revert_delete_vertex_label(&self, label_name: &str) -> UndoLogResult<()> {
        let label = self
            .data_store()
            .register_vertex_type(label_name.to_string(), None, |label| {
                let schema = crate::vertex::VertexSchema {
                    label_id: label,
                    label_name: label_name.to_string(),
                    properties: Vec::new(),
                    primary_key_index: 0,
                    schema_version: 1,
                };
                Ok(crate::vertex::ShardedVertexTable::new(
                    label,
                    label_name.to_string(),
                    schema,
                ))
            })
            .map_err(|error| UndoLogError::UndoFailed(error.to_string()))?;
        self.mark_vertex_modified(label);
        Ok(())
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> UndoLogResult<()> {
        let (src_label_id, dst_label_id) = {
            self.data_store().with_vertex_tables(|vertex_tables| {
                let src = vertex_tables
                    .values()
                    .find(|table| table.label_name() == src_label)
                    .map(|table| table.label())
                    .ok_or(UndoLogError::LabelNotFound(0))?;
                let dst = vertex_tables
                    .values()
                    .find(|table| table.label_name() == dst_label)
                    .map(|table| table.label())
                    .ok_or(UndoLogError::LabelNotFound(0))?;
                Ok((src, dst))
            })?
        };
        let edge_label_id = self
            .data_store()
            .register_edge_type(
                edge_label.to_string(),
                None,
                src_label_id,
                dst_label_id,
                |label| {
                    let schema = crate::edge::EdgeSchema {
                        label_id: label,
                        label_name: edge_label.to_string(),
                        src_label: src_label_id,
                        dst_label: dst_label_id,
                        properties: Vec::new(),
                        oe_strategy: crate::edge::EdgeStrategy::Multiple,
                        ie_strategy: crate::edge::EdgeStrategy::Multiple,
                        schema_version: 1,
                    };
                    crate::edge::EdgeStore::new(schema)
                },
            )
            .map_err(|error| UndoLogError::UndoFailed(error.to_string()))?;

        self.mark_edge_modified(edge_label_id);
        Ok(())
    }

    fn revert_rename_vertex_properties(
        &self,
        label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> UndoLogResult<()> {
        let label_id = {
            let catalog = self.data_store().catalog_write_set();
            TransactionOps::revert_rename_vertex_properties(
                &catalog.vertex_tables,
                &catalog.vertex_label_names,
                label,
                current_names,
                original_names,
            )?;
            catalog.vertex_label_names.get(label).copied()
        };
        if let Some(label_id) = label_id {
            self.mark_vertex_modified(label_id);
        }
        Ok(())
    }

    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> UndoLogResult<()> {
        let edge_label_id = {
            let mut catalog = self.data_store().catalog_write_set();
            let edge_labels = EdgeTypeLabelParams {
                src_label,
                dst_label,
                edge_label,
            };
            TransactionOps::revert_rename_edge_properties(
                &mut catalog.edge_tables,
                &mut catalog.edge_label_names,
                &catalog.vertex_tables,
                &edge_labels,
                current_names,
                original_names,
            )?;
            catalog.edge_label_names.get(edge_label).copied()
        };
        if let Some(label) = edge_label_id {
            self.mark_edge_modified(label);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::checked_internal_vertex_id;
    use graphdb_core::types::VertexId;

    #[test]
    fn checked_internal_vertex_id_rejects_non_integer_ids() {
        assert!(checked_internal_vertex_id(&VertexId::from_string("vertex-a")).is_err());
    }

    #[test]
    fn checked_internal_vertex_id_rejects_values_outside_u32() {
        assert!(
            checked_internal_vertex_id(&VertexId::from_int64(i64::from(u32::MAX) + 1)).is_err()
        );
    }
}
