use crate::core::types::{
    ColumnId, EdgeDeletionContext, EdgeIdentifier, EdgeKey, LabelId, PropertyValue, Timestamp,
    UndoLogError, UndoLogResult, UndoTarget, VertexIdentifier,
};
use crate::storage::engine::graph_storage::GraphStorageContext;
use crate::storage::engine::transaction::{
    DeleteEdgeParams, DeleteEdgeTypeParams, EdgeTypeLabelParams, RevertDeleteEdgeParams,
    TransactionOps, UpdateEdgePropertyUndoParams,
};

impl UndoTarget for GraphStorageContext {
    fn delete_vertex_type(&self, label: LabelId) -> UndoLogResult<()> {
        {
            let mut vertex_tables = self.data_store().vertex_tables().write();
            let mut edge_tables = self.data_store().edge_tables().write();
            let mut vertex_label_names = self.data_store().vertex_label_names().write();
            let mut edge_label_names = self.data_store().edge_label_names().write();
            TransactionOps::delete_vertex_type(
                &mut vertex_tables,
                &mut edge_tables,
                &mut vertex_label_names,
                &mut edge_label_names,
                label,
            )?;
        }
        self.mark_vertex_modified(label);
        Ok(())
    }

    fn delete_edge_type(&self, edge_key: EdgeKey) -> UndoLogResult<()> {
        let params = DeleteEdgeTypeParams {
            src_label: edge_key.src_label,
            dst_label: edge_key.dst_label,
            edge_label: edge_key.edge_label,
        };
        {
            let mut edge_tables = self.data_store().edge_tables().write();
            let mut edge_label_names = self.data_store().edge_label_names().write();
            TransactionOps::delete_edge_type(&mut edge_tables, &mut edge_label_names, params)?;
        }
        self.mark_edge_modified(edge_key.edge_label);
        Ok(())
    }

    fn delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()> {
        {
            let mut vertex_tables = self.data_store().vertex_tables().write();
            TransactionOps::delete_vertex(&mut vertex_tables, vertex.label, vertex.vid, ts)?;
        }
        self.mark_vertex_modified(vertex.label);
        Ok(())
    }

    fn delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
        let params = DeleteEdgeParams {
            src_label: edge_ctx.edge_id.src_label,
            src_vid: edge_ctx.edge_id.src_vid.as_int64().unwrap_or(0) as u32,
            dst_label: edge_ctx.edge_id.dst_label,
            dst_vid: edge_ctx.edge_id.dst_vid.as_int64().unwrap_or(0) as u32,
            edge_label: edge_ctx.edge_id.edge_label,
            rank: edge_ctx.edge_id.rank,
        };
        {
            let mut edge_tables = self.data_store().edge_tables().write();
            TransactionOps::delete_edge(
                &mut edge_tables,
                params,
                edge_ctx.oe_offset,
                edge_ctx.ie_offset,
                edge_ctx.timestamp,
            )?;
        }
        self.mark_edge_modified(edge_ctx.edge_id.edge_label);
        Ok(())
    }

    fn undo_update_vertex_property(
        &self,
        vertex: VertexIdentifier,
        col_id: ColumnId,
        value: PropertyValue,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        {
            let mut vertex_tables = self.data_store().vertex_tables().write();
            TransactionOps::update_vertex_property_undo(
                &mut vertex_tables,
                vertex.label,
                vertex.vid,
                col_id,
                value,
                ts,
            )?;
        }
        self.mark_vertex_modified(vertex.label);
        Ok(())
    }

    fn undo_update_edge_property(
        &self,
        edge_id: EdgeIdentifier,
        oe_offset: i32,
        ie_offset: i32,
        col_id: ColumnId,
        value: PropertyValue,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let params = UpdateEdgePropertyUndoParams {
            src_label: edge_id.src_label,
            src_vid: edge_id.src_vid.as_int64().unwrap_or(0) as u32,
            dst_label: edge_id.dst_label,
            dst_vid: edge_id.dst_vid.as_int64().unwrap_or(0) as u32,
            edge_label: edge_id.edge_label,
            rank: edge_id.rank,
        };
        {
            let mut edge_tables = self.data_store().edge_tables().write();
            TransactionOps::update_edge_property_undo(
                &mut edge_tables,
                params,
                oe_offset,
                ie_offset,
                col_id.0 as u16,
                value,
                ts,
            )?;
        }
        self.mark_edge_modified(edge_id.edge_label);
        Ok(())
    }

    fn revert_delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()> {
        {
            let mut vertex_tables = self.data_store().vertex_tables().write();
            TransactionOps::revert_delete_vertex(&mut vertex_tables, vertex.label, vertex.vid, ts)?;
        }
        self.mark_vertex_modified(vertex.label);
        Ok(())
    }

    fn revert_delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
        let params = RevertDeleteEdgeParams {
            src_label: edge_ctx.edge_id.src_label,
            dst_label: edge_ctx.edge_id.dst_label,
            edge_label: edge_ctx.edge_id.edge_label,
            src_vid: edge_ctx.edge_id.src_vid.as_int64().unwrap_or(0) as u32,
            dst_vid: edge_ctx.edge_id.dst_vid.as_int64().unwrap_or(0) as u32,
            rank: edge_ctx.edge_id.rank,
        };
        {
            let mut edge_tables = self.data_store().edge_tables().write();
            TransactionOps::revert_delete_edge(
                &mut edge_tables,
                params,
                edge_ctx.oe_offset,
                edge_ctx.ie_offset,
                edge_ctx.timestamp,
            )?;
        }
        self.mark_edge_modified(edge_ctx.edge_id.edge_label);
        Ok(())
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> UndoLogResult<()> {
        let label_id = {
            let mut vertex_tables = self.data_store().vertex_tables().write();
            let mut vertex_label_names = self.data_store().vertex_label_names().write();
            TransactionOps::revert_delete_vertex_properties(
                &mut vertex_tables,
                &mut vertex_label_names,
                label_name,
                prop_names,
            )?;
            vertex_label_names.get(label_name).copied()
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
            let vertex_tables = self.data_store().vertex_tables().read();
            let mut edge_tables = self.data_store().edge_tables().write();
            let mut edge_label_names = self.data_store().edge_label_names().write();
            let edge_labels = EdgeTypeLabelParams {
                src_label,
                dst_label,
                edge_label,
            };
            TransactionOps::revert_delete_edge_properties(
                &mut edge_tables,
                &mut edge_label_names,
                &vertex_tables,
                prop_names,
                &edge_labels,
            )?;
            edge_label_names.get(edge_label).copied()
        };
        if let Some(label) = edge_label_id {
            self.mark_edge_modified(label);
        }
        Ok(())
    }

    fn revert_delete_vertex_label(&self, label_name: &str) -> UndoLogResult<()> {
        let label;
        {
            let mut vertex_tables = self.data_store().vertex_tables().write();
            let mut vertex_label_names = self.data_store().vertex_label_names().write();
            let mut vertex_label_counter = self.data_store().vertex_label_counter().write();
            label = *vertex_label_counter;
            TransactionOps::create_vertex_type_undo(
                &mut vertex_tables,
                &mut vertex_label_names,
                &mut vertex_label_counter,
                label_name,
            )?;
        }
        self.mark_vertex_modified(label);
        Ok(())
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> UndoLogResult<()> {
        let edge_label_id = {
            let vertex_tables = self.data_store().vertex_tables().read();
            let mut edge_tables = self.data_store().edge_tables().write();
            let mut edge_label_names = self.data_store().edge_label_names().write();
            let mut edge_label_counter = self.data_store().edge_label_counter().write();
            TransactionOps::create_edge_type_undo(
                &mut edge_tables,
                &mut edge_label_names,
                &mut edge_label_counter,
                &vertex_tables,
                edge_label,
                src_label,
                dst_label,
            )
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
            edge_label_names
                .get(edge_label)
                .copied()
                .ok_or(UndoLogError::LabelNotFound(0))?
        };

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
            let mut vertex_tables = self.data_store().vertex_tables().write();
            let mut vertex_label_names = self.data_store().vertex_label_names().write();
            TransactionOps::revert_rename_vertex_properties(
                &mut vertex_tables,
                &mut vertex_label_names,
                label,
                current_names,
                original_names,
            )?;
            vertex_label_names.get(label).copied()
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
            let vertex_tables = self.data_store().vertex_tables().read();
            let mut edge_tables = self.data_store().edge_tables().write();
            let mut edge_label_names = self.data_store().edge_label_names().write();
            let edge_labels = EdgeTypeLabelParams {
                src_label,
                dst_label,
                edge_label,
            };
            TransactionOps::revert_rename_edge_properties(
                &mut edge_tables,
                &mut edge_label_names,
                &vertex_tables,
                &edge_labels,
                current_names,
                original_names,
            )?;
            edge_label_names.get(edge_label).copied()
        };
        if let Some(label) = edge_label_id {
            self.mark_edge_modified(label);
        }
        Ok(())
    }
}
