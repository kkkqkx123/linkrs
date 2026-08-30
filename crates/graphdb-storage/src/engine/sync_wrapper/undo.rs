use super::SyncWrapper;
use crate::StorageClient;

impl<S: graphdb_transaction::UndoTarget + StorageClient> graphdb_transaction::UndoTarget
    for SyncWrapper<S>
{
    fn delete_vertex_type(
        &self,
        label: graphdb_core::types::LabelId,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_vertex_type(label)
    }

    fn delete_edge_type(
        &self,
        edge_key: graphdb_core::types::EdgeKey,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_edge_type(edge_key)
    }

    fn delete_vertex(
        &self,
        vertex: graphdb_core::types::VertexIdentifier,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_vertex(vertex, ts)
    }

    fn delete_edge(
        &self,
        edge_ctx: graphdb_core::types::EdgeDeletionContext,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_edge(edge_ctx)
    }

    fn restore_edge(
        &self,
        edge: graphdb_core::types::EdgeIdentifier,
        properties: Vec<(String, graphdb_core::Value)>,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.restore_edge(edge, properties, ts)
    }

    fn undo_update_vertex_property(
        &self,
        vertex: graphdb_core::types::VertexIdentifier,
        col_id: graphdb_core::types::ColumnId,
        value: graphdb_core::Value,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner
            .undo_update_vertex_property(vertex, col_id, value, ts)
    }

    fn undo_update_edge_property(
        &self,
        edge_id: graphdb_core::types::EdgeIdentifier,
        col_id: graphdb_core::types::ColumnId,
        value: graphdb_core::Value,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner
            .undo_update_edge_property(edge_id, col_id, value, ts)
    }

    fn revert_delete_vertex(
        &self,
        vertex: graphdb_core::types::VertexIdentifier,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_delete_vertex(vertex, ts)
    }

    fn revert_delete_edge(
        &self,
        edge_ctx: graphdb_core::types::EdgeDeletionContext,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_delete_edge(edge_ctx)
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_delete_vertex_properties(label_name, prop_names)
    }

    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_delete_edge_properties(src_label, dst_label, edge_label, prop_names)
    }

    fn revert_delete_vertex_label(
        &self,
        label_name: &str,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_delete_vertex_label(label_name)
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_delete_edge_label(src_label, dst_label, edge_label)
    }

    fn revert_rename_vertex_properties(
        &self,
        label_name: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_rename_vertex_properties(label_name, current_names, original_names)
    }

    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_rename_edge_properties(
            src_label,
            dst_label,
            edge_label,
            current_names,
            original_names,
        )
    }
}
